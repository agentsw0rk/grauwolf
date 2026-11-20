//! `trust-domain set <domain>` (ADR-0120 decision 4): reads every place that holds the trust
//! domain, refuses what cannot simply be changed, and prints the plan: each object with the
//! line-level change, the volume to recreate, the restarts, the OpenBao roles.

use std::collections::BTreeSet;
use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use grauwolf_k8s::{Cluster, Kind, kinds};
use serde_json::{Value, json};

use crate::agent;
use crate::cli::TrustDomainCommand;
use crate::consumers::{self, Consumers};
use crate::hcl::{
    self, LineChange, Syntax, block, has_federation, rewrite_trust_domain, string_assignment,
};
use crate::io::Io;
use crate::services::Services;
use crate::spire::{self, AGENT_CONFIG, CONTROLLER_CONFIG, OIDC_CONFIG, OIDC_NAME, SERVER_CONFIG};
use crate::x509;

pub async fn run(
    services: &Services,
    io: &mut Io<'_>,
    command: TrustDomainCommand,
) -> Result<i32, String> {
    match command {
        TrustDomainCommand::Set {
            domain,
            from,
            consumers,
            apply,
            yes,
            backup,
            timeout,
        } => {
            let options = Options {
                apply,
                yes,
                backup,
                timeout: Duration::from_secs(timeout),
            };
            set(services, io, &domain, from, consumers, options).await
        }
        TrustDomainCommand::Restore {
            file,
            apply,
            yes,
            backup,
            timeout,
        } => {
            let options = Options {
                apply,
                yes,
                backup,
                timeout: Duration::from_secs(timeout),
            };
            crate::restore::run(services, io, &file, options).await
        }
    }
}

/// A SPIFFE trust domain as this tool accepts it: lowercase letters, digits, `.`, `-`, `_`.
pub fn validate_domain(domain: &str) -> Result<(), String> {
    let valid = !domain.is_empty()
        && domain.len() <= 255
        && domain
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'));
    if valid {
        Ok(())
    } else {
        Err(format!(
            "'{domain}' is not a valid trust domain: use lowercase letters, digits, '.', '-' and '_' (at most 255), without 'spiffe://' or a path"
        ))
    }
}

// ---- the plan ------------------------------------------------------------------------------

/// The new text of some keys of one ConfigMap.
pub struct ConfigChange {
    pub namespace: String,
    pub name: String,
    /// The whole object as it is now (for the backup).
    pub object: Value,
    pub keys: Vec<KeyChange>,
}

pub struct KeyChange {
    pub key: String,
    pub new_text: String,
    pub changes: Vec<LineChange>,
}

/// A workload that is restarted (or, for the SPIRE server, scaled).
pub struct Workload {
    pub kind: Kind,
    pub namespace: String,
    pub name: String,
    pub object: Value,
}

impl Workload {
    pub fn label(&self) -> String {
        format!("{} {}/{}", self.kind.kind, self.namespace, self.name)
    }
}

pub struct RoleChange {
    pub mount: String,
    pub name: String,
    /// The role as OpenBao returns it now.
    pub data: Value,
    pub old_subject: String,
    pub new_subject: String,
}

/// Where the k8sbundle notifier publishes the server's CA.
#[derive(Clone)]
pub struct BundleRef {
    pub namespace: String,
    pub name: String,
    pub key: String,
}

pub struct PersistentClaim {
    pub name: String,
    pub exists: bool,
}

/// The SPIRE server's data is recreated: scale to 0, delete the claims, scale back.
pub struct Recreate {
    pub server: Workload,
    /// The replica count to come back to.
    pub replicas: u64,
    /// A previous run stopped at 0 replicas.
    pub scaled_down: bool,
    pub claims: Vec<PersistentClaim>,
}

pub struct Plan {
    pub old: String,
    pub new: String,
    /// Where the trust domain is now: `(place, domain)`.
    pub places: Vec<(String, String)>,
    /// Places that already carry the new domain, and parts that were not found.
    pub settled: Vec<String>,
    pub configs: Vec<ConfigChange>,
    pub leftovers: Vec<String>,
    pub bundle: BundleRef,
    pub recreate: Option<Recreate>,
    /// Restarted after the server has a CA of the new domain.
    pub restarts: Vec<Workload>,
    pub roles: Vec<RoleChange>,
    pub consumer_configs: Vec<ConfigChange>,
    pub consumer_restarts: Vec<Workload>,
    pub consumer_notes: Vec<String>,
}

/// What to do about the request.
pub enum Outcome {
    /// Everything holds the new domain already.
    Done {
        note: Option<String>,
    },
    Plan(Box<Plan>),
}

pub(crate) struct Read {
    pub label: String,
    pub namespace: String,
    pub name: String,
    pub key: String,
    pub syntax: Syntax,
    pub object: Value,
    pub text: String,
    pub domain: Option<String>,
}

pub(crate) fn describe_config(namespace: &str, name: &str, key: &str) -> String {
    format!("ConfigMap {namespace}/{name}, key {key}")
}

pub(crate) async fn get(
    cluster: &Cluster,
    kind: &Kind,
    namespace: &str,
    name: &str,
) -> Result<Option<Value>, String> {
    cluster
        .get(kind, Some(namespace), name)
        .await
        .map_err(|error| format!("{} {namespace}/{name}: {error}", kind.kind))
}

