//! The tool catalog in a registry (ADR-0124): the index stored as an OCI artifact, read with its
//! digest and written conditionally with bounded retries, entries made from the packages in the
//! registry, the index signature, and the reader's policy applied to an entry.

use std::collections::BTreeMap;
use std::time::Duration;

use grauwolf_oci::cosign::{self, PublicKey, Signature, SigningKey};
use grauwolf_oci::{IMAGE_MANIFEST, OciError, OciImage};
use grauwolf_spec::OciReference;
use grauwolf_tools::PackageManifest;
use grauwolf_tools::catalog::{
    CATALOG_ARTIFACT_TYPE, CATALOG_REPOSITORY, CATALOG_TAG, CatalogEntry, CatalogIndex,
    CatalogSignature, CatalogSignatureEntry, CatalogTool,
};
use serde_json::{Value, json};

use crate::layout::Layout;
use crate::metadata::*;
use crate::{ARTIFACT_TYPE, PackageFetchError, Policy, RegistryClient, sha256_digest};

/// Media type of the empty config of the catalog artifact.
const EMPTY_TYPE: &str = "application/vnd.oci.empty.v1+json";

/// How often an update is tried when other writers keep changing the index.
pub const MAX_ATTEMPTS: usize = 5;

/// Shortest and longest wait before the index is read again to check a write on a registry that
/// ignores conditional writes (the checking writer waits a random time in between, so that
/// concurrent writers do not check in step).
pub const DEFAULT_SETTLE: (Duration, Duration) =
    (Duration::from_millis(300), Duration::from_millis(1000));

/// A digest no manifest has: the probe write that tells whether a registry honours `If-Match`.
const NO_SUCH_DIGEST: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// The catalog of `value`: a full reference (`registry/repository[:tag]`) or only a registry
/// host (`registry.example.com`, `localhost:5000`), which means
/// `<registry>/grauwolf-catalog:latest`.
pub fn catalog_reference(value: &str) -> Result<OciReference, String> {
    let value = value.trim();
    let full = if value.contains('/') {
        value.to_owned()
    } else {
        format!("{value}/{CATALOG_REPOSITORY}:{CATALOG_TAG}")
    };
    let reference =
        OciReference::parse(&full).map_err(|reason| format!("catalog {value}: {reason}"))?;
    if reference.digest.is_some() {
        return Err(format!(
            "catalog {value}: name it by tag, not by digest – it changes with every update"
        ));
    }
    Ok(reference)
}

/// `registry/repository:tag` of a catalog reference, for messages.
pub fn catalog_name(reference: &OciReference) -> String {
    format!(
        "{}/{}:{}",
        reference.registry,
        reference.repository,
        reference.reference()
    )
}

/// What a reader can say about the signature of the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexSignature {
    /// Signed by the key with this fingerprint.
    Valid { fingerprint: String },
    /// There are signatures, but none is valid for a known key; the text says why.
    Invalid(String),
    /// There is no signature.
    Unsigned,
    /// There are signatures, but no key to check them with.
    Unchecked { signatures: usize },
}

/// The index as read: content and the digest of its manifest (`None`: there is no index yet).
#[derive(Debug, Clone, PartialEq)]
pub struct ReadIndex {
    pub index: CatalogIndex,
    pub digest: Option<String>,
}

/// The result of an update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Updated {
    /// Digest of the index manifest now (`None`: still no index).
    pub digest: Option<String>,
    /// Did this update write the index? (No when the change was there already.)
    pub written: bool,
    /// How many attempts it took.
    pub attempts: usize,
    /// Did the registry turn out to ignore conditional writes (`If-Match`)? Then writers are
    /// checked only after the fact, see [`CatalogStore::update`].
    pub ignores_conditions: bool,
}

/// The catalog at one reference.
pub struct CatalogStore<'a> {
    client: &'a RegistryClient,
    reference: OciReference,
    settle: (Duration, Duration),
}

impl<'a> CatalogStore<'a> {
    pub fn new(client: &'a RegistryClient, reference: OciReference) -> Self {
        Self {
            client,
            reference,
            settle: DEFAULT_SETTLE,
        }
    }

    /// Sets the wait (random between `shortest` and `longest`) before a write on a registry
    /// that ignores conditions is checked. For tests: `Duration::ZERO` for both.
    pub fn with_settle(mut self, shortest: Duration, longest: Duration) -> Self {
        self.settle = (shortest, longest.max(shortest));
        self
    }

