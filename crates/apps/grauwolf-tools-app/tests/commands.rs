//! The `grauwolf-tools` program, tested from the outside: every command through the real binary
//! (`CARGO_BIN_EXE_grauwolf-tools`), the registry commands against an in-memory registry.
//! Tests that build WASM modules with cargo are `#[ignore = "slow: …"]`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use grauwolf_toolpkg::testing::{PONG_MODULE, TestPackage, TestRegistry};
use serde_json::json;

fn work(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("tools-app-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn tools(args: &[&str], dir: &Path) -> Output {
    tools_with_env(args, dir, &[])
}

fn tools_with_env(args: &[&str], dir: &Path, env: &[(&str, &str)]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_grauwolf-tools"))
        .args(args)
        .env("GRAUWOLF_TOOLS_CATALOG_SETTLE_MS", "0")
        .envs(env.iter().copied())
        .current_dir(dir)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

const ADD_CODE: &str =
    "let a = args[\"a\"].as_f64().ok_or(\"a is missing\")?;\nOk(a.to_string())\n";

fn write_package_folder(dir: &Path, code: &str) -> PathBuf {
    let folder = dir.join("pkg");
    std::fs::create_dir_all(folder.join("src")).unwrap();
    std::fs::write(
        folder.join("grauwolf-tools.json"),
        r#"{ "name": "math", "version": "1.0.0", "tools": [
            { "name": "add", "description": "Adds two numbers", "args": { "type": "object" } } ] }"#,
    )
    .unwrap();
    std::fs::write(folder.join("src/add.rs"), code).unwrap();
    folder
}

fn push_ping(registry: &TestRegistry) -> String {
    registry.push(
        "test/ping",
        "1.0",
        &TestPackage::new("ping-pack", "1.0.0").tool(
            "ping",
            "Answers pong",
            json!({ "type": "object" }),
            PONG_MODULE.as_bytes(),
        ),
    )
}

#[test]
fn the_program_names_itself_and_its_commands() {
    let dir = work("version");
    let version = tools(&["--version"], &dir);
    assert!(
        text(&version.stdout).starts_with("grauwolf-tools "),
        "{version:?}"
    );
    let help = text(&tools(&["--help"], &dir).stdout);
    for command in [
        "build", "pack", "push", "pull", "inspect", "lock", "keygen", "validate", "catalog",
    ] {
        assert!(help.contains(command), "{command} missing in {help}");
    }
    let unknown = tools(&["spec", "run"], &dir);
    assert_eq!(unknown.status.code(), Some(2));
}

#[test]
fn validate_accepts_a_good_package_folder_without_building() {
    let dir = work("validate-ok");
    write_package_folder(&dir, ADD_CODE);
    let output = tools(&["validate", "pkg"], &dir);
    assert!(output.status.success(), "{output:?}");
    assert!(
        text(&output.stdout).contains("pkg: valid (1 tools)"),
        "{output:?}"
    );
}

#[test]
fn validate_accepts_the_package_folder_of_the_user_guide() {
    let example = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../docs/user-guide/examples/03-package")
        .canonicalize()
        .unwrap();
    let output = tools(
        &["validate", example.to_str().unwrap()],
        &work("validate-guide"),
    );
    assert!(output.status.success(), "{output:?}");
    assert!(
        text(&output.stdout).contains("valid (2 tools)"),
        "{output:?}"
    );
}

#[test]
fn validate_reports_a_missing_manifest() {
    let dir = work("validate-manifest");
    std::fs::create_dir_all(dir.join("empty")).unwrap();
    let output = tools(&["validate", "empty"], &dir);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("grauwolf-tools.json"),
        "{output:?}"
    );
}

#[test]
fn validate_reports_a_missing_code_file_and_a_forbidden_construct_together() {
    let dir = work("validate-code");
    let folder = write_package_folder(&dir, "unsafe { std::hint::unreachable_unchecked() }\n");
    std::fs::write(
        folder.join("grauwolf-tools.json"),
        r#"{ "name": "math", "version": "1.0.0", "tools": [
            { "name": "add", "description": "Adds", "args": { "type": "object" } },
            { "name": "sub", "description": "Subtracts", "args": { "type": "object" } } ] }"#,
    )
    .unwrap();
    let output = tools(&["validate", "pkg"], &dir);
    assert_eq!(output.status.code(), Some(1));
    let errors = text(&output.stderr);
    assert!(
        errors.contains("src/add.rs") && errors.contains("unsafe"),
        "{errors}"
    );
    assert!(errors.contains("src/sub.rs"), "{errors}");
}

