//! Pushing images to a registry (ADR-0107): upload, round trip, token flow with the push scope,
//! refusals, secrets, Docker credentials.

use grauwolf_oci::testing::TestRegistry;
use grauwolf_oci::{
    IMAGE_MANIFEST, OciError, OciImage, RegistryAuth, RegistryClient, sha256_digest,
};
use grauwolf_spec::OciReference;

fn image() -> OciImage {
    image_with_layer(b"layer content".to_vec())
}

fn image_with_layer(layer: Vec<u8>) -> OciImage {
    let config = br#"{"kind":"config"}"#.to_vec();
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

fn reference(registry: &TestRegistry, rest: &str) -> OciReference {
    OciReference::parse(&format!("{}/{rest}", registry.host())).unwrap()
}

fn auth(registry: &TestRegistry, value: &str) -> RegistryAuth {
    RegistryAuth::from_json(&serde_json::json!({ registry.host(): value }).to_string()).unwrap()
}

fn cache(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("grauwolf-oci-push-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn a_pushed_image_can_be_fetched_with_the_same_digest() {
    let registry = TestRegistry::start().await;
    let image = image();
    let target = reference(&registry, "team/add:1.0");
    let client = RegistryClient::default();

    let pushed = client.push(&target, &image).await.unwrap();
    assert_eq!(pushed, image.digest);

    let fetched = client.manifest(&target).await.unwrap();
    assert_eq!(fetched.digest, image.digest);
    assert_eq!(fetched.bytes, image.manifest);
    for (digest, bytes) in &image.blobs {
        assert_eq!(&client.blob(&target, digest).await.unwrap(), bytes);
    }
}

#[tokio::test]
async fn pushing_the_same_large_image_again_works() {
    let registry = TestRegistry::start().await;
    let image = image_with_layer(vec![7u8; 100 * 1024]);
    let target = reference(&registry, "team/big:1.0");
    let client = RegistryClient::new_http2_prior_knowledge(RegistryAuth::none());

    client.push(&target, &image).await.unwrap();
    let again = client.push(&target, &image).await.unwrap();
    assert_eq!(again, image.digest);
}

#[tokio::test]
async fn pushing_with_the_token_flow_asks_for_the_push_scope() {
    let registry = TestRegistry::start_with_token_auth(Some(("ci", "s3cret"))).await;
    let image = image();
    let target = reference(&registry, "team/add:1.0");

    let client = RegistryClient::new(auth(&registry, "ci:s3cret"));
    client.push(&target, &image).await.unwrap();

    assert!(registry.requests().iter().any(|r| r == "GET /token"));
    assert_eq!(client.manifest(&target).await.unwrap().digest, image.digest);
}

#[tokio::test]
async fn blobs_the_registry_already_has_are_not_uploaded_again() {
    let registry = TestRegistry::start().await;
    let package = image();
    let client = RegistryClient::default();
    client
        .push(&reference(&registry, "team/add:1.0"), &package)
        .await
        .unwrap();
    let uploads = |registry: &TestRegistry| {
        registry
            .requests()
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count()
    };
    let first = uploads(&registry);
    assert_eq!(first, package.blobs.len());

    client
        .push(&reference(&registry, "team/add:2.0"), &package)
        .await
        .unwrap();

    assert_eq!(uploads(&registry), first, "no blob was uploaded again");
}

#[tokio::test]
async fn a_registry_that_issues_no_push_token_refuses_the_push() {
    let registry = TestRegistry::start_with_token_auth(None).await;
    let error = RegistryClient::default()
        .push(&reference(&registry, "team/add:1.0"), &image())
        .await
        .unwrap_err();
    assert!(matches!(error, OciError::Unauthorized { .. }), "{error}");
}

#[tokio::test]
async fn wrong_credentials_are_refused_and_never_appear_in_the_error() {
    let registry = TestRegistry::start_with_token_auth(Some(("ci", "s3cret"))).await;
    let client = RegistryClient::new(auth(&registry, "ci:wr0ng-secret"));
    let error = client
        .push(&reference(&registry, "team/add:1.0"), &image())
        .await
        .unwrap_err();
    assert!(matches!(error, OciError::Unauthorized { .. }), "{error}");
    let shown = format!("{error} {error:?}");
    assert!(!shown.contains("wr0ng-secret") && !shown.contains("s3cret"));
}

#[tokio::test]
async fn a_digest_reference_must_name_the_package() {
    let registry = TestRegistry::start().await;
    let other = format!("sha256:{}", "1".repeat(64));
    let error = RegistryClient::default()
        .push(
            &reference(&registry, &format!("team/add@{other}")),
            &image(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains(&other), "{error}");
    assert!(registry.requests().is_empty());
}

#[tokio::test]
async fn the_registry_refuses_a_manifest_whose_blobs_are_missing() {
    let registry = TestRegistry::start().await;
    let package = image();
    let url = format!("http://{}/v2/team/add/manifests/1.0", registry.host());

    let answer = reqwest::Client::new()
        .put(url)
        .header("content-type", IMAGE_MANIFEST)
        .body(package.manifest.clone())
        .send()
        .await
        .unwrap();

    assert_eq!(answer.status(), 400);
}

#[tokio::test]
async fn credentials_come_from_a_docker_configuration_behind_the_environment() {
    use base64::Engine;
    let registry = TestRegistry::start_with_token_auth(Some(("ci", "s3cret"))).await;
    let dir = cache("docker-config");
    let encoded = base64::engine::general_purpose::STANDARD.encode("ci:s3cret");
    let config = dir.join("config.json");
    std::fs::write(
        &config,
        serde_json::json!({ "auths": { registry.host(): { "auth": encoded } } }).to_string(),
    )
    .unwrap();
    let package = image();
    let target = reference(&registry, "team/add:1.0");

    let docker_only = RegistryAuth::none().with_docker_config(&config, vec![]);
    RegistryClient::new(docker_only)
        .push(&target, &package)
        .await
        .unwrap();

    // Wrong explicit credentials are not rescued by the Docker configuration.
    let explicit = auth(&registry, "ci:wr0ng").with_docker_config(&config, vec![]);
    let error = RegistryClient::new(explicit)
        .push(&target, &package)
        .await
        .unwrap_err();
    assert!(matches!(error, OciError::Unauthorized { .. }));
    assert!(error.to_string().contains("Docker"), "{error}");
}

#[tokio::test]
async fn a_missing_credential_helper_does_not_break_anonymous_pulls() {
    let registry = TestRegistry::start().await;
    let dir = cache("docker-helper");
    let config = dir.join("config.json");
    std::fs::write(&config, r#"{"credsStore":"nonexistent"}"#).unwrap();
    let auth = RegistryAuth::none().with_docker_config(&config, vec![dir.clone()]);
    let client = RegistryClient::new(auth);
    // The anonymous registry never challenges, so the helper is not consulted.
    client
        .push(&reference(&registry, "team/add:1.0"), &image())
        .await
        .unwrap();

    let secured = TestRegistry::start_with_token_auth(Some(("ci", "s3cret"))).await;
    let error = client
        .push(&reference(&secured, "team/add:1.0"), &image())
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("docker-credential-nonexistent"),
        "{error}"
    );
}

#[tokio::test]
async fn an_identity_token_of_the_docker_configuration_is_sent_after_a_challenge() {
    let registry = TestRegistry::start_with_token_auth(None).await;
    let package = image();
    let target = reference(&registry, "team/add:1.0");
    RegistryClient::default()
        .push(&target, &package)
        .await
        .unwrap_err();
    // The registry issues no push token to anonymous clients; a token the Docker configuration
    // carries is used as it is for reading.
    let dir = cache("identity");
    let config = dir.join("config.json");
    std::fs::write(
        &config,
        serde_json::json!({ "auths": { registry.host(): { "identitytoken": "test-token" } } })
            .to_string(),
    )
    .unwrap();
    let client = RegistryClient::new(RegistryAuth::none().with_docker_config(&config, vec![]));
    let error = client.manifest(&target).await.unwrap_err();
    // Nothing was pushed, so the manifest is unknown: the token was accepted, access is not
    // what failed.
    assert!(matches!(error, OciError::NotFound { .. }), "{error}");
    assert!(
        registry
            .requests()
            .iter()
            .filter(|r| r.contains("manifests"))
            .count()
            >= 2
    );
}
