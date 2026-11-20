//! `pull_git` and `inspect_git`: what `grauwolf tools pull|inspect` use for git packages
//! (ADR-0107). Against the in-process test git server.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use grauwolf_git::AllowedSigners;
use grauwolf_git::testing::{
    TestGitServer, TestSigner, commit_files, commit_files_signed, tag_signed,
};
use grauwolf_spec::{AgentSpec, AgentSpecExt, GitSource, ToolSpec};
use grauwolf_toolpkg::{
    GitSignatureStatus, ModuleBuilder, PackageFetchError, PackageResolver, Policy, RegistryClient,
};
use grauwolf_tools::ToolPolicy;
use serde_json::json;

const MANIFEST: &str =
    include_str!("../../../../docs/user-guide/examples/03-package/grauwolf-tools.json");
const ADD: &str = include_str!("../../../../docs/user-guide/examples/03-package/src/add.rs");
const COUNT: &str =
    include_str!("../../../../docs/user-guide/examples/03-package/src/count_words.rs");

fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "gitinspect-{name}-{}-{}",
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

fn source(text: &str) -> GitSource {
    GitSource::parse_reference(text).unwrap()
}

fn signers(signers: &[&TestSigner]) -> AllowedSigners {
    let lines: String = signers
        .iter()
        .map(|signer| signer.allowed_signers("test@example.com", None))
        .collect();
    AllowedSigners::parse(&lines).unwrap()
}

fn policy(rules: &str, allowed: Option<&[&TestSigner]>) -> Policy {
    let policy = Policy::new(ToolPolicy::parse(rules).unwrap(), Vec::new());
    match allowed {
        Some(allowed) => policy.with_git_signers(signers(allowed)),
        None => policy,
    }
}

fn resolver(work: &Path, builder: &Arc<Fake>, policy: Option<Policy>) -> PackageResolver {
    let resolver = PackageResolver::new(work.join("cache"), RegistryClient::default())
        .with_git(None, builder.clone());
    match policy {
        Some(policy) => resolver.with_policy(policy),
        None => resolver,
    }
}

const REQUIRE: &str =
    "[tool_packages]\nrequire_signatures = true\ngit_allowed_signers = \"signers\"\n";