#[test]
fn keygen_writes_a_key_pair_and_never_overwrites_it() {
    let dir = work("keygen");
    let first = tools(&["keygen", "keys-acme"], &dir);
    assert!(first.status.success(), "{first:?}");
    assert!(text(&first.stdout).contains("fingerprint: "), "{first:?}");
    assert!(dir.join("keys-acme.key").is_file() && dir.join("keys-acme.pub").is_file());
    let again = tools(&["keygen", "keys-acme"], &dir);
    assert_eq!(again.status.code(), Some(1));
    assert!(text(&again.stderr).contains("exists already"), "{again:?}");
}

#[test]
fn pack_reports_a_folder_without_manifest() {
    let dir = work("pack-error");
    std::fs::create_dir_all(dir.join("empty")).unwrap();
    let packed = tools(&["pack", "empty", "--cache", "cache"], &dir);
    assert_eq!(packed.status.code(), Some(1));
    assert!(
        text(&packed.stderr).contains("grauwolf-tools.json"),
        "{packed:?}"
    );
}

#[test]
#[ignore = "slow: builds tool code for WASM with cargo"]
fn pack_writes_a_deterministic_image_layout() {
    let dir = work("pack");
    let folder = write_package_folder(&dir, ADD_CODE);
    // The build date is part of the package; SOURCE_DATE_EPOCH fixes it.
    let epoch = [("SOURCE_DATE_EPOCH", "1700000000")];
    let first = tools_with_env(&["pack", "pkg", "--cache", "cache"], &dir, &epoch);
    assert!(first.status.success(), "{first:?}");
    let out = text(&first.stdout);
    assert!(out.contains("packed sha256:"), "{out}");
    for file in ["oci-layout", "index.json", "blobs/sha256"] {
        assert!(folder.join("dist").join(file).exists(), "{file} missing");
    }
    let second = tools_with_env(
        &["pack", "pkg", "--out", "again", "--cache", "cache"],
        &dir,
        &epoch,
    );
    let digest = |output: &str| output.split("packed ").nth(1).unwrap()[..71].to_owned();
    assert_eq!(digest(&out), digest(&text(&second.stdout)));
}

#[test]
#[ignore = "slow: builds tool code for WASM with cargo"]
fn push_a_package_folder_and_pull_and_inspect_it_again() {
    let dir = work("push");
    write_package_folder(&dir, ADD_CODE);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let registry = runtime.block_on(TestRegistry::start());
    let reference = format!("{}/test/math:1.0", registry.host());

    let pushed = tools(&["push", "pkg", &reference, "--cache", "cache"], &dir);
    assert!(pushed.status.success(), "{pushed:?}");
    let digest = text(&pushed.stdout)
        .lines()
        .next()
        .and_then(|line| line.split('@').nth(1))
        .unwrap()
        .to_owned();

    let pulled = tools(&["pull", &reference, "--cache", "pulled"], &dir);
    assert!(
        text(&pulled.stdout).contains(&format!("pulled {digest} (1 tools)")),
        "{pulled:?}"
    );
    let shown = tools(&["inspect", &reference, "--cache", "pulled"], &dir);
    assert!(
        text(&shown.stdout).contains("add: Adds two numbers"),
        "{shown:?}"
    );
}

#[test]
fn pull_and_inspect_work_against_a_registry() {
    let dir = work("pull");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let registry = runtime.block_on(TestRegistry::start());
    let digest = push_ping(&registry);
    let reference = format!("{}/test/ping:1.0", registry.host());

    let pulled = tools(&["pull", &reference, "--cache", "cache"], &dir);
    assert!(
        text(&pulled.stdout).contains(&format!("pulled {digest} (1 tools)")),
        "{pulled:?}"
    );
    let shown = tools(&["inspect", &reference, "--cache", "cache"], &dir);
    let shown = text(&shown.stdout);
    assert!(shown.contains(&format!("digest:  {digest}")), "{shown}");
    assert!(shown.contains("ping: Answers pong"), "{shown}");

    let missing = tools(
        &["pull", &format!("{}/test/none:1.0", registry.host())],
        &dir,
    );
    assert_eq!(missing.status.code(), Some(1));
}

