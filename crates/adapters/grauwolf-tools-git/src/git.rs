//! Git tools for one fixed repository (ADR-0104) on libgit2 (`git2`); no `git` program is needed.
//!
//! The token goes only into the `Authorization` header of the own HTTP transport
//! (`grauwolf_git`, in memory, and only for the host of the configured repository URL), never
//! in URLs, results or logs; it is masked in every error text. libgit2 is blocking: every call
//! runs in `spawn_blocking` under the directory lock.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures_util::future::BoxFuture;
use git2::build::{CheckoutBuilder, RepoBuilder};
use git2::{
    AutotagOption, ConfigLevel, Direction, FetchOptions, FetchPrune, IndexAddOption, PushOptions,
    Reference, Remote, RemoteCallbacks, Repository, Signature, StatusOptions,
};
use grauwolf_prompt::ToolDescriptor;
use grauwolf_tools::{DynTool, ToolError};
use serde_json::{Value, json};

/// Names of the git tools.
pub const GIT_TOOLS: &[&str] = &[
    "git_checkout",
    "git_list_branches",
    "git_create_branch",
    "git_commit",
    "git_push",
];

const NO_CHECKOUT: &str = "No checkout yet. Call checkout first.";
const DETACHED: &str =
    "Refused: HEAD is detached. Check out a branch or create one with createBranch first.";
const DEFAULT_AUTHOR_NAME: &str = "grauwolf agent";
const DEFAULT_AUTHOR_EMAIL: &str = "agent@grauwolf.local";

/// Which branches `git_push` may push.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PushPolicy {
    /// Only branches that `git_create_branch` created in this process.
    #[default]
    NewBranches,
    /// Every branch.
    Any,
}

/// The repository of the git tools.
#[derive(Clone)]
pub struct GitConfig {
    /// Remote URL; the spec requires `https://`, the tests also use local repositories.
    pub repo_url: String,
    /// Working directory of the clone.
    pub directory: PathBuf,
    /// Which branches may be pushed.
    pub push_policy: PushPolicy,
    /// `GIT_TOKEN`, if the repository needs it; user `x-access-token`.
    pub token: Option<String>,
    /// Workload identity instead of `GIT_TOKEN` (ADR-0110): mTLS with the agent's X.509-SVID
    /// (the server's SPIFFE ID is checked) or a fresh JWT-SVID as bearer on every call.
    pub auth: Option<Arc<grauwolf_spiffe::ClientAuth>>,
    /// Author and committer of `git_commit` (`GIT_AUTHOR_NAME`); default `grauwolf agent`.
    pub author_name: Option<String>,
    /// Author and committer mail (`GIT_AUTHOR_EMAIL`); default `agent@grauwolf.local`.
    pub author_email: Option<String>,
    /// Time limit of calls that use the network (default 120 s); the HTTP requests of the call
    /// also end by themselves one second later.
    pub network_timeout: Duration,
    /// Time limit of local calls (default 60 s).
    pub local_timeout: Duration,
}

impl GitConfig {
    /// A repository `repo_url` checked out in `directory`, push policy `new_branches`, no token.
    pub fn new(repo_url: impl Into<String>, directory: impl Into<PathBuf>) -> Self {
        Self {
            repo_url: repo_url.into(),
            directory: directory.into(),
            push_policy: PushPolicy::default(),
            token: None,
            auth: None,
            author_name: None,
            author_email: None,
            network_timeout: Duration::from_secs(120),
            local_timeout: Duration::from_secs(60),
        }
    }
}

impl std::fmt::Debug for GitConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitConfig")
            .field("repo_url", &self.repo_url)
            .field("directory", &self.directory)
            .field("push_policy", &self.push_policy)
            .field("token", &self.token.as_ref().map(|_| "***"))
            .field("auth", &self.auth)
            .finish_non_exhaustive()
    }
}

type Locks = Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>;
type Created = Mutex<HashMap<String, HashSet<String>>>;

