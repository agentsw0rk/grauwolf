//! Rules of the layered architecture (ADR-0016).

use xtask::arch::{Dependency, DependencyKind, Layer, Package, check, facade_logic_violations};

fn package(name: &str, layer: Layer, deps: &[(&str, DependencyKind)]) -> Package {
    Package {
        name: name.into(),
        layer,
        dependencies: deps
            .iter()
            .map(|(name, kind)| Dependency {
                name: (*name).into(),
                kind: *kind,
            })
            .collect(),
    }
}

#[test]
fn domain_must_not_depend_on_adapters() {
    let packages = [
        package(
            "grauwolf-llm",
            Layer::Domain,
            &[("grauwolf-openai", DependencyKind::Normal)],
        ),
        package("grauwolf-openai", Layer::Adapters, &[]),
    ];

    let violations = check(&packages);

    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("grauwolf-llm") && violations[0].contains("grauwolf-openai"));
}

#[test]
fn domain_uses_only_allowed_external_crates_and_the_testkit_only_for_tests() {
    let packages = [
        package(
            "grauwolf-agents",
            Layer::Domain,
            &[
                ("serde", DependencyKind::Normal),
                ("tokio", DependencyKind::Normal),
                ("grauwolf-test", DependencyKind::Normal),
                ("grauwolf-codegen", DependencyKind::Normal),
                ("futures-executor", DependencyKind::Dev),
            ],
        ),
        package("grauwolf-test", Layer::Support, &[]),
        package("grauwolf-codegen", Layer::Support, &[]),
    ];

    let violations = check(&packages);

    assert_eq!(violations.len(), 2, "{violations:?}");
    assert!(violations.iter().any(|v| v.contains("tokio")));
    assert!(violations.iter().any(|v| v.contains("grauwolf-test")));
}

#[test]
fn support_crates_stay_generic_except_the_testkit_on_domain() {
    let packages = [
        package(
            "grauwolf-codegen",
            Layer::Support,
            &[("grauwolf-spec", DependencyKind::Normal)],
        ),
        package(
            "grauwolf-test",
            Layer::Support,
            &[
                ("grauwolf-llm", DependencyKind::Normal),
                ("grauwolf-openai", DependencyKind::Normal),
            ],
        ),
        package("grauwolf-spec", Layer::Domain, &[]),
        package("grauwolf-llm", Layer::Domain, &[]),
        package("grauwolf-openai", Layer::Adapters, &[]),
    ];

    let violations = check(&packages);

    assert_eq!(violations.len(), 2, "{violations:?}");
    assert!(
        violations
            .iter()
            .any(|v| v.contains("grauwolf-codegen") && v.contains("grauwolf-spec"))
    );
    assert!(
        violations
            .iter()
            .any(|v| v.contains("grauwolf-test") && v.contains("grauwolf-openai"))
    );
}

#[test]
fn adapters_must_not_depend_on_apps() {
    let packages = [
        package(
            "grauwolf-openai",
            Layer::Adapters,
            &[
                ("grauwolf-llm-http", DependencyKind::Normal),
                ("grauwolf", DependencyKind::Normal),
            ],
        ),
        package("grauwolf-llm-http", Layer::Adapters, &[]),
        package("grauwolf", Layer::Apps, &[]),
    ];

    let violations = check(&packages);

    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("grauwolf-openai")
            && violations[0].contains("depend on grauwolf (dependency)")
    );
}

use DependencyKind::{Build, Dev, Normal};

fn apps(name: &str, deps: &[(&str, DependencyKind)]) -> Package {
    package(name, Layer::Apps, deps)
}

#[test]
fn the_app_order_is_host_then_server_then_cli_then_nothing_above() {
    let packages = [
        apps(
            "grauwolf-host",
            &[
                ("grauwolf-server", Normal),
                ("grauwolf-cli", Dev),
                ("grauwolf", Normal),
            ],
        ),
        apps(
            "grauwolf-server",
            &[
                ("grauwolf-host", Normal),
                ("grauwolf-cli", Normal),
                ("grauwolf", Dev),
            ],
        ),
        apps(
            "grauwolf-cli",
            &[
                ("grauwolf-host", Normal),
                ("grauwolf-server", Normal),
                ("grauwolf", Build),
            ],
        ),
        apps(
            "grauwolf",
            &[("grauwolf-host", Normal), ("grauwolf-server", Normal)],
        ),
    ];

    let violations = check(&packages);

    assert_eq!(violations.len(), 6, "{violations:#?}");
    for edge in [
        "grauwolf-host (Apps) must not depend on grauwolf-server (dependency)",
        "grauwolf-host (Apps) must not depend on grauwolf-cli (dev-dependency)",
        "grauwolf-host (Apps) must not depend on grauwolf (dependency)",
        "grauwolf-server (Apps) must not depend on grauwolf-cli (dependency)",
        "grauwolf-server (Apps) must not depend on grauwolf (dev-dependency)",
        "grauwolf-cli (Apps) must not depend on grauwolf (build-dependency)",
    ] {
        assert!(
            violations
                .iter()
                .any(|v| v.starts_with(edge) && v.contains("app order")),
            "missing: {edge}\n{violations:#?}"
        );
    }
}

