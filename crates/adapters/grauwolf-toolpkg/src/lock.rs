//! The lockfile `grauwolf.lock` next to the spec (ADR-0107): JSON, sorted, stable. Per package
//! reference as written in the spec the resolved manifest digest and the SHA-256 of every module;
//! git entries (key `git+<url>[//<path>]@<rev | tag:<tag> | branch:<branch>`) record the commit.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::PackageFetchError;

/// File name of the lockfile, next to the spec.
pub const LOCKFILE_NAME: &str = "grauwolf.lock";

/// Content of the lockfile.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Lockfile {
    /// Version of the lockfile format.
    pub lock_version: u32,
    /// Reference as written → what it resolved to.
    pub packages: BTreeMap<String, LockedPackage>,
    /// Git entry key (`git+<url>[//<path>]@<ref>`, see `GitSource::lock_key`) → what it
    /// resolved to. Absent in lockfiles without git sources.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub git: BTreeMap<String, LockedGit>,
    /// Git skill source key (same form as the git keys) → what it resolved to (ADR-0103).
    /// Absent in lockfiles without git skills.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub skills: BTreeMap<String, LockedSkills>,
}

/// What a git skills source resolved to: the commit and the hash of the file list of its folder
/// (`files.json` of the cache entry: every file with its SHA-256).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockedSkills {
    /// The full commit id, 40 hex digits.
    pub commit: String,
    /// `sha256:<hex>` of the sorted list of files and their hashes.
    pub tree: String,
}

/// What a git entry resolved to (ADR-0107 stage 3): the commit it was fetched at and the hash of
/// every module built from its sources.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockedGit {
    /// The full commit id, 40 hex digits.
    pub commit: String,
    /// Module file name → `sha256:<hex>` of its content.
    pub modules: BTreeMap<String, String>,
}

/// What a reference resolved to.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockedPackage {
    /// Digest of the OCI manifest.
    pub digest: String,
    /// Module file name → `sha256:<hex>` of its content.
    pub modules: BTreeMap<String, String>,
}

impl Default for Lockfile {
    fn default() -> Self {
        Self {
            lock_version: 1,
            packages: BTreeMap::new(),
            git: BTreeMap::new(),
            skills: BTreeMap::new(),
        }
    }
}

impl Lockfile {
    /// Reads the lockfile; a missing file is an empty lockfile.
    pub fn load(path: &Path) -> Result<Self, PackageFetchError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(io(path, error)),
        };
        let lock: Self = serde_json::from_str(&text)
            .map_err(|error| PackageFetchError::Lock(format!("{}: {error}", path.display())))?;
        if lock.lock_version != 1 {
            return Err(PackageFetchError::Lock(format!(
                "{}: unsupported lock_version {}",
                path.display(),
                lock.lock_version
            )));
        }
        Ok(lock)
    }

    /// The file content: pretty JSON with sorted keys and a final newline.
    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("a lockfile is serializable");
        text.push('\n');
        text
    }

    /// Writes the lockfile; an empty one removes the file.
    pub fn save(&self, path: &Path) -> Result<(), PackageFetchError> {
        if self.packages.is_empty() && self.git.is_empty() && self.skills.is_empty() {
            return match std::fs::remove_file(path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(io(path, error)),
                _ => Ok(()),
            };
        }
        std::fs::write(path, self.to_json()).map_err(|error| io(path, error))
    }
}

fn io(path: &Path, error: std::io::Error) -> PackageFetchError {
    PackageFetchError::Io {
        path: path.display().to_string(),
        reason: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_entries_are_written_in_their_own_sorted_section_and_read_back() {
        let mut lock = Lockfile::default();
        lock.git.insert(
            "git+https://h/r.git//text@4e1a9c2".into(),
            LockedGit {
                commit: "4e1a9c2".repeat(5) + "abcde",
                modules: BTreeMap::from([("add.wasm".into(), "sha256:00".into())]),
            },
        );
        let text = lock.to_json();
        assert!(text.contains("\"git\": {"), "{text}");
        assert_eq!(serde_json::from_str::<Lockfile>(&text).unwrap(), lock);
        // Without git entries the section is absent: old lockfiles stay byte-identical.
        assert!(!Lockfile::default().to_json().contains("git"));
    }
}
