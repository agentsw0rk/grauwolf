//! Architecture fitness test: checks the dependency rules of the layers (ADR-0016).

use std::collections::HashMap;

/// Layer of a crate, derived from `crates/<layer>/<crate>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// Domain core.
    Domain,
    /// Technical adapters.
    Adapters,
    /// Generic helper crates and test kit.
    Support,
    /// Assembly (facade, CLI).
    Apps,
    /// Outside the layers (e.g. xtask).
    Tooling,
}

/// Kind of a dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyKind {
    /// `[dependencies]`.
    Normal,
    /// `[dev-dependencies]`.
    Dev,
    /// `[build-dependencies]`.
    Build,
}

/// A dependency of a crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    /// Paketname.
    pub name: String,
    /// Kind of the dependency.
    pub kind: DependencyKind,
}

/// A crate of the workspace with its dependencies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    /// Paketname.
    pub name: String,
    /// Layer.
    pub layer: Layer,
    /// Direct dependencies.
    pub dependencies: Vec<Dependency>,
}

/// External crates that domain crates may use as a normal dependency (ADR-0016).
pub const DOMAIN_EXTERNAL_ALLOWLIST: &[&str] = &[
    "serde",
    "serde_json",
    // Format errors of specs with JSON pointers (ADR-0080); pure serde helper without IO.
    "serde_path_to_error",
    "schemars",
    // Parsing of the tool policy file (ADR-0107 stage 4); pure text-to-value, no IO.
    "toml",
    "thiserror",
    "futures-core",
    "futures-util",
    "async-lock",
    "pin-project-lite",
];

/// Test kit that domain crates may only use as a dev dependency.
const TESTKIT: &str = "grauwolf-test";

/// The facade: the public API (re-exports, macros, features), no layer of its own in the app order.
pub const FACADE: &str = "grauwolf";
/// Composition root; the lowest of the app crates.
pub const HOST: &str = "grauwolf-host";
/// HTTP surface; depends on the host.
pub const SERVER: &str = "grauwolf-server";
/// Command line; depends on host and server.
pub const CLI: &str = "grauwolf-cli";

/// The lean program for building and distributing tools (ADR-0118); the package is not called
/// `grauwolf-tools` because the domain crate has that name. Beside the host: the CLI may depend on
/// it, it depends on no other app crate.
pub const TOOLS_APP: &str = "grauwolf-tools-app";
/// The operator program `grauwolf-k8-ops` (ADR-0120): a sibling of the tool program.
pub const K8S_OPS_APP: &str = "grauwolf-k8-ops-app";
/// grauwolf studio (ADR-0126): the visual editor's backend. It may compose the host and the
/// server like the CLI; no app crate depends on it.
pub const STUDIO: &str = "grauwolf-studio";

/// The app crates that no other layer may depend on, in any kind of dependency.
const APP_CRATES: &[&str] = &[HOST, SERVER, CLI, FACADE, TOOLS_APP, K8S_OPS_APP, STUDIO];

/// Dev dependencies of non-app crates on app crates that are allowed (`(package, dependency)`).
/// None exist today; a new entry needs a reason here.
pub const APP_DEV_DEPENDENCY_EXCEPTIONS: &[(&str, &str)] = &[];

/// Rule of the app order (ADR-0108 stage 4d): which app crates must not depend on which, for
/// every kind of dependency.
const APP_ORDER: &[(&str, &[&str])] = &[
    (HOST, &[SERVER, CLI, FACADE, TOOLS_APP, K8S_OPS_APP, STUDIO]),
    (SERVER, &[CLI, FACADE, TOOLS_APP, K8S_OPS_APP, STUDIO]),
    (CLI, &[FACADE, K8S_OPS_APP, STUDIO]),
    (TOOLS_APP, &[HOST, SERVER, CLI, FACADE, K8S_OPS_APP, STUDIO]),
    (K8S_OPS_APP, &[HOST, SERVER, CLI, FACADE, TOOLS_APP, STUDIO]),
    (STUDIO, &[CLI, FACADE, TOOLS_APP, K8S_OPS_APP]),
];

