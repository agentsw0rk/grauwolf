//! Fetching manifests and blobs: verification against digests, not found, the bearer-token flow.

use grauwolf_oci::testing::TestRegistry;
use grauwolf_oci::{OciError, RegistryClient, base_url, sha256_digest};
use grauwolf_spec::OciReference;

fn reference(registry: &TestRegistry, rest: &str) -> OciReference {
    OciReference::parse(&format!("{}/{rest}", registry.host())).unwrap()
}

#[tokio::test]
async fn a_manifest_is_fetched_by_tag_and_by_digest() {
    let registry = TestRegistry::start().await;
    let manifest = br#"{"schemaVersion":2}"#;
    let digest = registry.push_raw_manifest("team/x", "1.0", manifest);
    let client = RegistryClient::default();

    let by_tag = client.manifest(&reference(&registry, "team/x:1.0")).await;
    assert_eq!(by_tag.unwrap().digest, digest);
    let by_digest = client
        .manifest(&reference(&registry, &format!("team/x@{digest}")))
        .await;
    assert_eq!(by_digest.unwrap().bytes, manifest);
}

#[tokio::test]
async fn index_and_docker_manifests_are_fetched_in_their_own_representation() {
    let registry = TestRegistry::start().await;
    let client = RegistryClient::default();
    for (n, media_type) in [
        "application/vnd.oci.image.index.v1+json",
        "application/vnd.docker.distribution.manifest.list.v2+json",
        "application/vnd.docker.distribution.manifest.v2+json",
    ]
    .into_iter()
    .enumerate()
    {
        let manifest = serde_json::json!({ "schemaVersion": 2, "mediaType": media_type, "n": n });
        let digest = registry.push_raw_manifest(
            "team/img",
            &format!("t{n}"),
            manifest.to_string().as_bytes(),
        );
        let fetched = client
            .manifest(&reference(&registry, &format!("team/img:t{n}")))
            .await
            .unwrap_or_else(|error| panic!("{media_type}: {error}"));
        assert_eq!(fetched.digest, digest, "{media_type}");
    }
}

#[tokio::test]
async fn a_blob_that_does_not_match_its_digest_is_rejected() {
    let registry = TestRegistry::start().await;
    let digest = registry.add_blob(b"original");
    registry.corrupt_blob(&digest, b"tampered");
    let error = RegistryClient::default()
        .blob(&reference(&registry, "team/x:1.0"), &digest)
        .await
        .unwrap_err();
    assert!(matches!(error, OciError::DigestMismatch { .. }), "{error}");
}

#[tokio::test]
async fn an_unknown_blob_is_not_found() {
    let registry = TestRegistry::start().await;
    let error = RegistryClient::default()
        .blob(&reference(&registry, "team/x:1.0"), &sha256_digest(b"none"))
        .await
        .unwrap_err();
    assert!(matches!(error, OciError::NotFound { .. }), "{error}");
}

#[tokio::test]
async fn the_bearer_token_flow_is_followed_anonymously() {
    let registry = TestRegistry::start_with_token_auth(None).await;
    registry.push_raw_manifest("team/x", "1.0", b"{}");
    let client = RegistryClient::default();
    client
        .manifest(&reference(&registry, "team/x:1.0"))
        .await
        .unwrap();
    assert!(registry.requests().iter().any(|r| r == "GET /token"));
}

#[test]
fn plain_http_is_only_used_for_local_registries() {
    assert_eq!(base_url("localhost:5000"), "http://localhost:5000");
    assert_eq!(base_url("127.0.0.1:1234"), "http://127.0.0.1:1234");
    assert_eq!(base_url("ghcr.io"), "https://ghcr.io");
    assert_eq!(base_url("localhost.evil.com"), "https://localhost.evil.com");
}
