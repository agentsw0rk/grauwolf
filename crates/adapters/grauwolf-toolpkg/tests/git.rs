//! Git sources of `tool_packages`: resolution, lockfile, cache and `--locked`, against the
//! in-process test git server. The modules come from a fake builder that counts its builds.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use grauwolf_git::testing::{TestGitServer, commit_files};
use grauwolf_spec::{AgentSpec, AgentSpecExt, ToolSpec};
use grauwolf_toolpkg::{
    Lockfile, ModuleBuilder, PackageFetchError, PackageResolver, RegistryClient,
};
use serde_json::{Value, json};

const MANIFEST: &str =
    include_str!("../../../../docs/user-guide/examples/03-package/grauwolf-tools.json");
const ADD: &str = include_str!("../../../../docs/user-guide/examples/03-package/src/add.rs");
const COUNT: &str =
    include_str!("../../../../docs/user-guide/examples/03-package/src/count_words.rs");

fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "git-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Builds `module of <tool> <salt>` and counts the builds.
struct Fake {
    builds: AtomicUsize,
    salt: &'static str,
}

impl Fake {
    fn new(salt: &'static str) -> Arc<Self> {
        Arc::new(Self {
            builds: AtomicUsize::new(0),
            salt,
        })
    }

    fn builds(&self) -> usize {
        self.builds.load(Ordering::SeqCst)
    }
}

impl ModuleBuilder for Fake {
    fn prepare(&self) -> Result<(), String> {
        Ok(())
    }

    fn build(&self, tool: &ToolSpec) -> Result<Vec<u8>, String> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        Ok(format!("module of {} {}", tool.name, self.salt).into_bytes())
    }
}

fn package_files(prefix: &str) -> BTreeMap<String, Vec<u8>> {
    [
        ("grauwolf-tools.json", MANIFEST),
        ("src/add.rs", ADD),
        ("src/count_words.rs", COUNT),
    ]
    .into_iter()
    .map(|(name, text)| (format!("{prefix}{name}"), text.as_bytes().to_vec()))
    .collect()
}

struct Fixture {
    work: PathBuf,
    repository: PathBuf,
    first: String,
}

