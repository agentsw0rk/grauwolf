//! Git sources of tool packages (ADR-0107 stage 3): `{ git, path?, rev | tag | branch }`.

use crate::{SkillSourceSpec, ToolPackageSpec};

/// What of the repository a tool package entry pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitRef {
    /// A commit, full or abbreviated (7 to 40 lowercase hex digits).
    Rev(String),
    /// A tag, resolved to its commit and locked.
    Tag(String),
    /// A branch, resolved to its tip commit and locked.
    Branch(String),
}

/// A parsed git source of a `tool_packages` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSource {
    /// Repository URL: `https://`, or `http://` for `localhost`/`127.0.0.1`.
    pub url: String,
    /// Folder of the package in the repository; `None` is the repository root.
    pub path: Option<String>,
    /// Revision, tag or branch.
    pub reference: GitRef,
}

/// A problem with a git entry: the field it belongs to (`git`, `path`, `rev`, …) and the reason,
/// which never repeats the URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSourceProblem {
    /// Name of the field in the entry.
    pub field: &'static str,
    /// What is wrong.
    pub reason: String,
}

impl GitSource {
    /// Parses the git fields of `entry` (which has `git` set); the error names the field.
    pub fn from_entry(entry: &ToolPackageSpec) -> Result<Self, GitSourceProblem> {
        Self::from_parts(
            entry.git.as_deref().unwrap_or_default(),
            entry.path.as_deref(),
            &entry.rev,
            &entry.tag,
            &entry.branch,
        )
    }

    /// Parses the git fields of a `skills` entry (which has `git` set); the error names the field.
    pub fn from_skill_source(entry: &SkillSourceSpec) -> Result<Self, GitSourceProblem> {
        Self::from_parts(
            entry.git.as_deref().unwrap_or_default(),
            entry.path.as_deref(),
            &entry.rev,
            &entry.tag,
            &entry.branch,
        )
    }

    /// Parses the command-line form of a source, which is the lock key:
    /// `git+<url>[//<path>]@<rev>`, `…@tag:<tag>` or `…@branch:<branch>`. The same rules as for a
    /// `tool_packages` entry apply; the error never repeats the URL.
    pub fn parse_reference(text: &str) -> Result<Self, String> {
        let example = "git+https://github.com/acme/tools.git//text@tag:v1";
        let rest = text
            .strip_prefix("git+")
            .ok_or_else(|| format!("a git reference starts with `git+`, e.g. `{example}`"))?;
        let marker = ["@tag:", "@branch:"]
            .iter()
            .filter_map(|marker| rest.find(marker))
            .min();
        let split = marker.or_else(|| rest.rfind('@')).ok_or_else(|| {
            format!(
                "the git reference names no revision: add `@<commit>`, `@tag:<tag>` or \
                 `@branch:<branch>`, e.g. `{example}`"
            )
        })?;
        let (location, selector) = rest.split_at(split);
        let selector = &selector[1..];
        let (mut rev, mut tag, mut branch) = (None, None, None);
        if let Some(name) = selector.strip_prefix("tag:") {
            tag = Some(name.to_owned());
        } else if let Some(name) = selector.strip_prefix("branch:") {
            branch = Some(name.to_owned());
        } else {
            rev = Some(selector.to_owned());
        }
        // The folder follows the first `//` after the scheme.
        let authority_start = location.find("://").map_or(0, |at| at + 3);
        let (url, path) = match location[authority_start..].find("//") {
            Some(at) => (
                &location[..authority_start + at],
                Some(&location[authority_start + at + 2..]),
            ),
            None => (location, None),
        };
        Self::from_parts(url, path, &rev, &tag, &branch)
            .map_err(|problem| format!("{}: {}", problem.field, problem.reason))
    }

    fn from_parts(
        url: &str,
        path: Option<&str>,
        rev: &Option<String>,
        tag: &Option<String>,
        branch: &Option<String>,
    ) -> Result<Self, GitSourceProblem> {
        let problem = |field, reason: &str| GitSourceProblem {
            field,
            reason: reason.to_owned(),
        };
        if let Some(reason) = url_problem(url) {
            return Err(problem("git", reason));
        }
        let path = match path {
            Some(path) => {
                if let Some(reason) = path_problem(path) {
                    return Err(problem("path", reason));
                }
                Some(path.to_owned())
            }
            None => None,
        };
        let reference = match (rev, tag, branch) {
            (Some(rev), None, None) => {
                let valid = (7..=40).contains(&rev.len())
                    && rev.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'));
                if !valid {
                    return Err(problem(
                        "rev",
                        "must be a commit of 7 to 40 lowercase hex digits",
                    ));
                }
                GitRef::Rev(rev.clone())
            }
            (None, Some(tag), None) => GitRef::Tag(checked_name("tag", tag)?),
            (None, None, Some(branch)) => GitRef::Branch(checked_name("branch", branch)?),
            _ => {
                return Err(problem(
                    "git",
                    "give exactly one of `rev` (commit), `tag` and `branch`",
                ));
            }
        };
        Ok(Self {
            url: url.to_owned(),
            path,
            reference,
        })
    }

    /// The key of the entry in the lockfile: `git+<url>[//<path>]@<ref>` with `<ref>` the commit
    /// as written, `tag:<tag>` or `branch:<branch>`.
    pub fn lock_key(&self) -> String {
        let path = self
            .path
            .as_deref()
            .map(|path| format!("//{path}"))
            .unwrap_or_default();
        let reference = match &self.reference {
            GitRef::Rev(rev) => rev.clone(),
            GitRef::Tag(tag) => format!("tag:{tag}"),
            GitRef::Branch(branch) => format!("branch:{branch}"),
        };
        format!("git+{}{path}@{reference}", self.url)
    }
}

fn checked_name(field: &'static str, name: &str) -> Result<String, GitSourceProblem> {
    let bad = name.is_empty()
        || name.starts_with(['-', '/'])
        || name.ends_with(['/', '.'])
        || name.contains("..")
        || name
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c));
    if bad {
        return Err(GitSourceProblem {
            field,
            reason: "is not a valid git reference name".to_owned(),
        });
    }
    Ok(name.to_owned())
}

fn url_problem(url: &str) -> Option<&'static str> {
    let (secure, rest) = match (url.strip_prefix("https://"), url.strip_prefix("http://")) {
        (Some(rest), _) => (true, rest),
        (None, Some(rest)) => (false, rest),
        _ => return Some("must start with https:// (http:// only for localhost and 127.0.0.1)"),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return Some("must not contain user info (user:password@); use GIT_TOKEN");
    }
    if authority.is_empty() {
        return Some("has no host");
    }
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, _)| host);
    if !secure && !matches!(host, "localhost" | "127.0.0.1") {
        return Some("must start with https:// (http:// only for localhost and 127.0.0.1)");
    }
    None
}

fn path_problem(path: &str) -> Option<&'static str> {
    let bad = path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..");
    bad.then_some("must be a relative folder without `..`, empty or `.` parts")
}