/// The SPIRE configuration texts; the server's is required, the others are skipped when absent.
pub(crate) async fn read_configs(
    services: &Services,
    settings: &crate::cli::Settings,
    notes: &mut Vec<String>,
) -> Result<Vec<Read>, String> {
    let spire_ns = &settings.spire_namespace;
    let mut reads = Vec::new();
    for (label, namespace, (name, key), syntax, required) in [
        ("SPIRE server", spire_ns, SERVER_CONFIG, Syntax::Hcl, true),
        ("SPIRE agent", spire_ns, AGENT_CONFIG, Syntax::Hcl, false),
        (
            "spire-controller-manager",
            spire_ns,
            CONTROLLER_CONFIG,
            Syntax::Yaml,
            false,
        ),
        (
            "OIDC discovery provider",
            &settings.oidc_namespace,
            OIDC_CONFIG,
            Syntax::Hcl,
            false,
        ),
    ] {
        let Some(object) = get(&services.cluster, &kinds::CONFIG_MAP, namespace, name).await?
        else {
            if required {
                return Err(format!(
                    "ConfigMap {namespace}/{name} not found: SPIRE is not installed in namespace {namespace} (grauwolf-k8-ops does not install SPIRE)"
                ));
            }
            notes.push(format!(
                "{label}: ConfigMap {namespace}/{name} not found, skipped"
            ));
            continue;
        };
        let Some(text) = object["data"][key].as_str().map(str::to_owned) else {
            return Err(format!(
                "ConfigMap {namespace}/{name} has no key {key} ({label})"
            ));
        };
        let domain = match syntax {
            Syntax::Hcl => hcl::trust_domain_of_hcl(&text),
            Syntax::Yaml => hcl::trust_domain_of_yaml(&text),
        };
        if required && domain.is_none() {
            return Err(format!(
                "no trust_domain in {namespace}/{name} ({key}) of the SPIRE server"
            ));
        }
        reads.push(Read {
            label: label.to_owned(),
            namespace: namespace.clone(),
            name: name.to_owned(),
            key: key.to_owned(),
            syntax,
            object,
            text,
            domain,
        });
    }
    Ok(reads)
}

/// The bundle ConfigMap of the `Notifier "k8sbundle"` of the server configuration.
pub(crate) fn bundle_ref(server_conf: &str, spire_ns: &str) -> BundleRef {
    let notifier = block(server_conf, "Notifier", "k8sbundle").unwrap_or_default();
    let value = |key: &str, default: &str| {
        string_assignment(&notifier, key).unwrap_or_else(|| default.to_owned())
    };
    BundleRef {
        namespace: value("namespace", spire_ns),
        name: value("config_map", "spire-bundle"),
        key: value("config_map_key", "bundle.crt"),
    }
}

