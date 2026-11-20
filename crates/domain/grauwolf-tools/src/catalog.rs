//! The tool catalog (ADR-0124): one JSON index of the tool packages of a registry, one entry per
//! package version, stored as an OCI artifact next to them. This module is the format and the
//! pure operations on it; reading, writing and signing belong to the adapters.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ToolLimits;

/// Version of the index format that this crate reads and writes.
pub const CATALOG_FORMAT: u32 = 1;

/// Artifact type (and media type of the index layer) of the catalog artifact.
pub const CATALOG_ARTIFACT_TYPE: &str = "application/vnd.grauwolf.catalog.v1+json";

/// Repository of the catalog in a registry when nothing else is configured.
pub const CATALOG_REPOSITORY: &str = "grauwolf-catalog";

/// Tag of the catalog when nothing else is configured.
pub const CATALOG_TAG: &str = "latest";

/// The index: when it was last written and its entries by [`CatalogEntry::key`].
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogIndex {
    /// Version of the index format.
    pub format: u32,
    /// When the index was written last (RFC 3339).
    pub updated: String,
    /// The entries, keyed by `<name>@<version>`.
    pub entries: BTreeMap<String, CatalogEntry>,
}

/// One package version in the catalog: what is needed to choose it and to trust it.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CatalogEntry {
    /// `registry/repository@sha256:…`: the package pinned by digest.
    pub reference: String,
    /// Name of the package.
    pub name: String,
    /// Version of the package.
    pub version: String,
    /// What the package is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The tools of the package.
    pub tools: Vec<CatalogTool>,
    /// Build date (RFC 3339).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    /// Where the sources live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The commit the package was built from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// SPDX licence expression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Authors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    /// Tool ABI of the modules.
    pub abi: u32,
    /// The grauwolf version that packed the package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built_with: Option<String>,
    /// The Rust toolchain that built the modules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rustc: Option<String>,
    /// The signatures of the package as found when the entry was written.
    pub signature: CatalogSignature,
}

/// A tool of a catalog entry.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CatalogTool {
    /// Name under which the LLM calls the tool.
    pub name: String,
    /// Description for the LLM.
    pub description: String,
    /// JSON schema of the arguments.
    pub args: serde_json::Value,
    /// File name of the module.
    pub module: String,
    /// `sha256:<hex>` of the module.
    pub digest: String,
    /// Size of the module in bytes.
    pub size: u64,
    /// Sandbox limits the tool asks for.
    #[serde(default, skip_serializing_if = "ToolLimits::is_empty")]
    pub limits: ToolLimits,
    /// What the tool needs from the host (empty under tool ABI v1).
    #[serde(default)]
    pub capabilities: Vec<serde_json::Value>,
}

/// The signature block of an entry: a fact about the registry when the entry was written. Whether
/// the package is allowed is decided by each reader against its own tool policy, from the
/// signatures themselves; it is never stored.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct CatalogSignature {
    /// There is at least one signature.
    pub signed: bool,
    /// Fingerprints of the keys known to the writer that verify a signature.
    #[serde(default)]
    pub fingerprints: Vec<String>,
    /// The cosign signatures: the simple-signing payload (JSON text) and the base64 signature.
    #[serde(default)]
    pub signatures: Vec<CatalogSignatureEntry>,
}

/// One cosign signature as stored in the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CatalogSignatureEntry {
    /// The signed payload (simple-signing JSON).
    pub payload: String,
    /// Base64 DER ECDSA signature.
    pub signature: String,
}

/// An index that cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
    /// Not JSON, or fields missing or of the wrong type.
    #[error("invalid tool catalog: {0}")]
    Format(String),
    /// The index format is newer than this version understands.
    #[error("unsupported tool catalog format {found} (supported: {CATALOG_FORMAT})")]
    UnsupportedFormat { found: u32 },
}

