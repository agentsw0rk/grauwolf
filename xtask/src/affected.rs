//! Selects the packages that a change affects, so a commit check only builds and tests what the
//! change can break (ADR-0108 stage 1). Pure functions over a description of the workspace;
//! reading `cargo metadata` and `git` happens elsewhere.

use std::collections::{BTreeMap, BTreeSet};

/// A package of the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// Package name.
    pub name: String,
    /// Directory of the package relative to the workspace root, with `/` as separator.
    pub dir: String,
    /// Names of the workspace packages it depends on (normal, dev and build dependencies).
    pub dependencies: Vec<String>,
}

/// What a change affects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Every package, with the reason.
    All(String),
    /// These packages, each with the reason it is selected (sorted by name).
    Packages(BTreeMap<String, String>),
}

impl Selection {
    /// A readable account of the selection, one line per package with the reason.
    pub fn summary(&self) -> String {
        match self {
            Selection::All(reason) => format!("all packages selected ({reason})"),
            Selection::Packages(packages) if packages.is_empty() => {
                "no packages selected (no source changes)".to_owned()
            }
            Selection::Packages(packages) => {
                let mut text = format!("{} packages selected:", packages.len());
                for (name, reason) in packages {
                    text.push_str(&format!("\n  {name} ({reason})"));
                }
                text
            }
        }
    }
}

/// Selects the packages affected by the changed files (paths relative to the workspace root).
pub fn select(members: &[Member], changed: &[String]) -> Selection {
    if let Some(path) = changed.iter().find(|path| is_root_file(path)) {
        return Selection::All(format!("changed: {path}"));
    }
    let mut packages = BTreeMap::new();
    for path in changed {
        if let Some(member) = members.iter().find(|member| contains(&member.dir, path)) {
            packages
                .entry(member.name.clone())
                .or_insert_with(|| format!("changed: {path}"));
        }
        for name in readers_of(path) {
            if members.iter().any(|member| member.name == *name) {
                packages
                    .entry((*name).to_owned())
                    .or_insert_with(|| format!("reads {path}"));
            }
        }
    }
    add_dependents(members, &mut packages);
    Selection::Packages(packages)
}

/// Adds every package that depends, directly or not, on a selected one.
fn add_dependents(members: &[Member], packages: &mut BTreeMap<String, String>) {
    let mut queue: Vec<String> = packages.keys().cloned().collect();
    while let Some(name) = queue.pop() {
        for member in members {
            if member.dependencies.contains(&name) && !packages.contains_key(&member.name) {
                packages.insert(member.name.clone(), format!("depends on {name}"));
                queue.push(member.name.clone());
            }
        }
    }
}

/// Packages whose tests or code read files outside their own directory: the example specs of the
/// user guide and the German handbook are checked by the host's `docs_examples` test; the corpus
/// `examples/` is read by the spec, interpreter, generator, builder agent, host, server and CLI;
/// `studio/web/lib/api/openapi.json` by the studio's test that keeps it current (ADR-0126).
fn readers_of(path: &str) -> &'static [&'static str] {
    if path.starts_with("docs/user-guide/examples/") || path.starts_with("docs/handbuch/beispiele/")
    {
        &["grauwolf-host"]
    } else if path.starts_with("studio/web/lib/api/") {
        // The committed OpenAPI description of the studio, compared by its test.
        &["grauwolf-studio"]
    } else if path.starts_with("examples/") {
        &[
            "grauwolf-spec",
            "grauwolf-interpreter",
            "grauwolf-generator",
            "grauwolf-builder-agent",
            "grauwolf-host",
            "grauwolf-server",
            "grauwolf-cli",
        ]
    } else {
        &[]
    }
}

/// Files that configure the whole workspace: they can change every package's build.
fn is_root_file(path: &str) -> bool {
    matches!(path, "Cargo.toml" | "Cargo.lock")
        || path.starts_with(".cargo/")
        || path.starts_with("xtask/")
        || path.starts_with("rust-toolchain")
}

fn contains(dir: &str, path: &str) -> bool {
    path.strip_prefix(dir)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// The changed files from the output of `git diff --name-only <base>` and of
/// `git status --porcelain` (which adds untracked files and renames), sorted and without
/// duplicates.
pub fn changed_paths(diff_names: &str, porcelain_status: &str) -> Vec<String> {
    let from_status = porcelain_status
        .lines()
        .filter_map(|line| line.get(3..))
        .flat_map(|paths| paths.split(" -> "));
    diff_names
        .lines()
        .chain(from_status)
        .map(|path| path.trim_matches('"').to_owned())
        .filter(|path| !path.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Reads the packages of the workspace from the output of `cargo metadata --no-deps`; of the
/// dependencies only those on other workspace packages are kept.
pub fn members_from_metadata(metadata: &serde_json::Value) -> Vec<Member> {
    let root = std::path::Path::new(metadata["workspace_root"].as_str().unwrap_or_default());
    let packages = metadata["packages"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    let names: BTreeSet<&str> = packages
        .iter()
        .filter_map(|package| package["name"].as_str())
        .collect();
    packages
        .iter()
        .map(|package| {
            let manifest =
                std::path::Path::new(package["manifest_path"].as_str().unwrap_or_default());
            let dir = manifest
                .parent()
                .and_then(|dir| dir.strip_prefix(root).ok())
                .map(|dir| dir.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let dependencies = package["dependencies"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|dependency| dependency["name"].as_str())
                .filter(|name| names.contains(name))
                .map(str::to_owned)
                .collect();
            Member {
                name: package["name"].as_str().unwrap_or_default().to_owned(),
                dir,
                dependencies,
            }
        })
        .collect()
}
