//! Format of a tool package, `grauwolf-tools.json` (ADR-0107): name, version, tool ABI version
//! and per tool the definition and the file name of its WASM module.

use serde::{Deserialize, Serialize};

use crate::ToolArtifact;

/// Version of the package format that this crate reads and writes.
pub const PACKAGE_FORMAT: u32 = 1;

/// Tool ABI version of the modules (ADR-0029).
pub const TOOL_ABI: u32 = 1;

/// Name of the package manifest, also the config blob of an OCI artifact.
pub const MANIFEST_FILE: &str = "grauwolf-tools.json";

/// Content of `grauwolf-tools.json`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackageManifest {
    /// Version of the package format.
    pub format: u32,
    /// Name of the package.
    pub name: String,
    /// Version of the package (free text, e.g. `1.2.0`).
    pub version: String,
    /// What the package is for (ADR-0124); written only when set, so manifests without metadata
    /// stay as they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// SPDX licence expression, e.g. `Apache-2.0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Authors, e.g. `Ada <ada@example.com>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    /// Where the sources live (a repository URL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Tool ABI version of all modules.
    pub abi: u32,
    /// The tools of the package.
    pub tools: Vec<PackageTool>,
}

/// Sandbox limits a package asks for, per tool (ADR-0124). The runtime applies the stricter of
/// these and its own; a missing value leaves the host's limit alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolLimits {
    /// Largest linear memory in MiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mib: Option<u64>,
    /// Compute budget per call (wasmtime fuel).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fuel: Option<u64>,
    /// Wall-clock time per call in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_ms: Option<u64>,
}

impl ToolLimits {
    /// No limit set.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The stricter of `self` and `other`, limit by limit.
    pub fn stricter(&self, other: &Self) -> Self {
        let min = |a: Option<u64>, b: Option<u64>| match (a, b) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        Self {
            memory_mib: min(self.memory_mib, other.memory_mib),
            fuel: min(self.fuel, other.fuel),
            time_ms: min(self.time_ms, other.time_ms),
        }
    }

    fn zero(&self) -> Option<&'static str> {
        [
            ("memory_mib", self.memory_mib),
            ("fuel", self.fuel),
            ("time_ms", self.time_ms),
        ]
        .into_iter()
        .find_map(|(name, value)| (value == Some(0)).then_some(name))
    }
}

/// A tool of a package: definition and the module that implements it.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackageTool {
    /// Name under which the LLM calls the tool (snake_case).
    pub name: String,
    /// Description for the LLM.
    pub description: String,
    /// JSON schema of the arguments.
    pub args: serde_json::Value,
    /// File name of the WASM module in the package.
    pub module: String,
    /// Sandbox limits the tool asks for (ADR-0124).
    #[serde(default, skip_serializing_if = "ToolLimits::is_empty")]
    pub limits: ToolLimits,
    /// What the tool needs from the host. Tool ABI v1 grants nothing, so the list must be empty;
    /// a later ABI with host calls lists them here (ADR-0124).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<serde_json::Value>,
}

/// `grauwolf-tools.json` of a package folder, as an author writes it: `format` and `abi` may be
/// left out (the current ones are meant), and so may the module file names, which are derived
/// from the tool names (`<tool>.wasm`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageSource {
    #[serde(default = "current_format")]
    format: u32,
    name: String,
    version: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default = "current_abi")]
    abi: u32,
    tools: Vec<SourceTool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceTool {
    name: String,
    description: String,
    args: serde_json::Value,
    #[serde(default)]
    module: Option<String>,
    #[serde(default)]
    limits: ToolLimits,
    #[serde(default)]
    capabilities: Vec<serde_json::Value>,
}

fn current_format() -> u32 {
    PACKAGE_FORMAT
}

fn current_abi() -> u32 {
    TOOL_ABI
}