#[test]
fn lock_writes_the_lockfile_for_a_spec_and_its_sub_agents() {
    let dir = work("lock");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let registry = runtime.block_on(TestRegistry::start());
    push_ping(&registry);
    let reference = format!("{}/test/ping:1.0", registry.host());
    let spec = |name: &str, extra: serde_json::Value| {
        let mut spec = json!({
            "spec_version": 1, "name": name,
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" },
            "tool_packages": [{ "oci": reference }]
        });
        spec.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        spec.to_string()
    };
    std::fs::create_dir_all(dir.join("agents")).unwrap();
    std::fs::write(dir.join("agents/sub.json"), spec("sub", json!({}))).unwrap();
    std::fs::write(
        dir.join("spec.json"),
        spec(
            "root",
            json!({ "agent_tools": { "helper": { "spec": "agents/sub.json", "description": "helps" } } }),
        ),
    )
    .unwrap();

    let refused = tools(&["lock", "spec.json", "--locked", "--cache", "cache"], &dir);
    assert_eq!(refused.status.code(), Some(1));
    assert!(!dir.join("grauwolf.lock").exists());

    let locked = tools(&["lock", "spec.json", "--cache", "cache"], &dir);
    assert!(locked.status.success(), "{locked:?}");
    assert!(
        text(&locked.stdout).contains("2 locked (2 tools)"),
        "{locked:?}"
    );
    let lock = std::fs::read_to_string(dir.join("grauwolf.lock")).unwrap();
    assert!(
        lock.contains(&format!("agents/sub.json#{reference}")),
        "{lock}"
    );

    let again = tools(&["lock", "spec.json", "--locked", "--cache", "cache"], &dir);
    assert!(again.status.success(), "{again:?}");
    assert_eq!(
        std::fs::read_to_string(dir.join("grauwolf.lock")).unwrap(),
        lock
    );
}

#[test]
fn lock_names_a_spec_without_anything_to_lock() {
    let dir = work("lock-none");
    std::fs::write(
        dir.join("spec.json"),
        json!({ "spec_version": 1, "name": "plain",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" } })
        .to_string(),
    )
    .unwrap();
    let output = tools(&["lock", "spec.json"], &dir);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("no `tool_packages` and no git `skills`"),
        "{output:?}"
    );
}

#[test]
fn lock_refuses_sub_agents_outside_the_directory_and_cycles() {
    let dir = work("lock-tree");
    let spec = |sub: &str| {
        json!({ "spec_version": 1, "name": "a",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" },
            "agent_tools": { "x": { "spec": sub, "description": "d" } } })
        .to_string()
    };
    std::fs::create_dir_all(dir.join("inner")).unwrap();
    std::fs::write(dir.join("outside.json"), spec("inner/a.json")).unwrap();
    std::fs::write(dir.join("inner/root.json"), spec("../outside.json")).unwrap();
    let outside = tools(&["lock", "inner/root.json"], &dir);
    assert!(
        text(&outside.stderr).contains("outside the agent's directory"),
        "{outside:?}"
    );
    std::fs::write(dir.join("cycle.json"), spec("cycle.json")).unwrap();
    let cycle = tools(&["lock", "cycle.json"], &dir);
    assert!(text(&cycle.stderr).contains("form a cycle"), "{cycle:?}");
}

#[test]
fn build_reports_a_missing_manifest() {
    let dir = work("build");
    let output = tools(&["build", "."], &dir);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("tools/tools.json"),
        "{output:?}"
    );
}

