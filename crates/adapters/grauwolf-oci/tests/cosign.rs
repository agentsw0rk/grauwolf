//! Cosign-compatible signatures with public keys (ADR-0107 stage 4): keys, payload, verification.

use grauwolf_oci::cosign::{
    CosignError, PublicKey, Signature, SigningKey, payload, signature_tag, verify,
};

const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const IDENTITY: &str = "ghcr.io/acme/tools/add";

fn signed(key: &SigningKey, identity: &str, digest: &str) -> Signature {
    let payload = payload(identity, digest);
    Signature {
        signature: key.sign(&payload),
        payload,
    }
}

#[test]
fn the_signature_tag_is_the_digest_with_a_dash_and_sig() {
    assert_eq!(
        signature_tag(DIGEST),
        "sha256-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.sig"
    );
}

#[test]
fn the_payload_is_cosign_simple_signing() {
    let value: serde_json::Value = serde_json::from_slice(&payload(IDENTITY, DIGEST)).unwrap();
    assert_eq!(
        value["critical"]["type"],
        "cosign container image signature"
    );
    assert_eq!(value["critical"]["identity"]["docker-reference"], IDENTITY);
    assert_eq!(value["critical"]["image"]["docker-manifest-digest"], DIGEST);
    assert!(value["optional"].is_null());
}

#[test]
fn keys_round_trip_through_pem_and_a_signature_verifies() {
    let key = SigningKey::generate();
    let again = SigningKey::from_pem(&key.to_pem()).unwrap();
    assert!(key.to_pem().starts_with("-----BEGIN PRIVATE KEY-----"));
    let public = PublicKey::from_pem(&key.public_key().to_pem()).unwrap();
    assert!(public.to_pem().starts_with("-----BEGIN PUBLIC KEY-----"));
    assert_eq!(public, again.public_key());
    assert!(public.fingerprint().starts_with("SHA256:"));
    let fingerprint = verify(
        &[signed(&key, IDENTITY, DIGEST)],
        std::slice::from_ref(&public),
        IDENTITY,
        DIGEST,
    );
    assert_eq!(fingerprint, Ok(public.fingerprint()));
}

#[test]
fn no_signatures_means_unsigned() {
    let key = SigningKey::generate().public_key();
    assert_eq!(
        verify(&[], &[key], IDENTITY, DIGEST),
        Err(CosignError::Unsigned)
    );
}

#[test]
fn a_signature_of_another_key_is_invalid() {
    let other = SigningKey::generate();
    let trusted = SigningKey::generate().public_key();
    let result = verify(
        &[signed(&other, IDENTITY, DIGEST)],
        &[trusted],
        IDENTITY,
        DIGEST,
    );
    assert!(matches!(result, Err(CosignError::Invalid(_))), "{result:?}");
}

#[test]
fn a_tampered_payload_is_invalid() {
    let key = SigningKey::generate();
    let mut signature = signed(&key, IDENTITY, DIGEST);
    signature.payload.push(b' ');
    let result = verify(&[signature], &[key.public_key()], IDENTITY, DIGEST);
    assert!(matches!(result, Err(CosignError::Invalid(_))), "{result:?}");
}

#[test]
fn a_signature_of_another_digest_or_repository_does_not_count() {
    let key = SigningKey::generate();
    let public = key.public_key();
    let other_digest = format!("sha256:{}", "f".repeat(64));
    let result = verify(
        &[signed(&key, IDENTITY, &other_digest)],
        std::slice::from_ref(&public),
        IDENTITY,
        DIGEST,
    );
    assert!(matches!(result, Err(CosignError::Invalid(_))), "{result:?}");
    let result = verify(
        &[signed(&key, "ghcr.io/evil/add", DIGEST)],
        &[public],
        IDENTITY,
        DIGEST,
    );
    assert!(matches!(result, Err(CosignError::Invalid(_))), "{result:?}");
}

#[test]
fn one_valid_signature_among_several_is_enough() {
    let key = SigningKey::generate();
    let other = SigningKey::generate();
    let signatures = [
        signed(&other, IDENTITY, DIGEST),
        signed(&key, IDENTITY, DIGEST),
    ];
    assert!(verify(&signatures, &[key.public_key()], IDENTITY, DIGEST).is_ok());
}

