//! `trust-domain set`: the plan, the refusals and (below) the change, against the fake cluster
//! with a small simulation of the controllers (a StatefulSet that scales, a SPIRE server that
//! makes a new CA when its volume is new).

mod common;

use std::sync::{Arc, Mutex};

use common::{Env, ca_pem, env};
use grauwolf_k8s::testing::FakeCluster;
use grauwolf_test::TestDir;
use serde_json::json;

const OLD: &str = "example.org";
const NEW: &str = "agentswork.de";
const SPIRE: &str = "/api/v1/namespaces/spire-system";
const SERVER_CM: &str = "/api/v1/namespaces/spire-system/configmaps/spire-server";
const AGENT_CM: &str = "/api/v1/namespaces/spire-system/configmaps/spire-agent";
const CONTROLLER_CM: &str = "/api/v1/namespaces/spire-system/configmaps/spire-controller-manager";
const OIDC_CM: &str = "/api/v1/namespaces/spire-oidc/configmaps/spire-oidc-discovery-provider";
const OIDC_SET: &str =
    "/apis/apps/v1/namespaces/spire-oidc/deployments/spire-oidc-discovery-provider";
const BUNDLE: &str = "/api/v1/namespaces/spire-system/configmaps/spire-bundle";
const SERVER_SET: &str = "/apis/apps/v1/namespaces/spire-system/statefulsets/spire-server";
const AGENT_SET: &str = "/apis/apps/v1/namespaces/spire-system/daemonsets/spire-agent";
const CLAIM: &str =
    "/api/v1/namespaces/spire-system/persistentvolumeclaims/spire-data-spire-server-0";

fn server_conf(domain: &str, extra: &str) -> String {
    format!(
        "server {{\n  bind_address = \"0.0.0.0\"\n  trust_domain = \"{domain}\"\n  data_dir = \"/run/spire/data\"\n  jwt_issuer = \"https://oidc.{domain}\"\n}}\n{extra}plugins {{\n  DataStore \"sql\" {{\n    plugin_data {{\n      database_type = \"sqlite3\"\n      connection_string = \"/run/spire/data/datastore.sqlite3\"\n    }}\n  }}\n  KeyManager \"disk\" {{\n    plugin_data {{\n      keys_path = \"/run/spire/data/keys.json\"\n    }}\n  }}\n  Notifier \"k8sbundle\" {{\n    plugin_data {{\n      namespace = \"spire-system\"\n      config_map = \"spire-bundle\"\n    }}\n  }}\n}}\n"
    )
}

/// What the simulated controllers did, in order.
#[derive(Default)]
struct Sim {
    /// The pods of the StatefulSet never go away.
    pods_stay: bool,
    /// A new volume does not give a new CA (the server never writes the bundle).
    bundle_stays: bool,
    /// The server never becomes ready.
    never_ready: bool,
    /// `"<path> bundle=<domains>"` at every restart of a DaemonSet or Deployment.
    restarts: Vec<String>,
}

struct World {
    env: Env,
    sim: Arc<Mutex<Sim>>,
}

impl std::ops::Deref for World {
    type Target = Env;
    fn deref(&self) -> &Env {
        &self.env
    }
}

fn bundle_domains(fake: &FakeCluster) -> String {
    let bundle = fake.object(BUNDLE).expect("bundle");
    let text = bundle["data"]["bundle.crt"].as_str().unwrap_or("");
    [OLD, NEW, "other.test"]
        .into_iter()
        .filter(|domain| text.contains(ca_pem(domain).trim()))
        .collect::<Vec<_>>()
        .join(",")
}

fn install_sim(fake: &FakeCluster, sim: Arc<Mutex<Sim>>) {
    fake.on_change(move |fake, method, path| {
        if method != "PATCH" {
            return;
        }
        let mut sim = sim.lock().unwrap();
        let Some(mut object) = fake.object(path) else {
            return;
        };
        let generation = object["metadata"]["generation"].as_u64().unwrap_or(0) + 1;
        object["metadata"]["generation"] = json!(generation);
        if path == SERVER_SET {
            let replicas = object["spec"]["replicas"].as_u64().unwrap_or(1);
            let ready = if replicas == 0 || sim.never_ready { 0 } else { replicas };
            object["status"] = json!({"observedGeneration": generation, "replicas": replicas, "readyReplicas": ready});
            for ordinal in 0..4 {
                let pod = format!("{SPIRE}/pods/spire-server-{ordinal}");
                if replicas == 0 && !sim.pods_stay {
                    fake.remove(&pod);
                } else if ordinal < replicas {
                    fake.insert(&pod, json!({"kind": "Pod"}));
                }
            }
            if replicas > 0 {
                let mut fresh = false;
                for ordinal in 0..replicas {
                    let claim = format!("{SPIRE}/persistentvolumeclaims/spire-data-spire-server-{ordinal}");
                    if fake.object(&claim).is_none() {
                        fake.insert(&claim, json!({"kind": "PersistentVolumeClaim"}));
                        fresh = true;
                    }
                }
                if fresh && !sim.bundle_stays {
                    let conf = fake.object(SERVER_CM).unwrap()["data"]["server.conf"]
                        .as_str()
                        .unwrap()
                        .to_owned();
                    let domain = [OLD, NEW, "other.test"]
                        .into_iter()
                        .find(|domain| conf.contains(&format!("trust_domain = \"{domain}\"")))
                        .expect("a known domain");
                    fake.insert(BUNDLE, json!({"data": {"bundle.crt": ca_pem(domain)}}));
                }
            }
        } else if path.contains("/daemonsets/") || path.contains("/deployments/") {
            let desired = object["status"]["desiredNumberScheduled"]
                .as_u64()
                .or(object["spec"]["replicas"].as_u64())
                .unwrap_or(1);
            object["status"] = json!({"observedGeneration": generation,
                "desiredNumberScheduled": desired, "numberReady": desired, "updatedNumberScheduled": desired,
                "replicas": desired, "readyReplicas": desired, "updatedReplicas": desired});
            sim.restarts.push(format!("{path} bundle={}", bundle_domains(fake)));
        }
        fake.insert(path, object);
    });
}

/// SPIRE with the trust domain `example.org`, an installed OIDC provider and the agent
/// `reviewer` bound to it, and simulated controllers.
async fn world() -> World {
    let env = env();
    let fake = &env.fake;
    common::seed_spire(fake, OLD);
    fake.insert(
        SERVER_CM,
        json!({"data": {"server.conf": server_conf(OLD, "")}}),
    );
    fake.insert(
        SERVER_SET,
        json!({"kind": "StatefulSet", "metadata": {"name": "spire-server", "generation": 1},
            "spec": {"replicas": 1, "selector": {"matchLabels": {"app": "spire-server"}},
                "template": {"spec": {"containers": [
                    {"name": "spire-server", "image": "ghcr.io/spiffe/spire-server:1.15.5",
                        "volumeMounts": [{"name": "spire-config", "mountPath": "/run/spire/config"},
                            {"name": "spire-data", "mountPath": "/run/spire/data"}]},
                    {"name": "spire-controller-manager", "image": "ghcr.io/spiffe/spire-controller-manager:0.6.3"}]}},
                "volumeClaimTemplates": [{"metadata": {"name": "spire-data"}}]},
            "status": {"observedGeneration": 1, "replicas": 1, "readyReplicas": 1}}),
    );
    fake.insert(
        &format!("{SPIRE}/pods/spire-server-0"),
        json!({"kind": "Pod"}),
    );
    fake.insert(CLAIM, json!({"kind": "PersistentVolumeClaim"}));
    fake.insert(BUNDLE, json!({"data": {"bundle.crt": ca_pem(OLD)}}));
    fake.insert(
        AGENT_SET,
        json!({"kind": "DaemonSet", "metadata": {"generation": 1},
            "status": {"observedGeneration": 1, "desiredNumberScheduled": 1, "numberReady": 1, "updatedNumberScheduled": 1}}),
    );
    name_objects(fake);
    env.prepare().await;
    assert_eq!(env.run(&["agent", "add", "reviewer"], "").await.code, 0);
    let sim = Arc::new(Mutex::new(Sim::default()));
    install_sim(fake, sim.clone());
    World { env, sim }
}