fn fixture(name: &str) -> Fixture {
    let work = dir(name);
    let repository = work.join("repo.git");
    let mut files = package_files("text/");
    files.insert("README.md".into(), b"hello".to_vec());
    let first = commit_files(&repository, "main", "one", &files).to_string();
    Fixture {
        work,
        repository,
        first,
    }
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

fn resolver(
    work: &Path,
    builder: &Arc<Fake>,
    token: Option<&str>,
    locked: bool,
) -> PackageResolver {
    PackageResolver::new(work.join("cache"), RegistryClient::default())
        .with_locked(locked)
        .with_git(token.map(str::to_owned), builder.clone())
}

fn names(tools: &[grauwolf_tools::ResolvedTool]) -> Vec<&str> {
    tools.iter().map(|tool| tool.name.as_str()).collect()
}

#[tokio::test]
async fn resolves_a_package_folder_at_a_rev_and_writes_the_commit_to_the_lockfile() {
    let fixture = fixture("rev");
    let server = TestGitServer::start(&fixture.repository, None).await;
    let url = server.url("r");
    let short = &fixture.first[..7];
    let spec = spec(json!([{ "git": url, "path": "text", "rev": short }]));
    let builder = Fake::new("a");
    let lock = fixture.work.join("grauwolf.lock");
    let tools = resolver(&fixture.work, &builder, None, false)
        .resolve(&spec, &lock)
        .await
        .unwrap();
    assert_eq!(names(&tools), ["add", "count_words"]);
    assert_eq!(tools[0].artifact.bytes, b"module of add a");
    assert_eq!(builder.builds(), 2);
    let locked = Lockfile::load(&lock).unwrap();
    let key = format!("git+{url}//text@{short}");
    assert_eq!(locked.git[&key].commit, fixture.first);
    assert_eq!(locked.git[&key].modules.len(), 2);
    assert!(locked.packages.is_empty());
}

#[tokio::test]
async fn a_tag_or_branch_is_locked_to_its_commit_and_a_locked_entry_ignores_later_changes() {
    let fixture = fixture("branch");
    let server = TestGitServer::start(&fixture.repository, None).await;
    let spec = spec(json!([{ "git": server.url("r"), "path": "text", "branch": "main" }]));
    let lock = fixture.work.join("grauwolf.lock");
    let builder = Fake::new("a");
    resolver(&fixture.work, &builder, None, false)
        .resolve(&spec, &lock)
        .await
        .unwrap();
    let locked = Lockfile::load(&lock).unwrap();
    assert_eq!(locked.git.values().next().unwrap().commit, fixture.first);
    // The branch moves on and the manifest changes; a fresh cache still gets the old commit.
    let mut files = package_files("text/");
    files.insert("text/src/add.rs".into(), b"Ok(String::new())".to_vec());
    commit_files(&fixture.repository, "main", "two", &files);
    let again = Fake::new("a");
    let tools = PackageResolver::new(fixture.work.join("cache2"), RegistryClient::default())
        .with_git(None, again.clone())
        .resolve(&spec, &lock)
        .await
        .unwrap();
    assert_eq!(tools[0].artifact.bytes, b"module of add a");
    assert_eq!(Lockfile::load(&lock).unwrap(), locked);
}

#[tokio::test]
async fn a_tag_resolves_to_the_commit_it_points_to() {
    let fixture = fixture("tag");
    grauwolf_git::testing::tag(
        &fixture.repository,
        "v1",
        fixture.first.parse().unwrap(),
        true,
    );
    let server = TestGitServer::start(&fixture.repository, None).await;
    let spec = spec(json!([{ "git": server.url("r"), "path": "text", "tag": "v1" }]));
    let lock = fixture.work.join("grauwolf.lock");
    resolver(&fixture.work, &Fake::new("a"), None, false)
        .resolve(&spec, &lock)
        .await
        .unwrap();
    let locked = Lockfile::load(&lock).unwrap();
    let (key, entry) = locked.git.iter().next().unwrap();
    assert!(key.ends_with("//text@tag:v1"), "{key}");
    assert_eq!(entry.commit, fixture.first);
}

#[tokio::test]
async fn a_locked_and_cached_package_needs_neither_server_nor_builder() {
    let fixture = fixture("offline");
    let server = TestGitServer::start(&fixture.repository, None).await;
    let spec = spec(json!([{ "git": server.url("r"), "path": "text", "rev": &fixture.first }]));
    let lock = fixture.work.join("grauwolf.lock");
    resolver(&fixture.work, &Fake::new("a"), None, true)
        .resolve(&spec, &lock)
        .await
        .unwrap_err();
    resolver(&fixture.work, &Fake::new("a"), None, false)
        .resolve(&spec, &lock)
        .await
        .unwrap();
    server.stop().await;
    let builder = Fake::new("b");
    let tools = resolver(&fixture.work, &builder, None, true)
        .resolve(&spec, &lock)
        .await
        .unwrap();
    assert_eq!(names(&tools), ["add", "count_words"]);
    assert_eq!(tools[0].artifact.bytes, b"module of add a");
    assert_eq!(builder.builds(), 0);
}

#[tokio::test]
async fn locked_fails_before_any_request_when_an_entry_is_missing_or_stale() {
    let fixture = fixture("stale");
    let server = TestGitServer::start(&fixture.repository, None).await;
    let url = server.url("r");
    let lock = fixture.work.join("grauwolf.lock");
    let first = spec(json!([{ "git": url, "path": "text", "rev": &fixture.first }]));
    let error = resolver(&fixture.work, &Fake::new("a"), None, true)
        .resolve(&first, &lock)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("is not locked"), "{error}");
    resolver(&fixture.work, &Fake::new("a"), None, false)
        .resolve(&first, &lock)
        .await
        .unwrap();
    let requests = server.requests().len();
    let other = spec(json!([{ "git": url, "path": "text", "tag": "v9" }]));
    let error = resolver(&fixture.work, &Fake::new("a"), None, true)
        .resolve(&other, &lock)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("is not locked"), "{error}");
    let none = spec(json!([]));
    let error = resolver(&fixture.work, &Fake::new("a"), None, true)
        .resolve(&none, &lock)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("no longer in the spec"),
        "{error}"
    );
    assert_eq!(server.requests().len(), requests);
}

#[tokio::test]
async fn rebuilt_modules_must_match_the_lockfile() {
    let fixture = fixture("hashes");
    let server = TestGitServer::start(&fixture.repository, None).await;
    let spec = spec(json!([{ "git": server.url("r"), "path": "text", "rev": &fixture.first }]));
    let lock = fixture.work.join("grauwolf.lock");
    resolver(&fixture.work, &Fake::new("a"), None, false)
        .resolve(&spec, &lock)
        .await
        .unwrap();
    // Another toolchain gives other bytes, and there is no cache to hide it.
    let error = PackageResolver::new(fixture.work.join("empty"), RegistryClient::default())
        .with_git(None, Fake::new("b"))
        .resolve(&spec, &lock)
        .await
        .unwrap_err();
    assert!(matches!(error, PackageFetchError::Lock(_)), "{error}");
    assert!(
        error.to_string().contains("do not match the lockfile"),
        "{error}"
    );
}