#[test]
fn the_allowed_app_edges_pass() {
    let packages = [
        apps("grauwolf-host", &[]),
        apps(
            "grauwolf-server",
            &[("grauwolf-host", Normal), ("grauwolf-host", Dev)],
        ),
        apps(
            "grauwolf-cli",
            &[("grauwolf-host", Normal), ("grauwolf-server", Normal)],
        ),
        apps(
            "grauwolf",
            &[("grauwolf-host", Normal), ("grauwolf-server", Normal)],
        ),
    ];

    assert!(check(&packages).is_empty());
}

#[test]
fn lower_layers_never_depend_on_app_crates_not_even_for_tests() {
    let mut packages = vec![
        package("grauwolf-llm", Layer::Domain, &[("grauwolf-host", Dev)]),
        package(
            "grauwolf-openai",
            Layer::Adapters,
            &[("grauwolf-server", Normal)],
        ),
        package(
            "grauwolf-test",
            Layer::Support,
            &[("grauwolf", Dev), ("grauwolf-cli", Build)],
        ),
    ];
    for name in [
        "grauwolf-host",
        "grauwolf-server",
        "grauwolf-cli",
        "grauwolf",
    ] {
        packages.push(apps(name, &[]));
    }

    let violations = check(&packages);

    assert_eq!(violations.len(), 4, "{violations:#?}");
    assert!(
        violations
            .iter()
            .all(|v| v.contains("never depend on the app crates"))
    );
    assert!(violations.iter().any(|v| {
        v.starts_with("grauwolf-llm (Domain) must not depend on grauwolf-host (dev-dependency)")
    }));
}

#[test]
fn the_facade_may_contain_re_exports_macros_and_features_only() {
    let clean = "//! The fn and struct of docs do not count.\n\
        pub use grauwolf_agents as agents;\n\
        #[cfg(feature = \"macros\")]\n\
        pub use grauwolf_macros::{facade_tool as tool, facade_toolset as toolset};\n";
    let logic =
        "pub use grauwolf_agents as agents;\npub fn helper() -> u8 { 1 } // fn in a comment\n";

    assert!(facade_logic_violations(&[("src/lib.rs".into(), clean.into())]).is_empty());
    let violations = facade_logic_violations(&[("src/lib.rs".into(), logic.into())]);

    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("src/lib.rs:2") && violations[0].contains("`fn`"));
    assert!(violations[0].contains("no logic"));
}

fn domain(name: &str, deps: &[(&str, DependencyKind)]) -> Package {
    package(name, Layer::Domain, deps)
}

#[test]
fn the_spec_crates_depend_only_downwards() {
    let packages = [
        domain("grauwolf-graph", &[("grauwolf-spec-model", Normal)]),
        domain(
            "grauwolf-spec-model",
            &[
                ("grauwolf-strategies", Normal),
                ("grauwolf-spec", Dev),
                ("grauwolf-graph", Normal),
            ],
        ),
        domain(
            "grauwolf-strategies",
            &[
                ("grauwolf-spec", Build),
                ("grauwolf-spec-model", Normal),
                ("grauwolf-graph", Normal),
            ],
        ),
        domain("grauwolf-spec", &[]),
    ];

    let violations = check(&packages);

    assert_eq!(violations.len(), 4, "{violations:#?}");
    for edge in [
        "grauwolf-graph must not depend on grauwolf-spec-model (Normal)",
        "grauwolf-spec-model must not depend on grauwolf-strategies (Normal)",
        "grauwolf-spec-model must not depend on grauwolf-spec (Dev)",
        "grauwolf-strategies must not depend on grauwolf-spec (Build)",
    ] {
        assert!(
            violations
                .iter()
                .any(|v| v.starts_with(edge) && v.contains("rule spec crates")),
            "missing: {edge}\n{violations:#?}"
        );
    }
}

#[test]
fn the_allowed_spec_crate_edges_pass() {
    let packages = [
        domain("grauwolf-graph", &[]),
        domain("grauwolf-spec-model", &[("grauwolf-graph", Normal)]),
        domain(
            "grauwolf-strategies",
            &[
                ("grauwolf-spec-model", Normal),
                ("grauwolf-graph", Normal),
                ("grauwolf-spec-model", Dev),
            ],
        ),
        domain(
            "grauwolf-spec",
            &[
                ("grauwolf-strategies", Normal),
                ("grauwolf-spec-model", Normal),
                ("grauwolf-graph", Normal),
            ],
        ),
    ];

    assert!(check(&packages).is_empty());
}

