//! Pushing packages to a registry (ADR-0107): the pushed package can be pulled again. Upload,
//! refusals and credentials are tested in grauwolf-oci.

use std::collections::BTreeMap;

use grauwolf_spec::OciReference;
use grauwolf_toolpkg::testing::{PONG_MODULE, TestRegistry};
use grauwolf_toolpkg::{
    PackageResolver, PackedPackage, RegistryAuth, RegistryClient, source_archive,
};
use grauwolf_tools::PackageManifest;

fn packed() -> PackedPackage {
    let manifest = PackageManifest::from_source_json(
        r#"{ "name": "acme-math", "version": "1.0.0", "tools": [
            { "name": "add", "description": "Adds", "args": { "type": "object" } } ] }"#,
    )
    .unwrap();
    let modules = BTreeMap::from([("add.wasm".to_owned(), PONG_MODULE.as_bytes().to_vec())]);
    let sources = BTreeMap::from([("src/add.rs".to_owned(), b"Ok(String::new())".to_vec())]);
    PackedPackage::new(&manifest, &modules, Some(source_archive(&sources).unwrap())).unwrap()
}

fn reference(registry: &TestRegistry, rest: &str) -> OciReference {
    OciReference::parse(&format!("{}/{rest}", registry.host())).unwrap()
}

fn auth(registry: &TestRegistry, value: &str) -> RegistryAuth {
    RegistryAuth::from_json(&serde_json::json!({ registry.host(): value }).to_string()).unwrap()
}

fn cache(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "grauwolf-toolpkg-push-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn a_pushed_package_can_be_pulled_with_the_same_digest() {
    let registry = TestRegistry::start().await;
    let package = packed();
    let target = reference(&registry, "team/add:1.0");

    let pushed = RegistryClient::default()
        .push(&target, &package)
        .await
        .unwrap();
    assert_eq!(pushed, package.digest);

    let pulled = PackageResolver::new(cache("round-trip"), RegistryClient::default())
        .pull(&target)
        .await
        .unwrap();
    assert_eq!(pulled.digest, package.digest);
    assert_eq!(pulled.manifest.name, "acme-math");
    assert_eq!(pulled.modules["add.wasm"].bytes, PONG_MODULE.as_bytes());
    assert!(pulled.source.is_some());
}

#[tokio::test]
async fn pushing_with_the_token_flow_asks_for_the_push_scope() {
    let registry = TestRegistry::start_with_token_auth(Some(("ci", "s3cret"))).await;
    let package = packed();
    let target = reference(&registry, "team/add:1.0");

    let client = RegistryClient::new(auth(&registry, "ci:s3cret"));
    client.push(&target, &package).await.unwrap();

    assert!(registry.requests().iter().any(|r| r == "GET /token"));
    let pulled = PackageResolver::new(cache("token"), client)
        .pull(&target)
        .await
        .unwrap();
    assert_eq!(pulled.digest, package.digest);
}