    async fn settle(&self) {
        let (shortest, longest) = self.settle;
        let spread = (longest - shortest).as_millis() as u64;
        let jitter = if spread == 0 {
            0
        } else {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| u64::from(since.subsec_nanos()));
            nanos % (spread + 1)
        };
        let wait = shortest + Duration::from_millis(jitter);
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    /// The reference of the catalog.
    pub fn reference(&self) -> &OciReference {
        &self.reference
    }

    /// Reads the index; a missing one is an empty index without digest.
    pub async fn read(&self) -> Result<ReadIndex, PackageFetchError> {
        let manifest = match self.client.manifest(&self.reference).await {
            Ok(manifest) => manifest,
            Err(OciError::NotFound { .. }) => {
                return Ok(ReadIndex {
                    index: CatalogIndex::new(""),
                    digest: None,
                });
            }
            Err(error) => return Err(error.into()),
        };
        let wrong = |what: String| {
            PackageFetchError::WrongMediaType(format!(
                "{} is no tool catalog: {what}",
                catalog_name(&self.reference)
            ))
        };
        let value: Value = serde_json::from_slice(&manifest.bytes)
            .map_err(|error| wrong(format!("the manifest is not JSON: {error}")))?;
        if value["artifactType"] != CATALOG_ARTIFACT_TYPE {
            return Err(wrong(format!("artifact type {}", value["artifactType"])));
        }
        let layer = value["layers"]
            .as_array()
            .and_then(|layers| {
                layers
                    .iter()
                    .find(|layer| layer["mediaType"] == CATALOG_ARTIFACT_TYPE)
            })
            .and_then(|layer| layer["digest"].as_str())
            .ok_or_else(|| wrong("no index layer".into()))?;
        let bytes = self.client.blob(&self.reference, layer).await?;
        let index = CatalogIndex::from_json(&String::from_utf8_lossy(&bytes))
            .map_err(|error| PackageFetchError::InvalidPackage(error.to_string()))?;
        Ok(ReadIndex {
            index,
            digest: Some(manifest.digest),
        })
    }

    /// Applies `change` to the current index and writes it, conditionally on the digest it was
    /// read at. `change` returns whether it changed anything; when not (the change is there
    /// already), nothing is written. `present` tells whether the index holds *our change* (the
    /// entry with our digest, or the entry gone) – never whether it is the index we wrote, as
    /// others may legitimately add to it. `updated` gives the time stamp of a written index; with
    /// `key`, the index holding our change is signed.
    ///
    /// A registry that honours the condition answers `412` when the index moved; the round is
    /// then repeated (read, change, write), at most [`MAX_ATTEMPTS`] times. Many registries
    /// (the distribution registry among them) ignore `If-Match`, and two writers can then
    /// overwrite each other. The first write is therefore a probe with a condition that cannot
    /// hold: if it succeeds the registry ignores conditions. Such a write is checked after the
    /// fact: after a short random wait (see [`with_settle`](Self::with_settle)) the index is
    /// read again, and when `present` does not hold the round is repeated.
    pub async fn update(
        &self,
        change: &mut dyn FnMut(&mut CatalogIndex) -> Result<bool, String>,
        present: &dyn Fn(&CatalogIndex) -> bool,
        updated: &dyn Fn() -> String,
        key: Option<&SigningKey>,
    ) -> Result<Updated, PackageFetchError> {
        let mut ignores = None;
        for attempt in 1..=MAX_ATTEMPTS {
            let ReadIndex { mut index, digest } = self.read().await?;
            if !change(&mut index).map_err(PackageFetchError::Lock)? {
                return Ok(Updated {
                    digest,
                    written: false,
                    attempts: attempt,
                    ignores_conditions: ignores == Some(true),
                });
            }
            index.updated = updated();
            let image = index_image(&index);
            let mut written = false;
            if ignores.is_none() {
                match self
                    .client
                    .push_if(&self.reference, &image, Some(Some(NO_SUCH_DIGEST)))
                    .await
                {
                    Err(OciError::Conflict { .. }) => ignores = Some(false),
                    Err(error) => return Err(error.into()),
                    // The write went through although it cannot have held: the probe was the
                    // (unconditional) write.
                    Ok(_) => {
                        ignores = Some(true);
                        written = true;
                    }
                }
            }
            if !written {
                match self
                    .client
                    .push_if(&self.reference, &image, Some(digest.as_deref()))
                    .await
                {
                    Err(OciError::Conflict { .. }) => continue,
                    Err(error) => return Err(error.into()),
                    Ok(_) => {}
                }
            }
            let signed_digest = if ignores == Some(true) {
                self.settle().await;
                let now = self.read().await?;
                if !present(&now.index) {
                    continue;
                }
                now.digest.unwrap_or_else(|| image.digest.clone())
            } else {
                let now = self.client.manifest(&self.reference).await?;
                if now.digest != image.digest {
                    continue;
                }
                image.digest.clone()
            };
            if let Some(key) = key {
                self.client
                    .push_signature(&self.reference, &signed_digest, key)
                    .await
                    .map_err(|error| PackageFetchError::Registry {
                        registry: self.reference.registry.clone(),
                        reason: format!("signing the catalog failed: {error}"),
                    })?;
            }
            return Ok(Updated {
                digest: Some(signed_digest),
                written: true,
                attempts: attempt,
                ignores_conditions: ignores == Some(true),
            });
        }
        Err(PackageFetchError::Registry {
            registry: self.reference.registry.clone(),
            reason: format!(
                "the catalog {} kept changing while it was updated; gave up after {MAX_ATTEMPTS} \
                 attempts – try again",
                catalog_name(&self.reference)
            ),
        })
    }