/// Spec model: the data types of the spec DSL; depends on no other spec crate.
pub const SPEC_MODEL: &str = "grauwolf-spec-model";
/// Strategy templates; depend only on the spec model (and the node catalog).
pub const STRATEGIES: &str = "grauwolf-strategies";
/// Loading with strategy expansion and validation; the top of the spec crates.
pub const SPEC: &str = "grauwolf-spec";
/// Node catalog; the base of the spec crates.
const GRAPH: &str = "grauwolf-graph";

/// Rule of the spec crates (ADR-0108 stage 6): for each crate the grauwolf crates of the spec group
/// it must not depend on, for every kind of dependency. Order: graph -> spec-model -> strategies
/// -> spec; nothing depends on a crate above it.
const SPEC_ORDER: &[(&str, &[&str])] = &[
    (GRAPH, &[SPEC_MODEL, STRATEGIES, SPEC]),
    (SPEC_MODEL, &[STRATEGIES, SPEC]),
    (STRATEGIES, &[SPEC]),
];

/// Keywords that mark logic in the facade; it may only contain re-exports (and macros re-exported
/// from `grauwolf-macros`, features and documentation).
const FACADE_LOGIC_KEYWORDS: &[&str] = &[
    "fn",
    "struct",
    "enum",
    "impl",
    "trait",
    "static",
    "const",
    "type",
    "macro_rules",
];

/// Checks all rules and returns the violations as readable messages.
pub fn check(packages: &[Package]) -> Vec<String> {
    let layers: HashMap<&str, Layer> = packages
        .iter()
        .map(|package| (package.name.as_str(), package.layer))
        .collect();
    let mut violations = Vec::new();
    for package in packages {
        for dependency in &package.dependencies {
            if let Some(violation) = app_violation(package, dependency) {
                violations.push(violation);
                continue;
            }
            if let Some(violation) = spec_violation(package, dependency) {
                violations.push(violation);
                continue;
            }
            if dependency.kind == DependencyKind::Dev {
                continue;
            }
            let target = layers.get(dependency.name.as_str()).copied();
            if !allowed(package, &dependency.name, target) {
                violations.push(format!(
                    "{} ({:?}) must not depend on {}",
                    package.name, package.layer, dependency.name
                ));
            }
        }
    }
    violations
}

/// Violation of the spec crate order for one dependency (all kinds), if any.
fn spec_violation(package: &Package, dependency: &Dependency) -> Option<String> {
    let forbidden = SPEC_ORDER
        .iter()
        .find(|(name, _)| *name == package.name)
        .is_some_and(|(_, forbidden)| forbidden.contains(&dependency.name.as_str()));
    forbidden.then(|| {
        format!(
            "{} must not depend on {} ({:?}): rule spec crates {GRAPH} -> {SPEC_MODEL} -> \
             {STRATEGIES} -> {SPEC}; {SPEC_MODEL} depends on no other spec crate, {STRATEGIES} \
             only on the model (and the node catalog), and nothing below {SPEC} depends on it",
            package.name, dependency.name, dependency.kind
        )
    })
}

/// Violation of the app order for one dependency (all kinds, dev dependencies included), if any.
fn app_violation(package: &Package, dependency: &Dependency) -> Option<String> {
    let kind = match dependency.kind {
        DependencyKind::Normal => "dependency",
        DependencyKind::Dev => "dev-dependency",
        DependencyKind::Build => "build-dependency",
    };
    let edge = format!(
        "{} ({:?}) must not depend on {} ({kind})",
        package.name, package.layer, dependency.name
    );
    if package.layer == Layer::Apps {
        let forbidden = APP_ORDER
            .iter()
            .find(|(name, _)| *name == package.name)
            .is_some_and(|(_, forbidden)| forbidden.contains(&dependency.name.as_str()));
        return forbidden.then(|| {
            format!(
                "{edge}: rule app order host -> server -> cli (the facade `{FACADE}` is only the \
                 public API on top); an app crate depends only on those below it; \
                 `grauwolf-tools` ({TOOLS_APP}) depends on no other app crate (ADR-0118) and \
                 only the CLI may depend on it; `grauwolf-k8-ops` ({K8S_OPS_APP}, ADR-0120) \
                 depends on no other app crate and nothing depends on it; grauwolf studio \
                 ({STUDIO}, ADR-0126) may use the host and the server, and nothing depends on it"
            )
        });
    }
    let is_app_crate = APP_CRATES.contains(&dependency.name.as_str());
    let excepted = dependency.kind == DependencyKind::Dev
        && APP_DEV_DEPENDENCY_EXCEPTIONS
            .contains(&(package.name.as_str(), dependency.name.as_str()));
    (package.layer != Layer::Tooling && is_app_crate && !excepted).then(|| {
        format!(
            "{edge}: rule domain, adapters and support crates never depend on the app crates \
             ({HOST}, {SERVER}, {CLI}, {FACADE}, {TOOLS_APP}, {K8S_OPS_APP}, {STUDIO})"
        )
    })
}