/// Which volume claim templates of the StatefulSet hold the server's data (the SQLite file and
/// the keys of the `disk` key manager), or why they cannot simply be recreated.
pub fn data_claims(server_conf: &str, statefulset: &Value) -> Result<Vec<String>, String> {
    let store = block(server_conf, "DataStore", "sql").ok_or(
        "the server configuration has no DataStore \"sql\" block: cannot tell where its data lives",
    )?;
    let database = string_assignment(&store, "database_type").unwrap_or_default();
    if database != "sqlite3" {
        return Err(format!(
            "the SPIRE server uses the datastore '{}', not sqlite3: its data is not on the server's own volume and cannot simply be recreated",
            if database.is_empty() {
                "(not set)"
            } else {
                &database
            }
        ));
    }
    let connection = string_assignment(&store, "connection_string")
        .ok_or("the DataStore \"sql\" block has no connection_string")?;
    let file = connection
        .strip_prefix("file:")
        .unwrap_or(&connection)
        .split('?')
        .next()
        .unwrap_or("");
    if !file.starts_with('/') {
        return Err(format!(
            "the SQLite connection_string '{connection}' is not a file path"
        ));
    }
    let mut paths = vec![("the SQLite file", file.to_owned())];
    match block(server_conf, "KeyManager", "disk") {
        Some(keys) => paths.push((
            "the keys of the KeyManager \"disk\"",
            string_assignment(&keys, "keys_path")
                .ok_or("the KeyManager \"disk\" block has no keys_path")?,
        )),
        None if block(server_conf, "KeyManager", "memory").is_none() => {
            // The default key manager keeps its keys in the data dir.
            if let Some(dir) = string_assignment(server_conf, "data_dir") {
                paths.push(("the server's keys", format!("{dir}/keys.json")));
            }
        }
        None => {}
    }
    let pod = &statefulset["spec"]["template"]["spec"];
    let containers = pod["containers"].as_array().cloned().unwrap_or_default();
    let container = containers
        .iter()
        .find(|c| {
            c["name"] == "spire-server"
                || c["image"]
                    .as_str()
                    .is_some_and(|image| image.contains("spire-server"))
        })
        .ok_or("the StatefulSet has no spire-server container")?;
    let templates: Vec<&str> = statefulset["spec"]["volumeClaimTemplates"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|template| template["metadata"]["name"].as_str())
        .collect();
    let mut claims = BTreeSet::new();
    for (what, path) in paths {
        let mount = container["volumeMounts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|mount| Some((mount["mountPath"].as_str()?, mount["name"].as_str()?)))
            .filter(|(mount_path, _)| {
                let dir = mount_path.trim_end_matches('/');
                path == *mount_path || path.starts_with(&format!("{dir}/"))
            })
            .max_by_key(|(mount_path, _)| mount_path.len());
        let Some((mount_path, volume)) = mount else {
            return Err(format!(
                "{what} ({path}) is not on a mounted volume of the spire-server container: it cannot simply be recreated"
            ));
        };
        if !templates.contains(&volume) {
            return Err(format!(
                "{what} ({path}) lies on the volume '{volume}' (mounted at {mount_path}), which is not a volume claim template of the StatefulSet: its data cannot simply be recreated"
            ));
        }
        claims.insert(volume.to_owned());
    }
    Ok(claims.into_iter().collect())
}

/// The new text of one ConfigMap key, if it changes.
fn key_change(key: &str, text: &str, old: &str, new: &str) -> Option<KeyChange> {
    let new_text = consumers::replace_domain(text, old, new);
    if new_text == text {
        return None;
    }
    let changes = text
        .split('\n')
        .zip(new_text.split('\n'))
        .enumerate()
        .filter(|(_, (before, after))| before != after)
        .map(|(index, (before, after))| LineChange {
            line: index + 1,
            old: before.trim().to_owned(),
            new: after.trim().to_owned(),
        })
        .collect();
    Some(KeyChange {
        key: key.to_owned(),
        new_text,
        changes,
    })
}

async fn consumer_changes(
    cluster: &Cluster,
    consumers: &Consumers,
    old: &str,
    new: &str,
    plan: &mut Plan,
) -> Result<(), String> {
    for reference in &consumers.config_maps {
        let (namespace, name) = (&reference.namespace, &reference.name);
        let object = get(cluster, &kinds::CONFIG_MAP, namespace, name)
            .await?
            .ok_or_else(|| format!("consumer ConfigMap {namespace}/{name} not found"))?;
        let data = object["data"].as_object().cloned().unwrap_or_default();
        let wanted: Vec<String> = match &reference.keys {
            Some(keys) => {
                if let Some(missing) = keys.iter().find(|key| !data.contains_key(*key)) {
                    return Err(format!(
                        "consumer ConfigMap {namespace}/{name} has no key {missing}"
                    ));
                }
                keys.clone()
            }
            None => data.keys().cloned().collect(),
        };
        let mut keys = Vec::new();
        for key in wanted {
            let text = data[&key].as_str().unwrap_or("");
            if let Some(change) = key_change(&key, text, old, new) {
                keys.push(change);
            }
            for (line, content) in
                consumers::mentions(&consumers::replace_domain(text, old, new), old, new)
            {
                plan.leftovers.push(format!(
                    "{}, line {line}: {content}",
                    describe_config(namespace, name, &key)
                ));
            }
        }
        if keys.is_empty() {
            plan.consumer_notes.push(format!(
                "ConfigMap {namespace}/{name}: no spiffe://{old} ID or quoted \"{old}\" in it, nothing to rewrite"
            ));
        } else {
            plan.consumer_configs.push(ConfigChange {
                namespace: namespace.clone(),
                name: name.clone(),
                object,
                keys,
            });
        }
    }
    for restart in &consumers.restarts {
        let object = get(cluster, &restart.kind, &restart.namespace, &restart.name)
            .await?
            .ok_or_else(|| {
                format!(
                    "consumer {} {}/{} not found",
                    restart.kind.kind, restart.namespace, restart.name
                )
            })?;
        plan.consumer_restarts.push(Workload {
            kind: restart.kind,
            namespace: restart.namespace.clone(),
            name: restart.name.clone(),
            object,
        });
    }
    Ok(())
}

/// The server's data is recreated because its CA does not have the domain `wanted`: refuses what
/// cannot simply be recreated (judged by every text of the server configuration in `texts`) and
/// finds the claims to delete.
pub(crate) async fn plan_recreate(
    services: &Services,
    texts: &[&str],
    bundle: &BundleRef,
    bundle_exists: bool,
    wanted: &str,
    server_set: Value,
) -> Result<Recreate, String> {
    let spire_ns = &services.settings.spire_namespace;
    let cluster = &services.cluster;
    if !bundle_exists {
        return Err(format!(
            "ConfigMap {}/{} (the bundle of the k8sbundle notifier) not found: cannot tell when the server has a CA of {wanted}",
            bundle.namespace, bundle.name
        ));
    }
    let mut templates = Vec::new();
    for text in texts {
        for template in data_claims(text, &server_set)? {
            if !templates.contains(&template) {
                templates.push(template);
            }
        }
    }
    let annotations = &server_set["metadata"]["annotations"];
    let replicas_now = server_set["spec"]["replicas"].as_u64().unwrap_or(1);
    let scaled_down = replicas_now == 0;
    let replicas = if scaled_down {
        annotations[REPLICAS_ANNOTATION]
            .as_str()
            .and_then(|text| text.parse::<u64>().ok())
            .filter(|count| *count > 0)
            .ok_or_else(|| format!(
                "StatefulSet {spire_ns}/spire-server is scaled to 0 and does not say how many replicas it had: scale it back by hand (`kubectl scale statefulset spire-server -n {spire_ns} --replicas=1`) and run the command again"
            ))?
    } else {
        replicas_now
    };
    let mut claims = Vec::new();
    for template in &templates {
        for ordinal in 0..replicas {
            let name = format!("{template}-spire-server-{ordinal}");
            let exists = get(cluster, &kinds::PERSISTENT_VOLUME_CLAIM, spire_ns, &name)
                .await?
                .is_some();
            claims.push(PersistentClaim { name, exists });
        }
    }
    Ok(Recreate {
        server: Workload {
            kind: kinds::STATEFUL_SET,
            namespace: spire_ns.clone(),
            name: "spire-server".to_owned(),
            object: server_set,
        },
        replicas,
        scaled_down,
        claims,
    })
}

/// The SPIRE workloads restarted once the server has the wanted CA: the agents, the
/// controller-manager and the OIDC provider (the last two if they exist).
pub(crate) async fn spire_restarts(services: &Services) -> Result<Vec<Workload>, String> {
    let cluster = &services.cluster;
    let spire_ns = &services.settings.spire_namespace;
    let mut restarts = Vec::new();
    let agents_set = get(cluster, &kinds::DAEMON_SET, spire_ns, "spire-agent")
        .await?
        .ok_or_else(|| {
            format!("DaemonSet {spire_ns}/spire-agent not found: the agents cannot be restarted")
        })?;
    restarts.push(Workload {
        kind: kinds::DAEMON_SET,
        namespace: spire_ns.clone(),
        name: "spire-agent".to_owned(),
        object: agents_set,
    });
    for (namespace, name) in [
        (spire_ns.as_str(), "spire-controller-manager"),
        (services.settings.oidc_namespace.as_str(), OIDC_NAME),
    ] {
        if let Some(object) = get(cluster, &kinds::DEPLOYMENT, namespace, name).await? {
            restarts.push(Workload {
                kind: kinds::DEPLOYMENT,
                namespace: namespace.to_owned(),
                name: name.to_owned(),
                object,
            });
        }
    }
    Ok(restarts)
}

/// Reads the cluster and OpenBao and decides what has to change. Changes nothing.
pub async fn make_plan(
    services: &Services,
    new: &str,
    from: Option<&str>,
    consumers: Option<&Consumers>,
) -> Result<Outcome, String> {
    let settings = &services.settings;
    let cluster = &services.cluster;
    let spire_ns = &settings.spire_namespace;
    let mut settled = Vec::new();
    let reads = read_configs(services, settings, &mut settled).await?;
    let server = &reads[0];
    let bundle = bundle_ref(&server.text, spire_ns);

    // Every place that names a domain.
    let mut places: Vec<(String, String)> = Vec::new();
    for read in &reads {
        match &read.domain {
            Some(domain) => places.push((
                format!(
                    "{} ({})",
                    read.label,
                    describe_config(&read.namespace, &read.name, &read.key)
                ),
                domain.clone(),
            )),
            None => settled.push(format!(
                "{}: no trust domain in {}, left alone",
                read.label,
                describe_config(&read.namespace, &read.name, &read.key)
            )),
        }
    }
    let bundle_object = get(cluster, &kinds::CONFIG_MAP, &bundle.namespace, &bundle.name).await?;
    let bundle_domains = bundle_object
        .as_ref()
        .and_then(|object| object["data"][bundle.key.as_str()].as_str())
        .map(x509::spiffe_trust_domains)
        .unwrap_or_default();
    for domain in &bundle_domains {
        places.push((
            format!(
                "CA of the SPIRE server ({})",
                describe_config(&bundle.namespace, &bundle.name, &bundle.key)
            ),
            domain.clone(),
        ));
    }
    let agents = agent::agents(cluster).await?;
    let mut roles = Vec::new();
    if !agents.is_empty() {
        let bao = services.admin()?;
        for (agent, namespace) in &agents {
            let role = agent::role_name(agent);
            let data = bao
                .read_jwt_role(&settings.jwt_mount, &role)
                .await
                .map_err(|error| format!("OpenBao role {role}: {error}"))?;
            let Some(data) = data else {
                settled.push(format!(
                    "OpenBao role auth/{}/role/{role} of agent {agent} not found, skipped (`agent add {agent}` creates it for the new domain)",
                    settings.jwt_mount
                ));
                continue;
            };
            let subject = data["bound_subject"].as_str().unwrap_or("").to_owned();
            if let Some(domain) = spire::domain_of_id(&subject) {
                places.push((
                    format!("OpenBao role auth/{}/role/{role}", settings.jwt_mount),
                    domain.to_owned(),
                ));
            }
            roles.push((agent.clone(), namespace.clone(), role, data, subject));
        }
    }

    // The domain to replace: the one other domain the cluster holds.
    let others: BTreeSet<&str> = places
        .iter()
        .map(|(_, domain)| domain.as_str())
        .filter(|domain| *domain != new)
        .collect();
    if let Some(from) = from {
        if from == new {
            return Err("--from is the new domain: nothing to replace".to_owned());
        }
        if let Some(found) = others.iter().find(|found| **found != from) {
            return Err(format!(
                "--from says {from}, but the cluster also holds {found}: refusing to guess"
            ));
        }
    }
    if others.len() > 1 {
        let listing: Vec<String> = places
            .iter()
            .filter(|(_, domain)| domain != new)
            .map(|(place, domain)| format!("{domain} in {place}"))
            .collect();
        return Err(format!(
            "the cluster holds more than one trust domain besides {new}: {}; make them agree (or finish the interrupted change to the domain you intended) before changing it",
            listing.join("; ")
        ));
    }
    let Some(old) = from
        .map(str::to_owned)
        .or(others.iter().next().map(|d| (*d).to_owned()))
    else {
        let note = consumers.is_some().then(|| {
            "the consumers file was not checked: nothing in the cluster names the old domain; pass --from <old domain> to rewrite them".to_owned()
        });
        return Ok(Outcome::Done { note });
    };

    // Refuse what is out of scope.
    if has_federation(&server.text) {
        return Err("the SPIRE server configuration contains federation (federation { ... } or federates_with): changing a federated trust domain is out of scope".to_owned());
    }

    let mut plan = Plan {
        old: old.clone(),
        new: new.to_owned(),
        places,
        settled,
        configs: Vec::new(),
        leftovers: Vec::new(),
        bundle: bundle.clone(),
        recreate: None,
        restarts: Vec::new(),
        roles: Vec::new(),
        consumer_configs: Vec::new(),
        consumer_restarts: Vec::new(),
        consumer_notes: Vec::new(),
    };

    // Configuration rewrites.
    for read in &reads {
        let describe = describe_config(&read.namespace, &read.name, &read.key);
        match read.domain.as_deref() {
            Some(domain) if domain == new => {
                plan.settled
                    .push(format!("{} ({describe}) is already {new}", read.label));
            }
            Some(_) => {
                let rewrite = rewrite_trust_domain(&read.text, &old, new, read.syntax);
                for (line, content) in consumers::mentions(&rewrite.text, &old, new) {
                    plan.leftovers
                        .push(format!("{describe}, line {line}: {content}"));
                }
                plan.configs.push(ConfigChange {
                    namespace: read.namespace.clone(),
                    name: read.name.clone(),
                    object: read.object.clone(),
                    keys: vec![KeyChange {
                        key: read.key.clone(),
                        new_text: rewrite.text,
                        changes: rewrite.changes,
                    }],
                });
            }
            None => {}
        }
    }

    // The server's data.
    let server_set = get(cluster, &kinds::STATEFUL_SET, spire_ns, "spire-server")
        .await?
        .ok_or_else(|| format!("StatefulSet {spire_ns}/spire-server not found"))?;
    if !bundle_domains.iter().any(|domain| domain == new) {
        plan.recreate = Some(
            plan_recreate(
                services,
                &[&server.text],
                &bundle,
                bundle_object.is_some(),
                new,
                server_set,
            )
            .await?,
        );
    } else {
        plan.settled.push(format!(
            "the server's CA already has the trust domain {new}: its data is not recreated"
        ));
    }

    // Restarts after the new bundle.
    plan.restarts = spire_restarts(services).await?;

    // OpenBao roles.
    for (agent, namespace, role, data, subject) in roles {
        if spire::domain_of_id(&subject) == Some(new) {
            plan.settled.push(format!(
                "OpenBao role auth/{}/role/{role} is already bound to {subject}",
                settings.jwt_mount
            ));
            continue;
        }
        plan.roles.push(RoleChange {
            mount: settings.jwt_mount.clone(),
            name: role,
            data,
            old_subject: subject,
            new_subject: spire::spiffe_id(new, &namespace, &agent),
        });
    }

    if let Some(consumers) = consumers {
        consumer_changes(cluster, consumers, &old, new, &mut plan).await?;
    }
    Ok(Outcome::Plan(Box::new(plan)))
}

/// The annotation that keeps the replica count while the server is scaled to 0.
pub const REPLICAS_ANNOTATION: &str = "grauwolf.io/trust-domain-replicas";

pub(crate) fn print_changes(io: &mut Io<'_>, config: &ConfigChange) {
    for key in &config.keys {
        io.line(&format!(
            "  {}:",
            describe_config(&config.namespace, &config.name, &key.key)
        ));
        for change in &key.changes {
            io.line(&format!("    line {}: - {}", change.line, change.old));
            io.line(&format!(
                "    {} + {}",
                " ".repeat(format!("line {}:", change.line).len()),
                change.new
            ));
        }
    }
}

/// The steps of recreating the server's data, as the plan lists them.
pub(crate) fn print_recreate(io: &mut Io<'_>, recreate: &Recreate) {
    let server = recreate.server.label();
    if recreate.scaled_down {
        io.line(&format!(
            "  {server} is scaled to 0 by an earlier run; it comes back to {} replica(s)",
            recreate.replicas
        ));
    } else {
        io.line(&format!(
            "  scale {server} from {} to 0 and wait until its pod(s) are gone",
            recreate.replicas
        ));
    }
    for claim in &recreate.claims {
        io.line(&format!(
            "  delete PersistentVolumeClaim {}/{} ({})",
            recreate.server.namespace,
            claim.name,
            if claim.exists {
                "exists"
            } else {
                "already gone"
            }
        ));
    }
    io.line(&format!(
        "  scale {server} back to {} and wait until it is ready",
        recreate.replicas
    ));
}

pub fn print_plan(io: &mut Io<'_>, plan: &Plan) {
    let (old, new) = (&plan.old, &plan.new);
    io.line(&format!("trust domain change: {old} -> {new}"));
    io.line("");
    io.line("Where the trust domain is now:");
    for (place, domain) in &plan.places {
        let mark = if domain == new { "  (already new)" } else { "" };
        io.line(&format!("  {place}: {domain}{mark}"));
    }
    io.line("");
    io.line(if plan.configs.is_empty() {
        "Configuration to rewrite: nothing (all already carry the new domain)"
    } else {
        "Configuration to rewrite (only the trust domain on these lines):"
    });
    for config in &plan.configs {
        print_changes(io, config);
    }
    if !plan.leftovers.is_empty() {
        io.line("");
        io.line(&format!(
            "Warning: these lines still mention {old} after the rewrite (not changed, look at them):"
        ));
        for left in &plan.leftovers {
            io.line(&format!("  {left}"));
        }
    }
    io.line("");
    match &plan.recreate {
        Some(recreate) => {
            io.line(&format!(
                "SPIRE server data (its database and signing keys) is recreated, because its CA still has the domain {old}:"
            ));
            print_recreate(io, recreate);
        }
        None => io.line("SPIRE server data: kept (the server's CA already has the new domain)"),
    }
    io.line(&format!(
        "  then wait until ConfigMap {}/{}, key {} carries a CA with the URI SAN spiffe://{new}",
        plan.bundle.namespace, plan.bundle.name, plan.bundle.key
    ));
    io.line("");
    io.line("Restarted after that, one after the other (each waits until it is ready):");
    for workload in &plan.restarts {
        io.line(&format!("  {}", workload.label()));
    }
    io.line("");
    if plan.roles.is_empty() {
        io.line("OpenBao roles: nothing to change");
    } else {
        io.line("OpenBao roles (bound_subject; the other fields stay):");
        for role in &plan.roles {
            io.line(&format!(
                "  auth/{}/role/{}: {} -> {}",
                role.mount, role.name, role.old_subject, role.new_subject
            ));
        }
    }
    if !plan.consumer_configs.is_empty()
        || !plan.consumer_restarts.is_empty()
        || !plan.consumer_notes.is_empty()
    {
        io.line("");
        io.line("Extra consumers (from the consumers file; spiffe://<old> IDs and quoted \"<old>\" values only):");
        for config in &plan.consumer_configs {
            print_changes(io, config);
        }
        for note in &plan.consumer_notes {
            io.line(&format!("  {note}"));
        }
        for workload in &plan.consumer_restarts {
            io.line(&format!("  restart {}", workload.label()));
        }
    }
    if !plan.settled.is_empty() {
        io.line("");
        io.line("Left alone:");
        for line in &plan.settled {
            io.line(&format!("  {line}"));
        }
    }
    io.line("");
    io.line("What this means:");
    io.line(&format!(
        "  - Every SPIFFE ID changes (spiffe://{old}/... becomes spiffe://{new}/...); every workload gets a new SVID."
    ));
    io.line("  - The SPIRE server starts with an empty database and a new CA; the old trust bundle is gone.");
    io.line(&format!(
        "  - Everything that still holds {old} and is not listed above (ACLs, other ConfigMaps, other systems' policies) stops working until it is changed; list such ConfigMaps and workloads in a --consumers file."
    ));
}

pub(crate) struct Options {
    pub apply: bool,
    pub yes: bool,
    pub backup: Option<PathBuf>,
    pub timeout: Duration,
}

async fn set(
    services: &Services,
    io: &mut Io<'_>,
    domain: &str,
    from: Option<String>,
    consumers_file: Option<PathBuf>,
    options: Options,
) -> Result<i32, String> {
    validate_domain(domain)?;
    if let Some(from) = &from {
        validate_domain(from).map_err(|error| format!("--from: {error}"))?;
    }
    let consumers = match &consumers_file {
        Some(file) => {
            let text = std::fs::read_to_string(file)
                .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
            Some(consumers::parse(&text).map_err(|error| format!("{}: {error}", file.display()))?)
        }
        None => None,
    };
    let plan = match make_plan(services, domain, from.as_deref(), consumers.as_ref()).await? {
        Outcome::Done { note } => {
            io.line(&format!("already {domain}, nothing to do"));
            if let Some(note) = note {
                io.line(&note);
            }
            return Ok(0);
        }
        Outcome::Plan(plan) => plan,
    };
    print_plan(io, &plan);
    io.line("");
    if !options.apply {
        io.line("This was only the plan: nothing was changed. Add --apply to do it.");
        return Ok(0);
    }
    if !options.yes && !io.confirm() {
        return Err("cancelled, nothing was changed".to_owned());
    }
    let backup = options.backup.clone().unwrap_or_else(|| {
        PathBuf::from(format!("trust-domain-backup-{}.json", Utc::now().compact()))
    });
    io.line("step 4: backup");
    write_backup(&backup, &plan).map_err(|error| format!("{error} (nothing was changed)"))?;
    io.line(&format!("  written to {}", backup.display()));
    apply(services, io, &plan, options.timeout)
        .await
        .map_err(|error| {
            format!(
                "{error}\nthe objects as they were before are in {}; the cluster may be partly changed: fix the cause and run the same command again, it continues where this one stopped",
                backup.display()
            )
        })?;
    Ok(0)
}

// ---- time ----------------------------------------------------------------------------------

pub(crate) struct Utc {
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
}

impl Utc {
    pub(crate) fn now() -> Self {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0);
        Self::from_unix(seconds)
    }

    /// The civil date of a Unix time (days since 1970-01-01, proleptic Gregorian).
    fn from_unix(seconds: u64) -> Self {
        let days = (seconds / 86_400) as i64 + 719_468;
        let era = days.div_euclid(146_097);
        let day_of_era = days.rem_euclid(146_097);
        let year_of_era =
            (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_index = (5 * day_of_year + 2) / 153;
        let month = if month_index < 10 {
            month_index + 3
        } else {
            month_index - 9
        };
        let rest = seconds % 86_400;
        Self {
            year: year_of_era + era * 400 + i64::from(month <= 2),
            month: month as u32,
            day: (day_of_year - (153 * month_index + 2) / 5 + 1) as u32,
            hour: (rest / 3_600) as u32,
            minute: (rest % 3_600 / 60) as u32,
            second: (rest % 60) as u32,
        }
    }

    /// `20261114T153000Z`.
    pub(crate) fn compact(&self) -> String {
        format!(
            "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// `2026-11-14T15:30:00Z`.
    pub(crate) fn rfc3339(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

// ---- the backup ----------------------------------------------------------------------------

/// Every object the change touches, as it is now, and the OpenBao roles (no secret values: no
/// Secret is read, and the roles hold none).
fn write_backup(file: &std::path::Path, plan: &Plan) -> Result<(), String> {
    let mut config_maps: Vec<&Value> = Vec::new();
    for config in plan.configs.iter().chain(&plan.consumer_configs) {
        config_maps.push(&config.object);
    }
    let mut workloads: Vec<&Value> = Vec::new();
    if let Some(recreate) = &plan.recreate {
        workloads.push(&recreate.server.object);
    }
    for workload in plan.restarts.iter().chain(&plan.consumer_restarts) {
        workloads.push(&workload.object);
    }
    let roles: Vec<Value> = plan
        .roles
        .iter()
        .map(|role| json!({"mount": role.mount, "name": role.name, "data": role.data}))
        .collect();
    write_backup_file(
        file,
        &Backup {
            tool: "grauwolf-k8-ops trust-domain set",
            from: &plan.old,
            to: &plan.new,
            config_maps: &config_maps,
            workloads: &workloads,
            roles,
        },
    )
}

/// The content of a backup file: the objects as they were when it was written. `from` is the
/// trust domain they held, `to` the one the command that wrote the file changes to.
pub(crate) struct Backup<'a> {
    pub tool: &'a str,
    pub from: &'a str,
    pub to: &'a str,
    pub config_maps: &'a [&'a Value],
    pub workloads: &'a [&'a Value],
    pub roles: Vec<Value>,
}

/// The backup format this tool writes (`format`); a file without the key is a `set` backup of
/// stage 2, which is the same format.
pub(crate) const BACKUP_FORMAT: u64 = 1;

/// Writes the file, never over an existing one.
pub(crate) fn write_backup_file(file: &std::path::Path, backup: &Backup<'_>) -> Result<(), String> {
    let backup = json!({
        "tool": backup.tool,
        "format": BACKUP_FORMAT,
        "created": Utc::now().rfc3339(),
        "from": backup.from,
        "to": backup.to,
        "configmaps": backup.config_maps,
        "workloads": backup.workloads,
        "openbao_roles": backup.roles,
    });
    let text = serde_json::to_string_pretty(&backup).map_err(|error| error.to_string())?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut handle = options
        .open(file)
        .map_err(|error| format!("cannot write the backup {}: {error}", file.display()))?;
    std::io::Write::write_all(&mut handle, text.as_bytes())
        .and_then(|()| std::io::Write::write_all(&mut handle, b"\n"))
        .map_err(|error| format!("cannot write the backup {}: {error}", file.display()))
}

// ---- the change ----------------------------------------------------------------------------

const POLL: Duration = Duration::from_millis(500);

/// Polls `check` until it is true; an error of `check` ends the wait at once.
pub(crate) async fn wait_until<F, Fut>(
    timeout: Duration,
    what: &str,
    mut check: F,
) -> Result<(), String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<bool, String>>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if check().await? {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "timed out after {} s waiting for {what}",
                timeout.as_secs()
            ));
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Has the workload's controller finished the last change (restart or scale)?
fn rolled_out(kind: &Kind, workload: &Value) -> bool {
    let number = |pointer: &str| workload.pointer(pointer).and_then(Value::as_u64);
    if let Some(generation) = number("/metadata/generation")
        && number("/status/observedGeneration").unwrap_or(0) < generation
    {
        return false;
    }
    let replicas = number("/spec/replicas").unwrap_or(1);
    let status = |field: &str| number(&format!("/status/{field}")).unwrap_or(0);
    match kind.kind {
        "DaemonSet" => {
            let desired = status("desiredNumberScheduled");
            status("updatedNumberScheduled") == desired && status("numberReady") == desired
        }
        "StatefulSet" => status("readyReplicas") == replicas,
        _ => {
            status("updatedReplicas") == replicas
                && status("readyReplicas") == replicas
                && status("replicas") == replicas
        }
    }
}

async fn wait_rolled_out(
    cluster: &Cluster,
    workload: &Workload,
    timeout: Duration,
) -> Result<(), String> {
    wait_until(
        timeout,
        &format!("{} to be ready", workload.label()),
        || async {
            let object = get(cluster, &workload.kind, &workload.namespace, &workload.name)
                .await?
                .ok_or_else(|| format!("{} disappeared", workload.label()))?;
            Ok(rolled_out(&workload.kind, &object))
        },
    )
    .await
}

pub(crate) async fn restart(
    cluster: &Cluster,
    io: &mut Io<'_>,
    workload: &Workload,
    timeout: Duration,
) -> Result<(), String> {
    let patch = json!({"spec": {"template": {"metadata": {"annotations": {
        "kubectl.kubernetes.io/restartedAt": Utc::now().rfc3339()}}}}});
    cluster
        .patch(
            &workload.kind,
            Some(&workload.namespace),
            &workload.name,
            &patch,
        )
        .await
        .map_err(|error| format!("{}: {error}", workload.label()))?;
    io.line(&format!(
        "  {} restarted, waiting until it is ready",
        workload.label()
    ));
    wait_rolled_out(cluster, workload, timeout).await
}

pub(crate) async fn rewrite_configs(
    cluster: &Cluster,
    io: &mut Io<'_>,
    configs: &[ConfigChange],
) -> Result<(), String> {
    for config in configs {
        let data: serde_json::Map<String, Value> = config
            .keys
            .iter()
            .map(|key| (key.key.clone(), Value::String(key.new_text.clone())))
            .collect();
        cluster
            .patch(
                &kinds::CONFIG_MAP,
                Some(&config.namespace),
                &config.name,
                &json!({ "data": data }),
            )
            .await
            .map_err(|error| format!("ConfigMap {}/{}: {error}", config.namespace, config.name))?;
        let keys: Vec<&str> = config.keys.iter().map(|key| key.key.as_str()).collect();
        io.line(&format!(
            "  ConfigMap {}/{} ({}): rewritten",
            config.namespace,
            config.name,
            keys.join(", ")
        ));
    }
    Ok(())
}

pub(crate) async fn recreate_server(
    cluster: &Cluster,
    io: &mut Io<'_>,
    recreate: &Recreate,
    timeout: Duration,
) -> Result<(), String> {
    let server = &recreate.server;
    let label = server.label();
    let scale = |replicas: u64, memo: Value| json!({"metadata": {"annotations": {REPLICAS_ANNOTATION: memo}}, "spec": {"replicas": replicas}});
    let patch = |body: Value| async move {
        cluster
            .patch(&server.kind, Some(&server.namespace), &server.name, &body)
            .await
            .map_err(|error| format!("{label}: {error}", label = server.label()))
    };
    if !recreate.scaled_down {
        // The memo keeps the replica count if this run stops with the server at 0.
        patch(scale(0, Value::String(recreate.replicas.to_string()))).await?;
        io.line(&format!("  {label} scaled to 0"));
    }
    for ordinal in 0..recreate.replicas {
        let pod = format!("{}-{ordinal}", server.name);
        wait_until(
            timeout,
            &format!("pod {}/{pod} to be gone", server.namespace),
            || async {
                Ok(get(cluster, &kinds::POD, &server.namespace, &pod)
                    .await?
                    .is_none())
            },
        )
        .await?;
    }
    for claim in &recreate.claims {
        cluster
            .delete(
                &kinds::PERSISTENT_VOLUME_CLAIM,
                Some(&server.namespace),
                &claim.name,
            )
            .await
            .map_err(|error| {
                format!(
                    "PersistentVolumeClaim {}/{}: {error}",
                    server.namespace, claim.name
                )
            })?;
        wait_until(
            timeout,
            &format!(
                "PersistentVolumeClaim {}/{} to be gone",
                server.namespace, claim.name
            ),
            || async {
                Ok(get(
                    cluster,
                    &kinds::PERSISTENT_VOLUME_CLAIM,
                    &server.namespace,
                    &claim.name,
                )
                .await?
                .is_none())
            },
        )
        .await?;
        io.line(&format!(
            "  PersistentVolumeClaim {}/{} deleted",
            server.namespace, claim.name
        ));
    }
    patch(scale(recreate.replicas, Value::Null)).await?;
    io.line(&format!(
        "  {label} scaled back to {}, waiting until it is ready",
        recreate.replicas
    ));
    wait_rolled_out(cluster, server, timeout).await
}

/// Has the bundle ConfigMap a CA of the trust domain `new`?
pub(crate) async fn bundle_has(
    cluster: &Cluster,
    bundle: &BundleRef,
    new: &str,
) -> Result<bool, String> {
    let object = get(cluster, &kinds::CONFIG_MAP, &bundle.namespace, &bundle.name).await?;
    Ok(object
        .and_then(|object| {
            object["data"][bundle.key.as_str()]
                .as_str()
                .map(str::to_owned)
        })
        .is_some_and(|text| {
            x509::spiffe_trust_domains(&text)
                .iter()
                .any(|domain| domain == new)
        }))
}

async fn apply(
    services: &Services,
    io: &mut Io<'_>,
    plan: &Plan,
    timeout: Duration,
) -> Result<(), String> {
    let cluster = &services.cluster;
    let new = plan.new.as_str();
    let step =
        |number: u32, what: &str, error: String| format!("step {number} ({what}) failed: {error}");

    io.line("step 5: rewrite the configuration");
    rewrite_configs(cluster, io, &plan.configs)
        .await
        .map_err(|error| step(5, "rewrite the configuration", error))?;
    for left in &plan.leftovers {
        io.line(&format!("  warning: still mentions {}: {left}", plan.old));
    }

    io.line("step 6: recreate the SPIRE server's data");
    match &plan.recreate {
        Some(recreate) => recreate_server(cluster, io, recreate, timeout)
            .await
            .map_err(|error| step(6, "recreate the SPIRE server's data", error))?,
        None => io.line("  skipped: the server's CA already has the new domain"),
    }

    io.line("step 7: wait for the new bundle, then restart the agents");
    wait_until(
        timeout,
        &format!(
            "ConfigMap {}/{} to carry a CA of spiffe://{new}",
            plan.bundle.namespace, plan.bundle.name
        ),
        || bundle_has(cluster, &plan.bundle, new),
    )
    .await
    .map_err(|error| step(7, "wait for the new bundle", error))?;
    io.line(&format!(
        "  ConfigMap {}/{} carries a CA of spiffe://{new}",
        plan.bundle.namespace, plan.bundle.name
    ));
    for workload in &plan.restarts {
        restart(cluster, io, workload, timeout)
            .await
            .map_err(|error| step(7, "restart the agents and the provider", error))?;
    }

    io.line("step 8: OpenBao roles");
    if plan.roles.is_empty() {
        io.line("  nothing to change");
    }
    for role in &plan.roles {
        let mut data = role.data.clone();
        data["bound_subject"] = Value::String(role.new_subject.clone());
        services
            .admin()
            .map_err(|error| step(8, "OpenBao roles", error.to_string()))?
            .ensure_jwt_role(&role.mount, &role.name, &data)
            .await
            .map_err(|error| step(8, "OpenBao roles", format!("role {}: {error}", role.name)))?;
        io.line(&format!(
            "  auth/{}/role/{}: bound_subject {}",
            role.mount, role.name, role.new_subject
        ));
    }

    io.line("step 9: consumers");
    if plan.consumer_configs.is_empty() && plan.consumer_restarts.is_empty() {
        io.line("  none");
    }
    rewrite_configs(cluster, io, &plan.consumer_configs)
        .await
        .map_err(|error| step(9, "consumers", error))?;
    for workload in &plan.consumer_restarts {
        restart(cluster, io, workload, timeout)
            .await
            .map_err(|error| step(9, "consumers", error))?;
    }

    io.line("step 10: verify");
    let (places, _) = verify(services, new, &plan.bundle, &[])
        .await
        .map_err(|error| step(10, "verify", error))?;
    io.line(&format!(
        "done: the trust domain is {new} in all {places} place(s) read"
    ));
    Ok(())
}

/// Every place the plan looked at holds `new` now; the number of places.
/// Roles named in `tolerated` that are bound elsewhere are warnings (returned), not failures.
pub(crate) async fn verify(
    services: &Services,
    new: &str,
    bundle: &BundleRef,
    tolerated: &[String],
) -> Result<(usize, Vec<String>), String> {
    let settings = &services.settings;
    let mut notes = Vec::new();
    let mut wrong = Vec::new();
    let mut warnings = Vec::new();
    let mut places = 0;
    for read in read_configs(services, settings, &mut notes).await? {
        match read.domain.as_deref() {
            Some(domain) if domain == new => places += 1,
            Some(domain) => wrong.push(format!("{} still has {domain}", read.label)),
            None => {}
        }
    }
    if bundle_has(&services.cluster, bundle, new).await? {
        places += 1;
    } else {
        wrong.push("the server's CA does not have the new domain".to_owned());
    }
    let agents = agent::agents(&services.cluster).await?;
    if !agents.is_empty() {
        let bao = services.admin()?;
        for (agent, _) in agents {
            let role = agent::role_name(&agent);
            if let Some(data) = bao
                .read_jwt_role(&settings.jwt_mount, &role)
                .await
                .map_err(|error| format!("OpenBao role {role}: {error}"))?
            {
                let subject = data["bound_subject"].as_str().unwrap_or("");
                if spire::domain_of_id(subject) == Some(new) {
                    places += 1;
                } else if tolerated.contains(&role) {
                    warnings.push(format!("OpenBao role {role} is still bound to {subject}"));
                } else {
                    wrong.push(format!("OpenBao role {role} is bound to {subject}"));
                }
            }
        }
    }
    if wrong.is_empty() {
        Ok((places, warnings))
    } else {
        Err(format!("not everywhere {new}: {}", wrong.join("; ")))
    }
}
