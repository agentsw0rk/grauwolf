//! Resolves the `tool_packages` of a spec: lockfile, pull, cache, selection and renaming.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use grauwolf_git::{AllowedSigners, SignedObjects};
use grauwolf_oci::cosign::{self, PublicKey};
use grauwolf_spec::{AgentSpec, GitRef, GitSource, OciReference, ToolOrigin, ToolPackageSpec};
use grauwolf_tools::{PackageManifest, ResolvedTool, ToolArtifact};

use crate::layout::Layout;
use crate::{
    LockedGit, LockedPackage, LockedSkills, Lockfile, ModuleBuilder, Package, PackageCache,
    PackageFetchError, Policy, RegistryClient, pack_sources, sha256_digest,
};

/// What `tools inspect` says about the signature of a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureStatus {
    /// Signed by the key with this fingerprint.
    Signed { fingerprint: String },
    /// There is no signature.
    Unsigned,
    /// There are signatures, but none is valid for a given key; the text says why.
    Invalid(String),
    /// There are signatures, but no key to check them with (no policy key and no `--key`).
    Unchecked { signatures: usize },
}

/// What `tools inspect` says about the signature of a git commit or tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitSignatureStatus {
    /// Signed by an allowed signer; `principal` is the e-mail of the committer or tagger.
    Signed {
        principal: Option<String>,
        fingerprint: String,
    },
    /// Neither the commit nor the tag carries a signature.
    Unsigned,
    /// There is a signature, but it is not valid for an allowed signer; the text says why.
    Invalid(String),
    /// There is a signature, but no allowed signers to check it with.
    Unchecked,
}

/// What `tools inspect` shows of a git package.
#[derive(Debug, Clone)]
pub struct GitInspection {
    /// The commit the reference resolves to (40 hex digits).
    pub commit: String,
    /// The manifest read from the sources at that commit.
    pub manifest: PackageManifest,
    /// The package as it is in the cache (with its module hashes), if it was pulled before.
    pub cached: Option<Package>,
    /// The signature of the commit or tag.
    pub signature: GitSignatureStatus,
}

/// Which files of a git folder are fetched.
#[derive(Clone, Copy)]
enum Include {
    /// Only the commit and its objects (for signature checks).
    Nothing,
    /// The manifest and the sources of a tool package.
    Package,
    /// Everything below the folder (skills).
    All,
}

/// A git skills source resolved to a local directory (ADR-0103).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSkills {
    /// Index of the source in `spec.skills`.
    pub index: usize,
    /// Lock key of the source (`GitSource::lock_key`).
    pub key: String,
    /// The commit the source is at.
    pub commit: String,
    /// Directory in the cache with one sub-directory per skill (the `path` of the source).
    pub dir: std::path::PathBuf,
}

/// What resolving git sources needs: the token for the repositories and the module build.
struct GitSupport {
    token: Option<String>,
    builder: Arc<dyn ModuleBuilder>,
}

/// Pulls packages into the tool cache and resolves specs against the lockfile.
pub struct PackageResolver {
    cache: PackageCache,
    client: RegistryClient,
    locked: bool,
    git: Option<GitSupport>,
    policy: Option<Policy>,
}

impl PackageResolver {
    /// Resolver with the cache below `tool_cache` (e.g. `.grauwolf/tools`) and `client`.
    pub fn new(tool_cache: impl AsRef<Path>, client: RegistryClient) -> Self {
        Self {
            cache: PackageCache::new(tool_cache),
            client,
            locked: false,
            git: None,
            policy: None,
        }
    }

    /// Enables git sources: `token` (`GIT_TOKEN`, sent only to the host of the repository
    /// URL) and the `builder` that compiles the modules from the sources of a package folder.
    pub fn with_git(mut self, token: Option<String>, builder: Arc<dyn ModuleBuilder>) -> Self {
        self.git = Some(GitSupport { token, builder });
        self
    }

    /// Applies the tool policy to every package that is pulled or resolved, cache and lockfile
    /// hits included.
    pub fn with_policy(mut self, policy: Policy) -> Self {
        self.policy = Some(policy);
        self
    }

    /// With `locked`, resolving fails instead of changing the lockfile (CI, images).
    pub fn with_locked(mut self, locked: bool) -> Self {
        self.locked = locked;
        self
    }

    /// The package of `reference`, from the cache if its digest is known (a digest reference,
    /// or a lock) and otherwise from the registry; verified either way. The tool policy decides
    /// first: the allowlist, and with `require_signatures` a cosign signature of a policy key
    /// (checked against the signatures stored with the cache entry, fetched if needed).
    pub async fn pull(&self, reference: &OciReference) -> Result<Package, PackageFetchError> {
        if let Some(policy) = &self.policy {
            policy.check_oci(reference)?;
        }
        let (package, fresh) = self.fetch_package(reference).await?;
        self.require_signature(reference, &package.digest, fresh)
            .await?;
        Ok(package)
    }

