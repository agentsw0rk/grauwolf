//! Git sources of `skills` (ADR-0103): resolution, lockfile section, cache, `--locked`, policy and
//! signatures, against the in-process test git server.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use grauwolf_git::AllowedSigners;
use grauwolf_git::testing::{
    TestGitServer, TestSigner, commit_files, commit_files_signed, tag as git_tag,
};
use grauwolf_spec::{AgentSpec, AgentSpecExt, ToolSpec};
use grauwolf_toolpkg::{
    LockedGit, Lockfile, ModuleBuilder, PackageFetchError, PackageResolver, Policy, RegistryClient,
};
use grauwolf_tools::ToolPolicy;
use serde_json::{Value, json};

struct NoBuild;

impl ModuleBuilder for NoBuild {
    fn prepare(&self) -> Result<(), String> {
        Ok(())
    }
    fn build(&self, _: &ToolSpec) -> Result<Vec<u8>, String> {
        Err("skills are never built".into())
    }
}

fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "gitskills-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn files(prefix: &str, body: &str) -> BTreeMap<String, Vec<u8>> {
    [
        (
            "haiku-writer/SKILL.md",
            format!("---\nname: haiku-writer\ndescription: Writes haiku\n---\n{body}"),
        ),
        ("haiku-writer/refs/style.md", "Nature.".to_owned()),
    ]
    .into_iter()
    .map(|(name, text)| (format!("{prefix}{name}"), text.into_bytes()))
    .collect()
}

fn spec(skills: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" },
            "skills": skills
        })
        .to_string(),
    )
    .unwrap()
}

fn resolver(work: &Path, locked: bool) -> PackageResolver {
    PackageResolver::new(work.join("cache"), RegistryClient::default())
        .with_locked(locked)
        .with_git(None, Arc::new(NoBuild))
}

struct Fixture {
    work: PathBuf,
    repository: PathBuf,
    first: String,
}

fn fixture(name: &str) -> Fixture {
    let work = dir(name);
    let repository = work.join("repo.git");
    let first = commit_files(&repository, "main", "one", &files("skills/", "Five.")).to_string();
    Fixture {
        work,
        repository,
        first,
    }
}

#[tokio::test]
async fn resolves_a_folder_at_a_rev_to_a_cache_directory_and_locks_the_commit() {
    let fixture = fixture("rev");
    let server = TestGitServer::start(&fixture.repository, None).await;
    let url = server.url("r");
    let short = &fixture.first[..7];
    let spec = spec(json!([{ "dir": "local" }, { "git": url, "path": "skills", "rev": short }]));
    let lock = fixture.work.join("grauwolf.lock");
    let resolved = resolver(&fixture.work, false)
        .resolve_skills(&spec, &lock)
        .await
        .unwrap();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].index, 1);
    assert_eq!(resolved[0].commit, fixture.first);
    let skill = std::fs::read_to_string(resolved[0].dir.join("haiku-writer/SKILL.md")).unwrap();
    assert!(skill.contains("Five."), "{skill}");
    assert!(resolved[0].dir.join("haiku-writer/refs/style.md").is_file());
    let locked = Lockfile::load(&lock).unwrap();
    assert_eq!(
        locked.skills[&format!("git+{url}//skills@{short}")].commit,
        fixture.first
    );
    assert!(locked.packages.is_empty() && locked.git.is_empty());
}

#[tokio::test]
async fn a_tag_resolves_to_its_commit_and_the_other_lock_sections_stay() {
    let fixture = fixture("tag");
    git_tag(
        &fixture.repository,
        "v1",
        fixture.first.parse().unwrap(),
        true,
    );
    let server = TestGitServer::start(&fixture.repository, None).await;
    let lock = fixture.work.join("grauwolf.lock");
    let mut before = Lockfile::default();
    before.git.insert(
        "git+https://h/r.git@4e1a9c2".into(),
        LockedGit {
            commit: "4e1a9c2".repeat(5) + "abcde",
            modules: BTreeMap::new(),
        },
    );
    before.save(&lock).unwrap();
    let spec = spec(json!([{ "git": server.url("r"), "path": "skills", "tag": "v1" }]));
    resolver(&fixture.work, false)
        .resolve_skills(&spec, &lock)
        .await
        .unwrap();
    let after = Lockfile::load(&lock).unwrap();
    assert_eq!(after.git, before.git);
    let (key, entry) = after.skills.iter().next().unwrap();
    assert!(key.ends_with("//skills@tag:v1"), "{key}");
    assert_eq!(entry.commit, fixture.first);
}

#[tokio::test]
async fn a_locked_and_cached_source_works_offline_and_ignores_later_changes() {
    let fixture = fixture("offline");
    let server = TestGitServer::start(&fixture.repository, None).await;
    let spec = spec(json!([{ "git": server.url("r"), "path": "skills", "branch": "main" }]));
    let lock = fixture.work.join("grauwolf.lock");
    resolver(&fixture.work, true)
        .resolve_skills(&spec, &lock)
        .await
        .unwrap_err();
    resolver(&fixture.work, false)
        .resolve_skills(&spec, &lock)
        .await
        .unwrap();
    commit_files(
        &fixture.repository,
        "main",
        "two",
        &files("skills/", "Changed."),
    );
    server.stop().await;
    let resolved = resolver(&fixture.work, true)
        .resolve_skills(&spec, &lock)
        .await
        .unwrap();
    assert_eq!(resolved[0].commit, fixture.first);
    let skill = std::fs::read_to_string(resolved[0].dir.join("haiku-writer/SKILL.md")).unwrap();
    assert!(skill.contains("Five."), "{skill}");
}

