//! Selection of the packages a change affects (ADR-0108 stage 1).

use xtask::affected::{Member, Selection, changed_paths, members_from_metadata, select};

fn member(name: &str, dir: &str, deps: &[&str]) -> Member {
    Member {
        name: name.into(),
        dir: dir.into(),
        dependencies: deps.iter().map(|dep| (*dep).into()).collect(),
    }
}

fn paths(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).into()).collect()
}

/// `prompt` <- `llm` <- `cli`, plus an unrelated `rag`.
fn workspace() -> Vec<Member> {
    vec![
        member("grauwolf-prompt", "crates/domain/grauwolf-prompt", &[]),
        member(
            "grauwolf-llm",
            "crates/domain/grauwolf-llm",
            &["grauwolf-prompt"],
        ),
        member(
            "grauwolf-cli",
            "crates/apps/grauwolf-cli",
            &["grauwolf-llm"],
        ),
        member("grauwolf-rag", "crates/domain/grauwolf-rag", &[]),
    ]
}

fn names(selection: &Selection) -> Vec<&str> {
    match selection {
        Selection::Packages(packages) => packages.keys().map(String::as_str).collect(),
        Selection::All(_) => panic!("expected packages, got {selection:?}"),
    }
}

#[test]
fn a_changed_file_selects_the_package_that_contains_it() {
    let selection = select(
        &workspace(),
        &paths(&["crates/domain/grauwolf-rag/src/lib.rs"]),
    );

    assert_eq!(names(&selection), ["grauwolf-rag"]);
}

#[test]
fn packages_that_depend_on_a_changed_package_are_selected_with_the_reason() {
    let selection = select(
        &workspace(),
        &paths(&["crates/domain/grauwolf-prompt/src/lib.rs"]),
    );

    assert_eq!(
        names(&selection),
        ["grauwolf-cli", "grauwolf-llm", "grauwolf-prompt"]
    );
    let Selection::Packages(packages) = selection else {
        unreachable!()
    };
    assert_eq!(
        packages["grauwolf-prompt"],
        "changed: crates/domain/grauwolf-prompt/src/lib.rs"
    );
    assert_eq!(packages["grauwolf-llm"], "depends on grauwolf-prompt");
    assert_eq!(packages["grauwolf-cli"], "depends on grauwolf-llm");
}

#[test]
fn dev_dependencies_count_as_dependencies() {
    // The member list does not distinguish kinds: a dev-dependency is a dependency too.
    let mut members = workspace();
    members[3] = member(
        "grauwolf-rag",
        "crates/domain/grauwolf-rag",
        &["grauwolf-prompt"],
    );

    let selection = select(
        &members,
        &paths(&["crates/domain/grauwolf-prompt/Cargo.toml"]),
    );

    assert!(names(&selection).contains(&"grauwolf-rag"));
}

#[test]
fn root_files_select_every_package() {
    for file in [
        "Cargo.toml",
        "Cargo.lock",
        ".cargo/config.toml",
        "rust-toolchain.toml",
        "xtask/src/main.rs",
    ] {
        let selection = select(&workspace(), &paths(&[file]));

        assert_eq!(
            selection,
            Selection::All(format!("changed: {file}")),
            "{file}"
        );
    }
}

#[test]
fn doc_examples_select_the_host_package() {
    let mut members = workspace();
    members.push(member("grauwolf-host", "crates/apps/grauwolf-host", &[]));

    for file in [
        "docs/user-guide/examples/01-hello.json",
        "docs/handbuch/beispiele/01-hallo.json",
    ] {
        let selection = select(&members, &paths(&[file]));

        assert_eq!(names(&selection), ["grauwolf-host"], "{file}");
    }
}

#[test]
fn example_specs_select_the_packages_whose_tests_read_them() {
    let members = [
        member("grauwolf-spec", "crates/domain/grauwolf-spec", &[]),
        member("grauwolf-rag", "crates/domain/grauwolf-rag", &[]),
        member(
            "grauwolf-interpreter",
            "crates/domain/grauwolf-interpreter",
            &["grauwolf-spec"],
        ),
    ];

    let selection = select(&members, &paths(&["examples/specs/calculator.json"]));

    assert_eq!(names(&selection), ["grauwolf-interpreter", "grauwolf-spec"]);
}

#[test]
fn the_studio_openapi_description_selects_the_studio() {
    let mut members = workspace();
    members.push(member(
        "grauwolf-studio",
        "crates/apps/grauwolf-studio",
        &[],
    ));

    let selection = select(&members, &paths(&["studio/web/lib/api/openapi.json"]));

    assert_eq!(names(&selection), ["grauwolf-studio"]);
}

#[test]
fn other_documents_select_no_package() {
    let selection = select(&workspace(), &paths(&["docs/adr/0108-x.md", "README.md"]));

    assert_eq!(selection, Selection::Packages(Default::default()));
}

#[test]
fn changed_paths_combine_git_diff_and_status_without_duplicates() {
    let diff = "crates/a/src/lib.rs\ndocs/x.md\n";
    let status = " M crates/a/src/lib.rs\nA  new.rs\n?? untracked.txt\nR  old.rs -> renamed.rs\n D gone.rs\n?? \"with space.md\"\n";

    assert_eq!(
        changed_paths(diff, status),
        [
            "crates/a/src/lib.rs",
            "docs/x.md",
            "gone.rs",
            "new.rs",
            "old.rs",
            "renamed.rs",
            "untracked.txt",
            "with space.md",
        ]
    );
}

#[test]
fn members_come_from_cargo_metadata_with_workspace_dependencies_only() {
    let metadata = serde_json::json!({
        "workspace_root": "/w",
        "packages": [
            {
                "name": "grauwolf-llm",
                "manifest_path": "/w/crates/domain/grauwolf-llm/Cargo.toml",
                "dependencies": [
                    { "name": "grauwolf-prompt", "kind": null },
                    { "name": "grauwolf-test", "kind": "dev" },
                    { "name": "serde", "kind": null }
                ]
            },
            {
                "name": "grauwolf-prompt",
                "manifest_path": "/w/crates/domain/grauwolf-prompt/Cargo.toml",
                "dependencies": []
            },
            {
                "name": "grauwolf-test",
                "manifest_path": "/w/crates/support/grauwolf-test/Cargo.toml",
                "dependencies": []
            }
        ]
    });

    let members = members_from_metadata(&metadata);

    assert_eq!(
        members[0],
        member(
            "grauwolf-llm",
            "crates/domain/grauwolf-llm",
            &["grauwolf-prompt", "grauwolf-test"]
        )
    );
    assert_eq!(members.len(), 3);
}

#[test]
fn the_summary_names_every_selected_package_with_its_reason() {
    let selection = select(
        &workspace(),
        &paths(&["crates/domain/grauwolf-prompt/src/lib.rs"]),
    );

    assert_eq!(
        selection.summary(),
        "3 packages selected:\n  grauwolf-cli (depends on grauwolf-llm)\n  grauwolf-llm (depends on grauwolf-prompt)\n  grauwolf-prompt (changed: crates/domain/grauwolf-prompt/src/lib.rs)"
    );
    assert_eq!(
        select(&workspace(), &paths(&["README.md"])).summary(),
        "no packages selected (no source changes)"
    );
    assert_eq!(
        select(&workspace(), &paths(&["Cargo.lock"])).summary(),
        "all packages selected (changed: Cargo.lock)"
    );
}
