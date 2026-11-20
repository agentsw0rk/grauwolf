//! The registry of a harness's default image (ADR-0128 H7). A harness is one agent tree (decision
//! 2); its image without an `image` of its own is `<registry>/<name>:<version>`, the registry from
//! `[harness] registry` of the nearest `grauwolf.toml` above the harness file.

use std::path::Path;

/// The file with the workspace's settings.
const CONFIG_FILE: &str = "grauwolf.toml";

/// `[harness] registry` of the nearest `grauwolf.toml` in the folder of `harness_file` or a folder
/// above it; `None` when there is none or the nearest has no such entry. A file that is not TOML,
/// or a `registry` that is no text, is an error naming the file.
pub fn harness_registry(harness_file: &Path) -> Result<Option<String>, String> {
    let start = harness_file.parent().unwrap_or(Path::new("."));
    let Some(config) = start
        .ancestors()
        .map(|dir| dir.join(CONFIG_FILE))
        .find(|file| file.is_file())
    else {
        return Ok(None);
    };
    let fail = |message: String| format!("{}: {message}", config.display());
    let text = std::fs::read_to_string(&config).map_err(|error| fail(error.to_string()))?;
    let table: toml::Table = text
        .parse()
        .map_err(|error: toml::de::Error| fail(error.to_string()))?;
    match table
        .get("harness")
        .and_then(|section| section.get("registry"))
    {
        None => Ok(None),
        Some(toml::Value::String(registry)) => Ok(Some(registry.clone())),
        Some(_) => Err(fail("[harness] registry must be text".to_owned())),
    }
}