#[tokio::test]
async fn pull_builds_and_caches_the_package_and_returns_the_commit() {
    let work = dir("pull");
    let repository = work.join("repo.git");
    let commit = commit_files(&repository, "main", "one", &package_files()).to_string();
    let server = TestGitServer::start(&repository, None).await;
    let builder = Arc::new(Fake::default());
    let reference = format!("git+{}@branch:main", server.url("r"));
    let (package, pulled) = resolver(&work, &builder, None)
        .pull_git(&source(&reference))
        .await
        .unwrap();
    assert_eq!(pulled, commit);
    assert_eq!(package.manifest.tools.len(), 2);
    assert_eq!(builder.0.load(Ordering::SeqCst), 2);
    server.stop().await;

    // A spec that names the commit now resolves from the cache: no network, no build.
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" },
            "tool_packages": [{ "git": source(&reference).url, "rev": &commit }]
        })
        .to_string(),
    )
    .unwrap();
    let again = Arc::new(Fake::default());
    let tools = resolver(&work, &again, None)
        .resolve(&spec, &work.join("grauwolf.lock"))
        .await
        .unwrap();
    assert_eq!(tools.len(), 2);
    assert_eq!(again.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn pull_and_inspect_refuse_a_host_that_the_policy_does_not_allow() {
    let work = dir("host");
    let repository = work.join("repo.git");
    commit_files(&repository, "main", "one", &package_files());
    let server = TestGitServer::start(&repository, None).await;
    let policy = || {
        policy(
            "[tool_packages]\nallowed_git_hosts = [\"github.com\"]\n",
            None,
        )
    };
    let reference = source(&format!("git+{}@branch:main", server.url("r")));
    let resolver = resolver(&work, &Arc::new(Fake::default()), Some(policy()));
    let pull = resolver.pull_git(&reference).await.unwrap_err();
    let inspect = resolver.inspect_git(&reference, None).await.unwrap_err();
    for error in [pull, inspect] {
        assert!(matches!(error, PackageFetchError::Policy(_)), "{error}");
    }
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn pull_refuses_an_unsigned_commit_under_require_signatures() {
    let work = dir("pull-unsigned");
    let repository = work.join("repo.git");
    commit_files(&repository, "main", "one", &package_files());
    let server = TestGitServer::start(&repository, None).await;
    let trusted = TestSigner::new();
    let builder = Arc::new(Fake::default());
    let error = resolver(&work, &builder, Some(policy(REQUIRE, Some(&[&trusted]))))
        .pull_git(&source(&format!("git+{}@branch:main", server.url("r"))))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("unsigned"), "{error}");
    assert_eq!(builder.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn inspect_shows_the_package_without_building_it() {
    let work = dir("inspect");
    let repository = work.join("repo.git");
    let commit = commit_files(&repository, "main", "one", &package_files()).to_string();
    let server = TestGitServer::start(&repository, None).await;
    let builder = Arc::new(Fake::default());
    let inspection = resolver(&work, &builder, None)
        .inspect_git(
            &source(&format!("git+{}@branch:main", server.url("r"))),
            None,
        )
        .await
        .unwrap();
    assert_eq!(inspection.commit, commit);
    assert_eq!(inspection.manifest.tools.len(), 2);
    assert!(inspection.cached.is_none());
    assert_eq!(builder.0.load(Ordering::SeqCst), 0);
    assert!(!work.join("cache").exists());
    assert_eq!(inspection.signature, GitSignatureStatus::Unsigned);
}

#[tokio::test]
async fn inspect_reports_the_modules_of_a_cached_package() {
    let work = dir("inspect-cached");
    let repository = work.join("repo.git");
    commit_files(&repository, "main", "one", &package_files());
    let server = TestGitServer::start(&repository, None).await;
    let reference = source(&format!("git+{}@branch:main", server.url("r")));
    let resolver = resolver(&work, &Arc::new(Fake::default()), None);
    let (pulled, _) = resolver.pull_git(&reference).await.unwrap();
    let inspection = resolver.inspect_git(&reference, None).await.unwrap();
    assert_eq!(inspection.cached.as_ref().unwrap().modules, pulled.modules);
}

#[tokio::test]
async fn inspect_checks_the_signature_against_the_signers() {
    let work = dir("inspect-signed");
    let repository = work.join("repo.git");
    let signer = TestSigner::new();
    commit_files_signed(&repository, "main", "one", &package_files(), &signer);
    let server = TestGitServer::start(&repository, None).await;
    let reference = source(&format!("git+{}@branch:main", server.url("r")));
    let builder = Arc::new(Fake::default());

    // The signers of the policy.
    let inspection = resolver(&work, &builder, Some(policy(REQUIRE, Some(&[&signer]))))
        .inspect_git(&reference, None)
        .await
        .unwrap();
    assert_eq!(
        inspection.signature,
        GitSignatureStatus::Signed {
            principal: Some("test@example.com".into()),
            fingerprint: signer.fingerprint(),
        }
    );

    // The signers of `--allowed-signers` instead, for a spec without a policy.
    let extra = signers(&[&signer]);
    let inspection = resolver(&work, &builder, None)
        .inspect_git(&reference, Some(&extra))
        .await
        .unwrap();
    assert!(matches!(
        inspection.signature,
        GitSignatureStatus::Signed { .. }
    ));

    // A signature without any signer to check it with is only reported.
    let inspection = resolver(&work, &builder, None)
        .inspect_git(&reference, None)
        .await
        .unwrap();
    assert_eq!(inspection.signature, GitSignatureStatus::Unchecked);
}

#[tokio::test]
async fn inspect_shows_a_signature_of_another_key_and_a_missing_one_instead_of_refusing() {
    let work = dir("inspect-wrong");
    let repository = work.join("repo.git");
    let stranger = TestSigner::new();
    commit_files_signed(&repository, "main", "one", &package_files(), &stranger);
    let plain = work.join("plain.git");
    commit_files(&plain, "main", "one", &package_files());
    let server = TestGitServer::start(&repository, None).await;
    let other = TestGitServer::start(&plain, None).await;
    let trusted = TestSigner::new();
    let allowed = signers(&[&trusted]);
    let resolver = resolver(
        &work,
        &Arc::new(Fake::default()),
        Some(policy(REQUIRE, Some(&[&trusted]))),
    );

    let wrong = resolver
        .inspect_git(
            &source(&format!("git+{}@branch:main", server.url("r"))),
            Some(&allowed),
        )
        .await
        .unwrap();
    assert!(
        matches!(&wrong.signature, GitSignatureStatus::Invalid(reason) if reason.contains("not an allowed signer")),
        "{:?}",
        wrong.signature
    );
    let unsigned = resolver
        .inspect_git(
            &source(&format!("git+{}@branch:main", other.url("r"))),
            None,
        )
        .await
        .unwrap();
    assert_eq!(unsigned.signature, GitSignatureStatus::Unsigned);
}

#[tokio::test]
async fn inspect_of_a_signed_tag_reports_the_tag_signer() {
    let work = dir("inspect-tag");
    let repository = work.join("repo.git");
    let signer = TestSigner::new();
    let commit = commit_files(&repository, "main", "one", &package_files());
    tag_signed(&repository, "v1", commit, &signer);
    let server = TestGitServer::start(&repository, None).await;
    let allowed = signers(&[&signer]);
    let inspection = resolver(&work, &Arc::new(Fake::default()), None)
        .inspect_git(
            &source(&format!("git+{}@tag:v1", server.url("r"))),
            Some(&allowed),
        )
        .await
        .unwrap();
    assert!(matches!(
        inspection.signature,
        GitSignatureStatus::Signed { .. }
    ));
}

#[tokio::test]
async fn errors_of_a_token_protected_repository_never_contain_the_token() {
    let work = dir("token");
    let repository = work.join("repo.git");
    commit_files(&repository, "main", "one", &package_files());
    let server = TestGitServer::start(&repository, Some("s3cr3t-token")).await;
    let reference = source(&format!("git+{}@branch:main", server.url("r")));
    let builder: Arc<Fake> = Arc::new(Fake::default());
    let wrong = PackageResolver::new(work.join("cache"), RegistryClient::default())
        .with_git(Some("wrong-token-value".into()), builder.clone());
    let pull = wrong.pull_git(&reference).await.unwrap_err().to_string();
    let inspect = wrong
        .inspect_git(&reference, None)
        .await
        .unwrap_err()
        .to_string();
    for text in [pull, inspect] {
        assert!(!text.contains("wrong-token-value"), "{text}");
        assert!(!text.contains("s3cr3t-token"), "{text}");
    }
}