    /// The package of `reference` for `tools inspect`: only the allowlist of the policy applies,
    /// a missing signature is reported in the status instead of refusing the package. The status
    /// is checked against the keys of the policy and `extra_keys`.
    pub async fn inspect(
        &self,
        reference: &OciReference,
        extra_keys: &[PublicKey],
    ) -> Result<(Package, SignatureStatus), PackageFetchError> {
        if let Some(policy) = &self.policy {
            policy.check_oci(reference)?;
        }
        let (package, _) = self.fetch_package(reference).await?;
        let mut keys: Vec<PublicKey> = extra_keys.to_vec();
        if let Some(policy) = &self.policy {
            keys.extend(policy.oci_keys().iter().cloned());
        }
        let identity = format!("{}/{}", reference.registry, reference.repository);
        let signatures = match self.client.signatures(reference, &package.digest).await {
            Ok(signatures) => signatures,
            Err(error) => {
                // Offline: what was stored with the cache entry still tells something.
                let stored = self.cache.load_signatures(&package.digest)?;
                if stored.is_empty() {
                    return Err(error.into());
                }
                stored
            }
        };
        let status = match cosign::verify(&signatures, &keys, &identity, &package.digest) {
            Ok(fingerprint) => SignatureStatus::Signed { fingerprint },
            Err(cosign::CosignError::Unsigned) => SignatureStatus::Unsigned,
            Err(_) if keys.is_empty() => SignatureStatus::Unchecked {
                signatures: signatures.len(),
            },
            Err(error) => SignatureStatus::Invalid(error.to_string()),
        };
        Ok((package, status))
    }

    /// The package, and whether it was fetched just now (not found in the cache).
    async fn fetch_package(
        &self,
        reference: &OciReference,
    ) -> Result<(Package, bool), PackageFetchError> {
        if let Some(digest) = &reference.digest {
            if let Some(package) = self.cache.load(digest)? {
                return Ok((package, false));
            }
        }
        let manifest = self.client.manifest(reference).await?;
        if let Some(package) = self.cache.load(&manifest.digest)? {
            return Ok((package, false));
        }
        let layout = Layout::parse(&manifest.bytes)?;
        let mut blobs = Vec::new();
        for digest in layout.blob_digests() {
            blobs.push((
                digest.to_owned(),
                self.client.blob(reference, digest).await?,
            ));
        }
        // Checks the package before it enters the cache.
        let package = layout.assemble(&manifest.digest, |digest| {
            blobs
                .iter()
                .find(|(known, _)| known == digest)
                .map(|(_, bytes)| bytes.clone())
                .ok_or_else(|| PackageFetchError::InvalidPackage(format!("blob {digest} missing")))
        })?;
        self.cache
            .store(&manifest.digest, &manifest.bytes, &blobs)?;
        Ok((package, true))
    }

    /// With `require_signatures`, the package of manifest `digest` must carry a signature of a
    /// policy key. The signatures stored with the cache entry are tried first, so that the check
    /// works offline; otherwise they are fetched from the registry (not for an entry that was
    /// cached earlier when the lockfile is enforced) and stored.
    async fn require_signature(
        &self,
        reference: &OciReference,
        digest: &str,
        fresh: bool,
    ) -> Result<(), PackageFetchError> {
        let Some(policy) = self
            .policy
            .as_ref()
            .filter(|p| p.rules().require_signatures)
        else {
            return Ok(());
        };
        let name = format!("{}/{}@{digest}", reference.registry, reference.repository);
        let refuse = |why: &str| {
            PackageFetchError::Policy(format!(
                "`{name}` does not satisfy the tool policy (`require_signatures`): {why}"
            ))
        };
        if policy.oci_keys().is_empty() {
            return Err(refuse(
                "the policy lists no `oci_keys` to verify OCI packages with",
            ));
        }
        let identity = format!("{}/{}", reference.registry, reference.repository);
        let stored = self.cache.load_signatures(digest)?;
        let from_stored = match cosign::verify(&stored, policy.oci_keys(), &identity, digest) {
            Ok(_) => return Ok(()),
            Err(error) => error,
        };
        if self.locked && !fresh {
            return Err(refuse(&match from_stored {
                cosign::CosignError::Unsigned => "no signature is cached for this package and \
                     `--locked` forbids fetching it; run `grauwolf tools pull` for the \
                     reference or resolve without `--locked`"
                    .to_owned(),
                other => other.to_string(),
            }));
        }
        let fetched = match self.client.signatures(reference, digest).await {
            Ok(fetched) => fetched,
            // The cached signatures are the better explanation than a network error.
            Err(_) if !stored.is_empty() => return Err(refuse(&from_stored.to_string())),
            Err(error) => return Err(error.into()),
        };
        cosign::verify(&fetched, policy.oci_keys(), &identity, digest)
            .map_err(|error| refuse(&error.to_string()))?;
        self.cache.store_signatures(digest, &fetched)?;
        Ok(())
    }

