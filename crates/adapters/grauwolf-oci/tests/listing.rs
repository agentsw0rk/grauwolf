//! Listing tags and repositories and writing manifests conditionally (ADR-0124).

use grauwolf_oci::testing::TestRegistry;
use grauwolf_oci::{IMAGE_MANIFEST, OciError, OciImage, RegistryClient, sha256_digest};
use grauwolf_spec::OciReference;

fn image(content: &str) -> OciImage {
    let config = b"{}".to_vec();
    let layer = content.as_bytes().to_vec();
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": IMAGE_MANIFEST,
        "config": { "mediaType": "application/x.config", "digest": sha256_digest(&config), "size": config.len() },
        "layers": [{ "mediaType": "application/x.layer", "digest": sha256_digest(&layer), "size": layer.len() }],
    })
    .to_string()
    .into_bytes();
    OciImage {
        digest: sha256_digest(&manifest),
        manifest,
        blobs: [
            (sha256_digest(&config), config),
            (sha256_digest(&layer), layer),
        ]
        .into(),
    }
}

fn reference(registry: &TestRegistry, path: &str) -> OciReference {
    OciReference::parse(&format!("{}/{path}", registry.host())).unwrap()
}

#[tokio::test]
async fn tags_lists_the_tags_of_a_repository() {
    let registry = TestRegistry::start().await;
    let client = RegistryClient::default();
    for tag in ["1.0", "1.1"] {
        client
            .push(
                &reference(&registry, &format!("acme/math:{tag}")),
                &image(tag),
            )
            .await
            .unwrap();
    }
    let tags = client
        .tags(&reference(&registry, "acme/math"))
        .await
        .unwrap();
    assert_eq!(tags, ["1.0", "1.1"]);
    assert!(
        client
            .tags(&reference(&registry, "acme/none"))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn repositories_come_from_the_catalog_endpoint_where_there_is_one() {
    let registry = TestRegistry::start().await;
    let client = RegistryClient::default();
    for path in ["acme/math:1", "acme/text:1"] {
        client
            .push(&reference(&registry, path), &image(path))
            .await
            .unwrap();
    }
    assert_eq!(client.repositories(&registry.host()).await.unwrap(), None);
    registry.offer_catalog();
    assert_eq!(
        client.repositories(&registry.host()).await.unwrap(),
        Some(vec!["acme/math".to_owned(), "acme/text".to_owned()])
    );
}

#[tokio::test]
async fn a_conditional_push_fails_when_the_tag_moved() {
    let registry = TestRegistry::start().await;
    registry.honour_conditional_writes();
    let client = RegistryClient::default();
    let target = reference(&registry, "acme/index:latest");
    let first = client
        .push_if(&target, &image("first"), Some(None))
        .await
        .unwrap();

    let again = client.push_if(&target, &image("again"), Some(None)).await;
    assert!(matches!(again, Err(OciError::Conflict { .. })), "{again:?}");
    let stale = client
        .push_if(&target, &image("stale"), Some(Some("sha256:0000")))
        .await;
    assert!(matches!(stale, Err(OciError::Conflict { .. })), "{stale:?}");
    client
        .push_if(&target, &image("next"), Some(Some(&first)))
        .await
        .unwrap();
    assert_eq!(
        registry.manifest("acme/index", "latest").unwrap(),
        image("next").manifest
    );
}