#[test]
fn build_refuses_tool_sources_that_changed_after_generation() {
    let dir = work("build-changed");
    std::fs::create_dir_all(dir.join("tools/add/src")).unwrap();
    std::fs::write(dir.join("tools/add/Cargo.toml"), "[package]\n").unwrap();
    std::fs::write(dir.join("tools/add/src/lib.rs"), "// edited\n").unwrap();
    std::fs::write(
        dir.join("tools/tools.json"),
        json!({ "format": 1, "tools": [
            { "name": "add", "module": "tools/add.wasm", "sha256": "sha256:00" } ] })
        .to_string(),
    )
    .unwrap();
    let output = tools(
        &["build", ".", "--format", "json", "--cache", "cache"],
        &dir,
    );
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["tools"][0]["ok"], false, "{report}");
    assert!(
        report["tools"][0]["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("changed since the crate was generated"),
        "{report}"
    );
}

#[test]
fn inspect_shows_the_metadata_and_limits_of_a_package() {
    let dir = work("inspect-metadata");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let registry = runtime.block_on(TestRegistry::start());
    registry.push(
        "test/meta",
        "1.0",
        &TestPackage::new("meta", "1.0.0")
            .tool(
                "ping",
                "Answers pong",
                json!({ "type": "object" }),
                PONG_MODULE.as_bytes(),
            )
            .limits("ping", json!({ "fuel": 1000, "time_ms": 50 }))
            .field("license", json!("MIT"))
            .annotation("org.opencontainers.image.description", "Pings")
            .annotation("org.opencontainers.image.created", "2026-11-16T09:30:00Z")
            .annotation(
                "org.opencontainers.image.source",
                "https://example.com/meta",
            )
            .annotation("vnd.grauwolf.tools.built-with", "0.1.0"),
    );
    let reference = format!("{}/test/meta:1.0", registry.host());
    let shown = text(&tools(&["inspect", &reference, "--cache", "cache"], &dir).stdout);
    for line in [
        "description: Pings",
        "created: 2026-11-16T09:30:00Z",
        "source: https://example.com/meta",
        "license: MIT",
        "built with: grauwolf 0.1.0",
        "    limits: fuel 1000, time 50 ms",
    ] {
        assert!(shown.contains(line), "{line} missing in {shown}");
    }
}

/// Two packages in the registry, `ping` signed with `keys/acme.key`, added to its catalog.
fn catalog_registry(dir: &Path, registry: &TestRegistry) -> String {
    let package = |name: &str, version: &str, description: &str| {
        TestPackage::new(name, version)
            .tool(
                "ping",
                "Answers pong",
                json!({ "type": "object" }),
                PONG_MODULE.as_bytes(),
            )
            .annotation("org.opencontainers.image.description", description)
    };
    registry.push(
        "acme/ping",
        "1.0",
        &package("ping", "1.0.0", "Pings politely"),
    );
    registry.push(
        "acme/ping",
        "1.1",
        &package("ping", "1.1.0", "Pings politely"),
    );
    registry.push("acme/echo", "2", &package("echo", "2.0.0", "Echoes words"));
    let host = registry.host();
    for reference in ["acme/ping:1.0", "acme/ping:1.1", "acme/echo:2"] {
        let added = tools(&["catalog", "add", &format!("{host}/{reference}")], dir);
        assert!(added.status.success(), "{added:?}");
        assert!(text(&added.stdout).contains("added"), "{added:?}");
    }
    host
}

