//! Git tools against local bare repositories (ADR-0104). The `https://` rule belongs to the
//! spec; the tools accept every URL that libgit2 understands. The tests need no `git` program:
//! seed and inspection use `git2` as well.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use git2::{
    BranchType, FetchOptions, IndexAddOption, ObjectType, PushOptions, Repository,
    RepositoryInitOptions, ResetType, Signature,
};
use grauwolf_tools::DynTool;
use grauwolf_tools_git::{GIT_TOOLS, GitConfig, GitTools, PushPolicy};
use serde_json::{Value, json};

/// A bare remote with `main` (README), `feature` and the tag `v1`; `work` is the directory of the clone.
struct Fixture {
    base: PathBuf,
    remote: PathBuf,
    work: PathBuf,
}

fn seed_signature() -> Signature<'static> {
    Signature::now("Seed", "seed@example.com").unwrap()
}

/// Stages everything in the working tree of `repository` and commits it on HEAD.
fn commit_all(repository: &Repository, message: &str) {
    let mut index = repository.index().unwrap();
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .unwrap();
    index.update_all(["*"].iter(), None).unwrap();
    index.write().unwrap();
    let tree = repository.find_tree(index.write_tree().unwrap()).unwrap();
    let parent = repository
        .head()
        .ok()
        .map(|head| head.peel_to_commit().unwrap());
    let signature = seed_signature();
    repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &parent.iter().collect::<Vec<_>>(),
        )
        .unwrap();
}

fn switch(repository: &Repository, branch: &str) {
    repository
        .set_head(&format!("refs/heads/{branch}"))
        .unwrap();
    repository
        .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
}

/// Pushes `refspec` of `repository` to `origin`.
fn push(repository: &Repository, refspec: &str) {
    repository
        .find_remote("origin")
        .unwrap()
        .push(&[refspec], Some(&mut PushOptions::new()))
        .unwrap();
}

/// The full commit hash `spec` names in the repository at `path`.
fn rev(path: &Path, spec: &str) -> String {
    let repository = Repository::open(path).unwrap();
    repository
        .revparse_single(spec)
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string()
}

/// Entries of `git status` of the work tree at `path` (including untracked ones).
fn status_entries(path: &Path) -> usize {
    let mut options = git2::StatusOptions::new();
    options.include_untracked(true).recurse_untracked_dirs(true);
    Repository::open(path)
        .unwrap()
        .statuses(Some(&mut options))
        .unwrap()
        .len()
}

/// Author and committer of the last commit on HEAD of `path`.
fn last_commit_people(path: &Path) -> String {
    let repository = Repository::open(path).unwrap();
    let commit = repository.head().unwrap().peel_to_commit().unwrap();
    let (author, committer) = (commit.author(), commit.committer());
    format!(
        "{} <{}> / {} <{}>",
        author.name().unwrap(),
        author.email().unwrap(),
        committer.name().unwrap(),
        committer.email().unwrap()
    )
}

fn local_config(path: &Path) -> git2::Config {
    Repository::open(path)
        .unwrap()
        .config()
        .unwrap()
        .open_level(git2::ConfigLevel::Local)
        .unwrap()
}

