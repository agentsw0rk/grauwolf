//! Command line and plans of `cargo xtask check` (ADR-0108 stage 1).

use xtask::affected::Selection;
use xtask::check::{Command, FEATURE_MATRIX, Mode, Step, affected_plan, full_plan, parse_args};

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).into()).collect()
}

#[test]
fn check_arch_stays_a_command() {
    assert_eq!(parse_args(&args(&["check-arch"])), Ok(Command::CheckArch));
}

#[test]
fn check_full_runs_everything() {
    assert_eq!(
        parse_args(&args(&["check", "--full"])),
        Ok(Command::Check(Mode::Full))
    );
}

#[test]
fn check_affected_compares_with_head_unless_told_otherwise() {
    assert_eq!(
        parse_args(&args(&["check", "--affected"])),
        Ok(Command::Check(Mode::Affected {
            base: "HEAD".into()
        }))
    );
    assert_eq!(
        parse_args(&args(&["check", "--affected", "--base", "main"])),
        Ok(Command::Check(Mode::Affected {
            base: "main".into()
        }))
    );
}

#[test]
fn check_needs_exactly_one_mode() {
    for bad in [
        &["check"][..],
        &["check", "--full", "--affected"],
        &["check", "--full", "--base", "main"],
        &["check", "--affected", "--base"],
        &["check", "--fast"],
        &[],
    ] {
        let error = parse_args(&args(bad)).unwrap_err();
        assert!(error.contains("Usage: cargo xtask"), "{bad:?}: {error}");
    }
}

#[test]
fn the_full_plan_runs_every_check_and_the_slow_tier() {
    let plan = full_plan();
    let workspace_steps: Vec<Step> = plan
        .iter()
        .filter(|step| !matches!(step, Step::Cargo { title, .. } if title.starts_with("features")))
        .cloned()
        .collect();

    assert_eq!(
        workspace_steps,
        [
            Step::Cargo {
                title: "fmt".into(),
                args: args(&["fmt", "--all", "--check"])
            },
            Step::Cargo {
                title: "clippy".into(),
                args: args(&[
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings"
                ])
            },
            Step::Cargo {
                title: "test".into(),
                args: args(&["test", "--workspace", "--", "--include-ignored"])
            },
            Step::Arch,
        ]
    );
}

#[test]
fn the_full_plan_lints_and_tests_every_build_of_the_feature_matrix_before_the_layer_rules() {
    let plan = full_plan();
    let matrix_steps: Vec<&Step> = plan
        .iter()
        .filter(|step| matches!(step, Step::Cargo { title, .. } if title.starts_with("features")))
        .collect();
    let expected: Vec<Step> = FEATURE_MATRIX
        .iter()
        .flat_map(|(package, features)| {
            let mut selection = vec!["-p", package, "--no-default-features"];
            let joined = features.join(",");
            if !features.is_empty() {
                selection.extend(["--features", joined.as_str()]);
            }
            let label = if features.is_empty() {
                format!("features {package}")
            } else {
                format!("features {package} +{joined}")
            };
            [
                Step::Cargo {
                    title: format!("{label} clippy"),
                    args: args(
                        &[
                            &["clippy"][..],
                            &selection,
                            &["--all-targets", "--", "-D", "warnings"],
                        ]
                        .concat(),
                    ),
                },
                Step::Cargo {
                    title: format!("{label} test"),
                    args: args(&[&["test"][..], &selection].concat()),
                },
            ]
        })
        .collect();

    assert_eq!(matrix_steps, expected.iter().collect::<Vec<_>>());
    assert!(FEATURE_MATRIX.contains(&("grauwolf-host", &["builtin"][..])));
    assert_eq!(plan.last(), Some(&Step::Arch));
}

fn selected(names: &[&str]) -> Selection {
    Selection::Packages(
        names
            .iter()
            .map(|name| ((*name).to_owned(), "changed".to_owned()))
            .collect(),
    )
}

#[test]
fn the_affected_plan_checks_and_tests_only_the_selected_packages() {
    let plan = affected_plan(&selected(&["grauwolf-cli", "grauwolf-llm"]));

    assert_eq!(
        plan,
        [
            Step::Cargo {
                title: "fmt".into(),
                args: args(&["fmt", "--all", "--check"])
            },
            Step::Cargo {
                title: "clippy".into(),
                args: args(&[
                    "clippy",
                    "-p",
                    "grauwolf-cli",
                    "-p",
                    "grauwolf-llm",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings"
                ])
            },
            Step::Cargo {
                title: "test".into(),
                args: args(&["test", "-p", "grauwolf-cli", "-p", "grauwolf-llm"])
            },
            Step::Arch,
        ]
    );
}

#[test]
fn the_affected_plan_for_documents_only_checks_the_format_and_the_layers() {
    let plan = affected_plan(&selected(&[]));

    assert_eq!(plan.len(), 2);
    assert_eq!(plan[1], Step::Arch);
}

#[test]
fn the_affected_plan_for_workspace_files_covers_the_whole_workspace_without_the_slow_tier() {
    let plan = affected_plan(&Selection::All("changed: Cargo.toml".into()));

    assert_eq!(
        plan[2],
        Step::Cargo {
            title: "test".into(),
            args: args(&["test", "--workspace"])
        }
    );
}
