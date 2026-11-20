//! The tool policy at work (ADR-0107 stage 4): allowlists for OCI registries and git hosts,
//! also for cache and lockfile hits, and loading of policy files.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use grauwolf_git::testing::TestSigner;
use grauwolf_oci::cosign::SigningKey;
use grauwolf_spec::{AgentSpec, AgentSpecExt, OciReference, ToolSpec};
use grauwolf_toolpkg::testing::{PONG_MODULE, TestPackage, TestRegistry};
use grauwolf_toolpkg::{ModuleBuilder, PackageFetchError, PackageResolver, Policy, RegistryClient};
use grauwolf_tools::ToolPolicy;
use serde_json::{Value, json};

fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "policy-{name}-{}-{}",
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

fn spec(packages: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" },
            "tool_packages": packages
        })
        .to_string(),
    )
    .unwrap()
}

fn policy(body: &str) -> Policy {
    Policy::new(
        ToolPolicy::parse(&format!("[tool_packages]\n{body}")).unwrap(),
        Vec::new(),
    )
}

fn resolver(work: &Path, policy: Policy) -> PackageResolver {
    PackageResolver::new(work.join("cache"), RegistryClient::default()).with_policy(policy)
}

fn policy_error(error: PackageFetchError) -> String {
    match error {
        PackageFetchError::Policy(text) => text,
        other => panic!("expected a policy error, got {other}"),
    }
}

#[tokio::test]
async fn an_allowed_registry_is_pulled_and_a_disallowed_one_is_refused_before_any_request() {
    let registry = TestRegistry::start().await;
    registry.push("team/add", "1.0", &package());
    let work = dir("allow");
    let reference = OciReference::parse(&format!("{}/team/add:1.0", registry.host())).unwrap();

    let allowed = policy(&format!(
        "allowed_registries = [\"{}/team\"]",
        registry.host()
    ));
    assert!(resolver(&work, allowed).pull(&reference).await.is_ok());

    let before = registry.requests().len();
    let denied = policy("allowed_registries = [\"ghcr.io/acme\"]");
    let error = policy_error(
        resolver(&dir("deny"), denied)
            .pull(&reference)
            .await
            .unwrap_err(),
    );
    assert!(
        error.contains(&format!("{}/team/add", registry.host())),
        "{error}"
    );
    assert!(error.contains("allowed_registries"), "{error}");
    assert_eq!(
        registry.requests().len(),
        before,
        "no request for a refused package"
    );
}