#[test]
fn catalog_list_search_and_show_read_what_add_wrote() {
    let dir = work("catalog-read");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let registry = runtime.block_on(TestRegistry::start());
    let host = catalog_registry(&dir, &registry);
    let env = [("GRAUWOLF_TOOLS_CATALOG", host.as_str())];

    let list = text(&tools_with_env(&["catalog", "list"], &dir, &env).stdout);
    assert!(
        list.contains(&format!("catalog {host}/grauwolf-catalog:latest")),
        "{list}"
    );
    assert!(list.contains("index: unsigned"), "{list}");
    let rows: Vec<&str> = list
        .lines()
        .filter(|line| line.contains("allowed"))
        .collect();
    assert_eq!(rows.len(), 3, "{list}");
    assert!(
        rows[0].starts_with("echo") && rows[2].contains("1.1.0"),
        "{list}"
    );

    let found = text(&tools_with_env(&["catalog", "search", "ECHOES"], &dir, &env).stdout);
    assert!(
        found.contains("echo") && !found.contains("ping "),
        "{found}"
    );

    let shown = text(&tools_with_env(&["catalog", "show", "ping"], &dir, &env).stdout);
    for line in [
        "ping 1.1.0",
        &format!("reference: {host}/acme/ping@sha256:"),
        "description: Pings politely",
        "versions: 1.0.0, 1.1.0",
        "signature: unsigned",
        "policy: allowed",
        "capabilities: none",
    ] {
        assert!(shown.contains(line), "{line} missing in {shown}");
    }
    let old = text(
        &tools_with_env(
            &["catalog", "show", "ping", "--version", "1.0.0"],
            &dir,
            &env,
        )
        .stdout,
    );
    assert!(old.starts_with("ping 1.0.0"), "{old}");
    let missing = tools_with_env(&["catalog", "show", "ping", "--version", "9"], &dir, &env);
    assert_eq!(missing.status.code(), Some(1));
    assert!(
        text(&missing.stderr).contains("versions: 1.0.0, 1.1.0"),
        "{missing:?}"
    );

    let json: serde_json::Value = serde_json::from_slice(
        &tools(&["catalog", "list", "--json", "--catalog", &host], &dir).stdout,
    )
    .unwrap();
    assert_eq!(json["entries"].as_array().unwrap().len(), 3, "{json}");
    assert_eq!(json["entries"][0]["policy"]["allowed"], true, "{json}");
    assert_eq!(json["signature"]["status"], "unsigned", "{json}");
}

#[test]
fn catalog_list_applies_the_readers_policy_per_entry() {
    let dir = work("catalog-policy");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let registry = runtime.block_on(TestRegistry::start());
    let host = catalog_registry(&dir, &registry);
    std::fs::write(
        dir.join("grauwolf.toml"),
        format!("[tool_packages]\nallowed_registries = [\"{host}/acme/ping\"]\n"),
    )
    .unwrap();

    let list = text(&tools(&["catalog", "list", "--catalog", &host], &dir).stdout);

    let echo = list.lines().find(|line| line.starts_with("echo")).unwrap();
    assert!(
        echo.contains("refused: ") && echo.contains("allowed_registries"),
        "{list}"
    );
    let ping = list.lines().find(|line| line.starts_with("ping")).unwrap();
    assert!(ping.ends_with("allowed"), "{list}");
}

#[test]
fn catalog_writes_say_once_that_the_registry_ignores_conditional_writes() {
    let dir = work("catalog-ignores");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let registry = runtime.block_on(TestRegistry::start());
    registry.push(
        "acme/ping",
        "1.0",
        &TestPackage::new("ping", "1.0.0").tool(
            "ping",
            "Answers pong",
            json!({ "type": "object" }),
            PONG_MODULE.as_bytes(),
        ),
    );
    let host = registry.host();

    let added = tools(&["catalog", "add", &format!("{host}/acme/ping:1.0")], &dir);

    assert!(added.status.success(), "{added:?}");
    let out = text(&added.stdout);
    let note = format!(
        "note: {host} ignores conditional writes; concurrent catalog writers are checked after the fact"
    );
    assert_eq!(out.matches(&note).count(), 1, "{out}");
    assert!(out.contains("added ping@1.0.0"), "{out}");
}

#[test]
fn catalog_remove_drops_one_version() {
    let dir = work("catalog-remove");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let registry = runtime.block_on(TestRegistry::start());
    let host = catalog_registry(&dir, &registry);

    let removed = tools(
        &["catalog", "remove", "ping@1.0.0", "--catalog", &host],
        &dir,
    );
    assert!(removed.status.success(), "{removed:?}");
    let again = tools(
        &["catalog", "remove", "ping@1.0.0", "--catalog", &host],
        &dir,
    );
    assert_eq!(again.status.code(), Some(1));
    assert!(
        text(&again.stderr).contains("not in the catalog"),
        "{again:?}"
    );
    let list = text(&tools(&["catalog", "list", "--catalog", &host], &dir).stdout);
    assert!(!list.contains("1.0.0") && list.contains("1.1.0"), "{list}");
}