    /// Pulls a git package into the cache for `tools pull`: the same policy checks, fetch,
    /// signature check and build as when a spec is resolved, without a lockfile. Returns the
    /// package and the commit it is at.
    pub async fn pull_git(
        &self,
        source: &GitSource,
    ) -> Result<(Package, String), PackageFetchError> {
        self.git_package(&source.lock_key(), source, None).await
    }

    /// Looks at a git package for `tools inspect` without building it: the commit, the manifest
    /// from the sources, the cached package if there is one, and the signature checked against the
    /// signers of the policy or else `extra_signers`. Only the allowlist of the policy applies; a
    /// missing signature is reported instead of refusing the package.
    pub async fn inspect_git(
        &self,
        source: &GitSource,
        extra_signers: Option<&AllowedSigners>,
    ) -> Result<GitInspection, PackageFetchError> {
        let key = source.lock_key();
        let git = self.git.as_ref().ok_or_else(|| {
            PackageFetchError::InvalidPackage(format!(
                "`{key}`: git sources are not available in this build"
            ))
        })?;
        if let Some(policy) = &self.policy {
            policy.check_git(&source.url)?;
        }
        let signers = extra_signers.or_else(|| self.policy.as_ref().and_then(Policy::git_signers));
        let path = source.path.as_deref();
        let tag = match &source.reference {
            GitRef::Tag(tag) => Some(tag.as_str()),
            _ => None,
        };
        let (commit, manifest, objects) = match self.inspect_from_cache(source)? {
            Some(found) => found,
            None => {
                let wanted = match &source.reference {
                    GitRef::Rev(rev) => grauwolf_git::Wanted::Commit(rev.clone()),
                    GitRef::Tag(tag) => grauwolf_git::Wanted::Tag(tag.clone()),
                    GitRef::Branch(branch) => grauwolf_git::Wanted::Branch(branch.clone()),
                };
                let fetched = self
                    .fetch_git(
                        git,
                        &key,
                        source,
                        wanted,
                        Include::Package,
                        source.path.clone(),
                    )
                    .await?;
                let json = fetched
                    .files
                    .get(grauwolf_tools::MANIFEST_FILE)
                    .ok_or_else(|| {
                        PackageFetchError::InvalidPackage(format!(
                            "`{key}`: {} not found in the repository at commit {}",
                            grauwolf_tools::MANIFEST_FILE,
                            fetched.commit
                        ))
                    })?;
                let manifest = PackageManifest::from_source_json(&String::from_utf8_lossy(json))
                    .map_err(|error| {
                        PackageFetchError::InvalidPackage(format!(
                            "`{key}`: {}: {error}",
                            grauwolf_tools::MANIFEST_FILE
                        ))
                    })?;
                (fetched.commit, manifest, fetched.objects)
            }
        };
        let signature = match signers {
            Some(signers) => match grauwolf_git::verify_objects(&objects, &commit, tag, signers) {
                Ok(verified) => GitSignatureStatus::Signed {
                    principal: verified.identity,
                    fingerprint: verified.fingerprint,
                },
                Err(grauwolf_git::SignatureError::Unsigned) => GitSignatureStatus::Unsigned,
                Err(error) => GitSignatureStatus::Invalid(error.to_string()),
            },
            None => {
                let signed = grauwolf_git::commit_signature(&objects.commit).is_some()
                    || objects
                        .tag
                        .as_deref()
                        .is_some_and(|raw| grauwolf_git::tag_signature(raw).is_some());
                if signed {
                    GitSignatureStatus::Unchecked
                } else {
                    GitSignatureStatus::Unsigned
                }
            }
        };
        let cached = self.cache.load_git(&commit, path)?;
        Ok(GitInspection {
            commit,
            manifest,
            cached,
            signature,
        })
    }

    /// A full `rev` that is cached with its signed objects tells everything without network.
    #[allow(clippy::type_complexity)]
    fn inspect_from_cache(
        &self,
        source: &GitSource,
    ) -> Result<Option<(String, PackageManifest, SignedObjects)>, PackageFetchError> {
        let GitRef::Rev(rev) = &source.reference else {
            return Ok(None);
        };
        if rev.len() != 40 {
            return Ok(None);
        }
        let path = source.path.as_deref();
        let (Some(package), Some(objects)) = (
            self.cache.load_git(rev, path)?,
            self.cache.load_git_objects(rev, path)?,
        ) else {
            return Ok(None);
        };
        Ok(Some((rev.clone(), package.manifest, objects)))
    }