/// One lock per directory: calls on a directory run one after another.
fn locks() -> &'static Locks {
    static LOCKS: OnceLock<Locks> = OnceLock::new();
    LOCKS.get_or_init(Default::default)
}

/// Branches that `git_create_branch` created, per directory, for the whole process: tool
/// sets may be built anew for every call (sub-agents), so per-instance state would forget.
fn created() -> &'static Created {
    static CREATED: OnceLock<Created> = OnceLock::new();
    CREATED.get_or_init(Default::default)
}

/// Git tools for one repository.
#[derive(Debug, Clone)]
pub struct GitTools {
    repo: Arc<GitRepo>,
}

impl GitTools {
    /// Tools for the repository of `config`.
    pub fn new(config: GitConfig) -> Self {
        grauwolf_git::init_libgit2();
        grauwolf_git::register();
        Self {
            repo: Arc::new(GitRepo::new(config)),
        }
    }

    /// The tools with the names `names`; unknown names are an error.
    pub fn tools<'n>(
        &self,
        names: impl IntoIterator<Item = &'n str>,
    ) -> Result<Vec<Arc<dyn DynTool>>, String> {
        names
            .into_iter()
            .map(|name| {
                let kind = match name {
                    "git_checkout" => GitToolKind::Checkout,
                    "git_list_branches" => GitToolKind::ListBranches,
                    "git_create_branch" => GitToolKind::CreateBranch,
                    "git_commit" => GitToolKind::Commit,
                    "git_push" => GitToolKind::Push,
                    other => return Err(format!("unknown built-in tool '{other}'")),
                };
                Ok(Arc::new(GitTool {
                    repo: self.repo.clone(),
                    kind,
                }) as Arc<dyn DynTool>)
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy)]
enum GitToolKind {
    Checkout,
    ListBranches,
    CreateBranch,
    Commit,
    Push,
}

struct GitTool {
    repo: Arc<GitRepo>,
    kind: GitToolKind,
}

impl DynTool for GitTool {
    fn name(&self) -> &str {
        match self.kind {
            GitToolKind::Checkout => "git_checkout",
            GitToolKind::ListBranches => "git_list_branches",
            GitToolKind::CreateBranch => "git_create_branch",
            GitToolKind::Commit => "git_commit",
            GitToolKind::Push => "git_push",
        }
    }

    fn descriptor(&self) -> ToolDescriptor {
        let one = |field: &str, description: &str| {
            json!({
                "type": "object",
                "properties": { field: { "type": "string", "description": description } },
                "required": [field]
            })
        };
        let none = json!({ "type": "object", "properties": {} });
        let (description, parameters) = match self.kind {
            GitToolKind::Checkout => (
                "Clone the repository on the first call, otherwise fetch, then switch to a branch, \
                 tag or commit. A branch is fast-forwarded to the remote. Refuses when there are \
                 uncommitted changes or unpushed commits.",
                one("ref", "Branch name, tag name or commit hash"),
            ),
            GitToolKind::ListBranches => {
                ("List the branches and tags of the remote repository", none)
            }
            GitToolKind::CreateBranch => (
                "Create a new branch at the current commit and switch to it",
                one("name", "Name of the new branch"),
            ),
            GitToolKind::Commit => (
                "Stage all changes in the checkout and commit them on the current branch",
                one("message", "Commit message"),
            ),
            GitToolKind::Push => (
                "Push the current branch to the remote, never with force",
                none,
            ),
        };
        ToolDescriptor {
            name: self.name().to_owned(),
            description: description.to_owned(),
            parameters,
        }
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let text = |field: &str| {
                args[field]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| ToolError::InvalidArgs {
                        tool: self.name().to_owned(),
                        reason: format!("missing field '{field}' (text)"),
                    })
            };
            let operation = match self.kind {
                GitToolKind::Checkout => Operation::Checkout(text("ref")?),
                GitToolKind::ListBranches => Operation::ListBranches,
                GitToolKind::CreateBranch => Operation::CreateBranch(text("name")?),
                GitToolKind::Commit => Operation::Commit(text("message")?),
                GitToolKind::Push => Operation::Push,
            };
            let lock = locks()
                .lock()
                .unwrap()
                .entry(self.repo.key())
                .or_default()
                .clone();
            // The guard moves into the blocking task: after a timeout the directory stays
            // locked until libgit2 has really stopped.
            let guard = lock.lock_owned().await;
            let repo = self.repo.clone();
            let command = operation.command(&repo);
            // A JWT-SVID is fetched here (async) and goes to the blocking call.
            let bearer = match &repo.config.auth {
                Some(auth) => auth.bearer().await.map_err(|error| {
                    ToolError::Execution(format!(
                        "Error: git {command} failed: workload identity: {error}"
                    ))
                })?,
                None => None,
            };
            let limit = operation.limit(&repo.config);
            let task = tokio::task::spawn_blocking(move || {
                let _guard = guard;
                grauwolf_git::with_settings(repo.transport_settings(bearer), || {
                    repo.execute(operation)
                })
            });
            let reply = match tokio::time::timeout(limit, task).await {
                Ok(Ok(reply)) => reply,
                Ok(Err(error)) => Err(format!("Error: git {command} failed: {error}")),
                Err(_) => Err(format!(
                    "Error: git {command} failed: timed out after {limit:?}"
                )),
            };
            // Refusals and failures go back to the model as text.
            Ok(reply.unwrap_or_else(|text| text))
        })
    }
}

