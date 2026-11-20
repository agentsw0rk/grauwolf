//! Tool sources in a generated crate (ADR-0095): the guest crate of every tool as text and the
//! manifest `tools/tools.json` that lets `grauwolf tools build` check them before compiling.

use serde::{Deserialize, Serialize};

/// Version of the format of `tools/tools.json`.
pub const TOOL_SOURCES_FORMAT: u32 = 1;

/// Path of the manifest relative to the generated crate.
pub const TOOL_SOURCES_FILE: &str = "tools/tools.json";

/// What a guest crate in `tools/` holds (ADR-0102): a tool, or a piece of code of a graph. They
/// are built the same way; the kind says where the module is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSourceKind {
    /// A tool of the spec.
    #[default]
    Tool,
    /// The body of a `code` node.
    Code,
    /// The check of an `on_code` condition.
    Condition,
    /// The transformation of an edge.
    Transform,
}

impl ToolSourceKind {
    fn is_tool(&self) -> bool {
        *self == ToolSourceKind::Tool
    }
}

/// The guest crate of a tool as text: exactly what `grauwolf-toolbuild` compiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSourceCrate {
    /// Name of the tool, or of the code piece (ADR-0102).
    pub name: String,
    /// What the crate holds.
    pub kind: ToolSourceKind,
    /// Content of `Cargo.toml`.
    pub manifest: String,
    /// Content of `src/lib.rs`.
    pub source: String,
    /// SHA-256 of the two files, `sha256:<hex>`; computed by whoever writes the sources (the
    /// domain has no hash function), checked again by `grauwolf tools build`.
    pub hash: String,
}

/// Content of `tools/tools.json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSourcesManifest {
    /// Version of the format ([`TOOL_SOURCES_FORMAT`]).
    pub format: u32,
    /// The tools, in the order of the spec.
    pub tools: Vec<ToolSourceEntry>,
}

/// A tool in `tools/tools.json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSourceEntry {
    /// Name of the tool or code piece; its guest crate is `tools/<name>`.
    pub name: String,
    /// What the crate holds; without it, a tool.
    #[serde(default, skip_serializing_if = "ToolSourceKind::is_tool")]
    pub kind: ToolSourceKind,
    /// File of the built module relative to the generated crate, `tools/<name>.wasm`.
    pub module: String,
    /// [`ToolSourceCrate::hash`] of the guest crate.
    pub sha256: String,
}

/// The manifest could not be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolSourcesError {
    /// Not JSON, or fields missing or unknown.
    #[error("invalid {TOOL_SOURCES_FILE}: {0}")]
    Format(String),
    /// Written by a newer version.
    #[error(
        "unsupported {TOOL_SOURCES_FILE} format version {found} (supported: {TOOL_SOURCES_FORMAT})"
    )]
    UnsupportedFormat { found: u32 },
}

impl ToolSourcesManifest {
    /// The manifest for `crates`.
    pub fn for_crates(crates: &[ToolSourceCrate]) -> Self {
        Self {
            format: TOOL_SOURCES_FORMAT,
            tools: crates
                .iter()
                .map(|tool| ToolSourceEntry {
                    name: tool.name.clone(),
                    kind: tool.kind,
                    module: format!("tools/{}.wasm", tool.name),
                    sha256: tool.hash.clone(),
                })
                .collect(),
        }
    }

    /// Parses and checks the format version.
    pub fn from_json(json: &str) -> Result<Self, ToolSourcesError> {
        let manifest: Self = serde_json::from_str(json)
            .map_err(|error| ToolSourcesError::Format(error.to_string()))?;
        if manifest.format != TOOL_SOURCES_FORMAT {
            return Err(ToolSourcesError::UnsupportedFormat {
                found: manifest.format,
            });
        }
        Ok(manifest)
    }

    /// The manifest as JSON text (pretty, with a final newline).
    pub fn to_json(&self) -> String {
        let mut json =
            serde_json::to_string_pretty(self).expect("a manifest is always serializable");
        json.push('\n');
        json
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, hash: &str) -> ToolSourceCrate {
        ToolSourceCrate {
            name: name.into(),
            kind: ToolSourceKind::Tool,
            manifest: "[package]\n".into(),
            source: "fn a() {}".into(),
            hash: hash.into(),
        }
    }

    #[test]
    fn the_manifest_round_trips_and_names_modules_and_hashes() {
        let crates = [tool("add", "sha256:aa"), tool("sub", "sha256:bb")];
        let manifest = ToolSourcesManifest::for_crates(&crates);
        assert_eq!(manifest.tools[1].module, "tools/sub.wasm");
        assert_eq!(manifest.tools[0].sha256, "sha256:aa");
        assert_eq!(
            ToolSourcesManifest::from_json(&manifest.to_json()),
            Ok(manifest)
        );
    }

    #[test]
    fn a_newer_format_is_refused() {
        let error = ToolSourcesManifest::from_json(r#"{"format":2,"tools":[]}"#).unwrap_err();
        assert_eq!(error, ToolSourcesError::UnsupportedFormat { found: 2 });
    }

    #[test]
    fn the_kind_is_written_for_code_pieces_only_and_defaults_to_a_tool() {
        let mut piece = tool("code_main_score", "sha256:cc");
        piece.kind = ToolSourceKind::Code;
        let manifest = ToolSourcesManifest::for_crates(&[tool("add", "sha256:aa"), piece]);

        let json = manifest.to_json();

        assert_eq!(json.matches("\"kind\"").count(), 1, "{json}");
        assert!(json.contains("\"kind\": \"code\""), "{json}");
        let read = ToolSourcesManifest::from_json(&json).unwrap();
        assert_eq!(read.tools[0].kind, ToolSourceKind::Tool);
        assert_eq!(read.tools[1].kind, ToolSourceKind::Code);
    }
}