    /// The signature of the index with manifest `digest`, checked against `keys`.
    pub async fn signature(
        &self,
        digest: &str,
        keys: &[PublicKey],
    ) -> Result<IndexSignature, PackageFetchError> {
        let signatures = self.client.signatures(&self.reference, digest).await?;
        let identity = format!("{}/{}", self.reference.registry, self.reference.repository);
        Ok(match cosign::verify(&signatures, keys, &identity, digest) {
            Ok(fingerprint) => IndexSignature::Valid { fingerprint },
            Err(cosign::CosignError::Unsigned) => IndexSignature::Unsigned,
            Err(_) if keys.is_empty() => IndexSignature::Unchecked {
                signatures: signatures.len(),
            },
            Err(error) => IndexSignature::Invalid(error.to_string()),
        })
    }
}

/// The note for a registry that ignores conditional writes, printed once by the commands.
pub fn ignores_conditions_note(registry: &str) -> String {
    format!(
        "note: {registry} ignores conditional writes; concurrent catalog writers are checked \
         after the fact"
    )
}

/// The catalog artifact of `index`: an empty config and the index as its one layer.
fn index_image(index: &CatalogIndex) -> OciImage {
    let config = b"{}".to_vec();
    let layer = index.to_json().into_bytes();
    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": IMAGE_MANIFEST,
        "artifactType": CATALOG_ARTIFACT_TYPE,
        "config": { "mediaType": EMPTY_TYPE, "digest": sha256_digest(&config), "size": config.len() },
        "layers": [{
            "mediaType": CATALOG_ARTIFACT_TYPE,
            "digest": sha256_digest(&layer),
            "size": layer.len(),
            "annotations": { crate::TITLE_ANNOTATION: "index.json" },
        }],
        "annotations": { CREATED_ANNOTATION: index.updated },
    })
    .to_string()
    .into_bytes();
    OciImage {
        digest: sha256_digest(&manifest),
        manifest,
        blobs: BTreeMap::from([
            (sha256_digest(&config), config),
            (sha256_digest(&layer), layer),
        ]),
    }
}

/// Classifies the manifest at `reference` without reading anything else: `None` for a tool
/// package, else `Some(media type)` naming what it is – its artifact type, else the media type
/// of its config (an ordinary image), else its own media type (an image index, a docker manifest
/// list). Image indexes and docker manifests are fetched like any other manifest, so they are
/// told apart here instead of failing the digest check.
pub async fn other_than_tool_package(
    client: &RegistryClient,
    reference: &OciReference,
) -> Result<Option<String>, PackageFetchError> {
    let manifest = client.manifest(reference).await?;
    let value: Value = serde_json::from_slice(&manifest.bytes).unwrap_or(Value::Null);
    if value["artifactType"] == ARTIFACT_TYPE && Layout::parse(&manifest.bytes).is_ok() {
        return Ok(None);
    }
    let media_type = ["/artifactType", "/config/mediaType", "/mediaType"]
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
        .unwrap_or("unknown");
    Ok(Some(media_type.to_owned()))
}

