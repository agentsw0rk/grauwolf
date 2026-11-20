//! The project repository (ADR-0130): a clone of one repository with a folder per harness.

use std::path::{Path, PathBuf};

/// Token for http(s) remotes. It goes to the crate's own transport (`Authorization: Basic`, user
/// `x-access-token`, only to the remote's origin), never into a URL, the configuration or a
/// message. `username` is not sent: GitHub takes a personal access token under any user name.
#[derive(Clone)]
pub struct Credentials {
    pub username: String,
    pub token: String,
}

/// A working clone of the project repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectClone {
    pub dir: PathBuf,
    pub branch: String,
}

pub fn ensure_project_clone(
    dir: &Path,
    remote_url: &str,
    branch: &str,
    credentials: Option<&Credentials>,
) -> Result<ProjectClone, String> {
    crate::init_libgit2();
    if let Ok(repository) = git2::Repository::open(dir) {
        let origin = repository
            .find_remote("origin")
            .ok()
            .and_then(|remote| remote.url().ok().map(crate::checkout::without_credentials));
        let wanted = crate::checkout::without_credentials(remote_url);
        if origin.as_deref() != Some(wanted.as_str()) {
            return Err(format!(
                "{} is a clone of {}, not of {wanted}",
                dir.display(),
                origin.as_deref().unwrap_or("no remote `origin`"),
            ));
        }
        return Ok(ProjectClone {
            dir: dir.to_owned(),
            branch: branch.to_owned(),
        });
    }
    if is_non_empty_dir(dir) {
        return adopt(dir, remote_url, branch, credentials);
    }
    // The branch may not exist yet (an empty remote): clone what there is, the first commit
    // creates the branch.
    let exists = remote_has_branch(remote_url, branch, credentials)?;
    let mut builder = git2::build::RepoBuilder::new();
    if exists {
        builder.branch(branch);
    }
    let repository = with_credentials(remote_url, credentials, || builder.clone(remote_url, dir))
        .map_err(|error| error.message().to_owned())?;
    if repository.head().is_err() {
        repository
            .set_head(&format!("refs/heads/{branch}"))
            .map_err(|error| error.message().to_owned())?;
    }
    Ok(ProjectClone {
        dir: dir.to_owned(),
        branch: branch.to_owned(),
    })
}

/// Runs `call` over the crate's transport (registered once) with the token for `remote_url`'s
/// origin; local remotes do not use it.
fn with_credentials<T>(
    remote_url: &str,
    credentials: Option<&Credentials>,
    call: impl FnOnce() -> T,
) -> T {
    crate::transport::register();
    crate::transport::with_settings(
        crate::transport::Settings {
            token: credentials.map(|credentials| credentials.token.clone()),
            origin: crate::transport::origin_of(remote_url),
            timeout: std::time::Duration::from_secs(120),
            auth: None,
            bearer: None,
        },
        call,
    )
}

fn remote_has_branch(
    remote_url: &str,
    branch: &str,
    credentials: Option<&Credentials>,
) -> Result<bool, String> {
    let message_of = |error: git2::Error| error.message().to_owned();
    let mut remote = git2::Remote::create_detached(remote_url).map_err(message_of)?;
    let wanted = format!("refs/heads/{branch}");
    with_credentials(remote_url, credentials, || {
        let connection = remote
            .connect_auth(git2::Direction::Fetch, None, None)
            .map_err(message_of)?;
        let found = connection
            .list()
            .map_err(message_of)?
            .iter()
            .any(|head| head.name() == wanted);
        Ok(found)
    })
}

fn is_non_empty_dir(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
}

