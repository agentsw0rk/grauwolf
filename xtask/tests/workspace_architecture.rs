//! The real workspace follows the layer rules (ADR-0016).

use xtask::arch::{Layer, check, workspace_packages};

#[test]
fn the_workspace_follows_the_layer_rules() {
    let packages = workspace_packages().unwrap();

    assert!(
        packages
            .iter()
            .any(|p| p.name == "grauwolf-graph" && p.layer == Layer::Domain),
        "Layers were not recognized: {packages:?}"
    );
    let violations = check(&packages);
    assert!(
        violations.is_empty(),
        "Architecture violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_facade_contains_only_re_exports() {
    let violations = xtask::arch::facade_violations().unwrap();
    assert!(
        violations.is_empty(),
        "Facade violations:\n{}",
        violations.join("\n")
    );
}

/// ADR-0118: the lean tool program stays lean. Measured on 2026-11-13 (see the ADR): 61%
/// of the crates of the CLI in this graph. The limit is a share of the CLI's own count, so it moves with
/// the dependencies both share; an accidental dependency on the host adds the model providers,
/// the sandbox, MCP, the HTTP server and more, and exceeds it by far.
#[test]
fn the_tools_program_has_far_fewer_dependencies_than_the_cli() {
    let metadata = xtask::arch::cargo_metadata_with_dependencies().unwrap();
    let count = |name: &str| {
        xtask::arch::normal_dependency_count(&metadata, name)
            .unwrap_or_else(|| panic!("{name} is not in the workspace"))
    };
    let (tools, cli) = (count("grauwolf-tools-app"), count("grauwolf-cli"));
    assert!(
        tools * 100 <= cli * xtask::arch::TOOLS_APP_MAX_PERCENT_OF_CLI,
        "grauwolf-tools-app has {tools} normal dependencies, grauwolf-cli {cli}: more than {}%          (a dependency on the host or an adapter the tool commands do not need?)",
        xtask::arch::TOOLS_APP_MAX_PERCENT_OF_CLI
    );
}
