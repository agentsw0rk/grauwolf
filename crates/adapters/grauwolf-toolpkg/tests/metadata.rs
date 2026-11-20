//! Package metadata as OCI annotations (ADR-0124): written by packing, read back by pulling.

use std::collections::BTreeMap;

use grauwolf_git::Checkout;
use grauwolf_spec::OciReference;
use grauwolf_toolpkg::testing::{PONG_MODULE, TestRegistry};
use grauwolf_toolpkg::{
    BuildInfo, PackageResolver, PackedPackage, RegistryClient, package_annotations,
};
use grauwolf_tools::PackageManifest;

fn manifest(extra: &str) -> PackageManifest {
    PackageManifest::from_source_json(&format!(
        r#"{{ "name": "math", "version": "1.2.0", {extra}
            "tools": [{{ "name": "ping", "description": "Pong", "args": {{ "type": "object" }} }}] }}"#
    ))
    .unwrap()
}

fn info(checkout: Option<Checkout>) -> BuildInfo {
    BuildInfo {
        created: 1_700_000_000,
        rustc: Some("rustc 1.99.1".into()),
        checkout,
    }
}

#[test]
fn the_annotations_carry_the_manifest_fields_and_the_build() {
    let manifest = manifest(
        r#""description": "Maths", "license": "MIT", "authors": ["Ada", "Bob"],
           "source": "https://example.com/math","#,
    );
    let annotations = package_annotations(
        &manifest,
        &info(Some(Checkout {
            remote: Some("https://elsewhere.example.com/x".into()),
            commit: "a".repeat(40),
        })),
    );
    let get = |key: &str| annotations.get(key).map(String::as_str);
    assert_eq!(get("org.opencontainers.image.title"), Some("math"));
    assert_eq!(get("org.opencontainers.image.version"), Some("1.2.0"));
    assert_eq!(get("org.opencontainers.image.description"), Some("Maths"));
    assert_eq!(get("org.opencontainers.image.licenses"), Some("MIT"));
    assert_eq!(get("org.opencontainers.image.authors"), Some("Ada, Bob"));
    assert_eq!(
        get("org.opencontainers.image.created"),
        Some("2023-11-14T22:13:20Z")
    );
    // The manifest's source wins over the checkout's remote; the revision comes from git.
    assert_eq!(
        get("org.opencontainers.image.source"),
        Some("https://example.com/math")
    );
    assert_eq!(
        get("org.opencontainers.image.revision"),
        Some("a".repeat(40).as_str())
    );
    assert_eq!(get("vnd.grauwolf.tools.abi"), Some("1"));
    assert_eq!(
        get("vnd.grauwolf.tools.built-with"),
        Some(grauwolf_toolpkg::GRAUWOLF_VERSION)
    );
    assert_eq!(get("vnd.grauwolf.tools.rustc"), Some("rustc 1.99.1"));
}

#[test]
fn without_a_source_the_checkout_remote_is_the_source_and_empty_fields_are_left_out() {
    let annotations = package_annotations(
        &manifest(""),
        &BuildInfo {
            rustc: None,
            ..info(Some(Checkout {
                remote: Some("https://example.com/acme/tools.git".into()),
                commit: "b".repeat(40),
            }))
        },
    );
    assert_eq!(
        annotations["org.opencontainers.image.source"],
        "https://example.com/acme/tools.git"
    );
    for key in [
        "org.opencontainers.image.description",
        "org.opencontainers.image.licenses",
        "org.opencontainers.image.authors",
        "vnd.grauwolf.tools.rustc",
    ] {
        assert!(!annotations.contains_key(key), "{key}");
    }
}

#[tokio::test]
async fn a_pushed_package_pulls_with_its_annotations() {
    let registry = TestRegistry::start().await;
    let manifest = manifest(r#""description": "Maths","#);
    let modules = BTreeMap::from([("ping.wasm".to_owned(), PONG_MODULE.as_bytes().to_vec())]);
    let plain = PackedPackage::new(&manifest, &modules, None).unwrap();
    let annotated = plain
        .clone()
        .with_annotations(package_annotations(&manifest, &info(None)));
    assert_ne!(annotated.digest, plain.digest);

    let reference = OciReference::parse(&format!("{}/acme/math:1.2.0", registry.host())).unwrap();
    let client = RegistryClient::default();
    client.push(&reference, &annotated).await.unwrap();
    let cache = grauwolf_test::TestDir::new("metadata-pull");
    let package = PackageResolver::new(&*cache, RegistryClient::default())
        .pull(&reference)
        .await
        .unwrap();

    assert_eq!(package.digest, annotated.digest);
    assert_eq!(
        package.annotations["org.opencontainers.image.description"],
        "Maths"
    );
    assert!(plain.package().unwrap().annotations.is_empty());
}