impl CatalogEntry {
    /// `<name>@<version>`, the key of the entry in the index.
    pub fn key(&self) -> String {
        entry_key(&self.name, &self.version)
    }

    /// Does `text` occur (ignoring case) in the name, the description, or a tool's name or
    /// description?
    pub fn matches(&self, text: &str) -> bool {
        let text = text.to_lowercase();
        let has = |value: &str| value.to_lowercase().contains(&text);
        has(&self.name)
            || self.description.as_deref().is_some_and(has)
            || self
                .tools
                .iter()
                .any(|tool| has(&tool.name) || has(&tool.description))
    }
}

/// `<name>@<version>`.
pub fn entry_key(name: &str, version: &str) -> String {
    format!("{name}@{version}")
}

impl CatalogIndex {
    /// An empty index written at `updated`.
    pub fn new(updated: impl Into<String>) -> Self {
        Self {
            format: CATALOG_FORMAT,
            updated: updated.into(),
            entries: BTreeMap::new(),
        }
    }

    /// Parses an index.
    pub fn from_json(json: &str) -> Result<Self, CatalogError> {
        #[derive(Deserialize)]
        struct Version {
            format: u32,
        }
        let version: Version =
            serde_json::from_str(json).map_err(|error| CatalogError::Format(error.to_string()))?;
        if version.format != CATALOG_FORMAT {
            return Err(CatalogError::UnsupportedFormat {
                found: version.format,
            });
        }
        serde_json::from_str(json).map_err(|error| CatalogError::Format(error.to_string()))
    }

    /// The index as JSON text (sorted, so the same index gives the same bytes).
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("an index is always serializable")
    }

    /// Adds `entry` or replaces the entry of the same name and version; returns whether the
    /// index changed.
    pub fn upsert(&mut self, entry: CatalogEntry) -> bool {
        let key = entry.key();
        if self.entries.get(&key) == Some(&entry) {
            return false;
        }
        self.entries.insert(key, entry);
        true
    }

    /// Removes the entry `name@version`, if there is one.
    pub fn remove(&mut self, name: &str, version: &str) -> Option<CatalogEntry> {
        self.entries.remove(&entry_key(name, version))
    }

    /// The entries in order of name, then version (newest last).
    pub fn list(&self) -> Vec<&CatalogEntry> {
        let mut entries: Vec<&CatalogEntry> = self.entries.values().collect();
        entries.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| compare_versions(&a.version, &b.version))
        });
        entries
    }

    /// The entries that [match](CatalogEntry::matches) `text`, in the order of [`Self::list`].
    pub fn search(&self, text: &str) -> Vec<&CatalogEntry> {
        self.list()
            .into_iter()
            .filter(|entry| entry.matches(text))
            .collect()
    }

    /// The entry of package `name` in `version`, or its newest version without one.
    pub fn find(&self, name: &str, version: Option<&str>) -> Option<&CatalogEntry> {
        match version {
            Some(version) => self.entries.get(&entry_key(name, version)),
            None => self
                .list()
                .into_iter()
                .rev()
                .find(|entry| entry.name == name),
        }
    }

    /// All versions of package `name`, oldest first.
    pub fn versions(&self, name: &str) -> Vec<&str> {
        self.list()
            .into_iter()
            .filter(|entry| entry.name == name)
            .map(|entry| entry.version.as_str())
            .collect()
    }
}

/// Orders versions by their dot-separated parts, numerically where both parts are numbers
/// (`1.10.0` after `1.9.2`), else as text.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parts = |version: &str| -> Vec<String> {
        version
            .trim_start_matches('v')
            .split(['.', '-', '+'])
            .map(str::to_owned)
            .collect()
    };
    let (a, b) = (parts(a), parts(b));
    for (x, y) in a.iter().zip(&b) {
        let order = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            _ => x.cmp(y),
        };
        if order != std::cmp::Ordering::Equal {
            return order;
        }
    }
    a.len().cmp(&b.len())
}