/// A package manifest that cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PackageError {
    /// Not JSON, or fields missing, unknown or of the wrong type.
    #[error("invalid package manifest: {0}")]
    Format(String),
    /// The package format is newer than this version understands.
    #[error("unsupported package format version {found} (supported: {PACKAGE_FORMAT})")]
    UnsupportedFormat { found: u32 },
    /// The modules are built for another tool ABI.
    #[error("unsupported tool ABI version {found} (supported: {TOOL_ABI})")]
    UnsupportedAbi { found: u32 },
    /// The package has no tools.
    #[error("the package contains no tools")]
    NoTools,
    /// A tool name is not snake_case.
    #[error("tool name '{name}' must be snake_case (a-z, 0-9, _)")]
    InvalidToolName { name: String },
    /// Two tools of the package have the same name.
    #[error("tool '{name}' is defined more than once")]
    DuplicateTool { name: String },
    /// A module name is not a plain file name.
    #[error("module name '{module}' must be a plain file name")]
    InvalidModuleName { module: String },
    /// A tool asks for host capabilities, which tool ABI v1 does not grant.
    #[error(
        "tool '{tool}' lists capabilities, but tool ABI {TOOL_ABI} grants none (no network, no \
         files): leave `capabilities` empty"
    )]
    UnsupportedCapabilities { tool: String },
    /// A limit of zero would make the tool unusable.
    #[error("tool '{tool}': the limit `{limit}` must be greater than 0")]
    InvalidLimit { tool: String, limit: String },
}

impl PackageManifest {
    /// Parses and validates a manifest.
    pub fn from_json(json: &str) -> Result<Self, PackageError> {
        let manifest: Self =
            serde_json::from_str(json).map_err(|error| PackageError::Format(error.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Parses the `grauwolf-tools.json` of a package folder (module file names optional, derived
    /// as `<tool>.wasm`) and validates it like a published manifest.
    pub fn from_source_json(json: &str) -> Result<Self, PackageError> {
        let source: PackageSource =
            serde_json::from_str(json).map_err(|error| PackageError::Format(error.to_string()))?;
        let manifest = Self {
            format: source.format,
            name: source.name,
            version: source.version,
            description: source.description,
            license: source.license,
            authors: source.authors,
            source: source.source,
            abi: source.abi,
            tools: source
                .tools
                .into_iter()
                .map(|tool| PackageTool {
                    module: tool.module.unwrap_or_else(|| format!("{}.wasm", tool.name)),
                    name: tool.name,
                    description: tool.description,
                    args: tool.args,
                    limits: tool.limits,
                    capabilities: tool.capabilities,
                })
                .collect(),
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// The manifest as JSON text.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a manifest is always serializable")
    }

    fn validate(&self) -> Result<(), PackageError> {
        if self.format != PACKAGE_FORMAT {
            return Err(PackageError::UnsupportedFormat { found: self.format });
        }
        if self.abi != TOOL_ABI {
            return Err(PackageError::UnsupportedAbi { found: self.abi });
        }
        if self.tools.is_empty() {
            return Err(PackageError::NoTools);
        }
        for (index, tool) in self.tools.iter().enumerate() {
            if !is_snake_case(&tool.name) {
                return Err(PackageError::InvalidToolName {
                    name: tool.name.clone(),
                });
            }
            if self.tools[..index]
                .iter()
                .any(|other| other.name == tool.name)
            {
                return Err(PackageError::DuplicateTool {
                    name: tool.name.clone(),
                });
            }
            if !is_plain_file_name(&tool.module) {
                return Err(PackageError::InvalidModuleName {
                    module: tool.module.clone(),
                });
            }
            if !tool.capabilities.is_empty() {
                return Err(PackageError::UnsupportedCapabilities {
                    tool: tool.name.clone(),
                });
            }
            if let Some(limit) = tool.limits.zero() {
                return Err(PackageError::InvalidLimit {
                    tool: tool.name.clone(),
                    limit: limit.to_owned(),
                });
            }
        }
        Ok(())
    }
}

fn is_snake_case(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// A file name without directories: nothing a package could use to reach outside its folder.
fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

/// A tool of a resolved package, ready to run: the definition as the spec sees it (after
/// renaming) and the verified module (ADR-0107).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTool {
    /// Name in the spec.
    pub name: String,
    /// Description for the LLM.
    pub description: String,
    /// JSON schema of the arguments.
    pub args: serde_json::Value,
    /// The module; `artifact.name` is `name`, `artifact.hash` the SHA-256 of the bytes.
    pub artifact: ToolArtifact,
    /// The sandbox limits the package asks for; the runtime applies the stricter of these and
    /// its own (ADR-0124).
    pub limits: ToolLimits,
}