    /// Resolves the packages of `spec` (OCI and git) against the lockfile `lock_path` and
    /// returns their tools, selected and renamed. Missing lock entries are written; with
    /// `locked` they are an error, as is every other change of the lockfile.
    pub async fn resolve(
        &self,
        spec: &AgentSpec,
        lock_path: &Path,
    ) -> Result<Vec<ResolvedTool>, PackageFetchError> {
        let old = Lockfile::load(lock_path)?;
        let (tools, new) = self.resolve_in(spec, &old, lock_path).await?;
        if new != old {
            if self.locked {
                return Err(lock_change(lock_path, "it would change".into()));
            }
            new.save(lock_path)?;
        }
        Ok(tools)
    }

    /// Like [`resolve`](Self::resolve) against the lockfile content `old` instead of the file:
    /// returns the tools and the lockfile as it should be (the `skills` section stays); the
    /// caller decides whether to write it. With `locked`, a missing or stale entry is an error
    /// naming `lock_path`. For trees, where several specs share one lockfile (ADR-0115).
    pub async fn resolve_in(
        &self,
        spec: &AgentSpec,
        old: &Lockfile,
        lock_path: &Path,
    ) -> Result<(Vec<ResolvedTool>, Lockfile), PackageFetchError> {
        let mut references: BTreeSet<&str> = BTreeSet::new();
        let mut sources: BTreeMap<String, GitSource> = BTreeMap::new();
        for entry in &spec.tool_packages {
            if let Some(reference) = entry.oci.as_deref() {
                references.insert(reference);
            } else if entry.git.is_some() {
                let source = GitSource::from_entry(entry).map_err(|problem| {
                    PackageFetchError::InvalidPackage(format!(
                        "tool_packages: `{}`: {}",
                        problem.field, problem.reason
                    ))
                })?;
                sources.insert(source.lock_key(), source);
            }
        }
        if self.locked {
            let missing = references
                .iter()
                .find(|reference| !old.packages.contains_key(**reference))
                .map(|reference| (*reference).to_owned())
                .or_else(|| {
                    sources
                        .keys()
                        .find(|key| !old.git.contains_key(*key))
                        .cloned()
                });
            if let Some(missing) = missing {
                return Err(lock_change(lock_path, format!("`{missing}` is not locked")));
            }
            let stale = old
                .packages
                .keys()
                .find(|reference| !references.contains(reference.as_str()))
                .or_else(|| old.git.keys().find(|key| !sources.contains_key(*key)));
            if let Some(stale) = stale {
                return Err(lock_change(
                    lock_path,
                    format!("`{stale}` is locked but no longer in the spec"),
                ));
            }
        }
        // The git skills are resolved on their own and kept as they are (ADR-0103).
        let mut new = Lockfile {
            skills: old.skills.clone(),
            ..Lockfile::default()
        };
        let mut packages: BTreeMap<String, Package> = BTreeMap::new();
        for reference_text in &references {
            let reference = OciReference::parse(reference_text).map_err(|reason| {
                PackageFetchError::InvalidPackage(format!("`{reference_text}`: {reason}"))
            })?;
            let package = match old.packages.get(*reference_text) {
                Some(locked) => {
                    if reference
                        .digest
                        .as_ref()
                        .is_some_and(|d| *d != locked.digest)
                    {
                        return Err(PackageFetchError::Lock(format!(
                            "`{reference_text}` names another digest than {}: remove the entry from \
                             the lockfile to lock it again",
                            lock_path.display()
                        )));
                    }
                    let pinned = OciReference {
                        digest: Some(locked.digest.clone()),
                        ..reference
                    };
                    let package = self.pull(&pinned).await?;
                    check_modules(reference_text, &locked.modules, &package)?;
                    new.packages
                        .insert((*reference_text).to_owned(), locked.clone());
                    package
                }
                None => {
                    let package = self.pull(&reference).await?;
                    new.packages.insert(
                        (*reference_text).to_owned(),
                        LockedPackage {
                            digest: package.digest.clone(),
                            modules: module_hashes(&package),
                        },
                    );
                    package
                }
            };
            packages.insert((*reference_text).to_owned(), package);
        }
        for (key, source) in &sources {
            let locked = old.git.get(key);
            let (package, commit) = self.git_package(key, source, locked).await?;
            if let Some(locked) = locked {
                check_modules(key, &locked.modules, &package)?;
            }
            new.git.insert(
                key.clone(),
                LockedGit {
                    commit,
                    modules: module_hashes(&package),
                },
            );
            packages.insert(key.clone(), package);
        }
        Ok((tools_of(spec, &packages)?, new))
    }