#[tokio::test]
async fn locked_fails_before_any_request_when_a_source_is_missing_or_stale() {
    let fixture = fixture("stale");
    let server = TestGitServer::start(&fixture.repository, None).await;
    let url = server.url("r");
    let lock = fixture.work.join("grauwolf.lock");
    let first = spec(json!([{ "git": url, "path": "skills", "rev": &fixture.first }]));
    let error = resolver(&fixture.work, true)
        .resolve_skills(&first, &lock)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("is not locked"), "{error}");
    resolver(&fixture.work, false)
        .resolve_skills(&first, &lock)
        .await
        .unwrap();
    let requests = server.requests().len();
    let none = spec(json!([]));
    let error = resolver(&fixture.work, true)
        .resolve_skills(&none, &lock)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("no longer in the spec"),
        "{error}"
    );
    assert_eq!(server.requests().len(), requests);
}

#[tokio::test]
async fn a_changed_cache_file_is_an_error_not_a_silent_refetch() {
    let fixture = fixture("damaged");
    let server = TestGitServer::start(&fixture.repository, None).await;
    let spec = spec(json!([{ "git": server.url("r"), "path": "skills", "rev": &fixture.first }]));
    let lock = fixture.work.join("grauwolf.lock");
    let resolved = resolver(&fixture.work, false)
        .resolve_skills(&spec, &lock)
        .await
        .unwrap();
    std::fs::write(resolved[0].dir.join("haiku-writer/SKILL.md"), "tampered").unwrap();
    let error = resolver(&fixture.work, false)
        .resolve_skills(&spec, &lock)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("was changed"), "{error}");
}

#[tokio::test]
async fn the_tool_policy_refuses_a_disallowed_host_before_any_request() {
    let fixture = fixture("policy");
    let server = TestGitServer::start(&fixture.repository, None).await;
    let policy = Policy::new(
        ToolPolicy::parse("[tool_packages]\nallowed_git_hosts = [\"github.com/acme\"]\n").unwrap(),
        Vec::new(),
    );
    let spec = spec(json!([{ "git": server.url("r"), "path": "skills", "rev": &fixture.first }]));
    let error = resolver(&fixture.work, false)
        .with_policy(policy)
        .resolve_skills(&spec, &fixture.work.join("grauwolf.lock"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("allowed_git_hosts"), "{error}");
    assert!(server.requests().is_empty());
}

fn signature_policy(signer: &TestSigner) -> Policy {
    Policy::new(
        ToolPolicy::parse(
            "[tool_packages]\nrequire_signatures = true\ngit_allowed_signers = \"signers\"\n",
        )
        .unwrap(),
        Vec::new(),
    )
    .with_git_signers(
        AllowedSigners::parse(&signer.allowed_signers("test@example.com", None)).unwrap(),
    )
}

#[tokio::test]
async fn require_signatures_refuses_an_unsigned_commit_and_accepts_a_signed_one() {
    let fixture = fixture("unsigned");
    let signer = TestSigner::new();
    let server = TestGitServer::start(&fixture.repository, None).await;
    let spec_unsigned =
        spec(json!([{ "git": server.url("r"), "path": "skills", "rev": &fixture.first }]));
    let error = resolver(&fixture.work, false)
        .with_policy(signature_policy(&signer))
        .resolve_skills(&spec_unsigned, &fixture.work.join("grauwolf.lock"))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, PackageFetchError::Policy(text) if text.contains("unsigned")),
        "{error}"
    );
    assert!(!fixture.work.join("grauwolf.lock").exists());

    let work = dir("signed");
    let repository = work.join("repo.git");
    let commit = commit_files_signed(
        &repository,
        "main",
        "one",
        &files("skills/", "Five."),
        &signer,
    )
    .to_string();
    let server = TestGitServer::start(&repository, None).await;
    let spec = spec(json!([{ "git": server.url("r"), "path": "skills", "rev": &commit }]));
    let lock = work.join("grauwolf.lock");
    resolver(&work, false)
        .with_policy(signature_policy(&signer))
        .resolve_skills(&spec, &lock)
        .await
        .unwrap();
    // Verified again from the stored objects, offline and locked.
    server.stop().await;
    resolver(&work, true)
        .with_policy(signature_policy(&signer))
        .resolve_skills(&spec, &lock)
        .await
        .unwrap();
    // A policy with another signer refuses the cached entry too.
    let other = TestSigner::new();
    resolver(&work, true)
        .with_policy(signature_policy(&other))
        .resolve_skills(&spec, &lock)
        .await
        .unwrap_err();
}
