//! `new <name>` (ADR-0124): scaffolds a tool package folder that builds, validates and packs as it
//! is – manifest with the metadata fields, one example tool, its test, a README and a CI workflow.

use std::path::{Path, PathBuf};

use crate::outln;
use crate::output::Io;

const MANIFEST: &str = include_str!("../templates/grauwolf-tools.json");
const TOOL: &str = include_str!("../templates/tool.rs");
const TEST: &str = include_str!("../templates/test.rs");
const CARGO: &str = include_str!("../templates/Cargo.toml.template");
const BUILD: &str = include_str!("../templates/support-build.rs");
const LIB: &str = include_str!("../templates/support-lib.rs");
const README: &str = include_str!("../templates/README.md");
const WORKFLOW: &str = include_str!("../templates/workflow.yml");
const GITIGNORE: &str = include_str!("../templates/gitignore");

/// The files of a new package `name` with the example tool `tool`: relative path → content.
pub fn scaffold(name: &str, tool: &str) -> Result<Vec<(String, String)>, String> {
    let valid_name = name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !valid_name {
        return Err(format!(
            "`{name}`: a package name starts with a-z and has only a-z, 0-9, `-` and `_`"
        ));
    }
    let valid_tool = tool
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && tool
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !valid_tool {
        return Err(format!(
            "`{tool}`: a tool name must be snake_case (a-z, 0-9, _)"
        ));
    }
    let fill = |template: &str| {
        template
            .replace("{{name}}", name)
            .replace("{{tool}}", tool)
            .replace("{{crate}}", &name.replace('_', "-"))
    };
    Ok(vec![
        (grauwolf_tools::MANIFEST_FILE.to_owned(), fill(MANIFEST)),
        (format!("src/{tool}.rs"), fill(TOOL)),
        (format!("tests/{tool}.rs"), fill(TEST)),
        ("tests/support/build.rs".to_owned(), fill(BUILD)),
        ("tests/support/lib.rs".to_owned(), fill(LIB)),
        ("Cargo.toml".to_owned(), fill(CARGO)),
        ("README.md".to_owned(), fill(README)),
        (
            ".github/workflows/grauwolf-tools.yml".to_owned(),
            fill(WORKFLOW),
        ),
        (".gitignore".to_owned(), fill(GITIGNORE)),
    ])
}

/// Writes the scaffold of `name` into `dir` (default `./<name>`), which must not exist or be
/// empty.
pub fn run(io: &Io, name: &str, tool: &str, dir: Option<&Path>) -> Result<(), String> {
    let files = scaffold(name, tool)?;
    let dir: PathBuf = dir.map_or_else(|| PathBuf::from(name), Path::to_path_buf);
    if dir
        .read_dir()
        .is_ok_and(|mut entries| entries.next().is_some())
    {
        return Err(format!(
            "{}: exists and is not empty; nothing written",
            dir.display()
        ));
    }
    for (relative, content) in &files {
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        std::fs::write(&path, content).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    outln!(
        io,
        "{}: new tool package `{name}` with the tool `{tool}`",
        dir.display()
    );
    outln!(
        io,
        "next: cd {} && grauwolf-tools validate . && grauwolf-tools pack .",
        dir.display()
    );
    Ok(())
}