    /// The package of a git entry and the commit it is at. A locked entry is fetched by its
    /// commit, never by tag or branch; a cached (commit, path) needs neither network nor
    /// toolchain.
    async fn git_package(
        &self,
        key: &str,
        source: &GitSource,
        locked: Option<&LockedGit>,
    ) -> Result<(Package, String), PackageFetchError> {
        let git = self.git.as_ref().ok_or_else(|| {
            PackageFetchError::InvalidPackage(format!(
                "`{key}`: git sources are not available in this build"
            ))
        })?;
        if let Some(policy) = &self.policy {
            policy.check_git(&source.url)?;
        }
        let fail = |reason: String| PackageFetchError::Git {
            entry: key.to_owned(),
            reason,
        };
        let path = source.path.as_deref();
        let known = match (locked, &source.reference) {
            (Some(locked), reference) => {
                let valid = locked.commit.len() == 40
                    && locked
                        .commit
                        .chars()
                        .all(|c| matches!(c, '0'..='9' | 'a'..='f'));
                if !valid {
                    return Err(PackageFetchError::Lock(format!(
                        "the commit of `{key}` in the lockfile is not a full commit id"
                    )));
                }
                if let GitRef::Rev(rev) = reference {
                    if !locked.commit.starts_with(rev.as_str()) {
                        return Err(PackageFetchError::Lock(format!(
                            "the commit of `{key}` in the lockfile does not match its `rev`"
                        )));
                    }
                }
                Some(locked.commit.clone())
            }
            (None, GitRef::Rev(rev)) if rev.len() == 40 => Some(rev.clone()),
            (None, _) => None,
        };
        let signers = self.required_git_signers(key)?;
        if let Some(commit) = &known {
            if let Some(package) = self.cache.load_git(commit, path)? {
                if let Some(signers) = signers {
                    self.verify_git(key, source, commit, signers, None, &self.cache)
                        .await?;
                }
                return Ok((package, commit.clone()));
            }
        }
        let wanted = match (&known, &source.reference) {
            (Some(commit), _) => grauwolf_git::Wanted::Commit(commit.clone()),
            (None, GitRef::Rev(rev)) => grauwolf_git::Wanted::Commit(rev.clone()),
            (None, GitRef::Tag(tag)) => grauwolf_git::Wanted::Tag(tag.clone()),
            (None, GitRef::Branch(branch)) => grauwolf_git::Wanted::Branch(branch.clone()),
        };
        let folder = source.path.clone();
        let fetched = self
            .fetch_git(git, key, source, wanted, Include::Package, folder)
            .await?;
        let commit = fetched.commit.clone();
        if known.as_ref().is_some_and(|known| *known != commit) {
            return Err(fail(format!("fetched commit {commit}, not the locked one")));
        }
        if known.is_none() {
            if let Some(package) = self.cache.load_git(&commit, path)? {
                if let Some(signers) = signers {
                    self.verify_git(
                        key,
                        source,
                        &commit,
                        signers,
                        Some(&fetched.objects),
                        &self.cache,
                    )
                    .await?;
                    self.cache
                        .store_git_objects(&commit, path, &fetched.objects)?;
                }
                return Ok((package, commit));
            }
        }
        // The signature is checked before anything of the repository is built.
        if let Some(signers) = signers {
            self.verify_git(
                key,
                source,
                &commit,
                signers,
                Some(&fetched.objects),
                &self.cache,
            )
            .await?;
        }
        let base = format!(
            "git+{}{}",
            source.url,
            path.map(|path| format!("//{path}")).unwrap_or_default()
        );
        let builder = git.builder.clone();
        let files = fetched.files;
        let objects = fetched.objects;
        let at = commit.clone();
        let packed = tokio::task::spawn_blocking(move || {
            let label = |relative: &str| format!("{base}/{relative}");
            let manifest = files
                .get(grauwolf_tools::MANIFEST_FILE)
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "{}: not found in the repository at commit {at}",
                        label(grauwolf_tools::MANIFEST_FILE)
                    )
                })?;
            let mut read = |relative: &str| {
                files.get(relative).cloned().ok_or_else(|| {
                    format!(
                        "{}: not found in the repository at commit {at}",
                        label(relative)
                    )
                })
            };
            pack_sources(manifest, &mut read, &label, builder.as_ref())
        })
        .await
        .map_err(|error| fail(error.to_string()))?
        .map_err(fail)?;
        let blobs: Vec<(String, Vec<u8>)> = packed
            .blobs
            .iter()
            .map(|(digest, bytes)| (digest.clone(), bytes.clone()))
            .collect();
        self.cache
            .store_git(&commit, path, &packed.manifest, &blobs)?;
        self.cache.store_git_objects(&commit, path, &objects)?;
        Ok((packed.package()?, commit))
    }

    /// Resolves the git sources of `spec.skills` (ADR-0103) against `grauwolf.lock` (section
    /// `skills`, the other sections stay as they are), fetches missing ones into the cache and
    /// returns their directories; local sources are not part of the result. Like the tool
    /// packages: a locked source is fetched by its commit, `--locked` never changes the lockfile
    /// and works offline from the cache, the tool policy applies, signatures are checked first.
    pub async fn resolve_skills(
        &self,
        spec: &AgentSpec,
        lock_path: &Path,
    ) -> Result<Vec<ResolvedSkills>, PackageFetchError> {
        let old = Lockfile::load(lock_path)?;
        let (resolved, new) = self.resolve_skills_in(spec, &old, lock_path).await?;
        if new != old {
            if self.locked {
                return Err(lock_change(lock_path, "it would change".into()));
            }
            new.save(lock_path)?;
        }
        Ok(resolved)
    }

    /// Like [`resolve_skills`](Self::resolve_skills) against the lockfile content `old`; returns
    /// the lockfile as it should be (the other sections stay). See
    /// [`resolve_in`](Self::resolve_in).
    pub async fn resolve_skills_in(
        &self,
        spec: &AgentSpec,
        old: &Lockfile,
        lock_path: &Path,
    ) -> Result<(Vec<ResolvedSkills>, Lockfile), PackageFetchError> {
        let mut sources: Vec<(usize, String, GitSource)> = Vec::new();
        for (index, entry) in spec.skills.iter().enumerate() {
            if entry.git.is_none() {
                continue;
            }
            let source = GitSource::from_skill_source(entry).map_err(|problem| {
                PackageFetchError::InvalidPackage(format!(
                    "skills: `{}`: {}",
                    problem.field, problem.reason
                ))
            })?;
            sources.push((index, source.lock_key(), source));
        }
        if self.locked {
            if let Some((_, key, _)) = sources
                .iter()
                .find(|(_, key, _)| !old.skills.contains_key(key))
            {
                return Err(lock_change(lock_path, format!("`{key}` is not locked")));
            }
            if let Some(stale) = old
                .skills
                .keys()
                .find(|key| !sources.iter().any(|(_, source_key, _)| source_key == *key))
            {
                return Err(lock_change(
                    lock_path,
                    format!("`{stale}` is locked but no longer in the spec"),
                ));
            }
        }
        let mut new = old.clone();
        new.skills.clear();
        let mut resolved = Vec::new();
        for (index, key, source) in sources {
            let locked = old.skills.get(&key);
            let (dir, commit, tree) = self.git_skills(&key, &source, locked).await?;
            new.skills.insert(
                key.clone(),
                LockedSkills {
                    commit: commit.clone(),
                    tree,
                },
            );
            resolved.push(ResolvedSkills {
                index,
                key,
                commit,
                dir,
            });
        }
        Ok((resolved, new))
    }

    /// The folder of a git skills source, the commit it is at and the hash of its file list.
    async fn git_skills(
        &self,
        key: &str,
        source: &GitSource,
        locked: Option<&LockedSkills>,
    ) -> Result<(std::path::PathBuf, String, String), PackageFetchError> {
        let git = self.git.as_ref().ok_or_else(|| {
            PackageFetchError::InvalidPackage(format!(
                "`{key}`: git sources are not available in this build"
            ))
        })?;
        if let Some(policy) = &self.policy {
            policy.check_git(&source.url)?;
        }
        let path = source.path.as_deref();
        let known = match (locked, &source.reference) {
            (Some(locked), reference) => {
                let valid = locked.commit.len() == 40
                    && locked
                        .commit
                        .chars()
                        .all(|c| matches!(c, '0'..='9' | 'a'..='f'));
                if !valid {
                    return Err(PackageFetchError::Lock(format!(
                        "the commit of `{key}` in the lockfile is not a full commit id"
                    )));
                }
                if let GitRef::Rev(rev) = reference {
                    if !locked.commit.starts_with(rev.as_str()) {
                        return Err(PackageFetchError::Lock(format!(
                            "the commit of `{key}` in the lockfile does not match its `rev`"
                        )));
                    }
                }
                Some(locked.commit.clone())
            }
            (None, GitRef::Rev(rev)) if rev.len() == 40 => Some(rev.clone()),
            (None, _) => None,
        };
        let signers = self.required_git_signers(key)?;
        let cache = self.cache.for_skills();
        let checked = |commit: &str, (dir, tree): (std::path::PathBuf, String)| {
            if locked.is_some_and(|locked| locked.tree != tree) {
                return Err(PackageFetchError::Lock(format!(
                    "the files of `{key}` do not match the lockfile"
                )));
            }
            Ok((dir, commit.to_owned(), tree))
        };
        if let Some(commit) = &known {
            if let Some(cached) = cache.load_skills(commit, path)? {
                if let Some(signers) = signers {
                    self.verify_git(key, source, commit, signers, None, &cache)
                        .await?;
                }
                return checked(commit, cached);
            }
        }
        let wanted = match (&known, &source.reference) {
            (Some(commit), _) => grauwolf_git::Wanted::Commit(commit.clone()),
            (None, GitRef::Rev(rev)) => grauwolf_git::Wanted::Commit(rev.clone()),
            (None, GitRef::Tag(tag)) => grauwolf_git::Wanted::Tag(tag.clone()),
            (None, GitRef::Branch(branch)) => grauwolf_git::Wanted::Branch(branch.clone()),
        };
        let fetched = self
            .fetch_git(git, key, source, wanted, Include::All, source.path.clone())
            .await?;
        let commit = fetched.commit.clone();
        if known.as_ref().is_some_and(|known| *known != commit) {
            return Err(PackageFetchError::Git {
                entry: key.to_owned(),
                reason: format!("fetched commit {commit}, not the locked one"),
            });
        }
        // The signature is checked before anything of the repository is stored.
        if let Some(signers) = signers {
            self.verify_git(
                key,
                source,
                &commit,
                signers,
                Some(&fetched.objects),
                &cache,
            )
            .await?;
        }
        let stored = match cache.load_skills(&commit, path)? {
            Some(cached) => cached,
            None => cache.store_skills(&commit, path, &fetched.files)?,
        };
        cache.store_git_objects(&commit, path, &fetched.objects)?;
        checked(&commit, stored)
    }

    /// Fetches `wanted` from the repository of `source`; `include` says which files below
    /// `folder` come along, besides the commit and its raw objects.
    async fn fetch_git(
        &self,
        git: &GitSupport,
        key: &str,
        source: &GitSource,
        wanted: grauwolf_git::Wanted,
        include: Include,
        folder: Option<String>,
    ) -> Result<grauwolf_git::Fetched, PackageFetchError> {
        let fail = |reason: String| PackageFetchError::Git {
            entry: key.to_owned(),
            reason,
        };
        let mut request = grauwolf_git::FetchRequest::new(source.url.clone(), wanted);
        request.token = git.token.clone();
        tokio::task::spawn_blocking(move || {
            grauwolf_git::fetch_files(&request, folder.as_deref(), &|relative| match include {
                Include::Nothing => false,
                Include::Package => {
                    relative == grauwolf_tools::MANIFEST_FILE || relative.starts_with("src/")
                }
                Include::All => true,
            })
        })
        .await
        .map_err(|error| fail(error.to_string()))?
        .map_err(|error| fail(error.to_string()))
    }

    /// The signers a git entry must be signed by: none without a policy that requires
    /// signatures, and an error when it requires them but names no `git_allowed_signers`.
    fn required_git_signers(
        &self,
        key: &str,
    ) -> Result<Option<&AllowedSigners>, PackageFetchError> {
        let Some(policy) = self
            .policy
            .as_ref()
            .filter(|policy| policy.rules().require_signatures)
        else {
            return Ok(None);
        };
        policy.git_signers().map(Some).ok_or_else(|| {
            PackageFetchError::Policy(format!(
                "`{key}` does not satisfy the tool policy (`require_signatures`): the policy \
                 lists no `git_allowed_signers` to verify git sources with"
            ))
        })
    }

    /// With `require_signatures`, the commit of a git entry (`rev`/`branch`) or its tag must
    /// carry a valid signature of an allowed signer: a `tag` entry a signed annotated tag or
    /// else a signed commit, the others a signed commit. The objects just fetched (`fresh`) are
    /// tried first, then those stored with the cache entry (so a changed policy applies
    /// offline); if neither passes, the objects are fetched again, which `--locked` forbids.
    async fn verify_git(
        &self,
        key: &str,
        source: &GitSource,
        commit: &str,
        signers: &AllowedSigners,
        fresh: Option<&SignedObjects>,
        cache: &PackageCache,
    ) -> Result<(), PackageFetchError> {
        let path = source.path.as_deref();
        let tag = match &source.reference {
            GitRef::Tag(tag) => Some(tag.as_str()),
            _ => None,
        };
        let check = |objects: &SignedObjects| {
            grauwolf_git::verify_objects(objects, commit, tag, signers)
                .map(|_| ())
                .map_err(|error| error.to_string())
        };
        let refuse = |why: &str| {
            PackageFetchError::Policy(format!(
                "`{key}` does not satisfy the tool policy (`require_signatures`): commit {commit}: \
                 {why}"
            ))
        };
        let mut why = None;
        if let Some(objects) = fresh {
            match check(objects) {
                Ok(()) => return Ok(()),
                Err(error) => why = Some(error),
            }
        }
        if let Some(objects) = cache.load_git_objects(commit, path)? {
            match check(&objects) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    why.get_or_insert(error);
                }
            }
        }
        if fresh.is_some() {
            return Err(refuse(&why.unwrap_or_default()));
        }
        if self.locked {
            return Err(refuse(&format!(
                "{} and `--locked` forbids fetching the signed objects; resolve without \
                 `--locked` once",
                why.unwrap_or_else(|| "no signed objects are cached".to_owned())
            )));
        }
        let git = self.git.as_ref().expect("git sources are enabled");
        let wanted = match tag {
            Some(tag) => grauwolf_git::Wanted::Tag(tag.to_owned()),
            None => grauwolf_git::Wanted::Commit(commit.to_owned()),
        };
        let fetched = self
            .fetch_git(git, key, source, wanted, Include::Nothing, None)
            .await?;
        if fetched.commit != commit {
            return Err(PackageFetchError::Git {
                entry: key.to_owned(),
                reason: format!("fetched commit {}, not {commit}", fetched.commit),
            });
        }
        check(&fetched.objects).map_err(|error| refuse(&error))?;
        cache.store_git_objects(commit, path, &fetched.objects)
    }
}

