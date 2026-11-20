//! Resolving the `tool_packages` of a spec: lockfile, `--locked`, selection, renaming.

use grauwolf_spec::AgentSpecExt;
use std::path::{Path, PathBuf};

use grauwolf_spec::AgentSpec;
use grauwolf_toolpkg::testing::{PONG_MODULE, TestPackage, TestRegistry};
use grauwolf_toolpkg::{Lockfile, PackageFetchError, PackageResolver, RegistryClient};
use serde_json::{Value, json};

fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "resolve-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn package() -> TestPackage {
    TestPackage::new("acme-math", "1.0.0")
        .tool(
            "add",
            "Adds",
            json!({ "type": "object" }),
            PONG_MODULE.as_bytes(),
        )
        .tool("sub", "Subtracts", json!({ "type": "object" }), b"(module)")
}

fn spec(packages: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" },
            "tools": [{ "name": "local", "description": "d", "args": { "type": "object" },
                        "code": "Ok(String::new())" }],
            "tool_packages": packages
        })
        .to_string(),
    )
    .unwrap()
}

fn resolver(work: &Path, locked: bool) -> PackageResolver {
    PackageResolver::new(work.join("cache"), RegistryClient::default()).with_locked(locked)
}

#[tokio::test]
async fn resolves_all_tools_and_writes_a_stable_lockfile() {
    let registry = TestRegistry::start().await;
    let digest = registry.push("test/math", "1.0", &package());
    let work = dir("lock");
    let reference = format!("{}/test/math:1.0", registry.host());

    let tools = resolver(&work, false)
        .resolve(
            &spec(json!([{ "oci": reference }])),
            &work.join("grauwolf.lock"),
        )
        .await
        .unwrap();

    assert_eq!(
        tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["add", "sub"]
    );
    assert_eq!(tools[0].description, "Adds");
    assert_eq!(tools[0].artifact.bytes, PONG_MODULE.as_bytes());
    let text = std::fs::read_to_string(work.join("grauwolf.lock")).unwrap();
    let lock = Lockfile::load(&work.join("grauwolf.lock")).unwrap();
    assert_eq!(lock.packages[&reference].digest, digest);
    assert_eq!(lock.packages[&reference].modules.len(), 2);
    assert!(text.ends_with("}\n"));
    assert_eq!(text, lock.to_json());
}

#[tokio::test]
async fn a_locked_entry_is_fetched_by_digest_and_not_by_tag() {
    let registry = TestRegistry::start().await;
    registry.push("test/math", "1.0", &package());
    let work = dir("pinned");
    let lock = work.join("grauwolf.lock");
    let spec = spec(json!([{ "oci": format!("{}/test/math:1.0", registry.host()) }]));
    resolver(&work, false).resolve(&spec, &lock).await.unwrap();
    // The tag moves to another package; the lock still gets the old one, from the cache.
    registry.push(
        "test/math",
        "1.0",
        &TestPackage::new("other", "2").tool("x", "d", json!({}), b"(module)"),
    );
    let requests = registry.requests().len();

    let tools = resolver(&work, true).resolve(&spec, &lock).await.unwrap();

    assert_eq!(tools.len(), 2);
    assert_eq!(registry.requests().len(), requests, "no request needed");
}

#[tokio::test]
async fn locked_fails_when_the_lockfile_would_change() {
    let registry = TestRegistry::start().await;
    registry.push("test/math", "1.0", &package());
    let work = dir("locked");
    let lock = work.join("grauwolf.lock");
    let spec = spec(json!([{ "oci": format!("{}/test/math:1.0", registry.host()) }]));

    let error = resolver(&work, true)
        .resolve(&spec, &lock)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("is not locked"), "{error}");
    assert!(!lock.exists());
    assert!(registry.requests().is_empty(), "failed before any request");
}

#[tokio::test]
async fn locked_fails_for_entries_that_are_no_longer_in_the_spec() {
    let registry = TestRegistry::start().await;
    registry.push("test/math", "1.0", &package());
    let work = dir("stale");
    let lock = work.join("grauwolf.lock");
    let with = spec(json!([{ "oci": format!("{}/test/math:1.0", registry.host()) }]));
    resolver(&work, false).resolve(&with, &lock).await.unwrap();

    let error = resolver(&work, true)
        .resolve(&spec(json!([])), &lock)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("no longer in the spec"),
        "{error}"
    );

    // Without --locked the stale entry is dropped.
    resolver(&work, false)
        .resolve(&spec(json!([])), &lock)
        .await
        .unwrap();
    assert!(!lock.exists());
}

#[tokio::test]
async fn rejects_a_lockfile_whose_module_hashes_do_not_match() {
    let registry = TestRegistry::start().await;
    registry.push("test/math", "1.0", &package());
    let work = dir("hashes");
    let lock = work.join("grauwolf.lock");
    let reference = format!("{}/test/math:1.0", registry.host());
    let spec = spec(json!([{ "oci": reference }]));
    resolver(&work, false).resolve(&spec, &lock).await.unwrap();
    let text = std::fs::read_to_string(&lock).unwrap();
    let hash = grauwolf_toolpkg::sha256_digest(PONG_MODULE.as_bytes());
    std::fs::write(
        &lock,
        text.replace(&hash, &format!("sha256:{}", "1".repeat(64))),
    )
    .unwrap();

    let error = resolver(&work, false)
        .resolve(&spec, &lock)
        .await
        .unwrap_err();
    assert!(matches!(error, PackageFetchError::Lock(_)), "{error}");
    assert!(
        error.to_string().contains("do not match the lockfile"),
        "{error}"
    );
}

#[tokio::test]
async fn selects_and_renames_tools() {
    let registry = TestRegistry::start().await;
    registry.push("test/math", "1.0", &package());
    let work = dir("rename");
    let reference = format!("{}/test/math:1.0", registry.host());

    let tools = resolver(&work, false)
        .resolve(
            &spec(json!([{ "oci": reference, "tools": ["add"], "as": { "add": "plus" } }])),
            &work.join("grauwolf.lock"),
        )
        .await
        .unwrap();

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "plus");
    assert_eq!(tools[0].artifact.name, "plus");
}

#[tokio::test]
async fn reports_unknown_tools_and_name_clashes() {
    let registry = TestRegistry::start().await;
    registry.push("test/math", "1.0", &package());
    let work = dir("errors");
    let lock = work.join("grauwolf.lock");
    let reference = format!("{}/test/math:1.0", registry.host());

    let unknown = resolver(&work, false)
        .resolve(
            &spec(json!([{ "oci": reference, "tools": ["mul"] }])),
            &lock,
        )
        .await
        .unwrap_err();
    assert!(
        unknown.to_string().contains("has no tool 'mul'"),
        "{unknown}"
    );

    let clash = resolver(&work, false)
        .resolve(
            &spec(json!([{ "oci": reference, "as": { "add": "local" } }])),
            &lock,
        )
        .await
        .unwrap_err();
    assert!(clash.to_string().contains("tool 'local'"), "{clash}");
}