#[test]
fn catalog_commands_name_a_missing_catalog() {
    let dir = work("catalog-none");
    let output = tools(&["catalog", "list"], &dir);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("GRAUWOLF_TOOLS_CATALOG"),
        "{output:?}"
    );
}

#[test]
fn catalog_sync_rebuilds_the_index_from_the_registry_and_skips_other_artifacts() {
    let dir = work("catalog-sync");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let registry = runtime.block_on(TestRegistry::start());
    let host = catalog_registry(&dir, &registry);
    let config = registry.add_blob(b"{}");
    let other = json!({ "schemaVersion": 2, "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": { "mediaType": "application/vnd.oci.image.config.v1+json", "digest": config, "size": 2 },
        "layers": [] });
    registry.push_raw_manifest("acme/app", "1", other.to_string().as_bytes());
    // A multi-arch image index and a docker manifest list, which the registry would only hand
    // out in another representation unless they are asked for.
    for (tag, media_type) in [
        ("multi", "application/vnd.oci.image.index.v1+json"),
        (
            "legacy",
            "application/vnd.docker.distribution.manifest.list.v2+json",
        ),
    ] {
        let index = json!({ "schemaVersion": 2, "mediaType": media_type, "manifests": [] });
        registry.push_raw_manifest("acme/app", tag, index.to_string().as_bytes());
    }
    tools(
        &["catalog", "remove", "echo@2.0.0", "--catalog", &host],
        &dir,
    );

    let refused = tools(&["catalog", "sync", "--catalog", &host], &dir);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        text(&refused.stderr).contains("--repository"),
        "{refused:?}"
    );

    registry.offer_catalog();
    let synced = tools(&["catalog", "sync", "--catalog", &host], &dir);
    assert!(synced.status.success(), "{synced:?}");
    let out = text(&synced.stdout);
    assert!(!out.contains("digest mismatch"), "{out}");
    assert!(
        !out.contains("skipped registry") && !out.contains(&format!("skipped {host}")),
        "no line per skipped tag: {out}"
    );
    assert!(out.contains("3 entries from"), "{out}");
    assert!(out.contains("3 not tool packages"), "{out}");

    let verbose = text(&tools(&["catalog", "sync", "--catalog", &host, "--verbose"], &dir).stdout);
    for line in [
        format!(
            "skipped {host}/acme/app:1: not a tool package (application/vnd.oci.image.config.v1+json)"
        ),
        format!(
            "skipped {host}/acme/app:multi: not a tool package (application/vnd.oci.image.index.v1+json)"
        ),
        format!(
            "skipped {host}/acme/app:legacy: not a tool package (application/vnd.docker.distribution.manifest.list.v2+json)"
        ),
    ] {
        assert!(verbose.contains(&line), "{line} missing in {verbose}");
    }
    let list = text(&tools(&["catalog", "list", "--catalog", &host], &dir).stdout);
    assert!(list.lines().any(|line| line.starts_with("echo")), "{list}");

    let only = tools(
        &[
            "catalog",
            "sync",
            "--catalog",
            &host,
            "--repository",
            "acme/echo",
        ],
        &dir,
    );
    assert!(
        text(&only.stdout).contains("1 entries from 1 repositories"),
        "{only:?}"
    );
}

/// An image layout of a ready package, so pushing needs no build.
fn packed_layout(dir: &Path, name: &str, version: &str) -> PathBuf {
    let manifest = grauwolf_tools::PackageManifest::from_source_json(&format!(
        r#"{{ "name": "{name}", "version": "{version}", "description": "Pings",
            "tools": [{{ "name": "ping", "description": "Answers pong", "args": {{ "type": "object" }} }}] }}"#
    ))
    .unwrap();
    let modules = [("ping.wasm".to_owned(), PONG_MODULE.as_bytes().to_vec())].into();
    let layout = dir.join(format!("{name}-{version}"));
    grauwolf_toolpkg::PackedPackage::new(&manifest, &modules, None)
        .unwrap()
        .write_layout(&layout)
        .unwrap();
    layout
}

