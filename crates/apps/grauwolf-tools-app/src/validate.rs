//! `validate <package folder>`: checks a tool package folder without building it (ADR-0118).

use std::path::Path;
use std::process::ExitCode;

use grauwolf_spec::ToolSpec;
use grauwolf_tools::{MANIFEST_FILE, PackageManifest};

use crate::output::Io;
use crate::{errln, outln};

/// Every problem of the package folder `dir`: the manifest as `tools pack` reads it, then per
/// tool the code file `src/<tool>.rs` – present, UTF-8, free of forbidden constructs – and
/// modules that more than one tool names. Nothing is compiled. Returns the number of tools when
/// there is no problem.
pub fn check_folder(dir: &Path) -> Result<usize, Vec<String>> {
    let label = |relative: &str| dir.join(relative).display().to_string();
    let text = std::fs::read(dir.join(MANIFEST_FILE))
        .map_err(|error| vec![format!("{}: {error}", label(MANIFEST_FILE))])?;
    let manifest = PackageManifest::from_source_json(&String::from_utf8_lossy(&text))
        .map_err(|error| vec![format!("{}: {error}", label(MANIFEST_FILE))])?;
    let mut errors = Vec::new();
    let mut modules = std::collections::BTreeSet::new();
    for tool in &manifest.tools {
        if !modules.insert(tool.module.as_str()) {
            errors.push(format!(
                "{}: the module '{}' is used by more than one tool",
                label(MANIFEST_FILE),
                tool.module
            ));
        }
        let relative = format!("src/{}.rs", tool.name);
        let code = match std::fs::read(dir.join(&relative)) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(code) => code,
                Err(_) => {
                    errors.push(format!("{}: not UTF-8 text", label(&relative)));
                    continue;
                }
            },
            Err(error) => {
                errors.push(format!("{}: {error}", label(&relative)));
                continue;
            }
        };
        let spec = ToolSpec {
            name: tool.name.clone(),
            description: tool.description.clone(),
            args: tool.args.clone(),
            code,
        };
        if let Err(error) = grauwolf_toolbuild::guest_crate(&spec) {
            errors.push(format!("{}: {error}", label(&relative)));
        }
    }
    if errors.is_empty() {
        Ok(manifest.tools.len())
    } else {
        Err(errors)
    }
}

pub fn run(io: &Io, dir: &Path) -> ExitCode {
    match check_folder(dir) {
        Ok(tools) => {
            outln!(io, "{}: valid ({tools} tools)", dir.display());
            ExitCode::SUCCESS
        }
        Err(errors) => {
            for error in errors {
                errln!(io, "{error}");
            }
            ExitCode::FAILURE
        }
    }
}