/// The error of `--locked` when the lockfile at `path` would change, because of `why`.
pub fn lock_change(path: &Path, why: String) -> PackageFetchError {
    PackageFetchError::Lock(format!(
        "{} is out of date: {why}; run `grauwolf tools lock <spec>` and commit the lockfile",
        path.display()
    ))
}

/// The module hashes of the lock must match the package: the registry must not change what a
/// digest contains.
fn check_modules(
    reference: &str,
    locked: &BTreeMap<String, String>,
    package: &Package,
) -> Result<(), PackageFetchError> {
    let actual: BTreeMap<&String, &String> = package
        .modules
        .iter()
        .map(|(file, module)| (file, &module.digest))
        .collect();
    let expected: BTreeMap<&String, &String> = locked.iter().collect();
    if actual != expected {
        return Err(PackageFetchError::Lock(format!(
            "the modules of `{reference}` do not match the lockfile"
        )));
    }
    Ok(())
}

fn module_hashes(package: &Package) -> BTreeMap<String, String> {
    package
        .modules
        .iter()
        .map(|(file, module)| (file.clone(), module.digest.clone()))
        .collect()
}

/// The key of an entry in the lockfile and in `packages`: the OCI reference as written, or the
/// git key (`GitSource::lock_key`).
fn entry_key(entry: &ToolPackageSpec) -> Option<String> {
    match (&entry.oci, &entry.git) {
        (Some(reference), _) => Some(reference.clone()),
        (None, Some(_)) => GitSource::from_entry(entry)
            .ok()
            .map(|source| source.lock_key()),
        _ => None,
    }
}