/// A real cluster's objects name themselves; `restore` reads that from a backup.
fn name_objects(fake: &FakeCluster) {
    for path in [SERVER_CM, AGENT_CM, CONTROLLER_CM, SERVER_SET, AGENT_SET] {
        let mut object = fake.object(path).unwrap();
        let mut parts = path.rsplit('/');
        let name = parts.next().unwrap().to_owned();
        let namespace = parts.nth(1).unwrap().to_owned();
        object["metadata"]["name"] = json!(name);
        object["metadata"]["namespace"] = json!(namespace);
        fake.insert(path, object);
    }
}

fn config_text(fake: &FakeCluster, path: &str, key: &str) -> String {
    fake.object(path).unwrap()["data"][key]
        .as_str()
        .unwrap()
        .to_owned()
}

fn role_subject(env: &Env) -> String {
    env.bao.lock().unwrap().roles["agent-reviewer"]["bound_subject"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// What a run must not have touched.
struct Marks {
    writes: usize,
    deleted: usize,
    bao: usize,
}

fn marks(env: &Env) -> Marks {
    Marks {
        writes: env.fake.writes().len(),
        deleted: env.fake.deleted().len(),
        bao: env.bao_writes(),
    }
}

fn assert_untouched(env: &Env, before: &Marks) {
    assert_eq!(env.fake.writes().len(), before.writes, "cluster writes");
    assert_eq!(env.fake.deleted().len(), before.deleted, "cluster deletes");
    assert_eq!(env.bao_writes(), before.bao, "OpenBao writes");
}

// ---- the plan ------------------------------------------------------------------------------

#[tokio::test]
async fn the_plan_lists_every_change_and_writes_nothing() {
    let world = world().await;
    let before = marks(&world);
    let run = world.run(&["trust-domain", "set", NEW], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_untouched(&world, &before);
    let out = run.out;
    for expected in [
        "trust domain change: example.org -> agentswork.de",
        // every place with its current domain
        "SPIRE server (ConfigMap spire-system/spire-server, key server.conf): example.org",
        "SPIRE agent (ConfigMap spire-system/spire-agent, key agent.conf): example.org",
        "spire-controller-manager (ConfigMap spire-system/spire-controller-manager, key controller-manager.yaml): example.org",
        "OIDC discovery provider (ConfigMap spire-oidc/spire-oidc-discovery-provider, key oidc-discovery-provider.conf): example.org",
        "CA of the SPIRE server (ConfigMap spire-system/spire-bundle, key bundle.crt): example.org",
        "OpenBao role auth/jwt/role/agent-reviewer: example.org",
        // the line-level changes
        "line 3: - trust_domain = \"example.org\"",
        "+ trust_domain = \"agentswork.de\"",
        "line 1: - trustDomain: example.org",
        "+ trustDomain: agentswork.de",
        // the volume and the waits
        "scale StatefulSet spire-system/spire-server from 1 to 0",
        "delete PersistentVolumeClaim spire-system/spire-data-spire-server-0 (exists)",
        "scale StatefulSet spire-system/spire-server back to 1",
        "carries a CA with the URI SAN spiffe://agentswork.de",
        // the restarts
        "DaemonSet spire-system/spire-agent",
        "Deployment spire-oidc/spire-oidc-discovery-provider",
        // OpenBao
        "auth/jwt/role/agent-reviewer: spiffe://example.org/ns/reviewer/sa/reviewer -> spiffe://agentswork.de/ns/reviewer/sa/reviewer",
        // the meaning
        "Every SPIFFE ID changes",
        "every workload gets a new SVID",
        "stops working",
        "This was only the plan: nothing was changed.",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
    }
    // The jwt_issuer on a line of its own is not an assignment of the trust domain: warned about.
    assert!(
        out.contains("still mention example.org")
            && out.contains("jwt_issuer = \"https://oidc.example.org\""),
        "{out}"
    );
}

#[tokio::test]
async fn the_same_domain_is_already_done_and_an_invalid_one_is_refused() {
    let world = world().await;
    let run = world.run(&["trust-domain", "set", OLD], "").await;
    assert_eq!(run.code, 0);
    assert!(
        run.out.contains("already example.org, nothing to do"),
        "{}",
        run.out
    );
    for bad in [
        "Example.org",
        "spiffe://example.org",
        "a/b",
        "",
        "with space",
        "ü.de",
    ] {
        let run = world.run(&["trust-domain", "set", bad], "").await;
        assert_eq!(run.code, 1, "{bad}");
        assert!(
            run.err.contains("not a valid trust domain"),
            "{bad}: {}",
            run.err
        );
    }
}

async fn refused(world: &World, reason: &str) {
    let before = marks(world);
    let run = world.run(&["trust-domain", "set", NEW], "").await;
    assert_eq!(run.code, 1, "{}", run.all());
    assert!(run.err.contains(reason), "{reason:?} not in {}", run.err);
    assert_untouched(world, &before);
}

#[tokio::test]
async fn a_server_that_is_not_on_sqlite_is_refused() {
    let world = world().await;
    let conf = server_conf(OLD, "").replace("sqlite3", "postgres");
    world
        .fake
        .insert(SERVER_CM, json!({"data": {"server.conf": conf}}));
    refused(&world, "uses the datastore 'postgres', not sqlite3").await;
}

#[tokio::test]
async fn data_that_is_not_on_a_volume_claim_template_is_refused() {
    let world = world().await;
    let mut set = world.fake.object(SERVER_SET).unwrap();
    set["spec"]["volumeClaimTemplates"] = json!([]);
    world.fake.insert(SERVER_SET, set);
    refused(&world, "lies on the volume 'spire-data'").await;

    let conf = server_conf(OLD, "").replace(
        "/run/spire/data/datastore.sqlite3",
        "/tmp/datastore.sqlite3",
    );
    world
        .fake
        .insert(SERVER_CM, json!({"data": {"server.conf": conf}}));
    refused(&world, "is not on a mounted volume").await;
}

#[tokio::test]
async fn keys_outside_the_claim_are_refused_too() {
    let world = world().await;
    let conf =
        server_conf(OLD, "").replace("/run/spire/data/keys.json", "/run/spire/config/keys.json");
    world
        .fake
        .insert(SERVER_CM, json!({"data": {"server.conf": conf}}));
    refused(&world, "the keys of the KeyManager \"disk\"").await;
}

#[tokio::test]
async fn federation_is_refused() {
    let world = world().await;
    let conf = server_conf(
        OLD,
        "federation {\n  bundle_endpoint { port = 8443 }\n  federates_with \"other.test\" {}\n}\n",
    );
    world
        .fake
        .insert(SERVER_CM, json!({"data": {"server.conf": conf}}));
    refused(&world, "contains federation").await;
}

#[tokio::test]
async fn a_second_foreign_domain_is_refused_as_ambiguous() {
    let world = world().await;
    world.fake.insert(
        AGENT_CM,
        json!({"data": {"agent.conf": "agent {\n  trust_domain = \"other.test\"\n}\n"}}),
    );
    refused(&world, "more than one trust domain besides agentswork.de").await;
}

#[tokio::test]
async fn a_server_scaled_to_zero_without_a_memo_is_refused() {
    let world = world().await;
    let mut set = world.fake.object(SERVER_SET).unwrap();
    set["spec"]["replicas"] = json!(0);
    world.fake.insert(SERVER_SET, set);
    refused(&world, "does not say how many replicas it had").await;
}

#[tokio::test]
async fn missing_spire_parts_are_refused_or_skipped() {
    let world = world().await;
    world.fake.remove(AGENT_SET);
    refused(&world, "DaemonSet spire-system/spire-agent not found").await;

    let world = self::world().await;
    world.fake.remove(AGENT_CM);
    world.fake.remove(OIDC_CM);
    let run = world.run(&["trust-domain", "set", NEW], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out
            .contains("SPIRE agent: ConfigMap spire-system/spire-agent not found, skipped"),
        "{}",
        run.out
    );

    let world = self::world().await;
    world.fake.remove(SERVER_CM);
    refused(&world, "SPIRE is not installed").await;
}

#[tokio::test]
async fn a_half_finished_change_shows_the_mixed_state() {
    let world = world().await;
    // An earlier run rewrote the agent and controller configuration and the role.
    world.fake.insert(
        AGENT_CM,
        json!({"data": {"agent.conf": "agent {\n  trust_domain = \"agentswork.de\"\n}\n"}}),
    );
    world.fake.insert(
        CONTROLLER_CM,
        json!({"data": {"controller-manager.yaml": "trustDomain: agentswork.de\n"}}),
    );
    world
        .env
        .bao
        .lock()
        .unwrap()
        .roles
        .get_mut("agent-reviewer")
        .unwrap()["bound_subject"] = json!("spiffe://agentswork.de/ns/reviewer/sa/reviewer");
    let before = marks(&world);
    let run = world.run(&["trust-domain", "set", NEW], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_untouched(&world, &before);
    let out = run.out;
    assert!(out.contains("SPIRE agent (ConfigMap spire-system/spire-agent, key agent.conf): agentswork.de  (already new)"), "{out}");
    assert!(out.contains("SPIRE agent (ConfigMap spire-system/spire-agent, key agent.conf) is already agentswork.de"), "{out}");
    assert!(out.contains("OpenBao role auth/jwt/role/agent-reviewer is already bound to spiffe://agentswork.de/ns/reviewer/sa/reviewer"), "{out}");
    assert!(out.contains("OpenBao roles: nothing to change"), "{out}");
    // The server still has the old domain, so it is still rewritten and its data recreated.
    assert!(
        out.contains("line 3: - trust_domain = \"example.org\""),
        "{out}"
    );
    assert!(out.contains("delete PersistentVolumeClaim"), "{out}");
}

#[tokio::test]
async fn when_the_server_already_has_a_new_ca_its_volume_is_kept() {
    let world = world().await;
    world
        .fake
        .insert(BUNDLE, json!({"data": {"bundle.crt": ca_pem(NEW)}}));
    let run = world.run(&["trust-domain", "set", NEW], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(run.out.contains("SPIRE server data: kept"), "{}", run.out);
    assert!(
        !run.out.contains("delete PersistentVolumeClaim"),
        "{}",
        run.out
    );
}

#[tokio::test]
async fn everything_new_is_done_and_a_consumers_file_needs_from() {
    let world = world().await;
    let dir = TestDir::new("trust-domain-done");
    let file = dir.join("consumers.toml");
    std::fs::write(
        &file,
        "[[restart]]\nkind = \"Deployment\"\nnamespace = \"x\"\nname = \"y\"\n",
    )
    .unwrap();
    for (path, key, text) in [
        (SERVER_CM, "server.conf", server_conf(NEW, "")),
        (
            AGENT_CM,
            "agent.conf",
            "agent {\n  trust_domain = \"agentswork.de\"\n}\n".to_owned(),
        ),
        (
            CONTROLLER_CM,
            "controller-manager.yaml",
            "trustDomain: agentswork.de\n".to_owned(),
        ),
        (
            OIDC_CM,
            "oidc-discovery-provider.conf",
            "workload_api {\n  trust_domain = \"agentswork.de\"\n}\n".to_owned(),
        ),
    ] {
        world.fake.insert(path, json!({"data": {key: text}}));
    }
    world
        .fake
        .insert(BUNDLE, json!({"data": {"bundle.crt": ca_pem(NEW)}}));
    world
        .env
        .bao
        .lock()
        .unwrap()
        .roles
        .get_mut("agent-reviewer")
        .unwrap()["bound_subject"] = json!("spiffe://agentswork.de/ns/reviewer/sa/reviewer");
    let run = world.run(&["trust-domain", "set", NEW], "").await;
    assert_eq!(
        (run.code, run.out.trim()),
        (0, "already agentswork.de, nothing to do")
    );

    let run = world
        .run(
            &[
                "trust-domain",
                "set",
                NEW,
                "--consumers",
                file.to_str().unwrap(),
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 0);
    assert!(
        run.out.contains("consumers file was not checked") && run.out.contains("--from"),
        "{}",
        run.out
    );
}

// ---- consumers -----------------------------------------------------------------------------

const SCION: &str = "/api/v1/namespaces/scion/configmaps/scion-server";
const SCION_DEPLOYMENT: &str = "/apis/apps/v1/namespaces/scion/deployments/scion-server";

fn seed_scion(fake: &FakeCluster) {
    fake.insert(
        SCION,
        json!({"data": {
            "acl.toml": "allow = [\n  \"spiffe://example.org/ns/reviewer/sa/reviewer\",\n]\nhost = \"foo.example.org\"\ndomain = \"example.org\"\n",
            "scion-server.toml": "trust_domain = \"example.org\"\n",
            "other.txt": "see spiffe://example.org/x\n"}}),
    );
    fake.insert(SCION_DEPLOYMENT, json!({"kind": "Deployment", "metadata": {"generation": 1}, "spec": {"replicas": 1},
        "status": {"observedGeneration": 1, "replicas": 1, "readyReplicas": 1, "updatedReplicas": 1}}));
}

const CONSUMERS: &str = "[[configmap]]\nnamespace = \"scion\"\nname = \"scion-server\"\nkeys = [\"acl.toml\", \"scion-server.toml\"]\n\n[[restart]]\nkind = \"Deployment\"\nnamespace = \"scion\"\nname = \"scion-server\"\n";

#[tokio::test]
async fn the_plan_shows_consumer_changes_by_the_boundary_rule() {
    let world = world().await;
    seed_scion(&world.fake);
    let dir = TestDir::new("trust-domain-consumers");
    let file = dir.join("consumers.toml");
    std::fs::write(&file, CONSUMERS).unwrap();
    let before = marks(&world);
    let run = world
        .run(
            &[
                "trust-domain",
                "set",
                NEW,
                "--consumers",
                file.to_str().unwrap(),
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_untouched(&world, &before);
    let out = run.out;
    for expected in [
        "ConfigMap scion/scion-server, key acl.toml:",
        "- \"spiffe://example.org/ns/reviewer/sa/reviewer\",",
        "+ \"spiffe://agentswork.de/ns/reviewer/sa/reviewer\",",
        "- domain = \"example.org\"",
        "+ domain = \"agentswork.de\"",
        "ConfigMap scion/scion-server, key scion-server.toml:",
        "restart Deployment scion/scion-server",
        // foo.example.org stays and is pointed out
        "ConfigMap scion/scion-server, key acl.toml, line 4: host = \"foo.example.org\"",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
    }
    assert!(
        !out.contains("other.txt"),
        "keys not listed are not touched:\n{out}"
    );
}

#[tokio::test]
async fn a_consumer_that_does_not_exist_is_refused_before_anything_happens() {
    let world = world().await;
    let dir = TestDir::new("trust-domain-missing-consumer");
    let file = dir.join("consumers.toml");
    std::fs::write(&file, CONSUMERS).unwrap();
    let before = marks(&world);
    let run = world
        .run(
            &[
                "trust-domain",
                "set",
                NEW,
                "--consumers",
                file.to_str().unwrap(),
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 1);
    assert!(
        run.err
            .contains("consumer ConfigMap scion/scion-server not found"),
        "{}",
        run.err
    );
    assert_untouched(&world, &before);

    std::fs::write(&file, "[[restart]]\nkind = \"Pod\"\n").unwrap();
    let run = world
        .run(
            &[
                "trust-domain",
                "set",
                NEW,
                "--consumers",
                file.to_str().unwrap(),
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 1);
    assert!(
        run.err.contains("consumers.toml") && run.err.contains("kind 'Pod'"),
        "{}",
        run.err
    );
}

// ---- the change ----------------------------------------------------------------------------

/// `trust-domain set <NEW> --apply --yes --backup <dir>/backup.json` and `extra`.
async fn apply(world: &World, dir: &TestDir, extra: &[&str]) -> common::Run {
    let backup = dir.join("backup.json");
    let backup = backup.to_str().unwrap();
    let mut args = vec![
        "trust-domain",
        "set",
        NEW,
        "--apply",
        "--yes",
        "--backup",
        backup,
        "--timeout",
        "1",
    ];
    args.extend(extra);
    world.run(&args, "").await
}

fn position(writes: &[String], suffix: &str, nth: usize) -> usize {
    writes
        .iter()
        .enumerate()
        .filter(|(_, write)| write.ends_with(suffix))
        .nth(nth)
        .map(|(index, _)| index)
        .unwrap_or_else(|| panic!("no write #{nth} to {suffix} in {writes:#?}"))
}

#[tokio::test]
async fn apply_changes_everything_in_the_documented_order() {
    let world = world().await;
    let dir = TestDir::new("trust-domain-apply");
    let before = marks(&world);
    let run = apply(&world, &dir, &[]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out
            .contains("done: the trust domain is agentswork.de in all 6 place(s) read"),
        "{}",
        run.out
    );

    // Configuration: only the assignments changed.
    let server = config_text(&world.fake, SERVER_CM, "server.conf");
    assert_eq!(
        server,
        server_conf(OLD, "").replace(
            "trust_domain = \"example.org\"",
            "trust_domain = \"agentswork.de\""
        )
    );
    assert!(server.contains("jwt_issuer = \"https://oidc.example.org\""));
    assert!(
        config_text(&world.fake, AGENT_CM, "agent.conf")
            .contains("trust_domain = \"agentswork.de\"")
    );
    assert!(
        config_text(&world.fake, CONTROLLER_CM, "controller-manager.yaml")
            .starts_with("trustDomain: agentswork.de\n")
    );
    assert!(
        config_text(&world.fake, OIDC_CM, "oidc-discovery-provider.conf")
            .contains("trust_domain = \"agentswork.de\"")
    );

    // The order of the writes: configuration, scale down, (PVC delete), scale up, agents, provider.
    let writes: Vec<String> = world.fake.writes().split_off(before.writes);
    let order = [
        position(&writes, "/configmaps/spire-server", 0),
        position(&writes, "/configmaps/spire-agent", 0),
        position(&writes, "/configmaps/spire-controller-manager", 0),
        position(&writes, "/configmaps/spire-oidc-discovery-provider", 0),
        position(&writes, "/statefulsets/spire-server", 0),
        position(&writes, "/statefulsets/spire-server", 1),
        position(&writes, "/daemonsets/spire-agent", 0),
        position(&writes, "/deployments/spire-oidc-discovery-provider", 0),
    ];
    assert!(
        order.windows(2).all(|pair| pair[0] < pair[1]),
        "{order:?} in {writes:#?}"
    );
    assert_eq!(world.fake.deleted()[before.deleted..], [CLAIM]);
    let set = world.fake.object(SERVER_SET).unwrap();
    assert_eq!(set["spec"]["replicas"], 1);
    assert!(
        set["metadata"]["annotations"]
            .get("grauwolf.io/trust-domain-replicas")
            .is_none(),
        "{set}"
    );
    assert_eq!(bundle_domains(&world.fake), NEW);

    // The agents and the provider were restarted only after the bundle had the new domain.
    let restarts = world.sim.lock().unwrap().restarts.clone();
    assert_eq!(restarts.len(), 2, "{restarts:?}");
    assert!(
        restarts
            .iter()
            .all(|restart| restart.ends_with("bundle=agentswork.de")),
        "{restarts:?}"
    );
    assert!(
        restarts[0].contains("/daemonsets/spire-agent")
            && restarts[1].contains("/deployments/spire-oidc"),
        "{restarts:?}"
    );
    let template =
        &world.fake.object(AGENT_SET).unwrap()["spec"]["template"]["metadata"]["annotations"];
    assert!(template["kubectl.kubernetes.io/restartedAt"].is_string());

    // OpenBao: the subject changed, the other fields stayed.
    assert_eq!(
        role_subject(&world),
        "spiffe://agentswork.de/ns/reviewer/sa/reviewer"
    );
    let role = world.bao.lock().unwrap().roles["agent-reviewer"].clone();
    assert_eq!(role["bound_audiences"], json!(["openbao"]));
    assert_eq!(role["token_policies"], json!(["agent-reviewer"]));

    // The doctor agrees.
    let doctor = world.run(&["doctor", "--json"], "").await;
    assert_eq!(doctor.code, 0, "{}", doctor.out);
    let checks: Vec<serde_json::Value> = serde_json::from_str(&doctor.out).unwrap();
    let check = checks
        .iter()
        .find(|check| check["check"] == "trust domain")
        .unwrap();
    assert_eq!(check["status"], "ok", "{check}");

    // A second run has nothing to do.
    let again = world.run(&["trust-domain", "set", NEW], "").await;
    assert_eq!(
        (again.code, again.out.trim()),
        (0, "already agentswork.de, nothing to do")
    );
}

#[tokio::test]
async fn the_backup_is_written_first_and_holds_the_old_objects() {
    let world = world().await;
    let dir = TestDir::new("trust-domain-backup");
    let run = apply(&world, &dir, &[]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    let backup: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("backup.json")).unwrap()).unwrap();
    assert_eq!(
        (backup["from"].as_str(), backup["to"].as_str()),
        (Some(OLD), Some(NEW))
    );
    let maps = backup["configmaps"].as_array().unwrap();
    assert_eq!(maps.len(), 4);
    // The texts are the ones from before the change.
    assert!(
        maps[0]["data"]["server.conf"]
            .as_str()
            .unwrap()
            .contains("trust_domain = \"example.org\"")
    );
    let workloads = backup["workloads"].as_array().unwrap();
    assert_eq!(workloads.len(), 3, "server set, agents, provider");
    assert_eq!(workloads[0]["spec"]["replicas"], 1);
    assert_eq!(backup["openbao_roles"][0]["name"], "agent-reviewer");
    assert_eq!(
        backup["openbao_roles"][0]["data"]["bound_subject"],
        "spiffe://example.org/ns/reviewer/sa/reviewer"
    );
    assert!(run.out.contains("step 4: backup"), "{}", run.out);
}

#[tokio::test]
async fn nothing_is_changed_when_the_backup_cannot_be_written() {
    let world = world().await;
    let dir = TestDir::new("trust-domain-no-backup");
    let before = marks(&world);
    let missing = dir.join("no-such-dir/backup.json");
    let run = world
        .run(
            &[
                "trust-domain",
                "set",
                NEW,
                "--apply",
                "--yes",
                "--backup",
                missing.to_str().unwrap(),
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 1);
    assert!(
        run.err.contains("cannot write the backup") && run.err.contains("nothing was changed"),
        "{}",
        run.err
    );
    assert_untouched(&world, &before);

    // An existing file is never overwritten.
    let existing = dir.join("existing.json");
    std::fs::write(&existing, "keep").unwrap();
    let run = world
        .run(
            &[
                "trust-domain",
                "set",
                NEW,
                "--apply",
                "--yes",
                "--backup",
                existing.to_str().unwrap(),
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 1);
    assert_eq!(std::fs::read_to_string(&existing).unwrap(), "keep");
    assert_untouched(&world, &before);
}

#[tokio::test]
async fn without_yes_it_asks_and_without_input_it_cancels() {
    let world = world().await;
    let dir = TestDir::new("trust-domain-confirm");
    let backup = dir.join("backup.json");
    let args = [
        "trust-domain",
        "set",
        NEW,
        "--apply",
        "--backup",
        backup.to_str().unwrap(),
        "--timeout",
        "1",
    ];
    let before = marks(&world);
    let run = world.run(&args, "").await;
    assert_eq!(run.code, 1);
    assert!(
        run.out.contains("Proceed? [y/N]") && run.err.contains("cancelled, nothing was changed"),
        "{}",
        run.all()
    );
    assert_untouched(&world, &before);
    assert!(!backup.exists(), "no backup for a cancelled run");

    let run = world.run(&args, "n\n").await;
    assert_eq!(run.code, 1);
    assert_untouched(&world, &before);

    let run = world.run(&args, "y\n").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_eq!(
        role_subject(&world),
        "spiffe://agentswork.de/ns/reviewer/sa/reviewer"
    );
}

#[tokio::test]
async fn consumers_are_rewritten_by_the_boundary_rule_and_restarted_last() {
    let world = world().await;
    seed_scion(&world.fake);
    let dir = TestDir::new("trust-domain-apply-consumers");
    let file = dir.join("consumers.toml");
    std::fs::write(&file, CONSUMERS).unwrap();
    let before = marks(&world);
    let run = apply(&world, &dir, &["--consumers", file.to_str().unwrap()]).await;
    assert_eq!(run.code, 0, "{}", run.all());

    let acl = config_text(&world.fake, SCION, "acl.toml");
    assert!(
        acl.contains("\"spiffe://agentswork.de/ns/reviewer/sa/reviewer\""),
        "{acl}"
    );
    assert!(acl.contains("domain = \"agentswork.de\""), "{acl}");
    assert!(
        acl.contains("host = \"foo.example.org\""),
        "foo.example.org stays: {acl}"
    );
    assert_eq!(
        config_text(&world.fake, SCION, "scion-server.toml"),
        "trust_domain = \"agentswork.de\"\n"
    );
    assert_eq!(
        config_text(&world.fake, SCION, "other.txt"),
        "see spiffe://example.org/x\n",
        "unlisted keys stay"
    );

    let writes: Vec<String> = world.fake.writes().split_off(before.writes);
    let bundle = position(&writes, "/statefulsets/spire-server", 1);
    let configmap = position(&writes, "/configmaps/scion-server", 0);
    let restart = position(&writes, "/deployments/scion-server", 0);
    assert!(bundle < configmap && configmap < restart, "{writes:#?}");
    let restarts = world.sim.lock().unwrap().restarts.clone();
    assert!(
        restarts
            .last()
            .unwrap()
            .contains("/deployments/scion-server"),
        "{restarts:?}"
    );

    let backup = std::fs::read_to_string(dir.join("backup.json")).unwrap();
    assert!(
        backup.contains("scion-server.toml"),
        "the consumer is in the backup"
    );
}

#[tokio::test]
async fn a_server_that_does_not_go_away_is_a_timeout_and_the_run_can_be_repeated() {
    let world = world().await;
    world.sim.lock().unwrap().pods_stay = true;
    let dir = TestDir::new("trust-domain-timeout-pods");
    let run = apply(&world, &dir, &[]).await;
    assert_eq!(run.code, 1);
    assert!(
        run.err
            .contains("step 6 (recreate the SPIRE server's data) failed"),
        "{}",
        run.err
    );
    assert!(
        run.err
            .contains("timed out after 1 s waiting for pod spire-system/spire-server-0 to be gone"),
        "{}",
        run.err
    );
    assert!(run.err.contains("backup.json"), "{}", run.err);
    assert!(dir.join("backup.json").exists());
    // Stopped at 0 replicas, remembering the count; configuration is new, roles are old.
    let set = world.fake.object(SERVER_SET).unwrap();
    assert_eq!(set["spec"]["replicas"], 0);
    assert_eq!(
        set["metadata"]["annotations"]["grauwolf.io/trust-domain-replicas"],
        "1"
    );
    assert!(
        config_text(&world.fake, SERVER_CM, "server.conf")
            .contains("trust_domain = \"agentswork.de\"")
    );
    assert_eq!(
        role_subject(&world),
        "spiffe://example.org/ns/reviewer/sa/reviewer"
    );

    // The plan of the repeated run shows the half-done state.
    let plan = world.run(&["trust-domain", "set", NEW], "").await;
    assert_eq!(plan.code, 0, "{}", plan.all());
    assert!(
        plan.out
            .contains("is scaled to 0 by an earlier run; it comes back to 1 replica(s)"),
        "{}",
        plan.out
    );

    // The pod finally goes away; the same command finishes the job.
    world.sim.lock().unwrap().pods_stay = false;
    world.fake.remove(&format!("{SPIRE}/pods/spire-server-0"));
    let dir = TestDir::new("trust-domain-timeout-pods-2");
    let run = apply(&world, &dir, &[]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_eq!(
        world.fake.object(SERVER_SET).unwrap()["spec"]["replicas"],
        1
    );
    assert_eq!(bundle_domains(&world.fake), NEW);
    assert_eq!(
        role_subject(&world),
        "spiffe://agentswork.de/ns/reviewer/sa/reviewer"
    );
}

#[tokio::test]
async fn a_bundle_that_never_changes_stops_before_the_agents_are_restarted() {
    let world = world().await;
    world.sim.lock().unwrap().bundle_stays = true;
    let dir = TestDir::new("trust-domain-timeout-bundle");
    let bao_before = world.bao_writes();
    let run = apply(&world, &dir, &[]).await;
    assert_eq!(run.code, 1);
    assert!(
        run.err.contains("step 7 (wait for the new bundle) failed"),
        "{}",
        run.err
    );
    assert!(run.err.contains("timed out after 1 s waiting for ConfigMap spire-system/spire-bundle to carry a CA of spiffe://agentswork.de"), "{}", run.err);
    assert!(
        world.sim.lock().unwrap().restarts.is_empty(),
        "the agents must wait for the bundle"
    );
    assert_eq!(world.bao_writes(), bao_before, "OpenBao is untouched");
    assert_eq!(
        role_subject(&world),
        "spiffe://example.org/ns/reviewer/sa/reviewer"
    );
}

#[tokio::test]
async fn a_server_that_never_gets_ready_is_a_timeout() {
    let world = world().await;
    world.sim.lock().unwrap().never_ready = true;
    let dir = TestDir::new("trust-domain-timeout-ready");
    let run = apply(&world, &dir, &[]).await;
    assert_eq!(run.code, 1);
    assert!(
        run.err.contains("step 6")
            && run.err.contains(
                "timed out after 1 s waiting for StatefulSet spire-system/spire-server to be ready"
            ),
        "{}",
        run.err
    );
}

#[tokio::test]
async fn a_server_whose_ca_is_already_new_keeps_its_volume_and_the_rest_is_finished() {
    let world = world().await;
    // Earlier run: configuration of the server and the volume are done, the rest is not.
    world.fake.insert(
        SERVER_CM,
        json!({"data": {"server.conf": server_conf(NEW, "")}}),
    );
    world
        .fake
        .insert(BUNDLE, json!({"data": {"bundle.crt": ca_pem(NEW)}}));
    let dir = TestDir::new("trust-domain-resume");
    let before = marks(&world);
    let run = apply(&world, &dir, &[]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_eq!(
        world.fake.deleted().len(),
        before.deleted,
        "the volume is not recreated again"
    );
    assert!(
        run.out
            .contains("skipped: the server's CA already has the new domain"),
        "{}",
        run.out
    );
    assert!(
        !world.fake.writes()[before.writes..]
            .iter()
            .any(|write| write.ends_with("/statefulsets/spire-server"))
    );
    assert_eq!(
        role_subject(&world),
        "spiffe://agentswork.de/ns/reviewer/sa/reviewer"
    );
    assert!(config_text(&world.fake, AGENT_CM, "agent.conf").contains("agentswork.de"));
    assert_eq!(world.sim.lock().unwrap().restarts.len(), 2);
}

#[tokio::test]
async fn from_finishes_the_consumers_when_nothing_else_names_the_old_domain() {
    let world = world().await;
    seed_scion(&world.fake);
    for (path, key, text) in [
        (SERVER_CM, "server.conf", server_conf(NEW, "")),
        (
            AGENT_CM,
            "agent.conf",
            "agent {\n  trust_domain = \"agentswork.de\"\n}\n".to_owned(),
        ),
        (
            CONTROLLER_CM,
            "controller-manager.yaml",
            "trustDomain: agentswork.de\n".to_owned(),
        ),
        (
            OIDC_CM,
            "oidc-discovery-provider.conf",
            "workload_api {\n  trust_domain = \"agentswork.de\"\n}\n".to_owned(),
        ),
    ] {
        world.fake.insert(path, json!({"data": {key: text}}));
    }
    world
        .fake
        .insert(BUNDLE, json!({"data": {"bundle.crt": ca_pem(NEW)}}));
    world
        .env
        .bao
        .lock()
        .unwrap()
        .roles
        .get_mut("agent-reviewer")
        .unwrap()["bound_subject"] = json!("spiffe://agentswork.de/ns/reviewer/sa/reviewer");
    let dir = TestDir::new("trust-domain-from");
    let file = dir.join("consumers.toml");
    std::fs::write(&file, CONSUMERS).unwrap();
    let run = apply(
        &world,
        &dir,
        &["--consumers", file.to_str().unwrap(), "--from", OLD],
    )
    .await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(config_text(&world.fake, SCION, "scion-server.toml").contains("agentswork.de"));
    assert_eq!(world.fake.deleted().len(), 0);

    // --from must agree with what the cluster holds.
    let run = world
        .run(&["trust-domain", "set", NEW, "--from", "other.test"], "")
        .await;
    assert_eq!(
        run.code,
        0,
        "everything is new, so --from alone changes nothing: {}",
        run.all()
    );
}

#[tokio::test]
async fn from_that_disagrees_with_the_cluster_is_refused() {
    let world = world().await;
    let before = marks(&world);
    let run = world
        .run(&["trust-domain", "set", NEW, "--from", "other.test"], "")
        .await;
    assert_eq!(run.code, 1);
    assert!(
        run.err
            .contains("--from says other.test, but the cluster also holds example.org"),
        "{}",
        run.err
    );
    assert_untouched(&world, &before);
}

#[tokio::test]
async fn the_example_consumers_file_works() {
    let world = world().await;
    seed_scion(&world.fake);
    let example = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../examples/k8-ops/trust-domain-consumers.toml"
    );
    let run = world
        .run(&["trust-domain", "set", NEW, "--consumers", example], "")
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out
            .contains("ConfigMap scion/scion-server, key acl.toml:")
            && run.out.contains("restart Deployment scion/scion-server"),
        "{}",
        run.out
    );
}

// ---- restore -------------------------------------------------------------------------------

/// `trust-domain set NEW --apply` to a backup, then the world is in the new domain.
async fn changed_world() -> (World, TestDir, std::path::PathBuf) {
    let world = world().await;
    let dir = TestDir::new("trust-domain-restore");
    let run = apply(&world, &dir, &[]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    let backup = dir.join("backup.json");
    // A restart within the same second as the last one changes nothing: age the last ones.
    for path in [AGENT_SET, OIDC_SET] {
        let mut object = world.fake.object(path).unwrap();
        object["spec"]["template"]["metadata"]["annotations"]["kubectl.kubernetes.io/restartedAt"] =
            json!("2000-01-01T00:00:00Z");
        world.fake.insert(path, object);
    }
    (world, dir, backup)
}

async fn restore_plan(world: &World, backup: &std::path::Path) -> common::Run {
    world
        .run(&["trust-domain", "restore", backup.to_str().unwrap()], "")
        .await
}

/// `trust-domain restore <backup> --apply --yes --backup <dir>/<name>`.
async fn restore_apply(
    world: &World,
    dir: &TestDir,
    backup: &std::path::Path,
    name: &str,
) -> common::Run {
    let new_backup = dir.join(name);
    world
        .run(
            &[
                "trust-domain",
                "restore",
                backup.to_str().unwrap(),
                "--apply",
                "--yes",
                "--backup",
                new_backup.to_str().unwrap(),
                "--timeout",
                "1",
            ],
            "",
        )
        .await
}

#[tokio::test]
async fn the_restore_plan_shows_the_old_domain_and_writes_nothing() {
    let (world, _dir, backup) = changed_world().await;
    let before = marks(&world);
    let run = restore_plan(&world, &backup).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_untouched(&world, &before);
    let out = run.out;
    for expected in [
        "trust domain restore: agentswork.de -> example.org",
        "ConfigMap spire-system/spire-server, key server.conf:",
        "line 3: - trust_domain = \"agentswork.de\"",
        "+ trust_domain = \"example.org\"",
        "ConfigMap spire-system/spire-controller-manager, key controller-manager.yaml:",
        "- trustDomain: agentswork.de",
        "SPIRE server data (its database and signing keys) is recreated, because its CA does not have the domain example.org",
        "delete PersistentVolumeClaim spire-system/spire-data-spire-server-0 (exists)",
        "carries a CA with the URI SAN spiffe://example.org",
        "DaemonSet spire-system/spire-agent",
        "Deployment spire-oidc/spire-oidc-discovery-provider",
        "auth/jwt/role/agent-reviewer:",
        "bound_subject: \"spiffe://agentswork.de/ns/reviewer/sa/reviewer\" -> \"spiffe://example.org/ns/reviewer/sa/reviewer\"",
        "Not restored:",
        "This was only the plan: nothing was changed.",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
    }
    assert!(!out.contains("not about the trust domain"), "{out}");
}

#[tokio::test]
async fn the_restore_plan_lists_other_changed_lines_separately() {
    let (world, _dir, backup) = changed_world().await;
    let conf = config_text(&world.fake, SERVER_CM, "server.conf").replace(
        "bind_address = \"0.0.0.0\"",
        "bind_address = \"0.0.0.0\"\n  log_level = \"DEBUG\"",
    );
    let mut object = world.fake.object(SERVER_CM).unwrap();
    object["data"]["server.conf"] = json!(conf);
    world.fake.insert(SERVER_CM, object);
    let before = marks(&world);
    let run = restore_plan(&world, &backup).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_untouched(&world, &before);
    assert!(
        run.out
            .contains("changed since the backup and are not about the trust domain")
            && run
                .out
                .contains("line 3: `log_level = \"DEBUG\"` -> (no line)"),
        "{}",
        run.out
    );
}

async fn restore_refused(world: &World, backup: &std::path::Path, reason: &str) {
    let before = marks(world);
    let run = restore_plan(world, backup).await;
    assert_eq!(run.code, 1, "{}", run.all());
    assert!(run.err.contains(reason), "{reason:?} not in {}", run.err);
    assert_untouched(world, &before);
}

#[tokio::test]
async fn a_broken_or_foreign_backup_is_refused() {
    let (world, dir, backup) = changed_world().await;
    let good: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&backup).unwrap()).unwrap();
    let write = |name: &str, content: String| {
        let file = dir.join(name);
        std::fs::write(&file, content).unwrap();
        file
    };
    let mut foreign = good.clone();
    foreign["tool"] = json!("kubectl");
    let mut no_from = good.clone();
    no_from.as_object_mut().unwrap().remove("from");
    let mut bad_from = good.clone();
    bad_from["from"] = json!("spiffe://Example.org/x");
    let mut no_maps = good.clone();
    no_maps.as_object_mut().unwrap().remove("configmaps");
    let mut future = good.clone();
    future["format"] = json!(2);
    for (name, content, reason) in [
        ("garbage.json", "not json".to_owned(), "not valid JSON"),
        ("foreign.json", foreign.to_string(), "written by 'kubectl'"),
        ("no-from.json", no_from.to_string(), "no 'from'"),
        (
            "bad-from.json",
            bad_from.to_string(),
            "not a valid trust domain",
        ),
        (
            "no-maps.json",
            no_maps.to_string(),
            "'configmaps' is missing",
        ),
        ("future.json", future.to_string(), "unknown backup format"),
    ] {
        let file = write(name, content);
        restore_refused(&world, &file, reason).await;
    }
    restore_refused(&world, &dir.join("absent.json"), "cannot read the backup").await;
}

#[tokio::test]
async fn a_configmap_that_is_gone_is_refused_by_name() {
    let (world, _dir, backup) = changed_world().await;
    world.fake.remove(OIDC_CM);
    restore_refused(
        &world,
        &backup,
        "ConfigMap spire-oidc/spire-oidc-discovery-provider of the backup does not exist any more",
    )
    .await;
}

#[tokio::test]
async fn restore_refuses_federation_and_other_datastores_like_set() {
    let (world, _dir, backup) = changed_world().await;
    let conf = server_conf(
        NEW,
        "federation {\n  bundle_endpoint { port = 8443 }\n  federates_with \"other.test\" {}\n}\n",
    );
    world
        .fake
        .insert(SERVER_CM, json!({"data": {"server.conf": conf}}));
    restore_refused(&world, &backup, "contains federation").await;

    let conf = server_conf(NEW, "").replace("sqlite3", "postgres");
    world
        .fake
        .insert(SERVER_CM, json!({"data": {"server.conf": conf}}));
    restore_refused(
        &world,
        &backup,
        "uses the datastore 'postgres', not sqlite3",
    )
    .await;
}

#[tokio::test]
async fn the_role_of_a_removed_agent_is_skipped() {
    let world = world().await;
    assert_eq!(world.run(&["agent", "add", "gone"], "").await.code, 0);
    let dir = TestDir::new("trust-domain-restore-gone");
    let run = apply(&world, &dir, &[]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_eq!(
        world
            .run(&["agent", "remove", "gone", "--yes"], "")
            .await
            .code,
        0
    );
    let backup = dir.join("backup.json");
    let plan = restore_plan(&world, &backup).await;
    assert_eq!(plan.code, 0, "{}", plan.all());
    assert!(
        plan.out
            .contains("skipped: OpenBao role auth/jwt/role/agent-gone: its agent no longer exists")
            && plan.out.contains("auth/jwt/role/agent-reviewer:"),
        "{}",
        plan.out
    );
    let run = restore_apply(&world, &dir, &backup, "undo.json").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        !world.bao.lock().unwrap().roles.contains_key("agent-gone"),
        "a removed agent does not get its role back"
    );
    assert_eq!(
        role_subject(&world),
        "spiffe://example.org/ns/reviewer/sa/reviewer"
    );
}

#[tokio::test]
async fn restore_writes_the_backup_first_and_nothing_if_it_cannot() {
    let (world, dir, backup) = changed_world().await;
    let before = marks(&world);
    let missing = dir.join("no-such-dir/undo.json");
    let run = world
        .run(
            &[
                "trust-domain",
                "restore",
                backup.to_str().unwrap(),
                "--apply",
                "--yes",
                "--backup",
                missing.to_str().unwrap(),
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 1);
    assert!(
        run.err.contains("cannot write the backup") && run.err.contains("nothing was changed"),
        "{}",
        run.err
    );
    assert_untouched(&world, &before);

    let run = restore_apply(&world, &dir, &backup, "undo.json").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out.find("step 4: backup").unwrap() < run.out.find("step 5").unwrap(),
        "{}",
        run.out
    );
    let undo: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("undo.json")).unwrap()).unwrap();
    assert_eq!(undo["tool"], "grauwolf-k8-ops trust-domain restore");
    assert_eq!(
        (undo["from"].as_str(), undo["to"].as_str()),
        (Some(NEW), Some(OLD))
    );
    assert!(
        undo["configmaps"][0]["data"]["server.conf"]
            .as_str()
            .unwrap()
            .contains("trust_domain = \"agentswork.de\"")
    );
    assert_eq!(
        undo["openbao_roles"][0]["data"]["bound_subject"],
        "spiffe://agentswork.de/ns/reviewer/sa/reviewer"
    );
}

#[tokio::test]
async fn restore_round_trips_in_order_and_a_restore_of_it_gives_the_new_domain_again() {
    let (world, dir, backup) = changed_world().await;
    let original_server = server_conf(OLD, "");
    let restarts_before = world.sim.lock().unwrap().restarts.len();
    let before = marks(&world);
    let run = restore_apply(&world, &dir, &backup, "undo.json").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out
            .contains("done: the trust domain is example.org in all 6 place(s) read"),
        "{}",
        run.out
    );
    assert_eq!(
        config_text(&world.fake, SERVER_CM, "server.conf"),
        original_server
    );
    assert!(
        config_text(&world.fake, CONTROLLER_CM, "controller-manager.yaml")
            .starts_with("trustDomain: example.org\n")
    );
    assert_eq!(bundle_domains(&world.fake), OLD);
    assert_eq!(
        role_subject(&world),
        "spiffe://example.org/ns/reviewer/sa/reviewer"
    );
    let role = world.bao.lock().unwrap().roles["agent-reviewer"].clone();
    assert_eq!(role["token_policies"], json!(["agent-reviewer"]));

    // The order: ConfigMaps, scale down, scale up, agents, provider.
    let writes: Vec<String> = world.fake.writes().split_off(before.writes);
    let order = [
        position(&writes, "/configmaps/spire-server", 0),
        position(&writes, "/configmaps/spire-oidc-discovery-provider", 0),
        position(&writes, "/statefulsets/spire-server", 0),
        position(&writes, "/statefulsets/spire-server", 1),
        position(&writes, "/daemonsets/spire-agent", 0),
        position(&writes, "/deployments/spire-oidc-discovery-provider", 0),
    ];
    assert!(
        order.windows(2).all(|pair| pair[0] < pair[1]),
        "{order:?} in {writes:#?}"
    );
    assert_eq!(world.fake.deleted()[before.deleted..], [CLAIM]);
    // The OpenBao role comes last, after the restarts.
    assert_eq!(
        world
            .bao
            .lock()
            .unwrap()
            .writes
            .last()
            .map(String::as_str)
            .map(|w| w.contains("agent-reviewer")),
        Some(true)
    );

    // Agents and provider restarted only after the bundle had the old domain's CA.
    let restarts = world
        .sim
        .lock()
        .unwrap()
        .restarts
        .split_off(restarts_before);
    assert_eq!(restarts.len(), 2, "{restarts:?}");
    assert!(
        restarts
            .iter()
            .all(|restart| restart.ends_with("bundle=example.org")),
        "{restarts:?}"
    );

    // The doctor agrees.
    let doctor = world.run(&["doctor", "--json"], "").await;
    let checks: Vec<serde_json::Value> = serde_json::from_str(&doctor.out).unwrap();
    let check = checks
        .iter()
        .find(|check| check["check"] == "trust domain")
        .unwrap();
    assert_eq!(check["status"], "ok", "{check}");

    // Restoring the backup of the restore gives the new domain again.
    let again = restore_apply(&world, &dir, &dir.join("undo.json"), "redo.json").await;
    assert_eq!(again.code, 0, "{}", again.all());
    assert_eq!(bundle_domains(&world.fake), NEW);
    assert!(
        config_text(&world.fake, SERVER_CM, "server.conf")
            .contains("trust_domain = \"agentswork.de\"")
    );
    assert_eq!(
        role_subject(&world),
        "spiffe://agentswork.de/ns/reviewer/sa/reviewer"
    );
}

#[tokio::test]
async fn a_restore_that_has_nothing_to_do_says_so_and_writes_no_backup() {
    let (world, dir, backup) = changed_world().await;
    let run = restore_apply(&world, &dir, &backup, "undo.json").await;
    assert_eq!(run.code, 0, "{}", run.all());
    let before = marks(&world);
    let run = restore_apply(&world, &dir, &backup, "noop.json").await;
    assert_eq!(
        (run.code, run.out.trim()),
        (0, "already as in the backup, nothing to do")
    );
    assert_untouched(&world, &before);
    assert!(!dir.join("noop.json").exists(), "no backup for a no-op");
}

#[tokio::test]
async fn an_interrupted_restore_is_finished_by_running_it_again() {
    let (world, dir, backup) = changed_world().await;
    world.sim.lock().unwrap().pods_stay = true;
    let run = restore_apply(&world, &dir, &backup, "undo.json").await;
    assert_eq!(run.code, 1);
    assert!(
        run.err
            .contains("step 6 (recreate the SPIRE server's data) failed")
            && run.err.contains("restoring from:")
            && run.err.contains("backup.json")
            && run.err.contains("undo.json"),
        "{}",
        run.err
    );
    assert!(
        config_text(&world.fake, SERVER_CM, "server.conf")
            .contains("trust_domain = \"example.org\"")
    );
    assert_eq!(
        role_subject(&world),
        "spiffe://agentswork.de/ns/reviewer/sa/reviewer"
    );

    world.sim.lock().unwrap().pods_stay = false;
    world.fake.remove(&format!("{SPIRE}/pods/spire-server-0"));
    let plan = restore_plan(&world, &backup).await;
    assert!(
        plan.out
            .contains("is scaled to 0 by an earlier run; it comes back to 1 replica(s)"),
        "{}",
        plan.out
    );
    let run = restore_apply(&world, &dir, &backup, "undo-2.json").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_eq!(bundle_domains(&world.fake), OLD);
    assert_eq!(
        role_subject(&world),
        "spiffe://example.org/ns/reviewer/sa/reviewer"
    );
    let again = restore_plan(&world, &backup).await;
    assert_eq!(again.out.trim(), "already as in the backup, nothing to do");
}

#[tokio::test]
async fn restore_puts_the_consumers_back_and_restarts_their_workloads_last() {
    let world = world().await;
    seed_scion(&world.fake);
    for (path, name) in [(SCION, "scion-server"), (SCION_DEPLOYMENT, "scion-server")] {
        let mut object = world.fake.object(path).unwrap();
        object["metadata"]["name"] = json!(name);
        object["metadata"]["namespace"] = json!("scion");
        world.fake.insert(path, object);
    }
    let dir = TestDir::new("trust-domain-restore-consumers");
    let file = dir.join("consumers.toml");
    std::fs::write(&file, CONSUMERS).unwrap();
    let run = apply(&world, &dir, &["--consumers", file.to_str().unwrap()]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    let backup = dir.join("backup.json");

    let plan = restore_plan(&world, &backup).await;
    assert!(
        plan.out
            .contains("ConfigMap scion/scion-server, key acl.toml:")
            && plan.out.contains("Deployment scion/scion-server"),
        "{}",
        plan.out
    );
    let restarts_before = world.sim.lock().unwrap().restarts.len();
    let run = restore_apply(&world, &dir, &backup, "undo.json").await;
    assert_eq!(run.code, 0, "{}", run.all());
    let acl = config_text(&world.fake, SCION, "acl.toml");
    assert!(
        acl.contains("\"spiffe://example.org/ns/reviewer/sa/reviewer\""),
        "{acl}"
    );
    assert!(acl.contains("host = \"foo.example.org\""), "{acl}");
    assert_eq!(
        config_text(&world.fake, SCION, "scion-server.toml"),
        "trust_domain = \"example.org\"\n"
    );
    let restarts = world
        .sim
        .lock()
        .unwrap()
        .restarts
        .split_off(restarts_before);
    assert!(
        restarts
            .last()
            .unwrap()
            .contains("/deployments/scion-server")
            && restarts.last().unwrap().ends_with("bundle=example.org"),
        "{restarts:?}"
    );
}