#[tokio::test]
async fn selects_and_renames_tools_of_a_git_package() {
    let fixture = fixture("select");
    let server = TestGitServer::start(&fixture.repository, None).await;
    let spec = spec(
        json!([{ "git": server.url("r"), "path": "text", "rev": &fixture.first,
        "tools": ["count_words"], "as": { "count_words": "word_count" } }]),
    );
    let tools = resolver(&fixture.work, &Fake::new("a"), None, false)
        .resolve(&spec, &fixture.work.join("grauwolf.lock"))
        .await
        .unwrap();
    assert_eq!(names(&tools), ["word_count"]);
}

#[tokio::test]
async fn the_repository_root_is_the_default_folder() {
    let work = dir("root");
    let repository = work.join("repo.git");
    let commit = commit_files(&repository, "main", "one", &package_files("")).to_string();
    let server = TestGitServer::start(&repository, None).await;
    let spec = spec(json!([{ "git": server.url("r"), "rev": commit }]));
    let tools = resolver(&work, &Fake::new("a"), None, false)
        .resolve(&spec, &work.join("grauwolf.lock"))
        .await
        .unwrap();
    assert_eq!(names(&tools), ["add", "count_words"]);
}

#[tokio::test]
async fn a_folder_without_a_manifest_or_an_unknown_commit_is_an_error_that_names_it() {
    let fixture = fixture("errors");
    let server = TestGitServer::start(&fixture.repository, None).await;
    let url = server.url("r");
    let lock = fixture.work.join("grauwolf.lock");
    let error = resolver(&fixture.work, &Fake::new("a"), None, false)
        .resolve(&spec(json!([{ "git": url, "rev": &fixture.first }])), &lock)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("grauwolf-tools.json"), "{error}");
    let error = resolver(&fixture.work, &Fake::new("a"), None, false)
        .resolve(
            &spec(json!([{ "git": url, "path": "text", "rev": "0123456" }])),
            &lock,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not found"), "{error}");
    assert!(!lock.exists());
}

#[tokio::test]
async fn the_token_goes_to_the_server_and_never_into_an_error() {
    let fixture = fixture("token");
    let server = TestGitServer::start(&fixture.repository, Some("s3cret-token")).await;
    let spec = spec(json!([{ "git": server.url("r"), "path": "text", "rev": &fixture.first }]));
    let lock = fixture.work.join("grauwolf.lock");
    let error = resolver(&fixture.work, &Fake::new("a"), Some("wrong-token"), false)
        .resolve(&spec, &lock)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("401"), "{error}");
    assert!(!error.to_string().contains("wrong-token"), "{error}");
    resolver(&fixture.work, &Fake::new("a"), Some("s3cret-token"), false)
        .resolve(&spec, &lock)
        .await
        .unwrap();
    assert!(!std::fs::read_to_string(&lock).unwrap().contains("s3cret"));
}

#[tokio::test]
async fn a_damaged_cache_entry_is_an_error() {
    let fixture = fixture("damaged");
    let server = TestGitServer::start(&fixture.repository, None).await;
    let spec = spec(json!([{ "git": server.url("r"), "path": "text", "rev": &fixture.first }]));
    let lock = fixture.work.join("grauwolf.lock");
    resolver(&fixture.work, &Fake::new("a"), None, false)
        .resolve(&spec, &lock)
        .await
        .unwrap();
    let entry = std::fs::read_dir(fixture.work.join("cache/packages"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let blob = std::fs::read_dir(entry.join("blobs"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(&blob, b"tampered").unwrap();
    let error = resolver(&fixture.work, &Fake::new("a"), None, false)
        .resolve(&spec, &lock)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("damaged"), "{error}");
}

#[tokio::test]
async fn git_sources_need_the_builder_to_be_configured() {
    let fixture = fixture("nobuilder");
    let spec = spec(json!([{ "git": "http://127.0.0.1:9/r.git", "rev": &fixture.first }]));
    let error = PackageResolver::new(fixture.work.join("cache"), RegistryClient::default())
        .resolve(&spec, &fixture.work.join("grauwolf.lock"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not available"), "{error}");
}