/// Selects and renames the tools of every package entry and checks the names.
fn tools_of(
    spec: &AgentSpec,
    packages: &BTreeMap<String, Package>,
) -> Result<Vec<ResolvedTool>, PackageFetchError> {
    // Everything but the package tools themselves is taken; package tools are added as they are
    // resolved, so a clash is reported at the later one (the namespace of ADR-0117).
    let namespace = spec.tool_namespace();
    let mut taken: BTreeSet<String> = namespace
        .names_except_origin(ToolOrigin::Package)
        .map(str::to_owned)
        .collect();
    let mut resolved = Vec::new();
    for entry in &spec.tool_packages {
        let Some(reference) = entry_key(entry) else {
            continue;
        };
        let reference = reference.as_str();
        let package = &packages[reference];
        let has = |name: &str| package.manifest.tools.iter().any(|tool| tool.name == name);
        for name in entry.tools.iter().flatten().chain(entry.renames.keys()) {
            if !has(name) {
                return Err(PackageFetchError::InvalidPackage(format!(
                    "`{reference}` has no tool '{name}'"
                )));
            }
        }
        for tool in &package.manifest.tools {
            if entry
                .tools
                .as_ref()
                .is_some_and(|names| !names.contains(&tool.name))
            {
                continue;
            }
            let name = entry.local_name(&tool.name).to_owned();
            if !taken.insert(name.clone()) {
                return Err(PackageFetchError::InvalidPackage(format!(
                    "tool '{name}' of `{reference}` clashes with another tool of the spec; \
                     rename it with `as`"
                )));
            }
            let module = &package.modules[&tool.module];
            resolved.push(ResolvedTool {
                name: name.clone(),
                description: tool.description.clone(),
                args: tool.args.clone(),
                artifact: ToolArtifact {
                    name,
                    hash: sha256_digest(&module.bytes)
                        .trim_start_matches("sha256:")
                        .to_owned(),
                    bytes: module.bytes.clone(),
                },
                limits: tool.limits,
            });
        }
    }
    Ok(resolved)
}