/// Turns a folder with files into a clone: `git init`, `origin`, and if the remote has `branch`,
/// `HEAD` and the index are moved to its tip while the files stay as they are (the first commit
/// records the difference).
fn adopt(
    dir: &Path,
    remote_url: &str,
    branch: &str,
    credentials: Option<&Credentials>,
) -> Result<ProjectClone, String> {
    let message = |error: git2::Error| error.message().to_owned();
    let repository = git2::Repository::init(dir).map_err(message)?;
    let mut remote = repository.remote("origin", remote_url).map_err(message)?;
    let refspec = format!("+refs/heads/{branch}:refs/remotes/origin/{branch}");
    with_credentials(remote_url, credentials, || {
        remote.fetch(&[refspec.as_str()], None, None)
    })
    .map_err(message)?;
    let tracking = format!("refs/remotes/origin/{branch}");
    if let Ok(tip) = repository.refname_to_id(&tracking) {
        let commit = repository.find_object(tip, None).map_err(message)?;
        repository
            .reference(&format!("refs/heads/{branch}"), tip, true, "project clone")
            .map_err(message)?;
        repository
            .set_head(&format!("refs/heads/{branch}"))
            .map_err(message)?;
        repository
            .reset(&commit, git2::ResetType::Mixed, None)
            .map_err(message)?;
    } else {
        repository
            .set_head(&format!("refs/heads/{branch}"))
            .map_err(message)?;
    }
    Ok(ProjectClone {
        dir: dir.to_owned(),
        branch: branch.to_owned(),
    })
}

/// Commits every change below `folder` (relative to `dir`) on the current branch and returns the
/// commit id, or `None` when the folder did not change. Nothing outside the folder is committed.
pub fn commit_folder(
    dir: &Path,
    folder: &str,
    message: &str,
    author_name: &str,
    author_email: &str,
) -> Result<Option<String>, String> {
    crate::init_libgit2();
    let message_of = |error: git2::Error| error.message().to_owned();
    let repository = git2::Repository::open(dir).map_err(message_of)?;
    let parent = repository
        .head()
        .ok()
        .and_then(|head| head.peel_to_commit().ok());
    let mut index = repository.index().map_err(message_of)?;
    // Start from the last commit, so that only this folder's changes get in.
    match &parent {
        Some(parent) => index.read_tree(&parent.tree().map_err(message_of)?),
        None => index.clear(),
    }
    .map_err(message_of)?;
    index
        .add_all([folder], git2::IndexAddOption::DEFAULT, None)
        .map_err(message_of)?;
    index.update_all([folder], None).map_err(message_of)?;
    index.write().map_err(message_of)?;
    let tree = repository
        .find_tree(index.write_tree().map_err(message_of)?)
        .map_err(message_of)?;
    let unchanged = match &parent {
        Some(parent) => parent.tree_id() == tree.id(),
        None => tree.is_empty(),
    };
    if unchanged {
        return Ok(None);
    }
    let signature = git2::Signature::now(author_name, author_email).map_err(message_of)?;
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let id = repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &parents,
        )
        .map_err(message_of)?;
    Ok(Some(id.to_string()))
}

/// Pushes `refs/heads/<branch>` to `origin`. A refusal (not a fast-forward, rejected by the
/// server) is an error with the server's message.
pub fn push(dir: &Path, branch: &str, credentials: Option<&Credentials>) -> Result<(), String> {
    crate::init_libgit2();
    let message_of = |error: git2::Error| error.message().to_owned();
    let repository = git2::Repository::open(dir).map_err(message_of)?;
    let mut remote = repository.find_remote("origin").map_err(message_of)?;
    let refused = std::cell::RefCell::new(None::<String>);
    let mut callbacks = git2::RemoteCallbacks::new();
    callbacks.push_update_reference(|reference, status| {
        if let Some(status) = status {
            *refused.borrow_mut() = Some(format!("{reference} was refused: {status}"));
        }
        Ok(())
    });
    let mut options = git2::PushOptions::new();
    options.remote_callbacks(callbacks);
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    let url = remote.url().unwrap_or_default().to_owned();
    with_credentials(&url, credentials, || {
        remote.push(&[refspec.as_str()], Some(&mut options))
    })
    .map_err(message_of)?;
    match refused.take() {
        Some(refusal) => Err(refusal),
        None => Ok(()),
    }
}