/// Violations of "the facade contains no logic": `sources` are the `(file name, content)` pairs of
/// its `src/` directory. Comments (also doc comments) and string literals are ignored; every item
/// keyword in [`FACADE_LOGIC_KEYWORDS`] outside of them is a violation.
pub fn facade_logic_violations(sources: &[(String, String)]) -> Vec<String> {
    let mut violations = Vec::new();
    for (file, source) in sources {
        for (number, line) in source.lines().enumerate() {
            let code = strip_strings(line.split("//").next().unwrap_or_default());
            for word in code.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
                if FACADE_LOGIC_KEYWORDS.contains(&word) {
                    violations.push(format!(
                        "{FACADE} ({file}:{}) contains `{word}`: rule the facade has no logic of \
                         its own beyond re-exports, macros and features; put it in {HOST} or {SERVER}",
                        number + 1
                    ));
                }
            }
        }
    }
    violations
}

/// Removes the content of simple string literals from one line.
fn strip_strings(line: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in line.chars() {
        if c == '"' {
            inside = !inside;
        } else if !inside {
            out.push(c);
        }
    }
    out
}

/// Reads the sources of the facade and returns the violations of its no-logic rule.
pub fn facade_violations() -> Result<Vec<String>, String> {
    let src = workspace_root()?.join("crates/apps/grauwolf/src");
    let mut sources = Vec::new();
    let mut stack = vec![src.clone()];
    while let Some(dir) = stack.pop() {
        for entry in
            std::fs::read_dir(&dir).map_err(|error| format!("{}: {error}", dir.display()))?
        {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let name = path
                    .strip_prefix(&src)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                let content = std::fs::read_to_string(&path)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                sources.push((format!("src/{name}"), content));
            }
        }
    }
    sources.sort();
    Ok(facade_logic_violations(&sources))
}

/// Violations of all rules for the real workspace: layers, app order and the facade.
pub fn workspace_violations() -> Result<(usize, Vec<String>), String> {
    let packages = workspace_packages()?;
    let mut violations = check(&packages);
    violations.extend(facade_violations()?);
    Ok((packages.len(), violations))
}

/// May `package` (normal or for building) depend on `dependency` in layer `target`?
/// `target == None` means: external crate.
fn allowed(package: &Package, dependency: &str, target: Option<Layer>) -> bool {
    match package.layer {
        Layer::Domain => match target {
            Some(Layer::Domain) => true,
            Some(Layer::Support) => dependency != TESTKIT,
            Some(_) => false,
            None => DOMAIN_EXTERNAL_ALLOWLIST.contains(&dependency),
        },
        Layer::Support => match target {
            None => true,
            Some(Layer::Domain) => package.name == TESTKIT,
            Some(_) => false,
        },
        Layer::Adapters => target != Some(Layer::Apps),
        Layer::Apps | Layer::Tooling => true,
    }
}

/// Reads the crates of the workspace via `cargo metadata`.
pub fn workspace_packages() -> Result<Vec<Package>, String> {
    Ok(packages_from_metadata(&cargo_metadata()?))
}

/// The root directory of the workspace.
pub fn workspace_root() -> Result<std::path::PathBuf, String> {
    let metadata = cargo_metadata()?;
    metadata["workspace_root"]
        .as_str()
        .map(std::path::PathBuf::from)
        .ok_or_else(|| "cargo metadata names no workspace root".to_owned())
}

