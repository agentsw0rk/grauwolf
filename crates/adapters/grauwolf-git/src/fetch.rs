//! Fetching one commit of a repository and reading files from its tree, without a working
//! directory (ADR-0107 stage 3): a bare scratch repository in a temporary directory, removed
//! afterwards.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use git2::{
    AutotagOption, FetchOptions, Object, ObjectType, Repository, TreeWalkMode, TreeWalkResult,
};

use crate::{Settings, SignedObjects, init_libgit2, origin_of, register, with_settings};

/// Upper limit for the content that [`fetch_files`] returns.
pub const MAX_FETCHED_BYTES: usize = 16 * 1024 * 1024;

/// What to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wanted {
    /// A commit, full (40 hex digits) or abbreviated (at least 7).
    Commit(String),
    /// A tag, resolved to its commit.
    Tag(String),
    /// A branch, resolved to its tip.
    Branch(String),
}

/// A repository, what to fetch from it and how to authenticate.
#[derive(Clone)]
pub struct FetchRequest {
    /// Repository URL (`https://`, or `http://` for tests).
    pub url: String,
    /// Commit, tag or branch.
    pub wanted: Wanted,
    /// Token for `Authorization: Basic`, sent only to the scheme and host of `url`.
    pub token: Option<String>,
    /// Time limit of the network requests.
    pub timeout: Duration,
}

impl std::fmt::Debug for FetchRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FetchRequest")
            .field("url", &self.url)
            .field("wanted", &self.wanted)
            .field("token", &self.token.as_ref().map(|_| "***"))
            .finish()
    }
}

impl FetchRequest {
    /// A request with the default timeout of 120 seconds and no token.
    pub fn new(url: impl Into<String>, wanted: Wanted) -> Self {
        Self {
            url: url.into(),
            wanted,
            token: None,
            timeout: Duration::from_secs(120),
        }
    }
}

/// The commit and the selected files of its tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    /// The full commit id, 40 hex digits.
    pub commit: String,
    /// Path relative to the folder → content.
    pub files: BTreeMap<String, Vec<u8>>,
    /// The raw commit object and, when a tag was asked for and it is an annotated tag, the raw
    /// tag object: what a signature check needs (see [`crate::verify_objects`]).
    pub objects: SignedObjects,
}

/// Why fetching failed; the text never contains the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitError(pub String);

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GitError {}

/// Fetches the wanted commit (shallow when the server allows it, otherwise the branch and tag
/// tips) and returns its id and the files below `folder` (`None` is the root) for which
/// `include(<path relative to the folder>)` is true. Symbolic links and submodules are skipped.
/// Blocking: call it from `spawn_blocking`.
pub fn fetch_files(
    request: &FetchRequest,
    folder: Option<&str>,
    include: &dyn Fn(&str) -> bool,
) -> Result<Fetched, GitError> {
    let mask = |text: &str| match request.token.as_deref() {
        Some(token) if !token.is_empty() => text.replace(token, "***"),
        _ => text.to_owned(),
    };
    run(request, folder, include).map_err(|text| GitError(mask(&text)))
}

fn run(
    request: &FetchRequest,
    folder: Option<&str>,
    include: &dyn Fn(&str) -> bool,
) -> Result<Fetched, String> {
    init_libgit2();
    register();
    let scratch = Scratch::new().map_err(|error| format!("scratch directory: {error}"))?;
    let repository = Repository::init_bare(scratch.path()).map_err(describe)?;
    let settings = Settings {
        token: request.token.clone(),
        origin: origin_of(&request.url),
        timeout: request.timeout,
        auth: None,
        bearer: None,
    };
    let commit = with_settings(settings, || fetch(&repository, request))?;
    let files = read_files(&repository, &commit, folder, include)?;
    let objects = read_objects(&repository, &commit, &request.wanted)?;
    Ok(Fetched {
        commit: commit.to_string(),
        files,
        objects,
    })
}

/// The raw commit object, and the raw tag object of an annotated tag that was asked for.
fn read_objects(
    repository: &Repository,
    commit: &git2::Oid,
    wanted: &Wanted,
) -> Result<SignedObjects, String> {
    let odb = repository.odb().map_err(describe)?;
    let raw = |id: git2::Oid| odb.read(id).map_err(describe);
    let tag = match wanted {
        Wanted::Tag(_) => repository
            .refname_to_id(WANTED_REF)
            .ok()
            .map(raw)
            .transpose()?
            .filter(|object| object.kind() == ObjectType::Tag)
            .map(|object| object.data().to_vec()),
        _ => None,
    };
    Ok(SignedObjects {
        commit: raw(*commit)?.data().to_vec(),
        tag,
    })
}

const WANTED_REF: &str = "refs/grauwolf/wanted";