#[test]
fn unusable_keys_are_errors_that_say_so() {
    assert!(matches!(
        SigningKey::from_pem("nonsense"),
        Err(CosignError::Key(_))
    ));
    assert!(matches!(
        PublicKey::from_pem("nonsense"),
        Err(CosignError::Key(_))
    ));
    let key = SigningKey::generate();
    assert!(PublicKey::from_pem(&key.to_pem()).is_err());
}

mod registry {
    use super::*;
    use grauwolf_oci::RegistryClient;
    use grauwolf_oci::testing::TestRegistry;
    use grauwolf_spec::OciReference;

    fn reference(registry: &TestRegistry) -> OciReference {
        OciReference::parse(&format!("{}/team/add:1.0", registry.host())).unwrap()
    }

    fn identity(registry: &TestRegistry) -> String {
        format!("{}/team/add", registry.host())
    }

    #[tokio::test]
    async fn an_image_without_signature_image_has_no_signatures() {
        let registry = TestRegistry::start().await;
        let found = RegistryClient::default()
            .signatures(&reference(&registry), DIGEST)
            .await
            .unwrap();
        assert!(found.is_empty());
    }

    #[tokio::test]
    async fn a_pushed_signature_is_found_and_verifies() {
        let registry = TestRegistry::start().await;
        let client = RegistryClient::default();
        let key = SigningKey::generate();
        client
            .push_signature(&reference(&registry), DIGEST, &key)
            .await
            .unwrap();
        let found = client
            .signatures(&reference(&registry), DIGEST)
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        let result = verify(&found, &[key.public_key()], &identity(&registry), DIGEST);
        assert_eq!(result, Ok(key.public_key().fingerprint()));
        // Stored where cosign looks: the tag sha256-<hex>.sig of the same repository.
        let tag = reference_with_tag(&registry, &signature_tag(DIGEST));
        assert!(client.manifest(&tag).await.is_ok());
    }

    fn reference_with_tag(registry: &TestRegistry, tag: &str) -> OciReference {
        OciReference::parse(&format!("{}/team/add:{tag}", registry.host())).unwrap()
    }

    #[tokio::test]
    async fn a_second_signature_is_added_and_the_first_is_kept() {
        let registry = TestRegistry::start().await;
        let client = RegistryClient::default();
        let (first, second) = (SigningKey::generate(), SigningKey::generate());
        let target = reference(&registry);
        client
            .push_signature(&target, DIGEST, &first)
            .await
            .unwrap();
        client
            .push_signature(&target, DIGEST, &second)
            .await
            .unwrap();
        // Signing again with the same key adds nothing.
        client
            .push_signature(&target, DIGEST, &second)
            .await
            .unwrap();
        let found = client.signatures(&target, DIGEST).await.unwrap();
        assert_eq!(found.len(), 2);
        let id = identity(&registry);
        assert!(verify(&found, &[first.public_key()], &id, DIGEST).is_ok());
        assert!(verify(&found, &[second.public_key()], &id, DIGEST).is_ok());
    }

    #[tokio::test]
    async fn layers_that_are_no_signatures_are_ignored() {
        let registry = TestRegistry::start().await;
        let blob = registry.add_blob(b"{}");
        let manifest = serde_json::json!({
            "schemaVersion": 2,
            "mediaType": grauwolf_oci::IMAGE_MANIFEST,
            "config": { "mediaType": "application/vnd.oci.image.config.v1+json", "digest": blob, "size": 2 },
            "layers": [
                { "mediaType": "application/x.other", "digest": blob, "size": 2,
                  "annotations": { "dev.cosignproject.cosign/signature": "AAAA" } },
                { "mediaType": "application/vnd.dev.cosign.simplesigning.v1+json",
                  "digest": blob, "size": 2 },
            ],
        })
        .to_string();
        registry.push_raw_manifest("team/add", &signature_tag(DIGEST), manifest.as_bytes());
        let found = RegistryClient::default()
            .signatures(&reference(&registry), DIGEST)
            .await
            .unwrap();
        assert!(found.is_empty());
    }
}
