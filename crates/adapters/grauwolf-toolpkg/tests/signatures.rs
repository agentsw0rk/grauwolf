//! Signed OCI packages under the tool policy (ADR-0107 stage 4): cosign signatures with public
//! keys, stored with the cache entry and checked again offline.

use std::path::{Path, PathBuf};

use grauwolf_oci::cosign::{PublicKey, SigningKey, payload, signature_tag};
use grauwolf_spec::{AgentSpec, AgentSpecExt, OciReference};
use grauwolf_toolpkg::testing::{PONG_MODULE, TestPackage, TestRegistry};
use grauwolf_toolpkg::{PackageFetchError, PackageResolver, Policy, RegistryClient};
use grauwolf_tools::ToolPolicy;
use serde_json::json;

fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "signatures-{name}-{}-{}",
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
    TestPackage::new("acme-math", "1.0.0").tool(
        "add",
        "Adds",
        json!({ "type": "object" }),
        PONG_MODULE.as_bytes(),
    )
}

/// Policy that requires signatures of `keys`.
fn requiring(keys: &[PublicKey]) -> Policy {
    let rules =
        ToolPolicy::parse("[tool_packages]\nrequire_signatures = true\noci_keys = [\"k.pub\"]\n")
            .unwrap();
    Policy::new(rules, keys.to_vec())
}

fn resolver(work: &Path, policy: Policy, locked: bool) -> PackageResolver {
    PackageResolver::new(work.join("cache"), RegistryClient::default())
        .with_policy(policy)
        .with_locked(locked)
}

fn reference(registry: &TestRegistry) -> OciReference {
    OciReference::parse(&format!("{}/team/add:1.0", registry.host())).unwrap()
}

async fn sign(registry: &TestRegistry, digest: &str, key: &SigningKey) {
    RegistryClient::default()
        .push_signature(&reference(registry), digest, key)
        .await
        .unwrap();
}

fn message(error: PackageFetchError) -> String {
    match error {
        PackageFetchError::Policy(text) => text,
        other => panic!("expected a policy error, got {other}"),
    }
}

#[tokio::test]
async fn an_unsigned_package_is_refused_when_signatures_are_required() {
    let registry = TestRegistry::start().await;
    registry.push("team/add", "1.0", &package());
    let key = SigningKey::generate();
    let error = resolver(&dir("unsigned"), requiring(&[key.public_key()]), false)
        .pull(&reference(&registry))
        .await
        .unwrap_err();
    let text = message(error);
    assert!(text.contains("require_signatures"), "{text}");
    assert!(text.contains("unsigned"), "{text}");
    assert!(
        text.contains(&format!("{}/team/add", registry.host())),
        "{text}"
    );
}

#[tokio::test]
async fn a_package_signed_with_a_policy_key_is_pulled() {
    let registry = TestRegistry::start().await;
    let digest = registry.push("team/add", "1.0", &package());
    let key = SigningKey::generate();
    sign(&registry, &digest, &key).await;
    let pulled = resolver(&dir("signed"), requiring(&[key.public_key()]), false)
        .pull(&reference(&registry))
        .await
        .unwrap();
    assert_eq!(pulled.digest, digest);
}

#[tokio::test]
async fn a_signature_of_another_key_is_refused_as_invalid() {
    let registry = TestRegistry::start().await;
    let digest = registry.push("team/add", "1.0", &package());
    sign(&registry, &digest, &SigningKey::generate()).await;
    let trusted = SigningKey::generate().public_key();
    let error = resolver(&dir("wrongkey"), requiring(&[trusted]), false)
        .pull(&reference(&registry))
        .await
        .unwrap_err();
    assert!(message(error).contains("signature invalid"));
}