fn fetch(repository: &Repository, request: &FetchRequest) -> Result<git2::Oid, String> {
    // Shallow first; servers without shallow support (or without fetching by commit) get the
    // whole history of the refs. An abbreviated commit can only be found in the fetched refs.
    let one = |spec: String| vec![spec];
    let attempts: Vec<(Vec<String>, i32)> = match &request.wanted {
        Wanted::Tag(tag) => {
            let spec = format!("+refs/tags/{tag}:{WANTED_REF}");
            vec![(one(spec.clone()), 1), (one(spec), 0)]
        }
        Wanted::Branch(branch) => {
            let spec = format!("+refs/heads/{branch}:{WANTED_REF}");
            vec![(one(spec.clone()), 1), (one(spec), 0)]
        }
        Wanted::Commit(rev) if rev.len() == 40 => {
            vec![(one(format!("+{rev}:{WANTED_REF}")), 1), (all_tips(), 0)]
        }
        Wanted::Commit(_) => vec![(all_tips(), 0)],
    };
    let mut last = None;
    for (specs, depth) in attempts {
        let outcome = run_fetch(repository, &request.url, &specs, depth)
            .and_then(|()| resolve(repository, &request.wanted));
        match outcome {
            Ok(commit) => return Ok(commit),
            Err(error) => last = Some(error),
        }
    }
    Err(last.unwrap_or_else(|| "nothing to fetch".to_owned()))
}

fn all_tips() -> Vec<String> {
    vec![
        "+refs/heads/*:refs/grauwolf/heads/*".to_owned(),
        "+refs/tags/*:refs/grauwolf/tags/*".to_owned(),
    ]
}

fn run_fetch(
    repository: &Repository,
    url: &str,
    specs: &[String],
    depth: i32,
) -> Result<(), String> {
    let mut remote = repository.remote_anonymous(url).map_err(describe)?;
    let mut options = FetchOptions::new();
    options.download_tags(AutotagOption::None);
    if depth > 0 {
        options.depth(depth);
    }
    remote
        .fetch(specs, Some(&mut options), None)
        .map_err(describe)
}

fn resolve(repository: &Repository, wanted: &Wanted) -> Result<git2::Oid, String> {
    let commit_of = |object: Object<'_>| {
        object
            .peel_to_commit()
            .map(|commit| commit.id())
            .map_err(describe)
    };
    match wanted {
        Wanted::Tag(tag) => {
            let object = repository
                .revparse_single(WANTED_REF)
                .map_err(|_| format!("tag `{tag}` not found"))?;
            commit_of(object)
        }
        Wanted::Branch(branch) => {
            let object = repository
                .revparse_single(WANTED_REF)
                .map_err(|_| format!("branch `{branch}` not found"))?;
            commit_of(object)
        }
        Wanted::Commit(rev) => {
            let object = repository.revparse_single(rev).map_err(|_| {
                format!("commit {rev} not found (it must be reachable from a branch or tag)")
            })?;
            let id = commit_of(object)?;
            if !id.to_string().starts_with(rev.as_str()) {
                return Err(format!("{rev} is not a commit"));
            }
            Ok(id)
        }
    }
}

fn read_files(
    repository: &Repository,
    commit: &git2::Oid,
    folder: Option<&str>,
    include: &dyn Fn(&str) -> bool,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let root = repository
        .find_commit(*commit)
        .and_then(|commit| commit.tree())
        .map_err(describe)?;
    let tree = match folder {
        None => root,
        Some(folder) => {
            let entry = root
                .get_path(Path::new(folder))
                .map_err(|_| format!("the folder `{folder}` does not exist in the repository"))?;
            entry
                .to_object(repository)
                .and_then(|object| object.peel_to_tree())
                .map_err(|_| format!("`{folder}` is not a folder"))?
        }
    };
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    let mut failure = None;
    let walked = tree.walk(TreeWalkMode::PreOrder, |parent, entry| {
        if entry.kind() != Some(ObjectType::Blob) || entry.filemode() == 0o120000 {
            return TreeWalkResult::Ok;
        }
        let Ok(name) = entry.name() else {
            return TreeWalkResult::Ok;
        };
        let relative = format!("{parent}{name}");
        if !include(&relative) {
            return TreeWalkResult::Ok;
        }
        match repository.find_blob(entry.id()) {
            Ok(blob) => {
                total += blob.content().len();
                if total > MAX_FETCHED_BYTES {
                    failure = Some(format!(
                        "the selected files are larger than {} MiB",
                        MAX_FETCHED_BYTES / (1024 * 1024)
                    ));
                    return TreeWalkResult::Abort;
                }
                files.insert(relative, blob.content().to_vec());
                TreeWalkResult::Ok
            }
            Err(error) => {
                failure = Some(describe(error));
                TreeWalkResult::Abort
            }
        }
    });
    if let Some(reason) = failure {
        return Err(reason);
    }
    if let Err(error) = walked {
        if error.code() != git2::ErrorCode::User {
            return Err(describe(error));
        }
    }
    Ok(files)
}

fn describe(error: git2::Error) -> String {
    error.message().trim().to_owned()
}

/// A scratch directory that is removed again.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> std::io::Result<Self> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "grauwolf-git-{}-{nanos}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
