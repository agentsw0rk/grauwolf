//! Signatures of git sources under `require_signatures` (ADR-0107 stage 4): commits and tags are
//! checked against the allowed-signers file of the policy, fresh, from the stored objects and
//! (when allowed) by fetching them again. Against the in-process test git server.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use grauwolf_git::AllowedSigners;
use grauwolf_git::testing::{
    TestGitServer, TestSigner, commit_files, commit_files_signed, tag, tag_signed,
};
use grauwolf_spec::{AgentSpec, AgentSpecExt, ToolSpec};
use grauwolf_toolpkg::{ModuleBuilder, PackageFetchError, PackageResolver, Policy, RegistryClient};
use grauwolf_tools::ToolPolicy;
use serde_json::{Value, json};

const MANIFEST: &str =
    include_str!("../../../../docs/user-guide/examples/03-package/grauwolf-tools.json");
const ADD: &str = include_str!("../../../../docs/user-guide/examples/03-package/src/add.rs");
const COUNT: &str =
    include_str!("../../../../docs/user-guide/examples/03-package/src/count_words.rs");

fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "gitsig-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Builds a module per tool and counts the builds.
#[derive(Default)]
struct Fake(AtomicUsize);

impl ModuleBuilder for Fake {
    fn prepare(&self) -> Result<(), String> {
        Ok(())
    }

    fn build(&self, tool: &ToolSpec) -> Result<Vec<u8>, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(format!("module of {}", tool.name).into_bytes())
    }
}