#[tokio::test]
async fn a_repository_that_only_shares_a_prefix_is_refused() {
    let registry = TestRegistry::start().await;
    registry.push("teamx/add", "1.0", &package());
    let reference = OciReference::parse(&format!("{}/teamx/add:1.0", registry.host())).unwrap();
    let allowed = policy(&format!(
        "allowed_registries = [\"{}/team\"]",
        registry.host()
    ));
    assert!(
        resolver(&dir("boundary"), allowed)
            .pull(&reference)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_policy_change_applies_to_cache_and_lockfile_without_network() {
    let registry = TestRegistry::start().await;
    registry.push("team/add", "1.0", &package());
    let work = dir("cache");
    let text = format!("{}/team/add:1.0", registry.host());
    let spec = spec(json!([{ "oci": text }]));
    let lock = work.join("grauwolf.lock");
    // First resolve without a policy: fills the cache and the lock.
    let plain = PackageResolver::new(work.join("cache"), RegistryClient::default());
    plain.resolve(&spec, &lock).await.unwrap();
    let requests = registry.requests().len();

    // The host now forbids the registry: the cached, locked package is refused as well.
    let denied = policy("allowed_registries = []");
    for locked in [false, true] {
        let error = resolver(&work, denied.clone())
            .with_locked(locked)
            .resolve(&spec, &lock)
            .await
            .unwrap_err();
        assert!(policy_error(error).contains("is not allowed"));
    }
    // An allowing policy still resolves from the cache.
    let allowed = policy(&format!("allowed_registries = [\"{}\"]", registry.host()));
    resolver(&work, allowed)
        .with_locked(true)
        .resolve(&spec, &lock)
        .await
        .unwrap();
    assert_eq!(registry.requests().len(), requests, "no network access");
}

struct NoBuild;

impl ModuleBuilder for NoBuild {
    fn prepare(&self) -> Result<(), String> {
        Ok(())
    }
    fn build(&self, _: &ToolSpec) -> Result<Vec<u8>, String> {
        Err("must not build".into())
    }
}

#[tokio::test]
async fn a_disallowed_git_host_is_refused_before_any_request() {
    let work = dir("git");
    let spec = spec(json!([{ "git": "https://evil.example/acme/tools.git", "rev": "abcdef1" }]));
    let denied = policy("allowed_git_hosts = [\"github.com/acme\"]");
    let error = resolver(&work, denied)
        .with_git(None, Arc::new(NoBuild))
        .resolve(&spec, &work.join("grauwolf.lock"))
        .await
        .unwrap_err();
    let text = policy_error(error);
    assert!(text.contains("evil.example/acme/tools"), "{text}");
    assert!(text.contains("allowed_git_hosts"), "{text}");
}

#[test]
fn a_policy_file_loads_its_keys_relative_to_itself() {
    let work = dir("load");
    std::fs::create_dir_all(work.join("keys")).unwrap();
    let key = SigningKey::generate();
    std::fs::write(work.join("keys/acme.pub"), key.public_key().to_pem()).unwrap();
    std::fs::write(
        work.join("grauwolf.toml"),
        "[tool_packages]\nrequire_signatures = true\noci_keys = [\"keys/acme.pub\"]\n",
    )
    .unwrap();
    let loaded = Policy::load(&work.join("grauwolf.toml")).unwrap();
    assert!(loaded.rules().require_signatures);
    assert_eq!(loaded.oci_keys(), [key.public_key()]);
}

#[test]
fn a_policy_file_with_a_missing_or_bad_key_is_an_error_naming_the_file() {
    let work = dir("badkey");
    let file = work.join("grauwolf.toml");
    std::fs::write(&file, "[tool_packages]\noci_keys = [\"missing.pub\"]\n").unwrap();
    let error = Policy::load(&file).unwrap_err();
    assert!(error.contains("missing.pub"), "{error}");
    std::fs::write(work.join("bad.pub"), "not a key").unwrap();
    std::fs::write(&file, "[tool_packages]\noci_keys = [\"bad.pub\"]\n").unwrap();
    assert!(Policy::load(&file).unwrap_err().contains("bad.pub"));
    std::fs::write(&file, "[tool_packages]\nbogus = 1\n").unwrap();
    assert!(Policy::load(&file).unwrap_err().contains("grauwolf.toml"));
}

#[test]
fn a_policy_file_loads_the_allowed_signers_relative_to_itself() {
    let work = dir("signers");
    let signer = TestSigner::new();
    std::fs::write(
        work.join("signers"),
        signer.allowed_signers("test@example.com", None),
    )
    .unwrap();
    let file = work.join("grauwolf.toml");
    std::fs::write(
        &file,
        "[tool_packages]\nrequire_signatures = true\ngit_allowed_signers = \"signers\"\n",
    )
    .unwrap();
    let loaded = Policy::load(&file).unwrap();
    assert_eq!(
        loaded.git_signers().unwrap().fingerprints(),
        [signer.fingerprint()]
    );
    std::fs::write(work.join("signers"), "test@example.com nonsense").unwrap();
    let error = Policy::load(&file).unwrap_err();
    assert!(error.contains("signers"), "{error}");
    std::fs::remove_file(work.join("signers")).unwrap();
    assert!(Policy::load(&file).unwrap_err().contains("signers"));
}

#[test]
fn the_example_policy_of_the_user_guide_is_valid() {
    let text = include_str!("../../../../docs/user-guide/examples/03-policy/grauwolf.toml");
    let rules = ToolPolicy::parse(text).unwrap();
    assert!(rules.require_signatures);
    assert_eq!(rules.oci_keys, ["keys/team.pub"]);
    let signers = include_str!("../../../../docs/user-guide/examples/03-policy/allowed_signers");
    assert_eq!(
        grauwolf_git::AllowedSigners::parse(signers)
            .unwrap()
            .fingerprints()
            .len(),
        1
    );
}
