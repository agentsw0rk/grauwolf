//! Cache of pulled packages under the tool cache, keyed by the manifest digest: a cached digest
//! is never fetched again; everything is verified again on every load.

use std::path::{Path, PathBuf};

use grauwolf_git::SignedObjects;
use grauwolf_oci::cosign::Signature;

use crate::layout::Layout;
use crate::{Package, PackageFetchError, sha256_digest};

/// Directory of the packages below the tool cache.
const PACKAGES_DIR: &str = "packages";
/// File with the cosign signatures of an entry.
const SIGNATURES_FILE: &str = "signatures.json";
/// Files with the raw git objects of a git entry, for checking its signatures again offline.
const COMMIT_OBJECT_FILE: &str = "commit.object";
const TAG_OBJECT_FILE: &str = "tag.object";
/// Skills entries: the files below `tree/`, their hashes in `files.json`.
const SKILLS_TREE_DIR: &str = "tree";
const SKILLS_INDEX_FILE: &str = "files.json";

/// Packages on disk: `<dir>/packages/sha256-<hex>/manifest.json` plus `blobs/<hex>`.
#[derive(Debug, Clone)]
pub struct PackageCache {
    root: PathBuf,
    /// Start of the names of git entries: `git` for packages, `skills` for skill folders.
    prefix: &'static str,
}

impl PackageCache {
    /// Cache below the tool cache directory `tool_cache` (e.g. `.grauwolf/tools`).
    pub fn new(tool_cache: impl AsRef<Path>) -> Self {
        Self {
            root: tool_cache.as_ref().join(PACKAGES_DIR),
            prefix: "git",
        }
    }

    /// The same cache for the git entries of skill folders (ADR-0103): `skills-<commit>[-<hash
    /// of the path>]/` with `files.json` and `tree/`, plus the signed objects like a package.
    pub(crate) fn for_skills(&self) -> Self {
        Self {
            root: self.root.clone(),
            prefix: "skills",
        }
    }