fn fixture(name: &str) -> Fixture {
    // Below the target directory of cargo; the checkout of the workspace may be on a mounted
    // volume whose directories are not always visible at once to several threads.
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("git-tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let remote = base.join("remote.git");
    let mut bare = RepositoryInitOptions::new();
    bare.bare(true).initial_head("main");
    Repository::init_opts(&remote, &bare).unwrap();
    let seed = base.join("seed");
    let mut options = RepositoryInitOptions::new();
    options.initial_head("main");
    let repository = Repository::init_opts(&seed, &options).unwrap();
    std::fs::write(seed.join("README.md"), "hello\n").unwrap();
    commit_all(&repository, "first");
    let first = repository.head().unwrap().peel(ObjectType::Commit).unwrap();
    repository.tag_lightweight("v1", &first, false).unwrap();
    repository
        .branch("feature", &first.peel_to_commit().unwrap(), false)
        .unwrap();
    switch(&repository, "feature");
    std::fs::write(seed.join("feature.txt"), "feature\n").unwrap();
    commit_all(&repository, "feature");
    switch(&repository, "main");
    repository
        .remote("origin", remote.to_str().unwrap())
        .unwrap();
    push(&repository, "refs/heads/main:refs/heads/main");
    push(&repository, "refs/heads/feature:refs/heads/feature");
    push(&repository, "refs/tags/v1:refs/tags/v1");
    Fixture {
        work: base.join("work"),
        remote,
        base,
    }
}

impl Fixture {
    fn url(&self) -> String {
        self.remote.display().to_string()
    }

    fn config(&self) -> GitConfig {
        GitConfig::new(self.url(), &self.work)
    }

    fn seed(&self) -> Repository {
        Repository::open(self.base.join("seed")).unwrap()
    }

    /// A commit on `branch` of the remote, made through the seed clone (first brought up to
    /// date with the remote).
    fn advance_remote(&self, branch: &str, file: &str) {
        let seed = self.seed();
        seed.find_remote("origin")
            .unwrap()
            .fetch(
                &["+refs/heads/*:refs/remotes/origin/*"],
                Some(&mut FetchOptions::new()),
                None,
            )
            .unwrap();
        let upstream = seed
            .find_branch(&format!("origin/{branch}"), BranchType::Remote)
            .unwrap()
            .get()
            .peel(ObjectType::Commit)
            .unwrap();
        switch(&seed, branch);
        seed.reset(&upstream, ResetType::Hard, None).unwrap();
        std::fs::write(self.base.join("seed").join(file), "more\n").unwrap();
        commit_all(&seed, file);
        push(&seed, &format!("refs/heads/{branch}:refs/heads/{branch}"));
    }

    fn remote_branches(&self) -> String {
        let repository = Repository::open(&self.remote).unwrap();
        let mut names: Vec<String> = repository
            .branches(Some(BranchType::Local))
            .unwrap()
            .map(|branch| branch.unwrap().0.name().unwrap().unwrap().to_owned())
            .collect();
        names.sort();
        names.join(",")
    }
}

struct Tools(Vec<Arc<dyn DynTool>>);

impl Tools {
    fn new(config: GitConfig) -> Self {
        Self(
            GitTools::new(config)
                .tools(GIT_TOOLS.iter().copied())
                .unwrap(),
        )
    }

    async fn call(&self, name: &str, args: Value) -> String {
        let tool = self.0.iter().find(|tool| tool.name() == name).unwrap();
        tool.call_json(args).await.unwrap()
    }

    async fn checkout(&self, reference: &str) -> String {
        self.call("git_checkout", json!({ "ref": reference })).await
    }
}

#[tokio::test]
async fn clones_on_the_first_checkout_and_names_branch_and_commit() {
    let fixture = fixture("clone");
    let tools = Tools::new(fixture.config());

    let result = tools.checkout("main").await;

    let head = rev(&fixture.work, "HEAD");
    assert_eq!(result, format!("On branch main at {head}"));
    assert!(fixture.work.join("README.md").exists());
}

#[tokio::test]
async fn switches_to_a_remote_branch_and_creates_it_locally() {
    let fixture = fixture("remote-branch");
    let tools = Tools::new(fixture.config());
    tools.checkout("main").await;

    let result = tools.checkout("feature").await;

    assert!(result.starts_with("On branch feature at "), "{result}");
    assert!(fixture.work.join("feature.txt").exists());
}

#[tokio::test]
async fn fetches_and_fast_forwards_a_branch_on_a_later_checkout() {
    let fixture = fixture("fast-forward");
    let tools = Tools::new(fixture.config());
    tools.checkout("main").await;
    fixture.advance_remote("main", "news.txt");

    let result = tools.checkout("main").await;

    assert!(result.starts_with("On branch main at "), "{result}");
    assert!(fixture.work.join("news.txt").exists());
}

#[tokio::test]
async fn refuses_a_checkout_with_uncommitted_changes() {
    let fixture = fixture("dirty");
    let tools = Tools::new(fixture.config());
    tools.checkout("main").await;
    std::fs::write(fixture.work.join("README.md"), "changed\n").unwrap();

    let result = tools.checkout("feature").await;

    assert_eq!(
        result,
        "Refused: the working tree has uncommitted changes. Commit and push them first."
    );
}

#[tokio::test]
async fn refuses_to_leave_a_branch_with_unpushed_commits() {
    let fixture = fixture("unpushed");
    let tools = Tools::new(fixture.config());
    tools.checkout("main").await;
    std::fs::write(fixture.work.join("local.txt"), "x").unwrap();
    tools
        .call("git_commit", json!({ "message": "local" }))
        .await;

    let result = tools.checkout("feature").await;

    assert_eq!(
        result,
        "Refused: branch main has commits that are not pushed."
    );
}

#[tokio::test]
async fn refuses_a_branch_that_diverged_from_the_remote() {
    let fixture = fixture("diverged");
    let tools = Tools::new(fixture.config());
    tools.checkout("main").await;
    std::fs::write(fixture.work.join("local.txt"), "x").unwrap();
    tools
        .call("git_commit", json!({ "message": "local" }))
        .await;
    fixture.advance_remote("main", "remote.txt");

    let result = tools.checkout("main").await;

    assert_eq!(result, "Refused: branch main has diverged from the remote.");
}

#[tokio::test]
async fn checks_out_a_tag_and_a_commit_hash_detached() {
    let fixture = fixture("detached");
    let tools = Tools::new(fixture.config());
    tools.checkout("main").await;
    let first = rev(&fixture.work, "v1");

    let tag = tools.checkout("v1").await;
    tools.checkout("main").await;
    let by_hash = tools.checkout(&first[..8]).await;

    assert_eq!(tag, format!("Detached at {first}"));
    assert_eq!(by_hash, format!("Detached at {first}"));
}

#[tokio::test]
async fn says_unknown_ref_for_a_name_that_is_nothing() {
    let fixture = fixture("unknown");
    let tools = Tools::new(fixture.config());
    tools.checkout("main").await;

    assert_eq!(tools.checkout("nope").await, "Unknown ref: nope");
    assert_eq!(tools.checkout("--force").await, "Unknown ref: --force");
}

#[tokio::test]
async fn lists_branches_and_tags_of_the_remote_sorted() {
    let fixture = fixture("list");
    let seed = fixture.seed();
    let head = seed.head().unwrap().peel(ObjectType::Commit).unwrap();
    seed.tag("v0", &head, &seed_signature(), "v0", false)
        .unwrap();
    push(&seed, "refs/tags/v0:refs/tags/v0");
    let tools = Tools::new(fixture.config());

    let result = tools.call("git_list_branches", json!({})).await;

    assert_eq!(result, "branches: feature, main\ntags: v0, v1");
}

#[tokio::test]
async fn create_branch_needs_a_checkout_first() {
    let fixture = fixture("create-no-checkout");
    let tools = Tools::new(fixture.config());

    let result = tools
        .call("git_create_branch", json!({ "name": "topic" }))
        .await;

    assert_eq!(result, "No checkout yet. Call checkout first.");
}

#[tokio::test]
async fn creates_a_branch_at_the_current_commit_and_switches_to_it() {
    let fixture = fixture("create");
    let tools = Tools::new(fixture.config());
    tools.checkout("main").await;
    let head = rev(&fixture.work, "HEAD");

    let result = tools
        .call("git_create_branch", json!({ "name": "agent/topic" }))
        .await;

    assert_eq!(result, format!("On branch agent/topic at {head}"));
}

#[tokio::test]
async fn create_branch_refuses_invalid_and_existing_names() {
    let fixture = fixture("create-rules");
    let tools = Tools::new(fixture.config());
    tools.checkout("main").await;
    tools
        .call("git_create_branch", json!({ "name": "mine" }))
        .await;
    tools.checkout("main").await;
    let create = |name: &'static str| tools.call("git_create_branch", json!({ "name": name }));

    assert_eq!(create("bad name").await, "Invalid branch name: bad name");
    assert_eq!(create("-x").await, "Invalid branch name: -x");
    assert_eq!(create("a..b").await, "Invalid branch name: a..b");
    assert_eq!(create("mine").await, "Refused: branch mine already exists.");
    assert_eq!(create("main").await, "Refused: branch main already exists.");
    // On the remote only, not fetched into this checkout yet.
    fixture.advance_remote("main", "x.txt");
    push(&fixture.seed(), "refs/heads/main:refs/heads/fresh");
    assert_eq!(
        create("fresh").await,
        "Refused: branch fresh already exists."
    );
}

#[tokio::test]
async fn commits_everything_including_deletions_with_the_configured_author() {
    let fixture = fixture("commit");
    let mut config = fixture.config();
    config.author_name = Some("Wolf".into());
    config.author_email = Some("wolf@example.com".into());
    let tools = Tools::new(config);
    tools.checkout("main").await;
    std::fs::remove_file(fixture.work.join("README.md")).unwrap();
    std::fs::write(fixture.work.join("new.txt"), "new\n").unwrap();

    let result = tools
        .call("git_commit", json!({ "message": "change it" }))
        .await;

    let head = rev(&fixture.work, "HEAD");
    assert_eq!(result, format!("Committed {} on branch main", &head[..7]));
    assert_eq!(status_entries(&fixture.work), 0);
    assert_eq!(
        last_commit_people(&fixture.work),
        "Wolf <wolf@example.com> / Wolf <wolf@example.com>"
    );
    // README.md deleted and new.txt added: two files in the commit.
    let repository = Repository::open(&fixture.work).unwrap();
    let commit = repository.head().unwrap().peel_to_commit().unwrap();
    let diff = repository
        .diff_tree_to_tree(
            Some(&commit.parent(0).unwrap().tree().unwrap()),
            Some(&commit.tree().unwrap()),
            None,
        )
        .unwrap();
    assert_eq!(diff.stats().unwrap().files_changed(), 2);
}

#[tokio::test]
async fn commit_uses_the_default_author_and_refuses_without_changes_checkout_or_branch() {
    let fixture = fixture("commit-rules");
    let tools = Tools::new(fixture.config());
    let commit = |message: &'static str| tools.call("git_commit", json!({ "message": message }));
    assert_eq!(commit("m").await, "No checkout yet. Call checkout first.");
    tools.checkout("main").await;
    assert_eq!(commit("m").await, "Nothing to commit");
    std::fs::write(fixture.work.join("a.txt"), "a").unwrap();
    commit("by default").await;
    assert_eq!(
        last_commit_people(&fixture.work),
        "grauwolf agent <agent@grauwolf.local> / grauwolf agent <agent@grauwolf.local>"
    );
    push(
        &Repository::open(&fixture.work).unwrap(),
        "refs/heads/main:refs/heads/main",
    );
    tools.checkout("v1").await;
    std::fs::write(fixture.work.join("b.txt"), "b").unwrap();

    assert_eq!(
        commit("m").await,
        "Refused: HEAD is detached. Check out a branch or create one with createBranch first."
    );
}

#[tokio::test]
async fn push_with_new_branches_allows_only_branches_the_agent_created() {
    let fixture = fixture("push-new-branches");
    let tools = Tools::new(fixture.config());
    tools.checkout("main").await;
    std::fs::write(fixture.work.join("a.txt"), "a").unwrap();
    tools
        .call("git_commit", json!({ "message": "on main" }))
        .await;

    let refused = tools.call("git_push", json!({})).await;
    tools
        .call("git_create_branch", json!({ "name": "topic" }))
        .await;
    std::fs::write(fixture.work.join("b.txt"), "b").unwrap();
    tools
        .call("git_commit", json!({ "message": "on topic" }))
        .await;
    let pushed = tools.call("git_push", json!({})).await;

    assert_eq!(
        refused,
        "Refused: the push policy allows only branches this agent created with createBranch."
    );
    assert_eq!(pushed, "Pushed topic");
    assert_eq!(fixture.remote_branches(), "feature,main,topic");
    assert_eq!(rev(&fixture.remote, "topic"), rev(&fixture.work, "topic"));
}

#[tokio::test]
async fn the_created_branches_are_remembered_for_the_directory_across_tool_sets() {
    let fixture = fixture("push-registry");
    Tools::new(fixture.config()).checkout("main").await;
    Tools::new(fixture.config())
        .call("git_create_branch", json!({ "name": "topic" }))
        .await;
    std::fs::write(fixture.work.join("a.txt"), "a").unwrap();
    let other = Tools::new(fixture.config());
    other.call("git_commit", json!({ "message": "m" })).await;

    assert_eq!(other.call("git_push", json!({})).await, "Pushed topic");
}

#[tokio::test]
async fn push_with_any_pushes_the_current_branch_and_never_forces() {
    let fixture = fixture("push-any");
    let mut config = fixture.config();
    config.push_policy = PushPolicy::Any;
    let tools = Tools::new(config);
    tools.checkout("main").await;
    std::fs::write(fixture.work.join("a.txt"), "a").unwrap();
    tools.call("git_commit", json!({ "message": "m" })).await;
    let pushed = tools.call("git_push", json!({})).await;
    let remote_head = rev(&fixture.remote, "main");
    // The remote moves on: the next push is not a fast-forward and must fail.
    std::fs::write(fixture.work.join("b.txt"), "b").unwrap();
    tools.call("git_commit", json!({ "message": "m2" })).await;
    fixture.advance_remote("main", "elsewhere.txt");

    let rejected = tools.call("git_push", json!({})).await;

    assert_eq!(pushed, "Pushed main");
    assert_ne!(remote_head, rev(&fixture.work, "HEAD"));
    assert!(rejected.starts_with("Push failed: "), "{rejected}");
    let remote = Repository::open(&fixture.remote).unwrap();
    let summary = remote
        .revparse_single("main")
        .unwrap()
        .peel_to_commit()
        .unwrap();
    assert_eq!(summary.summary().unwrap(), Some("elsewhere.txt"));
}

#[tokio::test]
async fn push_refuses_without_checkout_and_with_a_detached_head() {
    let fixture = fixture("push-rules");
    let mut config = fixture.config();
    config.push_policy = PushPolicy::Any;
    let tools = Tools::new(config);
    assert_eq!(
        tools.call("git_push", json!({})).await,
        "No checkout yet. Call checkout first."
    );
    tools.checkout("v1").await;

    assert_eq!(
        tools.call("git_push", json!({})).await,
        "Refused: HEAD is detached. Check out a branch or create one with createBranch first."
    );
}

async fn guard_refusals(fixture: &Fixture, tools: &Tools) -> Vec<String> {
    vec![
        tools.checkout("main").await,
        tools.call("git_list_branches", json!({})).await,
        tools.call("git_push", json!({})).await,
    ]
    .into_iter()
    .inspect(|_| {
        let _ = fixture;
    })
    .collect()
}

#[tokio::test]
async fn the_remote_guard_refuses_a_changed_origin_url() {
    let fixture = fixture("guard-url");
    let mut config = fixture.config();
    config.push_policy = PushPolicy::Any;
    let tools = Tools::new(config);
    tools.checkout("main").await;
    local_config(&fixture.work)
        .set_str("remote.origin.url", "https://evil.example.com/x.git")
        .unwrap();

    for reply in guard_refusals(&fixture, &tools).await {
        assert_eq!(
            reply,
            "Refused: remote.origin.url in .git/config is not the configured repository URL."
        );
    }
}

#[tokio::test]
async fn the_remote_guard_refuses_a_second_origin_url_a_pushurl_and_insteadof() {
    let fixture = fixture("guard-more");
    let mut config = fixture.config();
    config.push_policy = PushPolicy::Any;
    let tools = Tools::new(config);
    tools.checkout("main").await;
    let evil = "https://evil.example.com/x.git";

    // A regular expression that matches nothing adds a value instead of replacing one.
    local_config(&fixture.work)
        .set_multivar("remote.origin.url", "^$never", evil)
        .unwrap();
    assert!(
        tools
            .checkout("main")
            .await
            .starts_with("Refused: remote.origin.url")
    );
    local_config(&fixture.work)
        .remove_multivar("remote.origin.url", "evil")
        .unwrap();

    local_config(&fixture.work)
        .set_str("remote.origin.pushurl", evil)
        .unwrap();
    assert_eq!(
        tools.call("git_push", json!({})).await,
        "Refused: .git/config sets remote.origin.pushurl."
    );
    local_config(&fixture.work)
        .remove("remote.origin.pushurl")
        .unwrap();

    let mut config = local_config(&fixture.work);
    config
        .set_str(
            "url.https://evil.example.com/.insteadOf",
            "https://github.com/",
        )
        .unwrap();
    assert_eq!(
        tools.checkout("main").await,
        "Refused: .git/config rewrites URLs with url.*.insteadOf or pushInsteadOf."
    );
    config
        .remove("url.https://evil.example.com/.insteadOf")
        .unwrap();
    config
        .set_str(
            "url.https://evil.example.com/.pushInsteadOf",
            "https://github.com/",
        )
        .unwrap();
    assert_eq!(
        tools.call("git_list_branches", json!({})).await,
        "Refused: .git/config rewrites URLs with url.*.insteadOf or pushInsteadOf."
    );
}

/// A minimal HTTP server for the credential tests: `respond` gets the request head and answers.
/// It records the `Authorization` headers it sees and stops when dropped.
struct Server {
    port: u16,
    authorizations: Arc<Mutex<Vec<String>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    fn start(respond: impl Fn(&str) -> String + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let authorizations = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (seen, stopped) = (authorizations.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stopped.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                let mut head = Vec::new();
                let mut buffer = [0u8; 1024];
                while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                    match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => head.extend_from_slice(&buffer[..count]),
                    }
                }
                // Read the request body too: closing with unread data resets the connection.
                let end = head.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(0) + 4;
                let text =
                    String::from_utf8_lossy(&head[..end.min(head.len())]).to_ascii_lowercase();
                let length = text
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                while head.len() < end + length {
                    match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => head.extend_from_slice(&buffer[..count]),
                    }
                }
                let head = String::from_utf8_lossy(&head[..end.min(head.len())]).into_owned();
                for line in head.lines() {
                    if line.to_ascii_lowercase().starts_with("authorization:") {
                        seen.lock().unwrap().push(line.to_owned());
                    }
                }
                let _ = stream.write_all(respond(&head).as_bytes());
            }
        });
        Self {
            port,
            authorizations,
            stop,
            thread: Some(thread),
        }
    }

    fn url(&self, host: &str) -> String {
        format!("http://{host}:{}/team/repo.git", self.port)
    }

    fn authorizations(&self) -> Vec<String> {
        self.authorizations.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(("127.0.0.1", self.port));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

const UNAUTHORIZED: &str = "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"x\"\r\n\
                            Content-Length: 0\r\nConnection: close\r\n\r\n";

fn basic(user_and_token: &str) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = user_and_token.as_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[tokio::test]
async fn the_token_is_given_once_as_basic_auth_and_a_rejected_token_does_not_loop() {
    let fixture = fixture("token-flow");
    let server = Server::start(|_| UNAUTHORIZED.to_owned());
    let mut config = GitConfig::new(server.url("127.0.0.1"), &fixture.work);
    config.token = Some("tok-SECRET-123".into());
    let tools = Tools::new(config);

    let listing = tools.call("git_list_branches", json!({})).await;
    let checkout = tools.checkout("main").await;

    assert!(
        listing.starts_with("Error: git ls-remote failed: "),
        "{listing}"
    );
    assert!(
        checkout.starts_with("Error: git clone failed: "),
        "{checkout}"
    );
    for reply in [&listing, &checkout] {
        assert!(!reply.contains("SECRET"), "{reply}");
    }
    let expected = format!("Basic {}", basic("x-access-token:tok-SECRET-123"));
    let authorizations = server.authorizations();
    assert!(!authorizations.is_empty());
    for line in &authorizations {
        assert!(line.ends_with(&expected), "{line}");
    }
    // One answer per call and no retry loop.
    assert_eq!(authorizations.len(), 2, "{authorizations:?}");
}

#[tokio::test]
async fn without_use_token_no_credentials_are_sent() {
    let fixture = fixture("no-token");
    let server = Server::start(|_| UNAUTHORIZED.to_owned());
    let tools = Tools::new(GitConfig::new(server.url("127.0.0.1"), &fixture.work));

    let listing = tools.call("git_list_branches", json!({})).await;

    assert!(
        listing.starts_with("Error: git ls-remote failed: "),
        "{listing}"
    );
    assert!(server.authorizations().is_empty());
}

#[tokio::test]
async fn the_token_does_not_follow_a_redirect_to_another_host() {
    let fixture = fixture("token-redirect");
    let other = Server::start(|_| UNAUTHORIZED.to_owned());
    let target = format!(
        "http://localhost:{}/team/repo.git/info/refs?service=git-upload-pack",
        other.port
    );
    let server = Server::start(move |_| {
        format!(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
    });
    let mut config = GitConfig::new(server.url("127.0.0.1"), &fixture.work);
    config.token = Some("tok-SECRET-123".into());

    let listing = Tools::new(config)
        .call("git_list_branches", json!({}))
        .await;

    assert!(
        listing.starts_with("Error: git ls-remote failed: "),
        "{listing}"
    );
    assert!(!listing.contains("SECRET"), "{listing}");
    assert!(
        other.authorizations().is_empty(),
        "{:?}",
        other.authorizations()
    );
    // The configured host gets the token with its request, the redirect target never.
    assert_eq!(server.authorizations().len(), 1);
}

/// An answer that redirects to `location` with `status` (for example `307 Temporary Redirect`).
fn redirect(status: &str, location: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
}

#[tokio::test]
async fn a_same_host_redirect_without_a_query_is_an_error_and_not_a_crash() {
    // libgit2 itself dereferences the missing query when it applies such a redirect.
    let fixture = fixture("redirect-same-host");
    let server = Server::start(|_| redirect("307 Temporary Redirect", "/team/repo.git/info/refs"));
    let tools = Tools::new(GitConfig::new(server.url("127.0.0.1"), &fixture.work));

    let listing = tools.call("git_list_branches", json!({})).await;

    assert_eq!(
        listing,
        "Error: git ls-remote failed: the server answered with a redirect (HTTP 307); \
         redirects are not followed"
    );
}

#[tokio::test]
async fn every_redirect_is_refused_whatever_status_and_target() {
    let fixture = fixture("redirect-matrix");
    let other = Server::start(|_| UNAUTHORIZED.to_owned());
    let foreign = format!("http://localhost:{}/team/repo.git/info/refs", other.port);
    let with_query = format!("{foreign}?service=git-upload-pack");
    for status in [
        "301 Moved Permanently",
        "302 Found",
        "303 See Other",
        "307 Temporary Redirect",
        "308 Permanent Redirect",
    ] {
        let code = &status[..3];
        for target in ["absolute", "no-query", "foreign", "foreign-query"] {
            let server = Server::start({
                let (status, foreign, with_query) =
                    (status.to_owned(), foreign.clone(), with_query.clone());
                move |head| {
                    let host = head
                        .lines()
                        .find_map(|line| line.strip_prefix("host: "))
                        .unwrap_or_default();
                    let location = match target {
                        "absolute" => format!("http://{host}/team/repo.git/info/refs"),
                        "no-query" => "/team/repo.git/info/refs".to_owned(),
                        "foreign" => foreign.clone(),
                        _ => with_query.clone(),
                    };
                    redirect(&status, &location)
                }
            });
            let mut config = GitConfig::new(server.url("127.0.0.1"), &fixture.work);
            config.token = Some("tok-SECRET-123".into());

            let listing = Tools::new(config)
                .call("git_list_branches", json!({}))
                .await;

            assert_eq!(
                listing,
                format!(
                    "Error: git ls-remote failed: the server answered with a redirect \
                     (HTTP {code}); redirects are not followed"
                ),
                "{status} to {target}"
            );
        }
    }
    assert!(other.authorizations().is_empty());
}

fn pkt(text: &str) -> String {
    format!("{:04x}{text}", text.len() + 4)
}

/// The answer to `GET …/info/refs` of a smart server whose repository is empty.
fn empty_advertisement(service: &str) -> String {
    let body = format!(
        "{}0000{}0000",
        pkt(&format!("# service={service}\n")),
        pkt(&format!(
            "{} capabilities^{{}}\0report-status delete-refs ofs-delta agent=test\n",
            "0".repeat(40)
        ))
    );
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/x-{service}-advertisement\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[tokio::test]
async fn lists_the_refs_of_a_smart_http_server_with_a_user_agent_and_the_service_query() {
    let fixture = fixture("smart-listing");
    let heads = Arc::new(Mutex::new(Vec::new()));
    let server = Server::start({
        let heads = heads.clone();
        move |head| {
            heads.lock().unwrap().push(head.to_owned());
            let id = "1".repeat(40);
            let body = format!(
                "{}0000{}{}{}0000",
                pkt("# service=git-upload-pack\n"),
                pkt(&format!("{id} HEAD\0multi_ack agent=test\n")),
                pkt(&format!("{id} refs/heads/main\n")),
                pkt(&format!("{id} refs/tags/v1\n")),
            );
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/x-git-upload-pack-advertisement\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        }
    });
    let tools = Tools::new(GitConfig::new(server.url("127.0.0.1"), &fixture.work));

    let listing = tools.call("git_list_branches", json!({})).await;

    assert_eq!(listing, "branches: main\ntags: v1");
    let heads = heads.lock().unwrap();
    let request = heads[0].to_ascii_lowercase();
    assert!(
        request.starts_with("get /team/repo.git/info/refs?service=git-upload-pack "),
        "{request}"
    );
    assert!(
        request.contains("user-agent: git/2.0 (grauwolf)"),
        "{request}"
    );
}

#[tokio::test]
async fn a_server_that_is_not_smart_and_http_errors_are_named() {
    let fixture = fixture("smart-errors");
    let plain = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\
                 Connection: close\r\n\r\nok";
    let cases = [
        (
            plain.to_owned(),
            "the server does not speak the smart HTTP protocol (Content-Type text/plain, \
             expected application/x-git-upload-pack-advertisement)",
        ),
        (UNAUTHORIZED.to_owned(), "authentication failed (HTTP 401)"),
        (status_only("403 Forbidden"), "access denied (HTTP 403)"),
        (
            status_only("404 Not Found"),
            "repository not found (HTTP 404)",
        ),
        (
            status_only("500 Internal Server Error"),
            "the server answered HTTP 500",
        ),
    ];
    for (answer, reason) in cases {
        let server = Server::start(move |_| answer.clone());
        let tools = Tools::new(GitConfig::new(server.url("127.0.0.1"), &fixture.work));

        let listing = tools.call("git_list_branches", json!({})).await;

        assert_eq!(listing, format!("Error: git ls-remote failed: {reason}"));
    }
}

fn status_only(status: &str) -> String {
    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
}

#[tokio::test]
async fn a_redirect_ends_a_clone_and_a_push_as_well() {
    let fixture = fixture("redirect-clone-push");
    let to = "Location: /team/repo.git/info/refs";
    let moved = format!("HTTP/1.1 308 Permanent Redirect\r\n{to}\r\nContent-Length: 0\r\n\r\n");
    let server = Server::start(move |_| moved.clone());
    let mut config = GitConfig::new(server.url("127.0.0.1"), &fixture.work);
    config.token = Some("tok-SECRET-123".into());
    let tools = Tools::new(config);
    let refused = "the server answered with a redirect (HTTP 308); redirects are not followed";

    let clone = tools.checkout("main").await;

    assert_eq!(clone, format!("Error: git clone failed: {refused}"));

    // A push whose server answers the listing and redirects the upload.
    let work = Repository::clone(&fixture.url(), &fixture.work).unwrap();
    let push_server = Server::start(move |head| {
        if head.starts_with("POST") {
            format!("HTTP/1.1 307 Temporary Redirect\r\n{to}\r\nContent-Length: 0\r\n\r\n")
        } else {
            empty_advertisement("git-receive-pack")
        }
    });
    work.remote_set_url("origin", &push_server.url("127.0.0.1"))
        .unwrap();
    let mut config = GitConfig::new(push_server.url("127.0.0.1"), &fixture.work);
    config.push_policy = PushPolicy::Any;
    config.token = Some("tok-SECRET-123".into());
    let pushed = Tools::new(config).call("git_push", json!({})).await;
    let expected =
        "Push failed: the server answered with a redirect (HTTP 307); redirects are not followed";
    assert_eq!(pushed, expected);

    // And one whose server redirects already the listing.
    let moved = Tools::new({
        let mut config = GitConfig::new(server.url("127.0.0.1"), &fixture.work);
        config.push_policy = PushPolicy::Any;
        config
    });
    work.remote_set_url("origin", &server.url("127.0.0.1"))
        .unwrap();
    assert_eq!(
        moved.call("git_push", json!({})).await,
        format!("Push failed: {refused}")
    );
}

#[tokio::test]
async fn a_network_call_that_takes_too_long_ends_with_an_error() {
    let fixture = fixture("timeout");
    // A server that accepts and never answers; the connection is closed at the end so that the
    // abandoned libgit2 call (it keeps running after the timeout) can finish.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let connection = listener.accept();
        let _ = wait.recv();
        drop(connection);
    });
    let mut config = GitConfig::new(format!("http://127.0.0.1:{port}/x.git"), &fixture.work);
    config.network_timeout = std::time::Duration::from_millis(300);
    let tools = Tools::new(config);

    let result = tools.call("git_list_branches", json!({})).await;

    release.send(()).unwrap();
    holder.join().unwrap();
    assert_eq!(result, "Error: git ls-remote failed: timed out after 300ms");
}

#[tokio::test]
async fn calls_on_one_directory_run_one_after_another() {
    let fixture = fixture("lock");
    let tools = Tools::new(fixture.config());

    let (first, second) = tokio::join!(tools.checkout("main"), tools.checkout("main"));

    assert!(first.starts_with("On branch main at "), "{first}");
    assert!(second.starts_with("On branch main at "), "{second}");
}

#[tokio::test]
async fn an_unknown_tool_name_is_an_error_and_every_tool_describes_itself() {
    let fixture = fixture("describe");
    let tools = GitTools::new(fixture.config());

    assert!(tools.tools(["git_reset"]).is_err());
    for tool in tools.tools(GIT_TOOLS.iter().copied()).unwrap() {
        let descriptor = tool.descriptor();
        assert_eq!(descriptor.parameters["type"], "object");
        assert!(!descriptor.description.is_empty());
    }
    let missing = tools.tools(["git_checkout"]).unwrap()[0]
        .call_json(json!({}))
        .await;
    assert!(missing.unwrap_err().to_string().contains("ref"));
}