#[test]
fn push_adds_the_version_to_the_catalog_of_its_registry_unless_told_not_to() {
    let dir = work("push-catalog");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let registry = runtime.block_on(TestRegistry::start());
    let host = registry.host();
    let first = packed_layout(&dir, "ping", "1.0.0");
    let second = packed_layout(&dir, "ping", "2.0.0");

    let pushed = tools(
        &[
            "push",
            first.to_str().unwrap(),
            &format!("{host}/acme/ping:1.0.0"),
        ],
        &dir,
    );
    assert!(pushed.status.success(), "{pushed:?}");
    assert!(
        text(&pushed.stdout).contains(&format!(
            "catalog {host}/grauwolf-catalog:latest: added ping@1.0.0"
        )),
        "{pushed:?}"
    );
    let skipped = tools(
        &[
            "push",
            second.to_str().unwrap(),
            &format!("{host}/acme/ping:2.0.0"),
            "--no-catalog",
        ],
        &dir,
    );
    assert!(skipped.status.success(), "{skipped:?}");
    assert!(!text(&skipped.stdout).contains("catalog"), "{skipped:?}");

    let list = text(&tools(&["catalog", "list", "--catalog", &host], &dir).stdout);
    assert!(list.contains("1.0.0") && !list.contains("2.0.0"), "{list}");
}

#[test]
fn a_signed_push_signs_the_index_and_a_policy_that_requires_signatures_checks_it() {
    let dir = work("catalog-signed");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let registry = runtime.block_on(TestRegistry::start());
    let host = registry.host();
    assert!(tools(&["keygen", "acme"], &dir).status.success());
    let layout = packed_layout(&dir, "ping", "1.0.0");
    let pushed = tools(
        &[
            "push",
            layout.to_str().unwrap(),
            &format!("{host}/acme/ping:1.0.0"),
            "--sign-key",
            "acme.key",
        ],
        &dir,
    );
    assert!(pushed.status.success(), "{pushed:?}");
    let fingerprint = text(&tools(&["keygen", "other"], &dir).stdout); // a second key
    assert!(fingerprint.contains("fingerprint"));

    let checked = text(
        &tools(
            &["catalog", "list", "--catalog", &host, "--key", "acme.pub"],
            &dir,
        )
        .stdout,
    );
    assert!(checked.contains("index: signed by SHA256:"), "{checked}");
    assert!(checked.contains(": valid"), "{checked}");
    let shown = text(
        &tools(
            &[
                "catalog",
                "show",
                "ping",
                "--catalog",
                &host,
                "--key",
                "acme.pub",
            ],
            &dir,
        )
        .stdout,
    );
    assert!(shown.contains("signature: signed by SHA256:"), "{shown}");

    std::fs::write(
        dir.join("grauwolf.toml"),
        "[tool_packages]\nrequire_signatures = true\noci_keys = [\"acme.pub\"]\n",
    )
    .unwrap();
    let trusted = tools(&["catalog", "list", "--catalog", &host], &dir);
    assert!(trusted.status.success(), "{trusted:?}");
    assert!(
        text(&trusted.stdout)
            .lines()
            .any(|line| line.starts_with("ping") && line.ends_with("allowed")),
        "{trusted:?}"
    );

    std::fs::write(
        dir.join("grauwolf.toml"),
        "[tool_packages]\nrequire_signatures = true\noci_keys = [\"other.pub\"]\n",
    )
    .unwrap();
    let refused = tools(&["catalog", "list", "--catalog", &host], &dir);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        text(&refused.stderr).contains("signed: invalid"),
        "{refused:?}"
    );

    // An unsigned update leaves an unsigned index, which the policy refuses as well.
    std::fs::remove_file(dir.join("grauwolf.toml")).unwrap();
    tools(
        &["catalog", "remove", "ping@1.0.0", "--catalog", &host],
        &dir,
    );
    std::fs::write(
        dir.join("grauwolf.toml"),
        "[tool_packages]\nrequire_signatures = true\noci_keys = [\"acme.pub\"]\n",
    )
    .unwrap();
    let unsigned = tools(&["catalog", "list", "--catalog", &host], &dir);
    assert!(
        text(&unsigned.stderr).contains("index is unsigned"),
        "{unsigned:?}"
    );
}