    /// The files of a skills entry (relative path → content) at `commit` and `path`, if
    /// cached; every file is verified against `files.json`, a missing or changed one is an
    /// error. Returns the directory with the files and the hash of `files.json`.
    pub(crate) fn load_skills(
        &self,
        commit: &str,
        path: Option<&str>,
    ) -> Result<Option<(PathBuf, String)>, PackageFetchError> {
        let dir = self.git_entry(commit, path);
        let index_path = dir.join(SKILLS_INDEX_FILE);
        let index = match std::fs::read(&index_path) {
            Ok(index) => index,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io(&index_path, error)),
        };
        let hashes: std::collections::BTreeMap<String, String> = serde_json::from_slice(&index)
            .map_err(|error| {
                PackageFetchError::InvalidPackage(format!("{}: {error}", index_path.display()))
            })?;
        let tree = dir.join(SKILLS_TREE_DIR);
        for (relative, hash) in &hashes {
            let file = tree.join(relative);
            let bytes = std::fs::read(&file).map_err(|error| io(&file, error))?;
            if sha256_digest(&bytes) != *hash {
                return Err(PackageFetchError::InvalidPackage(format!(
                    "{}: the cached file was changed (expected {hash})",
                    file.display()
                )));
            }
        }
        Ok(Some((tree, sha256_digest(&index))))
    }

    /// Stores the files of a skills entry atomically; returns the directory with the files and
    /// the hash of `files.json` (see [`Self::load_skills`]).
    pub(crate) fn store_skills(
        &self,
        commit: &str,
        path: Option<&str>,
        files: &std::collections::BTreeMap<String, Vec<u8>>,
    ) -> Result<(PathBuf, String), PackageFetchError> {
        let target = self.git_entry(commit, path);
        let hashes: std::collections::BTreeMap<&String, String> = files
            .iter()
            .map(|(relative, bytes)| (relative, sha256_digest(bytes)))
            .collect();
        let index = serde_json::to_vec_pretty(&hashes).expect("a map of strings is serializable");
        if !target.exists() {
            std::fs::create_dir_all(&self.root).map_err(|error| io(&self.root, error))?;
            let staging = self.root.join(format!(
                ".tmp-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |elapsed| elapsed.as_nanos())
            ));
            let write = || -> std::io::Result<()> {
                for (relative, bytes) in files {
                    let file = staging.join(SKILLS_TREE_DIR).join(relative);
                    std::fs::create_dir_all(file.parent().unwrap_or(&staging))?;
                    std::fs::write(file, bytes)?;
                }
                std::fs::create_dir_all(staging.join(SKILLS_TREE_DIR))?;
                std::fs::write(staging.join(SKILLS_INDEX_FILE), &index)
            };
            match write().and_then(|()| std::fs::rename(&staging, &target)) {
                Ok(()) => {}
                // A parallel process stored the same entry first.
                Err(_) if target.exists() => {
                    let _ = std::fs::remove_dir_all(&staging);
                }
                Err(error) => {
                    let _ = std::fs::remove_dir_all(&staging);
                    return Err(io(&target, error));
                }
            }
        }
        Ok((target.join(SKILLS_TREE_DIR), sha256_digest(&index)))
    }

    fn entry(&self, digest: &str) -> PathBuf {
        self.root.join(digest.replace(':', "-"))
    }

    /// The package with manifest digest `digest`, if cached. Manifest and blobs are verified
    /// against their digests; a damaged entry is an error, not a silent refetch.
    pub fn load(&self, digest: &str) -> Result<Option<Package>, PackageFetchError> {
        self.load_dir(self.entry(digest), Some(digest))
    }

    /// The package built from `path` (default: the root) of the repository at `commit`, if
    /// cached, verified like an OCI entry. Its digest is that of its manifest.
    pub fn load_git(
        &self,
        commit: &str,
        path: Option<&str>,
    ) -> Result<Option<Package>, PackageFetchError> {
        self.load_dir(self.git_entry(commit, path), None)
    }

    /// Stores a package built from sources, atomically; see [`Self::load_git`].
    pub(crate) fn store_git(
        &self,
        commit: &str,
        path: Option<&str>,
        manifest: &[u8],
        blobs: &[(String, Vec<u8>)],
    ) -> Result<(), PackageFetchError> {
        self.store_dir(self.git_entry(commit, path), manifest, blobs)
    }

    /// The raw commit (and tag) object stored with the git entry, if any.
    pub fn load_git_objects(
        &self,
        commit: &str,
        path: Option<&str>,
    ) -> Result<Option<SignedObjects>, PackageFetchError> {
        let dir = self.git_entry(commit, path);
        let read = |name: &str| match std::fs::read(dir.join(name)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io(&dir.join(name), error)),
        };
        Ok(read(COMMIT_OBJECT_FILE)?.map(|commit| SignedObjects {
            commit,
            tag: read(TAG_OBJECT_FILE).ok().flatten(),
        }))
    }

    /// Replaces the objects stored with the git entry (atomically per file); nothing happens
    /// when the entry does not exist.
    pub fn store_git_objects(
        &self,
        commit: &str,
        path: Option<&str>,
        objects: &SignedObjects,
    ) -> Result<(), PackageFetchError> {
        let dir = self.git_entry(commit, path);
        if !dir.is_dir() {
            return Ok(());
        }
        let write = |name: &str, bytes: &[u8]| {
            let target = dir.join(name);
            let staging = dir.join(format!(".{name}.{}", std::process::id()));
            std::fs::write(&staging, bytes)
                .and_then(|()| std::fs::rename(&staging, &target))
                .map_err(|error| io(&target, error))
        };
        match &objects.tag {
            Some(tag) => write(TAG_OBJECT_FILE, tag)?,
            None => match std::fs::remove_file(dir.join(TAG_OBJECT_FILE)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io(&dir.join(TAG_OBJECT_FILE), error)),
            },
        }
        write(COMMIT_OBJECT_FILE, &objects.commit)
    }

    fn git_entry(&self, commit: &str, path: Option<&str>) -> PathBuf {
        let name = match path {
            None => format!("{}-{commit}", self.prefix),
            Some(path) => {
                let hash = sha256_digest(path.as_bytes());
                format!(
                    "{}-{commit}-{}",
                    self.prefix,
                    &hash["sha256:".len()..][..16]
                )
            }
        };
        self.root.join(name)
    }

    fn load_dir(
        &self,
        dir: PathBuf,
        expected: Option<&str>,
    ) -> Result<Option<Package>, PackageFetchError> {
        let manifest = match std::fs::read(dir.join("manifest.json")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io(&dir, error)),
        };
        let damaged = |reason: String| {
            PackageFetchError::InvalidPackage(format!(
                "cache entry {} is damaged ({reason}); delete it to fetch the package again",
                dir.display()
            ))
        };
        let digest = sha256_digest(&manifest);
        if expected.is_some_and(|expected| expected != digest) {
            return Err(damaged("manifest does not match its digest".into()));
        }
        let layout = Layout::parse(&manifest)?;
        layout
            .assemble(&digest, |blob_digest| {
                let hex = blob_digest.trim_start_matches("sha256:");
                let bytes = std::fs::read(dir.join("blobs").join(hex))
                    .map_err(|error| damaged(format!("{blob_digest}: {error}")))?;
                if sha256_digest(&bytes) != blob_digest {
                    return Err(damaged(format!("{blob_digest} does not match its digest")));
                }
                Ok(bytes)
            })
            .map(Some)
    }

    /// The signatures stored with the entry of manifest `digest` (none if there are none), so
    /// that the policy can be checked again without the registry.
    pub fn load_signatures(&self, digest: &str) -> Result<Vec<Signature>, PackageFetchError> {
        let path = self.entry(digest).join(SIGNATURES_FILE);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(io(&path, error)),
        };
        let damaged = || {
            PackageFetchError::InvalidPackage(format!(
                "cache file {} is damaged; delete it to fetch the signatures again",
                path.display()
            ))
        };
        let stored: Vec<serde_json::Value> =
            serde_json::from_slice(&bytes).map_err(|_| damaged())?;
        stored
            .iter()
            .map(|entry| {
                Ok(Signature {
                    payload: entry["payload"]
                        .as_str()
                        .ok_or_else(damaged)?
                        .as_bytes()
                        .to_vec(),
                    signature: entry["signature"].as_str().ok_or_else(damaged)?.to_owned(),
                })
            })
            .collect()
    }

    /// Replaces the signatures stored with the entry of manifest `digest` (atomically). Payloads
    /// that are not UTF-8 text (cosign's are JSON) are not stored.
    pub fn store_signatures(
        &self,
        digest: &str,
        signatures: &[Signature],
    ) -> Result<(), PackageFetchError> {
        let dir = self.entry(digest);
        if !dir.is_dir() {
            return Ok(());
        }
        let stored: Vec<serde_json::Value> = signatures
            .iter()
            .filter_map(|signature| {
                let payload = std::str::from_utf8(&signature.payload).ok()?;
                Some(serde_json::json!({ "payload": payload, "signature": signature.signature }))
            })
            .collect();
        let target = dir.join(SIGNATURES_FILE);
        let staging = dir.join(format!(".{SIGNATURES_FILE}.{}", std::process::id()));
        std::fs::write(&staging, serde_json::Value::Array(stored).to_string())
            .and_then(|()| std::fs::rename(&staging, &target))
            .map_err(|error| io(&target, error))
    }

    /// Stores the OCI manifest and the verified blobs of a package atomically.
    pub(crate) fn store(
        &self,
        digest: &str,
        manifest: &[u8],
        blobs: &[(String, Vec<u8>)],
    ) -> Result<(), PackageFetchError> {
        self.store_dir(self.entry(digest), manifest, blobs)
    }

    fn store_dir(
        &self,
        target: PathBuf,
        manifest: &[u8],
        blobs: &[(String, Vec<u8>)],
    ) -> Result<(), PackageFetchError> {
        if target.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.root).map_err(|error| io(&self.root, error))?;
        let unique = format!(
            ".tmp-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        );
        let staging = self.root.join(unique);
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(staging.join("blobs"))?;
            std::fs::write(staging.join("manifest.json"), manifest)?;
            for (blob_digest, bytes) in blobs {
                let hex = blob_digest.trim_start_matches("sha256:");
                std::fs::write(staging.join("blobs").join(hex), bytes)?;
            }
            Ok(())
        };
        let written = write();
        match written.and_then(|()| std::fs::rename(&staging, &target)) {
            Ok(()) => Ok(()),
            // A parallel process stored the same package first.
            Err(_) if target.exists() => {
                let _ = std::fs::remove_dir_all(&staging);
                Ok(())
            }
            Err(error) => {
                let _ = std::fs::remove_dir_all(&staging);
                Err(io(&target, error))
            }
        }
    }
}

fn io(path: &Path, error: std::io::Error) -> PackageFetchError {
    PackageFetchError::Io {
        path: path.display().to_string(),
        reason: error.to_string(),
    }
}