#[tokio::test]
async fn a_signature_of_another_image_under_the_tag_is_refused() {
    let registry = TestRegistry::start().await;
    let digest = registry.push("team/add", "1.0", &package());
    let key = SigningKey::generate();
    // A valid signature, but of a payload for another digest, stored under this image's tag.
    let other = format!("sha256:{}", "e".repeat(64));
    let signed = payload(&format!("{}/team/add", registry.host()), &other);
    let blob = registry.add_blob(&signed);
    let config = registry.add_blob(b"{}");
    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": { "mediaType": "application/vnd.oci.image.config.v1+json", "digest": config, "size": 2 },
        "layers": [{
            "mediaType": "application/vnd.dev.cosign.simplesigning.v1+json",
            "digest": blob, "size": signed.len(),
            "annotations": { "dev.cosignproject.cosign/signature": key.sign(&signed) },
        }],
    });
    registry.push_raw_manifest(
        "team/add",
        &signature_tag(&digest),
        manifest.to_string().as_bytes(),
    );
    let error = resolver(&dir("replayed"), requiring(&[key.public_key()]), false)
        .pull(&reference(&registry))
        .await
        .unwrap_err();
    assert!(message(error).contains("signature invalid"));
}

#[tokio::test]
async fn one_valid_signature_among_several_is_enough() {
    let registry = TestRegistry::start().await;
    let digest = registry.push("team/add", "1.0", &package());
    let (other, key) = (SigningKey::generate(), SigningKey::generate());
    sign(&registry, &digest, &other).await;
    sign(&registry, &digest, &key).await;
    assert!(
        resolver(&dir("several"), requiring(&[key.public_key()]), false)
            .pull(&reference(&registry))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn a_policy_that_requires_signatures_without_oci_keys_refuses_oci_packages() {
    let registry = TestRegistry::start().await;
    registry.push("team/add", "1.0", &package());
    let error = resolver(&dir("nokeys"), requiring(&[]), false)
        .pull(&reference(&registry))
        .await
        .unwrap_err();
    assert!(message(error).contains("oci_keys"));
}

#[tokio::test]
async fn cached_signatures_are_checked_again_against_the_current_keys_without_network() {
    let registry = TestRegistry::start().await;
    let digest = registry.push("team/add", "1.0", &package());
    let (key, other) = (SigningKey::generate(), SigningKey::generate());
    sign(&registry, &digest, &key).await;
    let work = dir("offline");
    let target = reference(&registry);
    resolver(&work, requiring(&[key.public_key()]), false)
        .pull(&target)
        .await
        .unwrap();
    let pinned = OciReference {
        digest: Some(digest),
        ..target
    };
    drop(registry);

    // Same keys: still fine, from the cache and the stored signature.
    resolver(&work, requiring(&[key.public_key()]), true)
        .pull(&pinned)
        .await
        .unwrap();
    // The host trusts another key now: refused, offline, with the reason from the stored data.
    let error = resolver(&work, requiring(&[other.public_key()]), true)
        .pull(&pinned)
        .await
        .unwrap_err();
    assert!(message(error).contains("signature invalid"));
    // Also without --locked the registry is unreachable, yet the answer is the stored verdict.
    let error = resolver(&work, requiring(&[other.public_key()]), false)
        .pull(&pinned)
        .await
        .unwrap_err();
    assert!(message(error).contains("signature invalid"));
}

#[tokio::test]
async fn an_entry_cached_before_signing_was_required_is_fetched_or_refused_under_locked() {
    let registry = TestRegistry::start().await;
    let digest = registry.push("team/add", "1.0", &package());
    let key = SigningKey::generate();
    let work = dir("late");
    let target = reference(&registry);
    // Cached without any policy.
    PackageResolver::new(work.join("cache"), RegistryClient::default())
        .pull(&target)
        .await
        .unwrap();
    sign(&registry, &digest, &key).await;
    let pinned = OciReference {
        digest: Some(digest),
        ..target
    };

    let error = resolver(&work, requiring(&[key.public_key()]), true)
        .pull(&pinned)
        .await
        .unwrap_err();
    let text = message(error);
    assert!(text.contains("--locked"), "{text}");
    // Without --locked the signature is fetched once and stored.
    resolver(&work, requiring(&[key.public_key()]), false)
        .pull(&pinned)
        .await
        .unwrap();
    let requests = registry.requests().len();
    resolver(&work, requiring(&[key.public_key()]), true)
        .pull(&pinned)
        .await
        .unwrap();
    assert_eq!(
        registry.requests().len(),
        requests,
        "now verified from the cache"
    );
}

#[tokio::test]
async fn a_fresh_locked_pull_fetches_the_signature_with_the_package() {
    let registry = TestRegistry::start().await;
    let digest = registry.push("team/add", "1.0", &package());
    let key = SigningKey::generate();
    sign(&registry, &digest, &key).await;
    let work = dir("fresh");
    let text = format!("{}/team/add:1.0", registry.host());
    let spec = AgentSpec::from_json(
        &json!({ "spec_version": 1, "name": "agent",
                 "model": { "provider": "ollama", "id": "m" },
                 "strategy": { "kind": "single_run" },
                 "tool_packages": [{ "oci": text }] })
        .to_string(),
    )
    .unwrap();
    let lock = work.join("grauwolf.lock");
    // Lock without a policy, then resolve with an empty cache, `--locked` and a policy.
    let first = work.join("other-cache");
    PackageResolver::new(first, RegistryClient::default())
        .resolve(&spec, &lock)
        .await
        .unwrap();
    let tools = resolver(&work, requiring(&[key.public_key()]), true)
        .resolve(&spec, &lock)
        .await
        .unwrap();
    assert_eq!(tools.len(), 1);
}

mod inspect {
    use super::*;
    use grauwolf_toolpkg::SignatureStatus;

    async fn status(
        work: &Path,
        policy: Policy,
        registry: &TestRegistry,
        extra: &[PublicKey],
    ) -> Result<SignatureStatus, PackageFetchError> {
        resolver(work, policy, false)
            .inspect(&reference(registry), extra)
            .await
            .map(|(_, status)| status)
    }

    #[tokio::test]
    async fn inspect_reports_signed_unsigned_invalid_and_unchecked() {
        let registry = TestRegistry::start().await;
        let digest = registry.push("team/add", "1.0", &package());
        let key = SigningKey::generate();
        let work = dir("inspect");

        // Unsigned: reported, not refused, although the policy requires signatures.
        let unsigned = status(&work, requiring(&[key.public_key()]), &registry, &[]).await;
        assert_eq!(unsigned.unwrap(), SignatureStatus::Unsigned);

        sign(&registry, &digest, &key).await;
        let signed = status(&work, requiring(&[key.public_key()]), &registry, &[]).await;
        assert_eq!(
            signed.unwrap(),
            SignatureStatus::Signed {
                fingerprint: key.public_key().fingerprint()
            }
        );
        // A key given on the command line counts like a policy key.
        let extra = status(&work, Policy::default(), &registry, &[key.public_key()]).await;
        assert!(matches!(extra.unwrap(), SignatureStatus::Signed { .. }));

        let other = SigningKey::generate().public_key();
        let invalid = status(&work, Policy::default(), &registry, &[other])
            .await
            .unwrap();
        assert!(
            matches!(invalid, SignatureStatus::Invalid(_)),
            "{invalid:?}"
        );

        let unchecked = status(&work, Policy::default(), &registry, &[])
            .await
            .unwrap();
        assert_eq!(unchecked, SignatureStatus::Unchecked { signatures: 1 });
    }

    #[tokio::test]
    async fn inspect_still_applies_the_allowlist() {
        let registry = TestRegistry::start().await;
        registry.push("team/add", "1.0", &package());
        let rules = ToolPolicy::parse("[tool_packages]\nallowed_registries = []\n").unwrap();
        let error = status(
            &dir("inspect-deny"),
            Policy::new(rules, Vec::new()),
            &registry,
            &[],
        )
        .await
        .unwrap_err();
        assert!(matches!(error, PackageFetchError::Policy(_)));
    }
}