/// Runs `cargo metadata --no-deps` (works offline) and returns its JSON.
pub fn cargo_metadata() -> Result<serde_json::Value, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = std::process::Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|error| format!("cannot run cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("cargo metadata returned no JSON: {error}"))
}

/// Converts the output of `cargo metadata --no-deps` into packages with a layer.
fn packages_from_metadata(metadata: &serde_json::Value) -> Vec<Package> {
    let root = metadata["workspace_root"].as_str().unwrap_or_default();
    let crates_dir = std::path::Path::new(root).join("crates");
    metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|package| {
            let manifest =
                std::path::Path::new(package["manifest_path"].as_str().unwrap_or_default());
            let layer = match manifest
                .strip_prefix(&crates_dir)
                .ok()
                .and_then(|relative| relative.iter().next())
                .and_then(|layer| layer.to_str())
            {
                Some("domain") => Layer::Domain,
                Some("adapters") => Layer::Adapters,
                Some("support") => Layer::Support,
                Some("apps") => Layer::Apps,
                _ => Layer::Tooling,
            };
            let dependencies = package["dependencies"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|dependency| Dependency {
                    name: dependency["name"].as_str().unwrap_or_default().to_owned(),
                    kind: match dependency["kind"].as_str() {
                        Some("dev") => DependencyKind::Dev,
                        Some("build") => DependencyKind::Build,
                        _ => DependencyKind::Normal,
                    },
                })
                .collect();
            Package {
                name: package["name"].as_str().unwrap_or_default().to_owned(),
                layer,
                dependencies,
            }
        })
        .collect()
}

/// The most that the normal dependencies of [`TOOLS_APP`] may be, in percent of those of the CLI
/// (ADR-0118). Measured on 2026-11-13 with `cargo metadata` (the graph with the features of the
/// whole workspace, so higher than a single build): 216 against 354 crates = 61%; 225 against 356 = 63% with the agent tree reader of ADR-0118. The threshold
/// leaves room for growth; a dependency on the host pulls in nearly all of the CLI's crates
/// (about 98%) and fails the test.
pub const TOOLS_APP_MAX_PERCENT_OF_CLI: usize = 75;

/// Runs `cargo metadata --offline` with the resolved dependency graph for the platform of the
/// build (other platforms' crates are not downloaded) and returns its JSON.
pub fn cargo_metadata_with_dependencies() -> Result<serde_json::Value, String> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let version = std::process::Command::new(rustc)
        .arg("-vV")
        .output()
        .map_err(|error| format!("cannot run rustc: {error}"))?;
    let host = String::from_utf8_lossy(&version.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(str::to_owned))
        .ok_or("rustc -vV names no host")?;
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = std::process::Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--offline"])
        .args(["--filter-platform", &host])
        .output()
        .map_err(|error| format!("cannot run cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("cargo metadata returned no JSON: {error}"))
}

/// The number of crates that the package `name` pulls in through normal dependencies, directly
/// or not (dev and build dependencies do not count; the package itself does not either), from the
/// output of `cargo metadata` with the dependency graph. `None` if the package is unknown.
pub fn normal_dependency_count(metadata: &serde_json::Value, name: &str) -> Option<usize> {
    let nodes = metadata["resolve"]["nodes"].as_array()?;
    let by_id: HashMap<&str, &serde_json::Value> = nodes
        .iter()
        .filter_map(|node| Some((node["id"].as_str()?, node)))
        .collect();
    // Package ids differ between cargo versions; match the name by the metadata's packages
    // when it has them, else by the id itself.
    let id = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|package| package["name"] == name)
        .and_then(|package| package["id"].as_str())
        .or_else(|| by_id.contains_key(name).then_some(name))?;
    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![id];
    while let Some(current) = stack.pop() {
        for dep in by_id.get(current)?["deps"].as_array().into_iter().flatten() {
            let normal = dep["dep_kinds"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|kind| kind["kind"].is_null());
            if let Some(pkg) = dep["pkg"].as_str().filter(|_| normal) {
                if seen.insert(pkg) {
                    stack.push(pkg);
                }
            }
        }
    }
    Some(seen.len())
}