fn package_files() -> BTreeMap<String, Vec<u8>> {
    [
        ("grauwolf-tools.json", MANIFEST),
        ("src/add.rs", ADD),
        ("src/count_words.rs", COUNT),
    ]
    .into_iter()
    .map(|(name, text)| (name.to_owned(), text.as_bytes().to_vec()))
    .collect()
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

/// A policy that requires signatures from `signers`.
fn policy(signers: &[&TestSigner]) -> Policy {
    let lines: String = signers
        .iter()
        .map(|signer| signer.allowed_signers("test@example.com", None))
        .collect();
    Policy::new(
        ToolPolicy::parse(
            "[tool_packages]\nrequire_signatures = true\ngit_allowed_signers = \"signers\"\n",
        )
        .unwrap(),
        Vec::new(),
    )
    .with_git_signers(AllowedSigners::parse(&lines).unwrap())
}

fn resolver(
    work: &Path,
    builder: &Arc<Fake>,
    policy: Option<Policy>,
    locked: bool,
) -> PackageResolver {
    let resolver = PackageResolver::new(work.join("cache"), RegistryClient::default())
        .with_locked(locked)
        .with_git(None, builder.clone());
    match policy {
        Some(policy) => resolver.with_policy(policy),
        None => resolver,
    }
}

fn refusal(error: PackageFetchError) -> String {
    match error {
        PackageFetchError::Policy(text) => text,
        other => panic!("expected a policy error, got {other}"),
    }
}

/// Removes the stored git objects of every cache entry, as if cached by an older version.
fn forget_objects(work: &Path) {
    for entry in std::fs::read_dir(work.join("cache/packages")).unwrap() {
        let entry = entry.unwrap().path();
        for name in ["commit.object", "tag.object"] {
            let _ = std::fs::remove_file(entry.join(name));
        }
    }
}

#[tokio::test]
async fn an_unsigned_commit_is_refused_before_anything_is_built() {
    let work = dir("unsigned");
    let repository = work.join("repo.git");
    let commit = commit_files(&repository, "main", "one", &package_files()).to_string();
    let server = TestGitServer::start(&repository, None).await;
    let builder = Arc::new(Fake::default());
    let spec = spec(json!([{ "git": server.url("r"), "rev": &commit }]));
    let signer = TestSigner::new();
    let error = resolver(&work, &builder, Some(policy(&[&signer])), false)
        .resolve(&spec, &work.join("grauwolf.lock"))
        .await
        .unwrap_err();
    let text = refusal(error);
    assert!(text.contains("require_signatures"), "{text}");
    assert!(text.contains("unsigned"), "{text}");
    assert!(text.contains(&commit), "{text}");
    assert_eq!(builder.0.load(Ordering::SeqCst), 0);
    assert!(!work.join("grauwolf.lock").exists());
}

#[tokio::test]
async fn a_commit_signed_by_an_allowed_signer_is_accepted_for_rev_and_branch() {
    let work = dir("signed");
    let repository = work.join("repo.git");
    let signer = TestSigner::new();
    let commit =
        commit_files_signed(&repository, "main", "one", &package_files(), &signer).to_string();
    let server = TestGitServer::start(&repository, None).await;
    for reference in [json!({ "rev": &commit }), json!({ "branch": "main" })] {
        let mut entry = json!({ "git": server.url("r") });
        entry
            .as_object_mut()
            .unwrap()
            .extend(reference.as_object().unwrap().clone());
        let spec = spec(json!([entry]));
        let work = work.join(format!("{}", reference.to_string().len()));
        std::fs::create_dir_all(&work).unwrap();
        let tools = resolver(
            &work,
            &Arc::new(Fake::default()),
            Some(policy(&[&signer])),
            false,
        )
        .resolve(&spec, &work.join("grauwolf.lock"))
        .await
        .unwrap();
        assert_eq!(tools.len(), 2);
    }
}

#[tokio::test]
async fn a_commit_signed_by_another_key_is_refused() {
    let work = dir("wrong");
    let repository = work.join("repo.git");
    let stranger = TestSigner::new();
    let commit =
        commit_files_signed(&repository, "main", "one", &package_files(), &stranger).to_string();
    let server = TestGitServer::start(&repository, None).await;
    let spec = spec(json!([{ "git": server.url("r"), "rev": &commit }]));
    let trusted = TestSigner::new();
    let error = resolver(
        &work,
        &Arc::new(Fake::default()),
        Some(policy(&[&trusted])),
        false,
    )
    .resolve(&spec, &work.join("grauwolf.lock"))
    .await
    .unwrap_err();
    let text = refusal(error);
    assert!(text.contains("signature invalid"), "{text}");
}

#[tokio::test]
async fn a_signed_tag_on_an_unsigned_commit_is_accepted_but_a_lightweight_tag_is_not() {
    let work = dir("tag");
    let repository = work.join("repo.git");
    let signer = TestSigner::new();
    let commit = commit_files(&repository, "main", "one", &package_files());
    tag_signed(&repository, "v1", commit, &signer);
    tag(&repository, "light", commit, false);
    let server = TestGitServer::start(&repository, None).await;

    let signed = spec(json!([{ "git": server.url("r"), "tag": "v1" }]));
    resolver(
        &work,
        &Arc::new(Fake::default()),
        Some(policy(&[&signer])),
        false,
    )
    .resolve(&signed, &work.join("grauwolf.lock"))
    .await
    .unwrap();

    let light = spec(json!([{ "git": server.url("r"), "tag": "light" }]));
    let other = work.join("other");
    std::fs::create_dir_all(&other).unwrap();
    let error = resolver(
        &other,
        &Arc::new(Fake::default()),
        Some(policy(&[&signer])),
        false,
    )
    .resolve(&light, &other.join("grauwolf.lock"))
    .await
    .unwrap_err();
    assert!(refusal(error).contains("unsigned"));
}

#[tokio::test]
async fn a_cached_signed_package_is_verified_again_without_network_and_when_locked() {
    let work = dir("offline");
    let repository = work.join("repo.git");
    let signer = TestSigner::new();
    let commit =
        commit_files_signed(&repository, "main", "one", &package_files(), &signer).to_string();
    let server = TestGitServer::start(&repository, None).await;
    let spec = spec(json!([{ "git": server.url("r"), "rev": &commit }]));
    let lock = work.join("grauwolf.lock");
    resolver(
        &work,
        &Arc::new(Fake::default()),
        Some(policy(&[&signer])),
        false,
    )
    .resolve(&spec, &lock)
    .await
    .unwrap();
    let requests = server.requests().len();
    for locked in [false, true] {
        resolver(
            &work,
            &Arc::new(Fake::default()),
            Some(policy(&[&signer])),
            locked,
        )
        .resolve(&spec, &lock)
        .await
        .unwrap();
    }
    assert_eq!(server.requests().len(), requests, "no network access");
}

#[tokio::test]
async fn a_changed_policy_applies_to_the_cached_package_without_network() {
    let work = dir("change");
    let repository = work.join("repo.git");
    let signer = TestSigner::new();
    let commit =
        commit_files_signed(&repository, "main", "one", &package_files(), &signer).to_string();
    let server = TestGitServer::start(&repository, None).await;
    let spec = spec(json!([{ "git": server.url("r"), "rev": &commit }]));
    let lock = work.join("grauwolf.lock");
    resolver(
        &work,
        &Arc::new(Fake::default()),
        Some(policy(&[&signer])),
        false,
    )
    .resolve(&spec, &lock)
    .await
    .unwrap();
    let requests = server.requests().len();
    let other = TestSigner::new();
    let error = resolver(
        &work,
        &Arc::new(Fake::default()),
        Some(policy(&[&other])),
        true,
    )
    .resolve(&spec, &lock)
    .await
    .unwrap_err();
    assert!(refusal(error).contains("--locked"));
    assert_eq!(server.requests().len(), requests, "no network access");
}

#[tokio::test]
async fn a_package_cached_before_the_policy_is_verified_by_fetching_the_objects() {
    let work = dir("before");
    let repository = work.join("repo.git");
    let signer = TestSigner::new();
    let commit =
        commit_files_signed(&repository, "main", "one", &package_files(), &signer).to_string();
    let server = TestGitServer::start(&repository, None).await;
    let spec = spec(json!([{ "git": server.url("r"), "rev": &commit }]));
    let lock = work.join("grauwolf.lock");
    resolver(&work, &Arc::new(Fake::default()), None, false)
        .resolve(&spec, &lock)
        .await
        .unwrap();
    forget_objects(&work);

    // `--locked` must not fetch, and says so.
    let error = resolver(
        &work,
        &Arc::new(Fake::default()),
        Some(policy(&[&signer])),
        true,
    )
    .resolve(&spec, &lock)
    .await
    .unwrap_err();
    assert!(refusal(error).contains("--locked"));

    let builder = Arc::new(Fake::default());
    resolver(&work, &builder, Some(policy(&[&signer])), false)
        .resolve(&spec, &lock)
        .await
        .unwrap();
    assert_eq!(builder.0.load(Ordering::SeqCst), 0, "no rebuild");
    // The objects are stored now: the check works offline again.
    server.stop().await;
    resolver(
        &work,
        &Arc::new(Fake::default()),
        Some(policy(&[&signer])),
        true,
    )
    .resolve(&spec, &lock)
    .await
    .unwrap();
}

#[tokio::test]
async fn a_git_source_needs_the_allowed_signers_when_signatures_are_required() {
    let work = dir("nosigners");
    let repository = work.join("repo.git");
    let signer = TestSigner::new();
    let commit =
        commit_files_signed(&repository, "main", "one", &package_files(), &signer).to_string();
    let server = TestGitServer::start(&repository, None).await;
    let spec = spec(json!([{ "git": server.url("r"), "rev": &commit }]));
    let without = Policy::new(
        ToolPolicy::parse("[tool_packages]\nrequire_signatures = true\noci_keys = [\"k\"]\n")
            .unwrap(),
        Vec::new(),
    );
    let error = resolver(&work, &Arc::new(Fake::default()), Some(without), false)
        .resolve(&spec, &work.join("grauwolf.lock"))
        .await
        .unwrap_err();
    assert!(refusal(error).contains("git_allowed_signers"));
}