/// Reply of an operation: `Err` is also text for the model, it only ends the operation early.
type Reply = Result<String, String>;

/// One call of a tool with its arguments.
enum Operation {
    Checkout(String),
    ListBranches,
    CreateBranch(String),
    Commit(String),
    Push,
}

impl Operation {
    /// The name that error texts and the timeout message use (`git <command> failed`).
    fn command(&self, repo: &GitRepo) -> &'static str {
        match self {
            Self::Checkout(_) if repo.has_checkout() => "fetch",
            Self::Checkout(_) => "clone",
            Self::ListBranches => "ls-remote",
            Self::CreateBranch(_) => "branch",
            Self::Commit(_) => "commit",
            Self::Push => "push",
        }
    }

    fn limit(&self, config: &GitConfig) -> Duration {
        match self {
            Self::Commit(_) => config.local_timeout,
            _ => config.network_timeout,
        }
    }
}

#[derive(Debug)]
struct GitRepo {
    config: GitConfig,
}

impl GitRepo {
    fn new(mut config: GitConfig) -> Self {
        let absolute = std::path::absolute(&config.directory).unwrap_or(config.directory.clone());
        config.directory = normalize(&absolute);
        Self { config }
    }

    fn key(&self) -> String {
        self.config.directory.display().to_string()
    }

    fn dir(&self) -> &Path {
        &self.config.directory
    }

    fn has_checkout(&self) -> bool {
        self.dir().join(".git").exists()
    }

    fn execute(&self, operation: Operation) -> Reply {
        match operation {
            Operation::Checkout(reference) => self.checkout(&reference),
            Operation::ListBranches => self.list_branches(),
            Operation::CreateBranch(name) => self.create_branch(&name),
            Operation::Commit(message) => self.commit(&message),
            Operation::Push => self.push(),
        }
    }

    /// What the HTTP transport needs for the calls of this repository.
    fn transport_settings(&self, bearer: Option<String>) -> grauwolf_git::Settings {
        grauwolf_git::Settings {
            token: self.config.token.clone(),
            origin: grauwolf_git::origin_of(&self.config.repo_url),
            timeout: self.config.network_timeout,
            auth: self.config.auth.clone(),
            bearer,
        }
    }

    fn mask(&self, text: &str) -> String {
        mask_token(self.config.token.as_deref(), text)
    }

