//! Pulling packages from a registry (ADR-0107): manifests, blobs, tokens, verification.

use grauwolf_spec::OciReference;
use grauwolf_toolpkg::testing::{PONG_MODULE, TestPackage, TestRegistry};
use grauwolf_toolpkg::{PackageFetchError, PackageResolver, RegistryAuth, RegistryClient};
use serde_json::json;

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

fn resolver(dir: &std::path::Path, client: RegistryClient) -> PackageResolver {
    PackageResolver::new(dir, client)
}

fn reference(registry: &TestRegistry, rest: &str) -> OciReference {
    OciReference::parse(&format!("{}/{rest}", registry.host())).unwrap()
}

#[tokio::test]
async fn pulls_a_package_by_tag_with_its_modules() {
    let registry = TestRegistry::start().await;
    let digest = registry.push("test/math", "1.0", &package());
    let cache = tempfile_dir("pull-tag");

    let pulled = resolver(&cache, RegistryClient::default())
        .pull(&reference(&registry, "test/math:1.0"))
        .await
        .unwrap();

    assert_eq!(pulled.digest, digest);
    assert_eq!(pulled.manifest.name, "acme-math");
    assert_eq!(pulled.manifest.tools.len(), 2);
    assert_eq!(pulled.modules["add.wasm"].bytes, PONG_MODULE.as_bytes());
    assert_eq!(pulled.source, None);
}

#[tokio::test]
async fn pulls_by_digest_and_never_fetches_a_cached_digest_again() {
    let registry = TestRegistry::start().await;
    let digest = registry.push("test/math", "1.0", &package());
    let cache = tempfile_dir("pull-digest");
    let by_digest = reference(&registry, &format!("test/math@{digest}"));
    let resolver = resolver(&cache, RegistryClient::default());

    resolver.pull(&by_digest).await.unwrap();
    let requests = registry.requests().len();
    let again = resolver.pull(&by_digest).await.unwrap();

    assert_eq!(again.digest, digest);
    assert_eq!(
        registry.requests().len(),
        requests,
        "second pull used the cache"
    );
}

#[tokio::test]
async fn follows_the_bearer_token_flow_anonymously() {
    let registry = TestRegistry::start_with_token_auth(None).await;
    registry.push("test/math", "1.0", &package());

    resolver(&tempfile_dir("anon-token"), RegistryClient::default())
        .pull(&reference(&registry, "test/math:1.0"))
        .await
        .unwrap();

    assert!(
        registry.requests().iter().any(|r| r == "GET /token"),
        "{:?}",
        registry.requests()
    );
}

#[tokio::test]
async fn sends_credentials_to_the_token_endpoint_only_when_given() {
    let registry = TestRegistry::start_with_token_auth(Some(("ci", "s3cret"))).await;
    registry.push("test/math", "1.0", &package());
    let target = reference(&registry, "test/math:1.0");

    let denied = resolver(&tempfile_dir("denied"), RegistryClient::default())
        .pull(&target)
        .await
        .unwrap_err();
    assert!(
        matches!(denied, PackageFetchError::Unauthorized { .. }),
        "{denied}"
    );

    let auth =
        RegistryAuth::from_json(&json!({ registry.host(): "ci:s3cret" }).to_string()).unwrap();
    resolver(&tempfile_dir("granted"), RegistryClient::new(auth))
        .pull(&target)
        .await
        .unwrap();
}

#[tokio::test]
async fn rejects_a_blob_whose_content_does_not_match_its_digest() {
    let registry = TestRegistry::start().await;
    registry.push("test/math", "1.0", &package());
    registry.corrupt_blob(
        &grauwolf_toolpkg::sha256_digest(PONG_MODULE.as_bytes()),
        b"evil",
    );

    let error = resolver(&tempfile_dir("tampered"), RegistryClient::default())
        .pull(&reference(&registry, "test/math:1.0"))
        .await
        .unwrap_err();

    assert!(
        matches!(error, PackageFetchError::DigestMismatch { .. }),
        "{error}"
    );
}

#[tokio::test]
async fn reports_an_unknown_digest_as_not_found() {
    let registry = TestRegistry::start().await;
    registry.push("test/math", "1.0", &package());
    let other = format!("sha256:{}", "0".repeat(64));
    let error = resolver(&tempfile_dir("wrong-digest"), RegistryClient::default())
        .pull(&reference(&registry, &format!("test/math:1.0@{other}")))
        .await
        .unwrap_err();
    // The registry does not know that digest.
    assert!(
        matches!(error, PackageFetchError::NotFound { .. }),
        "{error}"
    );
}

#[tokio::test]
async fn rejects_packages_of_another_media_type() {
    let registry = TestRegistry::start().await;
    registry.push(
        "test/other",
        "1",
        &package().config_media_type("application/vnd.oci.image.config.v1+json"),
    );
    let error = resolver(&tempfile_dir("media"), RegistryClient::default())
        .pull(&reference(&registry, "test/other:1"))
        .await
        .unwrap_err();
    assert!(
        matches!(error, PackageFetchError::WrongMediaType(_)),
        "{error}"
    );
}

#[tokio::test]
async fn rejects_an_unknown_abi_version() {
    let registry = TestRegistry::start().await;
    registry.push("test/future", "1", &package().abi(2));
    let error = resolver(&tempfile_dir("abi"), RegistryClient::default())
        .pull(&reference(&registry, "test/future:1"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("ABI version 2"), "{error}");
}

#[tokio::test]
async fn reports_a_missing_package_as_not_found() {
    let registry = TestRegistry::start().await;
    let error = resolver(&tempfile_dir("missing"), RegistryClient::default())
        .pull(&reference(&registry, "test/none:1"))
        .await
        .unwrap_err();
    assert!(
        matches!(error, PackageFetchError::NotFound { .. }),
        "{error}"
    );
}

#[tokio::test]
async fn detects_a_damaged_cache_entry() {
    let registry = TestRegistry::start().await;
    let digest = registry.push("test/math", "1.0", &package());
    let cache = tempfile_dir("damaged");
    let by_digest = reference(&registry, &format!("test/math@{digest}"));
    let resolver = resolver(&cache, RegistryClient::default());
    resolver.pull(&by_digest).await.unwrap();

    let module = grauwolf_toolpkg::sha256_digest(PONG_MODULE.as_bytes());
    let blob = cache
        .join("packages")
        .join(digest.replace(':', "-"))
        .join("blobs")
        .join(module.trim_start_matches("sha256:"));
    std::fs::write(blob, b"changed").unwrap();

    let error = resolver.pull(&by_digest).await.unwrap_err();
    assert!(error.to_string().contains("damaged"), "{error}");
}

/// A fresh empty directory below the target directory of the test run.
fn tempfile_dir(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "grauwolf-toolpkg-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
