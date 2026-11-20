//! What a local checkout says about itself (ADR-0124): where it comes from and which commit it is
//! at, so that a tool package packed from it can name its source and revision.

use std::path::Path;

/// The repository a folder belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    /// URL of the remote `origin` without credentials, if there is one.
    pub remote: Option<String>,
    /// The commit `HEAD` points to (40 hex digits).
    pub commit: String,
}

/// The checkout that `dir` lies in (searched upwards), or `None` outside a repository or before
/// the first commit. Only local files are read.
pub fn checkout_of(dir: &Path) -> Option<Checkout> {
    crate::init_libgit2();
    let repository = git2::Repository::discover(dir).ok()?;
    let commit = repository
        .head()
        .ok()?
        .peel_to_commit()
        .ok()?
        .id()
        .to_string();
    let remote = repository
        .find_remote("origin")
        .ok()
        .and_then(|remote| remote.url().ok().map(without_credentials));
    Some(Checkout { remote, commit })
}

/// The commit a project folder is built from in the cluster (ADR-0129): Kaniko clones `remote` at
/// `commit` on `branch` and builds `path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSource {
    /// URL of `origin` without credentials.
    pub remote: String,
    /// The branch `HEAD` is on, e.g. `refs/heads/main`.
    pub branch: String,
    /// The commit of `HEAD` (40 hex digits).
    pub commit: String,
    /// The folder relative to the repository's root, `/`-separated.
    pub path: String,
}

/// The build source of the project `folder`; an error says why it cannot be built from git.
pub fn build_source(folder: &Path) -> Result<BuildSource, String> {
    crate::init_libgit2();
    let shown = folder.display();
    let repository = git2::Repository::discover(folder)
        .map_err(|_| format!("{shown} is not in a git repository"))?;
    let head = repository
        .head()
        .map_err(|_| format!("the repository of {shown} has no commit yet"))?;
    let branch = head
        .name()
        .ok()
        .filter(|name| name.starts_with("refs/heads/"))
        .ok_or_else(|| format!("the repository of {shown} is not on a branch"))?
        .to_owned();
    let commit = head
        .peel_to_commit()
        .map_err(|error| error.message().to_owned())?
        .id()
        .to_string();
    let remote = repository
        .find_remote("origin")
        .ok()
        .and_then(|remote| remote.url().ok().map(without_credentials))
        .ok_or_else(|| {
            format!("the repository of {shown} has no remote `origin` a cluster could clone")
        })?;
    let root = repository
        .workdir()
        .ok_or_else(|| format!("the repository of {shown} has no working tree"))?;
    let path = folder
        .canonicalize()
        .ok()
        .and_then(|folder| {
            Some(
                folder
                    .strip_prefix(root.canonicalize().ok()?)
                    .ok()?
                    .to_owned(),
            )
        })
        .ok_or_else(|| format!("{shown} is not inside its repository"))?
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    // Kaniko builds the commit, not the working tree: a change below the folder would be missing.
    let mut options = git2::StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .pathspec(&path);
    let statuses = repository
        .statuses(Some(&mut options))
        .map_err(|error| error.message().to_owned())?;
    let changed: Vec<String> = statuses
        .iter()
        .filter(|entry| !entry.status().is_ignored())
        .filter_map(|entry| entry.path().ok().map(str::to_owned))
        .collect();
    if !changed.is_empty() {
        return Err(format!(
            "{shown} has changes that are not committed ({})",
            changed.join(", ")
        ));
    }
    // The cluster clones `origin`: the commit must be on its branch there (as of the last fetch).
    let tracking = format!(
        "refs/remotes/origin/{}",
        branch.trim_start_matches("refs/heads/")
    );
    let pushed = repository
        .refname_to_id(&tracking)
        .ok()
        .is_some_and(|remote_tip| {
            git2::Oid::from_str(&commit).is_ok_and(|commit| {
                remote_tip == commit
                    || repository
                        .graph_descendant_of(remote_tip, commit)
                        .unwrap_or(false)
            })
        });
    if !pushed {
        return Err(format!(
            "the commit {commit} of {shown} is not on origin yet: push {branch} first"
        ));
    }
    Ok(BuildSource {
        remote,
        branch,
        commit,
        path,
    })
}

/// `https://user:token@host/path` → `https://host/path`; other forms stay as they are.
pub(crate) fn without_credentials(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
            let host = authority
                .rsplit_once('@')
                .map_or(authority, |(_, host)| host);
            if path.is_empty() && !rest.contains('/') {
                format!("{scheme}://{host}")
            } else {
                format!("{scheme}://{host}/{path}")
            }
        }
        None => url.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_are_dropped_from_the_remote() {
        assert_eq!(
            without_credentials("https://x-access-token:secret@github.com/acme/tools.git"),
            "https://github.com/acme/tools.git"
        );
        assert_eq!(
            without_credentials("git@github.com:acme/tools.git"),
            "git@github.com:acme/tools.git"
        );
        assert_eq!(without_credentials("https://host"), "https://host");
    }
}