    /// Turns a libgit2 error of `command` into the text for the model.
    fn fail(&self, command: &'static str) -> impl Fn(git2::Error) -> String + '_ {
        move |error| self.failure(command, error.message())
    }

    fn failure(&self, command: &str, reason: &str) -> String {
        format!("Error: git {command} failed: {}", self.mask(reason.trim()))
    }

    fn open(&self, command: &'static str) -> Result<Repository, String> {
        Repository::open(self.dir()).map_err(self.fail(command))
    }

    fn fetch_options(&self) -> FetchOptions<'static> {
        FetchOptions::new()
    }

    fn clone_repository(&self) -> Result<Repository, String> {
        let parent = self.dir().parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)
            .map_err(|error| self.failure("clone", &error.to_string()))?;
        let fail = self.fail("clone");
        let repository = RepoBuilder::new()
            .fetch_options(self.fetch_options())
            .clone(&self.config.repo_url, self.dir())
            .map_err(&fail)?;
        // libgit2 may store a normalized form of a local path; the guard compares exactly.
        repository
            .remote_set_url("origin", &self.config.repo_url)
            .map_err(&fail)?;
        Ok(repository)
    }

    fn fetch(&self, repository: &Repository) -> Result<(), String> {
        let fail = self.fail("fetch");
        let mut remote = repository.find_remote("origin").map_err(&fail)?;
        let mut options = self.fetch_options();
        options
            .prune(FetchPrune::On)
            .download_tags(AutotagOption::All);
        remote
            .fetch(
                &[
                    "+refs/heads/*:refs/remotes/origin/*",
                    "+refs/tags/*:refs/tags/*",
                ],
                Some(&mut options),
                None,
            )
            .map_err(fail)
    }

    fn is_clean(&self, repository: &Repository) -> Result<bool, String> {
        let mut options = StatusOptions::new();
        options
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .include_ignored(false);
        let statuses = repository
            .statuses(Some(&mut options))
            .map_err(self.fail("status"))?;
        Ok(statuses.is_empty())
    }

    /// The current branch, `None` when HEAD is detached.
    fn branch(repository: &Repository) -> Option<String> {
        let head = repository.find_reference("HEAD").ok()?;
        let target = head.symbolic_target().ok()??;
        target.strip_prefix("refs/heads/").map(str::to_owned)
    }

    /// The commit of `refname`, `None` if there is no such reference.
    fn commit_of(repository: &Repository, refname: &str) -> Option<git2::Oid> {
        let reference = repository.find_reference(refname).ok()?;
        Some(reference.peel_to_commit().ok()?.id())
    }

    fn head(repository: &Repository) -> Option<git2::Oid> {
        Some(repository.head().ok()?.peel_to_commit().ok()?.id())
    }

    fn describe(repository: &Repository) -> Reply {
        let head =
            Self::head(repository).map_or_else(|| "no commit".to_owned(), |id| id.to_string());
        Ok(match Self::branch(repository) {
            Some(branch) => format!("On branch {branch} at {head}"),
            None => format!("Detached at {head}"),
        })
    }

    /// Updates index and working tree to `commit` (safe: nothing uncommitted is overwritten).
    fn checkout_commit(&self, repository: &Repository, commit: git2::Oid) -> Result<(), String> {
        let fail = self.fail("checkout");
        let object = repository.find_object(commit, None).map_err(&fail)?;
        repository
            .checkout_tree(&object, Some(&mut CheckoutBuilder::new()))
            .map_err(fail)
    }

    fn switch_branch(&self, repository: &Repository, name: &str) -> Result<(), String> {
        let refname = format!("refs/heads/{name}");
        let commit = Self::commit_of(repository, &refname)
            .ok_or_else(|| self.failure("switch", &format!("unknown branch {name}")))?;
        self.checkout_commit(repository, commit)?;
        repository.set_head(&refname).map_err(self.fail("switch"))
    }

    /// The configuration of the checkout must still name exactly the configured repository
    /// as `origin`: a file tool on the same directory must not redirect the token.
    fn remote_guard(&self) -> Result<(), String> {
        if !self.has_checkout() {
            return Ok(());
        }
        let fail = self.fail("config");
        let config = self
            .open("config")?
            .config()
            .and_then(|config| config.open_level(ConfigLevel::Local))
            .map_err(&fail)?;
        let values = |name: &str| -> Result<Vec<String>, String> {
            let mut values = Vec::new();
            match config.multivar(name, None) {
                Ok(entries) => entries
                    .for_each(|entry| values.push(entry.value().unwrap_or_default().to_owned()))
                    .map_err(&fail)?,
                Err(error) if error.code() == git2::ErrorCode::NotFound => {}
                Err(error) => return Err(fail(error)),
            }
            Ok(values)
        };
        if values("remote.origin.url")? != [self.config.repo_url.as_str()] {
            return Err(
                "Refused: remote.origin.url in .git/config is not the configured repository URL."
                    .to_owned(),
            );
        }
        if !values("remote.origin.pushurl")?.is_empty() {
            return Err("Refused: .git/config sets remote.origin.pushurl.".to_owned());
        }
        let mut rewrites = 0;
        config
            .entries(Some(r"^url\..*\.(insteadof|pushinsteadof)$"))
            .and_then(|entries| entries.for_each(|_| rewrites += 1))
            .map_err(&fail)?;
        if rewrites > 0 {
            return Err(
                "Refused: .git/config rewrites URLs with url.*.insteadOf or pushInsteadOf."
                    .to_owned(),
            );
        }
        Ok(())
    }

    fn checkout(&self, reference: &str) -> Reply {
        let repository = if self.has_checkout() {
            self.remote_guard()?;
            let repository = self.open("fetch")?;
            self.fetch(&repository)?;
            repository
        } else {
            self.clone_repository()?
        };
        let repository = &repository;
        if !self.is_clean(repository)? {
            return Err(
                "Refused: the working tree has uncommitted changes. Commit and push them first."
                    .to_owned(),
            );
        }
        let branch = Self::branch(repository);
        let current = branch.as_deref() == Some(reference);
        // Unpushed means not reachable from any remote branch, so a merged and deleted or a
        // fresh branch passes. Checked only when leaving a branch.
        if let (Some(_), Some(branch), false) = (Self::head(repository), &branch, current) {
            let fail = self.fail("rev-list");
            let mut walk = repository.revwalk().map_err(&fail)?;
            walk.push_head().map_err(&fail)?;
            walk.hide_glob("refs/remotes/origin/*").map_err(&fail)?;
            if walk.next().is_some() {
                return Err(format!(
                    "Refused: branch {branch} has commits that are not pushed."
                ));
            }
        }
        let remote_ref = format!("refs/remotes/origin/{reference}");
        let local_ref = format!("refs/heads/{reference}");
        let remote = Self::commit_of(repository, &remote_ref);
        let local = Self::commit_of(repository, &local_ref);
        let diverged = || format!("Refused: branch {reference} has diverged from the remote.");
        let is_branch = is_branch_name(reference);
        if let (Some(remote), true) = (remote, is_branch) {
            let Some(local) = local else {
                let fail = self.fail("switch");
                let commit = repository.find_commit(remote).map_err(&fail)?;
                let mut created = repository
                    .branch(reference, &commit, false)
                    .map_err(&fail)?;
                created
                    .set_upstream(Some(&format!("origin/{reference}")))
                    .map_err(&fail)?;
                self.switch_branch(repository, reference)?;
                return Self::describe(repository);
            };
            // Fast-forward only: the local branch must be an ancestor of the remote one.
            let behind = local == remote
                || repository
                    .graph_descendant_of(remote, local)
                    .map_err(self.fail("merge"))?;
            if !behind {
                return Err(diverged());
            }
            if !current {
                self.switch_branch(repository, reference)?;
            }
            if local != remote {
                self.checkout_commit(repository, remote)?;
                repository
                    .reference(&local_ref, remote, true, "grauwolf: fast-forward")
                    .map_err(self.fail("merge"))?;
            }
            return Self::describe(repository);
        }
        // An own branch that is not pushed yet: switch to it, nothing to fast-forward.
        if local.is_some() && is_branch {
            if !current {
                self.switch_branch(repository, reference)?;
            }
            return Self::describe(repository);
        }
        let mut target = repository
            .revparse_single(&format!("refs/tags/{reference}"))
            .and_then(|object| object.peel_to_commit())
            .ok();
        if target.is_none() && is_hex_hash(reference) {
            target = repository
                .revparse_single(reference)
                .and_then(|object| object.peel_to_commit())
                .ok();
        }
        let Some(target) = target else {
            return Err(format!("Unknown ref: {reference}"));
        };
        self.checkout_commit(repository, target.id())?;
        repository
            .set_head_detached(target.id())
            .map_err(self.fail("checkout"))?;
        Self::describe(repository)
    }

    /// Names of the remote's refs.
    fn remote_refs(&self) -> Result<Vec<String>, String> {
        self.remote_guard()?;
        let fail = self.fail("ls-remote");
        let mut remote = Remote::create_detached(self.config.repo_url.as_str()).map_err(&fail)?;
        let connection = remote
            .connect_auth(Direction::Fetch, None, None)
            .map_err(&fail)?;
        let names = connection
            .list()
            .map_err(&fail)?
            .iter()
            .map(|head| head.name().to_owned())
            .collect();
        Ok(names)
    }

    fn list_branches(&self) -> Reply {
        let refs = self.remote_refs()?;
        let names = |prefix: &str| {
            let mut names: Vec<&str> = refs
                .iter()
                .filter(|name| !name.ends_with("^{}"))
                .filter_map(|name| name.strip_prefix(prefix))
                .collect();
            names.sort_unstable();
            names.join(", ")
        };
        Ok(format!(
            "branches: {}\ntags: {}",
            names("refs/heads/"),
            names("refs/tags/")
        ))
    }

    fn create_branch(&self, name: &str) -> Reply {
        if !is_branch_name(name) {
            return Err(format!("Invalid branch name: {name}"));
        }
        if !self.has_checkout() {
            return Err(NO_CHECKOUT.to_owned());
        }
        let repository = self.open("branch")?;
        let Some(head) = Self::head(&repository) else {
            return Err(NO_CHECKOUT.to_owned());
        };
        let local = format!("refs/heads/{name}");
        let exists = Self::commit_of(&repository, &local).is_some()
            || Self::commit_of(&repository, &format!("refs/remotes/origin/{name}")).is_some()
            || self.remote_refs()?.contains(&local);
        if exists {
            return Err(format!("Refused: branch {name} already exists."));
        }
        let fail = self.fail("branch");
        let commit = repository.find_commit(head).map_err(&fail)?;
        repository.branch(name, &commit, false).map_err(&fail)?;
        self.switch_branch(&repository, name)?;
        created()
            .lock()
            .unwrap()
            .entry(self.key())
            .or_default()
            .insert(name.to_owned());
        Self::describe(&repository)
    }

    fn commit(&self, message: &str) -> Reply {
        if !self.has_checkout() {
            return Err(NO_CHECKOUT.to_owned());
        }
        let repository = self.open("commit")?;
        let Some(branch) = Self::branch(&repository) else {
            return Err(DETACHED.to_owned());
        };
        if self.is_clean(&repository)? {
            return Err("Nothing to commit".to_owned());
        }
        let fail = self.fail("commit");
        // Everything, deletions included (`git add -A`).
        let mut index = repository.index().map_err(&fail)?;
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .map_err(&fail)?;
        index.update_all(["*"].iter(), None).map_err(&fail)?;
        index.write().map_err(&fail)?;
        let tree = repository
            .find_tree(index.write_tree().map_err(&fail)?)
            .map_err(&fail)?;
        let signature = Signature::now(
            self.config
                .author_name
                .as_deref()
                .unwrap_or(DEFAULT_AUTHOR_NAME),
            self.config
                .author_email
                .as_deref()
                .unwrap_or(DEFAULT_AUTHOR_EMAIL),
        )
        .map_err(&fail)?;
        let parents: Vec<git2::Commit> = Self::head(&repository)
            .map(|id| repository.find_commit(id))
            .transpose()
            .map_err(&fail)?
            .into_iter()
            .collect();
        let parents: Vec<&git2::Commit> = parents.iter().collect();
        let message = if message.ends_with('\n') {
            message.to_owned()
        } else {
            format!("{message}\n")
        };
        let id = repository
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                &message,
                &tree,
                &parents,
            )
            .map_err(&fail)?;
        let short: String = id.to_string().chars().take(7).collect();
        Ok(format!("Committed {short} on branch {branch}"))
    }

    fn push(&self) -> Reply {
        if !self.has_checkout() {
            return Err(NO_CHECKOUT.to_owned());
        }
        let repository = self.open("push")?;
        let Some(branch) = Self::branch(&repository) else {
            return Err(DETACHED.to_owned());
        };
        // Fails closed: only exactly `any` allows every branch.
        let own = created()
            .lock()
            .unwrap()
            .get(&self.key())
            .is_some_and(|branches| branches.contains(&branch));
        if self.config.push_policy != PushPolicy::Any && !own {
            return Err(
                "Refused: the push policy allows only branches this agent created with createBranch."
                    .to_owned(),
            );
        }
        self.remote_guard()?;
        let failed = |reason: &str| format!("Push failed: {}", self.mask(reason.trim()));
        let mut remote = repository
            .find_remote("origin")
            .map_err(|error| failed(error.message()))?;
        let mut rejected: Vec<String> = Vec::new();
        let outcome = {
            let mut callbacks = RemoteCallbacks::new();
            callbacks.push_update_reference(|reference, status| {
                if let Some(status) = status {
                    rejected.push(format!("{reference}: {status}"));
                }
                Ok(())
            });
            let mut options = PushOptions::new();
            options.remote_callbacks(callbacks);
            // No `+`: never a force push.
            remote.push(
                &[format!("refs/heads/{branch}:refs/heads/{branch}")],
                Some(&mut options),
            )
        };
        if let Err(error) = outcome {
            return Err(failed(error.message()));
        }
        if !rejected.is_empty() {
            return Err(failed(&rejected.join("; ")));
        }
        Ok(format!("Pushed {branch}"))
    }
}

/// `text` with every occurrence of the token replaced by `***`.
fn mask_token(token: Option<&str>, text: &str) -> String {
    match token {
        Some(token) if !token.is_empty() => text.replace(token, "***"),
        _ => text.to_owned(),
    }
}

/// `name` is usable as a branch name (and not as an option: no leading `-`).
fn is_branch_name(name: &str) -> bool {
    !name.starts_with('-') && Reference::is_valid_name(&format!("refs/heads/{name}"))
}

/// A 4 to 40 digit hexadecimal commit hash.
fn is_hex_hash(text: &str) -> bool {
    (4..=40).contains(&text.len()) && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// `path` without `.` and `..` parts (lexically).
fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                result.pop();
            }
            other => result.push(other),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_is_masked_in_error_texts() {
        let masked = mask_token(Some("tok-SECRET"), "denied to tok-SECRET and tok-SECRET");
        assert_eq!(masked, "denied to *** and ***");
        assert_eq!(mask_token(None, "plain"), "plain");
        assert_eq!(mask_token(Some(""), "plain"), "plain");
    }
}
