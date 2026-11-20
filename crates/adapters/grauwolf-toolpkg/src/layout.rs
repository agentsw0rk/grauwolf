//! The layout of a package as an OCI artifact: media types, manifest parsing, assembling the
//! verified [`Package`] from the manifest and its blobs.

use std::collections::BTreeMap;

use grauwolf_oci::IMAGE_MANIFEST;
use grauwolf_tools::PackageManifest;
use serde::Deserialize;

use crate::{PackageFetchError, sha256_digest};

/// Artifact type of the manifest.
pub const ARTIFACT_TYPE: &str = "application/vnd.grauwolf.tools.v1";
/// Media type of the config blob: the `grauwolf-tools.json` of the package.
pub const CONFIG_TYPE: &str = "application/vnd.grauwolf.tools.config.v1+json";
/// Media type of a module layer (annotation `org.opencontainers.image.title` = file name).
pub const MODULE_TYPE: &str = "application/vnd.grauwolf.tools.module.v1+wasm";
/// Media type of the optional source layer.
pub const SOURCE_TYPE: &str = "application/vnd.grauwolf.tools.source.v1.tar+gzip";
/// Annotation that carries the file name of a layer.
pub const TITLE_ANNOTATION: &str = "org.opencontainers.image.title";

const ARTIFACT_MANIFEST: &str = "application/vnd.oci.artifact.manifest.v1+json";

/// A verified tool package.
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    /// Digest of the OCI manifest, `sha256:<hex>`.
    pub digest: String,
    /// The package definition (config blob).
    pub manifest: PackageManifest,
    /// Modules by file name.
    pub modules: BTreeMap<String, PackageModule>,
    /// The source layer (`.tar.gz`), if the package has one.
    pub source: Option<Vec<u8>>,
    /// The annotations of the OCI manifest (ADR-0124); empty for packages packed before them.
    pub annotations: BTreeMap<String, String>,
}

/// A module with its digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageModule {
    /// `sha256:<hex>` of the bytes.
    pub digest: String,
    /// The WASM module.
    pub bytes: Vec<u8>,
}

#[derive(Debug, Deserialize)]
struct RawManifest {
    #[serde(rename = "mediaType")]
    media_type: Option<String>,
    #[serde(rename = "artifactType")]
    artifact_type: Option<String>,
    config: Option<Descriptor>,
    layers: Option<Vec<Descriptor>>,
    /// The (deprecated) OCI artifact manifest calls its layers `blobs`.
    blobs: Option<Vec<Descriptor>>,
    #[serde(default)]
    annotations: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Descriptor {
    #[serde(rename = "mediaType")]
    media_type: String,
    digest: String,
    #[serde(default)]
    annotations: BTreeMap<String, String>,
}

/// Which blobs a manifest refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Layout {
    pub config: String,
    /// File name → digest.
    pub modules: BTreeMap<String, String>,
    pub source: Option<String>,
    pub annotations: BTreeMap<String, String>,
}

impl Layout {
    /// Parses the OCI manifest and checks that it describes a grauwolf tool package.
    pub(crate) fn parse(manifest: &[u8]) -> Result<Self, PackageFetchError> {
        let wrong = |text: String| PackageFetchError::WrongMediaType(text);
        let raw: RawManifest = serde_json::from_slice(manifest).map_err(|error| {
            PackageFetchError::InvalidPackage(format!("the manifest is not valid JSON: {error}"))
        })?;
        if let Some(media_type) = &raw.media_type {
            if media_type != IMAGE_MANIFEST && media_type != ARTIFACT_MANIFEST {
                return Err(wrong(format!("manifest type {media_type}")));
            }
        }
        if let Some(artifact_type) = &raw.artifact_type {
            if artifact_type != ARTIFACT_TYPE {
                return Err(wrong(format!(
                    "artifact type {artifact_type} (expected {ARTIFACT_TYPE})"
                )));
            }
        }
        let layers = raw.layers.or(raw.blobs).unwrap_or_default();
        // Image manifests name the config, artifact manifests carry it among the blobs.
        let (config, layers) = match raw.config {
            Some(config) => (config, layers),
            None => {
                let (configs, rest): (Vec<_>, Vec<_>) = layers
                    .into_iter()
                    .partition(|layer| layer.media_type == CONFIG_TYPE);
                let config = configs
                    .into_iter()
                    .next()
                    .ok_or_else(|| wrong("no config blob".into()))?;
                (config, rest)
            }
        };
        if config.media_type != CONFIG_TYPE {
            return Err(wrong(format!(
                "config type {} (expected {CONFIG_TYPE})",
                config.media_type
            )));
        }
        let (mut modules, mut source) = (BTreeMap::new(), None);
        for layer in layers {
            match layer.media_type.as_str() {
                MODULE_TYPE => {
                    let title = layer.annotations.get(TITLE_ANNOTATION).ok_or_else(|| {
                        PackageFetchError::InvalidPackage(format!(
                            "module layer {} has no `{TITLE_ANNOTATION}` annotation",
                            layer.digest
                        ))
                    })?;
                    modules.insert(title.clone(), layer.digest);
                }
                SOURCE_TYPE => source = Some(layer.digest),
                other => return Err(wrong(format!("layer type {other}"))),
            }
        }
        Ok(Self {
            config: config.digest,
            modules,
            source,
            annotations: raw.annotations,
        })
    }

    /// Digests of all blobs the package consists of.
    pub(crate) fn blob_digests(&self) -> Vec<&str> {
        std::iter::once(self.config.as_str())
            .chain(self.modules.values().map(String::as_str))
            .chain(self.source.as_deref())
            .collect()
    }

    /// Assembles the package from `blob` (digest → verified content).
    pub(crate) fn assemble(
        &self,
        digest: &str,
        mut blob: impl FnMut(&str) -> Result<Vec<u8>, PackageFetchError>,
    ) -> Result<Package, PackageFetchError> {
        let config = blob(&self.config)?;
        let text = String::from_utf8(config).map_err(|_| {
            PackageFetchError::InvalidPackage("the config is not UTF-8 text".into())
        })?;
        let manifest = PackageManifest::from_json(&text)?;
        let mut modules = BTreeMap::new();
        for tool in &manifest.tools {
            let module_digest = self.modules.get(&tool.module).ok_or_else(|| {
                PackageFetchError::InvalidPackage(format!(
                    "tool '{}' needs the module '{}', which the package does not contain",
                    tool.name, tool.module
                ))
            })?;
            let bytes = blob(module_digest)?;
            if sha256_digest(&bytes) != *module_digest {
                return Err(PackageFetchError::DigestMismatch {
                    what: format!("module {}", tool.module),
                    expected: module_digest.clone(),
                    actual: sha256_digest(&bytes),
                });
            }
            modules.insert(
                tool.module.clone(),
                PackageModule {
                    digest: module_digest.clone(),
                    bytes,
                },
            );
        }
        let source = self.source.as_deref().map(&mut blob).transpose()?;
        Ok(Package {
            digest: digest.to_owned(),
            manifest,
            modules,
            source,
            annotations: self.annotations.clone(),
        })
    }
}