/// The catalog entry of the package at `reference`: its manifest, config and annotations, the
/// module sizes and digests from the layer descriptors (modules are not downloaded), and its
/// signatures, with the fingerprints of those `keys` that verify one.
pub async fn catalog_entry(
    client: &RegistryClient,
    reference: &OciReference,
    keys: &[PublicKey],
) -> Result<CatalogEntry, PackageFetchError> {
    let manifest = client.manifest(reference).await?;
    let layout = Layout::parse(&manifest.bytes)?;
    let raw: Value = serde_json::from_slice(&manifest.bytes)
        .map_err(|error| PackageFetchError::InvalidPackage(error.to_string()))?;
    let config = client.blob(reference, &layout.config).await?;
    let package = PackageManifest::from_json(&String::from_utf8_lossy(&config))?;
    let sizes: BTreeMap<&str, u64> = raw["layers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|layer| Some((layer["digest"].as_str()?, layer["size"].as_u64()?)))
        .collect();
    let mut tools = Vec::new();
    for tool in &package.tools {
        let digest = layout.modules.get(&tool.module).ok_or_else(|| {
            PackageFetchError::InvalidPackage(format!(
                "tool '{}' needs the module '{}', which the package does not contain",
                tool.name, tool.module
            ))
        })?;
        tools.push(CatalogTool {
            name: tool.name.clone(),
            description: tool.description.clone(),
            args: tool.args.clone(),
            module: tool.module.clone(),
            digest: digest.clone(),
            size: sizes.get(digest.as_str()).copied().unwrap_or(0),
            limits: tool.limits,
            capabilities: tool.capabilities.clone(),
        });
    }
    let annotation = |key: &str| layout.annotations.get(key).cloned();
    let identity = format!("{}/{}", reference.registry, reference.repository);
    let signatures = client.signatures(reference, &manifest.digest).await?;
    let fingerprints = keys
        .iter()
        .filter(|key| {
            cosign::verify(
                &signatures,
                std::slice::from_ref(*key),
                &identity,
                &manifest.digest,
            )
            .is_ok()
        })
        .map(PublicKey::fingerprint)
        .collect();
    let authors = match annotation(AUTHORS_ANNOTATION) {
        Some(authors) => authors.split(", ").map(str::to_owned).collect(),
        None => package.authors.clone(),
    };
    Ok(CatalogEntry {
        reference: format!("{identity}@{}", manifest.digest),
        description: annotation(DESCRIPTION_ANNOTATION).or(package.description.clone()),
        created: annotation(CREATED_ANNOTATION),
        source: annotation(SOURCE_ANNOTATION).or(package.source.clone()),
        revision: annotation(REVISION_ANNOTATION),
        license: annotation(LICENSES_ANNOTATION).or(package.license.clone()),
        authors,
        abi: package.abi,
        built_with: annotation(BUILT_WITH_ANNOTATION),
        rustc: annotation(RUSTC_ANNOTATION),
        signature: CatalogSignature {
            signed: !signatures.is_empty(),
            fingerprints,
            signatures: signatures
                .iter()
                .map(|signature| CatalogSignatureEntry {
                    payload: String::from_utf8_lossy(&signature.payload).into_owned(),
                    signature: signature.signature.clone(),
                })
                .collect(),
        },
        name: package.name,
        version: package.version,
        tools,
    })
}

/// Would `policy` (the reader's tool policy; `None`: no policy) allow the package of `entry`?
/// The allowlist, and with `require_signatures` a signature in the entry that verifies with a
/// policy key. Evaluated when the catalog is read, never stored.
pub fn entry_verdict(policy: Option<&Policy>, entry: &CatalogEntry) -> Result<(), String> {
    let Some(policy) = policy else {
        return Ok(());
    };
    let reference = OciReference::parse(&entry.reference).map_err(|reason| reason.to_string())?;
    policy
        .check_oci(&reference)
        .map_err(|error| error.to_string())?;
    if !policy.rules().require_signatures {
        return Ok(());
    }
    if policy.oci_keys().is_empty() {
        return Err("the policy requires signatures but lists no `oci_keys`".into());
    }
    let digest = reference.digest.as_deref().unwrap_or_default();
    let identity = format!("{}/{}", reference.registry, reference.repository);
    let signatures: Vec<Signature> = entry
        .signature
        .signatures
        .iter()
        .map(|signature| Signature {
            payload: signature.payload.clone().into_bytes(),
            signature: signature.signature.clone(),
        })
        .collect();
    cosign::verify(&signatures, policy.oci_keys(), &identity, digest)
        .map(|_| ())
        .map_err(|error| match error {
            cosign::CosignError::Unsigned => "unsigned, and the policy requires signatures".into(),
            other => format!("signature: {other}"),
        })
}
