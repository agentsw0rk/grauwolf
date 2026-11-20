//! Tool packages from a registry (ADR-0107), tested from the outside against an in-memory
//! registry on 127.0.0.1.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use grauwolf_toolpkg::testing::{PONG_MODULE, TestPackage, TestRegistry};
use serde_json::json;

/// Fresh working directory below the target directory of the test run (not on the mounted
/// workspace: parallel builds there sometimes lose their working directory).
fn work(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("pkg-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Packed packages carry their build date (ADR-0124); a fixed `SOURCE_DATE_EPOCH` keeps the
/// digests of two packs of the same folder equal.
const SOURCE_DATE_EPOCH: (&str, &str) = ("SOURCE_DATE_EPOCH", "1700000000");

fn grauwolf(args: &[&str], dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(args)
        .env(SOURCE_DATE_EPOCH.0, SOURCE_DATE_EPOCH.1)
        .current_dir(dir)
        .output()
        .unwrap()
}

fn grauwolf_with_env(args: &[&str], dir: &Path, env: &[(&str, &str)]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(args)
        .env(SOURCE_DATE_EPOCH.0, SOURCE_DATE_EPOCH.1)
        .envs(env.iter().copied())
        .current_dir(dir)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn registry_with_ping(registry: &TestRegistry) -> String {
    registry.push(
        "test/ping",
        "1.0",
        &TestPackage::new("ping-pack", "1.0.0").tool(
            "ping",
            "Answers pong",
            json!({ "type": "object", "properties": { "who": { "type": "string" } } }),
            PONG_MODULE.as_bytes(),
        ),
    )
}

fn write_spec(dir: &Path, registry: &TestRegistry) -> PathBuf {
    let file = dir.join("spec.json");
    std::fs::write(
        &file,
        json!({
            "spec_version": 1, "name": "pinger",
            "model": { "provider": "ollama", "id": "m" },
            "tool_packages": [{ "oci": format!("{}/test/ping:1.0", registry.host()) }],
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    file
}

#[tokio::test(flavor = "multi_thread")]
async fn pull_fetches_and_inspect_shows_the_package() {
    let registry = TestRegistry::start().await;
    let digest = registry_with_ping(&registry);
    let dir = work("pull");
    let reference = format!("{}/test/ping:1.0", registry.host());

    let pulled = grauwolf(&["tools", "pull", &reference, "--cache", "cache"], &dir);
    assert!(pulled.status.success(), "{pulled:?}");
    assert!(
        text(&pulled.stdout).contains(&format!("pulled {digest} (1 tools)")),
        "{pulled:?}"
    );
    assert!(dir.join("cache/packages").is_dir());

    let inspected = grauwolf(&["tools", "inspect", &reference, "--cache", "cache"], &dir);
    let out = text(&inspected.stdout);
    assert!(inspected.status.success(), "{inspected:?}");
    for expected in [
        &digest,
        "ping-pack 1.0.0",
        "abi:     1",
        "ping: Answers pong",
        "ping.wasm sha256:",
    ] {
        assert!(out.contains(expected), "{expected} missing in {out}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pull_reports_unknown_packages_and_bad_references() {
    let registry = TestRegistry::start().await;
    let dir = work("pull-errors");

    let missing = grauwolf(
        &["tools", "pull", &format!("{}/test/none:1", registry.host())],
        &dir,
    );
    assert_eq!(missing.status.code(), Some(1));
    assert!(text(&missing.stderr).contains("not found"), "{missing:?}");

    let bad = grauwolf(&["tools", "pull", "not-a-reference"], &dir);
    assert_eq!(bad.status.code(), Some(1));
    assert!(
        text(&bad.stderr).contains("expected `registry/repository`"),
        "{bad:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn lock_writes_the_lockfile_and_locked_refuses_to_change_it() {
    let registry = TestRegistry::start().await;
    registry_with_ping(&registry);
    let dir = work("lock");
    let spec = write_spec(&dir, &registry);
    let spec = spec.to_str().unwrap();

    let refused = grauwolf(
        &["tools", "lock", spec, "--locked", "--cache", "cache"],
        &dir,
    );
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        text(&refused.stderr).contains("is not locked"),
        "{refused:?}"
    );
    assert!(!dir.join("grauwolf.lock").exists());

    let locked = grauwolf(&["tools", "lock", spec, "--cache", "cache"], &dir);
    assert!(locked.status.success(), "{locked:?}");
    let lock = std::fs::read_to_string(dir.join("grauwolf.lock")).unwrap();
    assert!(lock.contains("\"digest\": \"sha256:"), "{lock}");

    let again = grauwolf(
        &["tools", "lock", spec, "--locked", "--cache", "cache"],
        &dir,
    );
    assert!(again.status.success(), "{again:?}");
    assert_eq!(
        std::fs::read_to_string(dir.join("grauwolf.lock")).unwrap(),
        lock
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn run_offers_the_tools_of_a_registry_package() {
    use wiremock::matchers::{body_string_contains, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let registry = TestRegistry::start().await;
    registry_with_ping(&registry);
    let dir = work("run");
    let spec = write_spec(&dir, &registry);
    let llm = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("pong"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "message": { "role": "assistant", "content": "The tool said pong." }, "done": true
        })))
        .mount(&llm)
        .await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("\"ping\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "message": { "role": "assistant", "content": "", "tool_calls": [
                { "function": { "name": "ping", "arguments": { "who": "wolf" } } } ] },
            "done": true
        })))
        .mount(&llm)
        .await;
    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_grauwolf"))
            .args([
                "spec",
                "run",
                spec.to_str().unwrap(),
                "Ping the wolf.",
                "--cache",
                "cache",
            ])
            .args(extra)
            .current_dir(&dir)
            .env("OLLAMA_HOST", llm.uri())
            .output()
            .unwrap()
    };

    let first = run(&[]);
    assert!(first.status.success(), "{first:?}");
    assert_eq!(text(&first.stdout), "The tool said pong.\n");
    assert!(
        dir.join("grauwolf.lock").exists(),
        "the run wrote the lockfile"
    );

    // With the lock and the cache the registry is not needed any more.
    drop(registry);
    let second = run(&["--locked"]);
    assert!(second.status.success(), "{second:?}");
    assert_eq!(text(&second.stdout), "The tool said pong.\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn run_with_locked_fails_without_a_lockfile() {
    let registry = TestRegistry::start().await;
    registry_with_ping(&registry);
    let dir = work("run-locked");
    let spec = write_spec(&dir, &registry);

    let output = grauwolf(
        &[
            "spec",
            "run",
            spec.to_str().unwrap(),
            "hi",
            "--locked",
            "--cache",
            "cache",
        ],
        &dir,
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).contains("is not locked"), "{output:?}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: starts the binary, which builds tools with cargo"]
async fn build_fetches_the_packages_of_the_spec() {
    let registry = TestRegistry::start().await;
    registry_with_ping(&registry);
    let dir = work("build");
    let spec = write_spec(&dir, &registry);

    let output = grauwolf(
        &["spec", "build", spec.to_str().unwrap(), "--cache", "cache"],
        &dir,
    );

    assert!(output.status.success(), "{output:?}");
    assert!(text(&output.stdout).starts_with("ping: "), "{output:?}");
    assert!(dir.join("grauwolf.lock").exists());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: starts the binary, which builds tools with cargo"]
async fn generate_embeds_the_modules_also_with_the_dockerfile() {
    let registry = TestRegistry::start().await;
    registry_with_ping(&registry);
    let dir = work("generate");
    let spec = write_spec(&dir, &registry);
    let out = dir.join("crate");

    let output = grauwolf(
        &[
            "spec",
            "generate",
            spec.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--cache",
            "cache",
            "--dockerfile",
        ],
        &dir,
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        std::fs::read(out.join("tools/ping.wasm")).unwrap(),
        PONG_MODULE.as_bytes()
    );
    assert!(out.join("Dockerfile").exists());
    let lib = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    assert!(lib.contains("../tools/ping.wasm"), "{lib}");
    assert!(
        !std::fs::read_to_string(out.join("Cargo.toml"))
            .unwrap()
            .contains(&registry.host())
    );
}

fn write_package_folder(dir: &Path) -> PathBuf {
    let folder = dir.join("pkg");
    std::fs::create_dir_all(folder.join("src")).unwrap();
    std::fs::write(
        folder.join("grauwolf-tools.json"),
        r#"{ "name": "math", "version": "1.0.0", "tools": [
            { "name": "add", "description": "Adds two numbers", "args": { "type": "object" } } ] }"#,
    )
    .unwrap();
    std::fs::write(
        folder.join("src/add.rs"),
        "let a = args[\"a\"].as_f64().ok_or(\"a is missing\")?;\nOk(a.to_string())\n",
    )
    .unwrap();
    folder
}

#[test]
#[ignore = "slow: starts the binary, which builds tools with cargo"]
fn pack_writes_a_deterministic_image_layout() {
    let dir = work("pack");
    let folder = write_package_folder(&dir);
    let first = grauwolf(&["tools", "pack", "pkg", "--cache", "cache"], &dir);
    assert!(first.status.success(), "{first:?}");
    let out = text(&first.stdout);
    assert!(out.contains("packed sha256:"), "{out}");
    for file in ["oci-layout", "index.json", "blobs/sha256"] {
        assert!(folder.join("dist").join(file).exists(), "{file} missing");
    }

    let second = grauwolf(
        &[
            "tools", "pack", "pkg", "--out", "again", "--cache", "cache-2",
        ],
        &dir,
    );
    assert!(second.status.success(), "{second:?}");
    let digest = |output: &str| output.split("packed ").nth(1).unwrap()[..71].to_owned();
    assert_eq!(digest(&out), digest(&text(&second.stdout)));
    assert!(dir.join("again/index.json").is_file());
}

#[test]
fn pack_reports_a_folder_without_manifest() {
    let dir = work("pack-error");
    std::fs::create_dir_all(dir.join("empty")).unwrap();
    let packed = grauwolf(&["tools", "pack", "empty", "--cache", "cache"], &dir);
    assert_eq!(packed.status.code(), Some(1));
    assert!(
        text(&packed.stderr).contains("grauwolf-tools.json"),
        "{packed:?}"
    );
}

/// The digest after `@` in the first line of the output of `tools push` (the next lines are
/// about the signature and the tool catalog).
fn pushed_digest(output: &Output) -> String {
    let out = text(&output.stdout);
    out.lines()
        .next()
        .unwrap_or_default()
        .rsplit_once('@')
        .expect("<ref>@<digest>")
        .1
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: starts the binary, which builds tools with cargo"]
async fn push_sends_a_packed_layout_and_pull_gives_the_same_digest() {
    let registry = TestRegistry::start().await;
    let dir = work("push-layout");
    write_package_folder(&dir);
    let packed = grauwolf(&["tools", "pack", "pkg", "--cache", "cache"], &dir);
    assert!(packed.status.success(), "{packed:?}");
    let reference = format!("{}/team/add:1.0", registry.host());

    let pushed = grauwolf(&["tools", "push", "pkg/dist", &reference], &dir);

    assert!(pushed.status.success(), "{pushed:?}");
    let digest = pushed_digest(&pushed);
    assert!(text(&pushed.stdout).starts_with(&format!("{reference}@sha256:")));
    assert!(text(&packed.stdout).contains(&digest), "{packed:?}");
    let pulled = grauwolf(&["tools", "pull", &reference, "--cache", "cache"], &dir);
    assert!(
        text(&pulled.stdout).contains(&format!("pulled {digest} (1 tools)")),
        "{pulled:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: starts the binary, which builds tools with cargo"]
async fn push_packs_a_package_folder_first() {
    let registry = TestRegistry::start().await;
    let dir = work("push-folder");
    write_package_folder(&dir);
    let reference = format!("{}/team/add", registry.host());

    let pushed = grauwolf(
        &["tools", "push", "pkg", &reference, "--cache", "cache"],
        &dir,
    );

    assert!(pushed.status.success(), "{pushed:?}");
    let packed = grauwolf(&["tools", "pack", "pkg", "--cache", "cache"], &dir);
    assert_eq!(pushed_digest(&pushed), pushed_digest_of_pack(&packed));
    let inspected = grauwolf(&["tools", "inspect", &format!("{reference}:latest")], &dir);
    assert!(
        text(&inspected.stdout).contains("math 1.0.0"),
        "{inspected:?}"
    );
}

fn pushed_digest_of_pack(output: &Output) -> String {
    let out = text(&output.stdout);
    out.split("packed ").nth(1).unwrap()[..71].to_owned()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: starts the binary, which builds tools with cargo"]
async fn push_needs_credentials_and_never_prints_them() {
    let registry = TestRegistry::start_with_token_auth(Some(("ci", "s3cret"))).await;
    let dir = work("push-auth");
    write_package_folder(&dir);
    let reference = format!("{}/team/add:1.0", registry.host());

    let refused = grauwolf(&["tools", "push", "pkg", &reference], &dir);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        text(&refused.stderr).contains("GRAUWOLF_REGISTRY_AUTH"),
        "{refused:?}"
    );

    let wrong = json!({ registry.host(): "ci:wr0ng" }).to_string();
    let denied = grauwolf_with_env(
        &["tools", "push", "pkg", &reference],
        &dir,
        &[("GRAUWOLF_REGISTRY_AUTH", &wrong)],
    );
    assert_eq!(denied.status.code(), Some(1));
    assert!(!text(&denied.stderr).contains("wr0ng"), "{denied:?}");

    let right = json!({ registry.host(): "ci:s3cret" }).to_string();
    let accepted = grauwolf_with_env(
        &["tools", "push", "pkg", &reference],
        &dir,
        &[("GRAUWOLF_REGISTRY_AUTH", &right)],
    );
    assert!(accepted.status.success(), "{accepted:?}");
    assert!(!text(&accepted.stdout).contains("s3cret"));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: starts the binary, which builds tools with cargo"]
async fn push_takes_its_credentials_from_the_docker_configuration_and_a_helper() {
    use std::os::unix::fs::PermissionsExt;
    let registry = TestRegistry::start_with_token_auth(Some(("ci", "s3cret"))).await;
    let dir = work("push-docker");
    write_package_folder(&dir);
    let packed = grauwolf(&["tools", "pack", "pkg", "--cache", "cache"], &dir);
    assert!(packed.status.success(), "{packed:?}");
    let reference = format!("{}/team/add:1.0", registry.host());

    // A credential helper that is only found through the PATH the command is started with.
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let helper = bin.join("docker-credential-vault");
    std::fs::write(
        &helper,
        "#!/bin/sh\nread server\nprintf '%s' '{\"Username\":\"ci\",\"Secret\":\"s3cret\"}'\n",
    )
    .unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let docker = dir.join("docker");
    std::fs::create_dir_all(&docker).unwrap();
    std::fs::write(docker.join("config.json"), r#"{"credsStore":"vault"}"#).unwrap();
    let docker_dir = docker.to_str().unwrap();
    let path = bin.to_str().unwrap();

    let without_helper = grauwolf_with_env(
        &["tools", "push", "pkg/dist", &reference],
        &dir,
        &[("DOCKER_CONFIG", docker_dir), ("PATH", "/nonexistent")],
    );
    assert_eq!(without_helper.status.code(), Some(1));
    assert!(
        text(&without_helper.stderr).contains("docker-credential-vault"),
        "{without_helper:?}"
    );

    let pushed = grauwolf_with_env(
        &["tools", "push", "pkg/dist", &reference],
        &dir,
        &[("DOCKER_CONFIG", docker_dir), ("PATH", path)],
    );
    assert!(pushed.status.success(), "{pushed:?}");
    let shown = format!("{}{}", text(&pushed.stdout), text(&pushed.stderr));
    assert!(!shown.contains("s3cret"));
}

/// A packed layout of one tool without building anything (the module is WAT text).
fn write_layout(dir: &Path) -> PathBuf {
    let manifest = grauwolf_tools::PackageManifest::from_json(
        &json!({ "format": 1, "name": "ping-pack", "version": "1.0.0", "abi": 1,
                 "tools": [{ "name": "ping", "description": "Answers pong",
                             "args": { "type": "object" }, "module": "ping.wasm" }] })
        .to_string(),
    )
    .unwrap();
    let modules = [("ping.wasm".to_owned(), PONG_MODULE.as_bytes().to_vec())].into();
    let layout = dir.join("layout");
    grauwolf_toolpkg::PackedPackage::new(&manifest, &modules, None)
        .unwrap()
        .write_layout(&layout)
        .unwrap();
    layout
}

#[tokio::test(flavor = "multi_thread")]
async fn keygen_push_sign_key_inspect_and_the_policy_roundtrip() {
    let registry = TestRegistry::start().await;
    let dir = work("signing");
    write_layout(&dir);
    let reference = format!("{}/test/ping:1.0", registry.host());

    let keygen = grauwolf(&["tools", "keygen", "keys-acme"], &dir);
    assert!(keygen.status.success(), "{keygen:?}");
    let out = text(&keygen.stdout);
    assert!(out.contains("unencrypted") && out.contains("0600"), "{out}");
    assert!(dir.join("keys-acme.key").is_file() && dir.join("keys-acme.pub").is_file());
    let again = grauwolf(&["tools", "keygen", "keys-acme"], &dir);
    assert_eq!(again.status.code(), Some(1));

    // Unsigned package: inspect says so; a policy that requires signatures refuses to pull it.
    let unsigned = grauwolf(&["tools", "push", "layout", &reference], &dir);
    assert!(unsigned.status.success(), "{unsigned:?}");
    let shown = grauwolf(
        &[
            "tools",
            "inspect",
            &reference,
            "--key",
            "keys-acme.pub",
            "--cache",
            "c1",
        ],
        &dir,
    );
    assert!(
        text(&shown.stdout).contains("signature: unsigned"),
        "{shown:?}"
    );
    std::fs::write(
        dir.join("policy.toml"),
        "[tool_packages]\nrequire_signatures = true\noci_keys = [\"keys-acme.pub\"]\n",
    )
    .unwrap();
    let env = [("GRAUWOLF_TOOL_POLICY", "policy.toml")];
    let refused = grauwolf_with_env(&["tools", "pull", &reference, "--cache", "c2"], &dir, &env);
    assert_eq!(refused.status.code(), Some(1));
    assert!(text(&refused.stderr).contains("unsigned"), "{refused:?}");

    // Sign while pushing: inspect shows the key, the policy accepts the package.
    let signed = grauwolf(
        &[
            "tools",
            "push",
            "layout",
            &reference,
            "--sign-key",
            "keys-acme.key",
        ],
        &dir,
    );
    assert!(signed.status.success(), "{signed:?}");
    assert!(text(&signed.stdout).contains("signed"));
    let shown = grauwolf(
        &[
            "tools",
            "inspect",
            &reference,
            "--key",
            "keys-acme.pub",
            "--cache",
            "c3",
        ],
        &dir,
    );
    assert!(
        text(&shown.stdout).contains("signature: signed by SHA256:"),
        "{shown:?}"
    );
    let pulled = grauwolf_with_env(&["tools", "pull", &reference, "--cache", "c4"], &dir, &env);
    assert!(pulled.status.success(), "{pulled:?}");

    // Another key does not verify.
    assert!(
        grauwolf(&["tools", "keygen", "keys-other"], &dir)
            .status
            .success()
    );
    let wrong = grauwolf(
        &[
            "tools",
            "inspect",
            &reference,
            "--key",
            "keys-other.pub",
            "--cache",
            "c5",
        ],
        &dir,
    );
    assert!(
        text(&wrong.stdout).contains("signature: signature invalid"),
        "{wrong:?}"
    );
}

/// `tools lock` and `spec build` cover the whole agent tree (ADR-0115): the packages of a
/// sub-agent are locked in the lockfile next to the root, under the sub-spec's path.
#[tokio::test(flavor = "multi_thread")]
async fn lock_and_build_cover_the_packages_of_sub_agents() {
    let registry = TestRegistry::start().await;
    registry_with_ping(&registry);
    let dir = work("tree-lock");
    std::fs::create_dir_all(dir.join("agents")).unwrap();
    let reference = format!("{}/test/ping:1.0", registry.host());
    std::fs::write(
        dir.join("agents/pinger.json"),
        json!({
            "spec_version": 1, "name": "pinger",
            "model": { "provider": "ollama", "id": "m" },
            "tool_packages": [{ "oci": reference }],
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("root.json"),
        json!({
            "spec_version": 1, "name": "root",
            "model": { "provider": "ollama", "id": "m" },
            "agent_tools": { "pinger": { "spec": "agents/pinger.json", "description": "Pings" } },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();

    let locked = grauwolf(&["tools", "lock", "root.json", "--cache", "cache"], &dir);
    assert!(locked.status.success(), "{locked:?}");
    let lock = std::fs::read_to_string(dir.join("grauwolf.lock")).unwrap();
    assert!(
        lock.contains(&format!("\"agents/pinger.json#{reference}\"")),
        "{lock}"
    );
    assert!(!dir.join("agents/grauwolf.lock").exists());

    let built = grauwolf(
        &["spec", "build", "root.json", "--cache", "cache", "--locked"],
        &dir,
    );
    assert!(built.status.success(), "{built:?}");
    assert!(text(&built.stdout).contains("ping: "), "{built:?}");
}

/// `generate --dockerfile` copies the packages of a sub-agent with the lockfile into `app/`, so
/// the service loads them locked and offline (ADR-0115).
#[tokio::test(flavor = "multi_thread")]
async fn generate_dockerfile_copies_the_packages_of_sub_agents_with_the_lockfile() {
    let registry = TestRegistry::start().await;
    registry_with_ping(&registry);
    let dir = work("tree-generate");
    std::fs::create_dir_all(dir.join("agents")).unwrap();
    let reference = format!("{}/test/ping:1.0", registry.host());
    std::fs::write(
        dir.join("agents/pinger.json"),
        json!({
            "spec_version": 1, "name": "pinger",
            "model": { "provider": "ollama", "id": "m" },
            "tool_packages": [{ "oci": reference }],
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("root.json"),
        json!({
            "spec_version": 1, "name": "root",
            "model": { "provider": "ollama", "id": "m" },
            "agent_tools": { "pinger": { "spec": "agents/pinger.json", "description": "Pings" } },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();

    let generated = grauwolf(
        &[
            "spec",
            "generate",
            "root.json",
            "--out",
            "out",
            "--dockerfile",
            "--cache",
            "cache",
        ],
        &dir,
    );

    assert!(generated.status.success(), "{generated:?}");
    let app = dir.join("out/app");
    assert!(app.join("agents/pinger.json").is_file());
    assert!(app.join("grauwolf.lock").is_file());
    let packages = std::fs::read_dir(app.join(".grauwolf/tools/packages"))
        .unwrap()
        .count();
    assert_eq!(packages, 1);
}