#[test]
fn the_tools_program_depends_on_no_other_app_crate() {
    let packages = [
        apps("grauwolf-host", &[]),
        apps("grauwolf-server", &[("grauwolf-host", Normal)]),
        apps(
            "grauwolf-tools-app",
            &[
                ("grauwolf-host", Normal),
                ("grauwolf-server", Dev),
                ("grauwolf-cli", Build),
                ("grauwolf", Normal),
            ],
        ),
        apps("grauwolf-cli", &[]),
        apps("grauwolf", &[]),
    ];

    let violations = check(&packages);

    assert_eq!(violations.len(), 4, "{violations:#?}");
    for edge in [
        "grauwolf-tools-app (Apps) must not depend on grauwolf-host (dependency)",
        "grauwolf-tools-app (Apps) must not depend on grauwolf-server (dev-dependency)",
        "grauwolf-tools-app (Apps) must not depend on grauwolf-cli (build-dependency)",
        "grauwolf-tools-app (Apps) must not depend on grauwolf (dependency)",
    ] {
        assert!(
            violations
                .iter()
                .any(|v| v.starts_with(edge) && v.contains("grauwolf-tools")),
            "missing: {edge}\n{violations:#?}"
        );
    }
}

#[test]
fn the_cli_may_depend_on_the_tools_program_but_no_lower_layer_may() {
    let packages = [
        apps("grauwolf-tools-app", &[]),
        apps("grauwolf-cli", &[("grauwolf-tools-app", Normal)]),
        apps("grauwolf-host", &[("grauwolf-tools-app", Normal)]),
        package(
            "grauwolf-toolpkg",
            Layer::Adapters,
            &[("grauwolf-tools-app", Dev)],
        ),
    ];

    let violations = check(&packages);

    assert_eq!(violations.len(), 2, "{violations:#?}");
    assert!(violations.iter().any(|v| v.starts_with("grauwolf-host")));
    assert!(violations.iter().any(|v| v.starts_with("grauwolf-toolpkg")));
}

#[test]
fn the_dependency_closure_counts_normal_dependencies_only() {
    use serde_json::json;
    let metadata = json!({ "resolve": { "nodes": [
        { "id": "app", "deps": [
            { "pkg": "a", "dep_kinds": [{ "kind": null }] },
            { "pkg": "dev-only", "dep_kinds": [{ "kind": "dev" }] },
            { "pkg": "b", "dep_kinds": [{ "kind": "dev" }, { "kind": null }] } ] },
        { "id": "a", "deps": [ { "pkg": "c", "dep_kinds": [{ "kind": null }] } ] },
        { "id": "b", "deps": [ { "pkg": "c", "dep_kinds": [{ "kind": null }] } ] },
        { "id": "c", "deps": [ { "pkg": "build-only", "dep_kinds": [{ "kind": "build" }] } ] },
        { "id": "dev-only", "deps": [] }, { "id": "build-only", "deps": [] }
    ] } });

    // a, b, c: the crate itself, dev and build dependencies do not count.
    assert_eq!(
        xtask::arch::normal_dependency_count(&metadata, "app"),
        Some(3)
    );
    assert_eq!(
        xtask::arch::normal_dependency_count(&metadata, "nope"),
        None
    );
}

#[test]
fn the_operator_program_depends_on_no_other_app_crate_and_nothing_depends_on_it() {
    let packages = [
        apps(
            "grauwolf-k8-ops-app",
            &[("grauwolf-host", Normal), ("grauwolf-tools-app", Dev)],
        ),
        apps("grauwolf-cli", &[("grauwolf-k8-ops-app", Normal)]),
        apps("grauwolf-host", &[]),
        apps("grauwolf-tools-app", &[]),
        package(
            "grauwolf-k8s",
            Layer::Adapters,
            &[("grauwolf-k8-ops-app", Dev)],
        ),
    ];

    let violations = check(&packages);

    assert_eq!(violations.len(), 4, "{violations:#?}");
    for edge in [
        "grauwolf-k8-ops-app (Apps) must not depend on grauwolf-host (dependency)",
        "grauwolf-k8-ops-app (Apps) must not depend on grauwolf-tools-app (dev-dependency)",
        "grauwolf-cli (Apps) must not depend on grauwolf-k8-ops-app (dependency)",
        "grauwolf-k8s (Adapters) must not depend on grauwolf-k8-ops-app (dev-dependency)",
    ] {
        assert!(
            violations.iter().any(|v| v.starts_with(edge)),
            "missing: {edge}\n{violations:#?}"
        );
    }
}

#[test]
fn the_studio_may_use_host_and_server_but_nothing_depends_on_it() {
    let packages = [
        apps(
            "grauwolf-studio",
            &[
                ("grauwolf-host", Normal),
                ("grauwolf-server", Normal),
                ("grauwolf-cli", Dev),
            ],
        ),
        apps("grauwolf-cli", &[("grauwolf-studio", Normal)]),
        apps("grauwolf-host", &[]),
        apps("grauwolf-server", &[]),
        package("grauwolf-spec", Layer::Domain, &[("grauwolf-studio", Dev)]),
    ];

    let violations = check(&packages);

    assert_eq!(violations.len(), 3, "{violations:#?}");
    for edge in [
        "grauwolf-studio (Apps) must not depend on grauwolf-cli (dev-dependency)",
        "grauwolf-cli (Apps) must not depend on grauwolf-studio (dependency)",
        "grauwolf-spec (Domain) must not depend on grauwolf-studio (dev-dependency)",
    ] {
        assert!(
            violations.iter().any(|v| v.starts_with(edge)),
            "missing: {edge}\n{violations:#?}"
        );
    }
}
