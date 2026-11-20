//! `cargo xtask check`: the commit check as one command (ADR-0108 stage 1) – the command line,
//! the plan of steps for a run and the runner.

use crate::affected::{Selection, changed_paths, members_from_metadata, select};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// How much of the workspace a check covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Everything, including the slow tier of the tests.
    Full,
    /// The packages changed since `base` and those that depend on them, fast tier only.
    Affected {
        /// Git revision to compare with.
        base: String,
    },
}

/// A command of the tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `check-arch`: the layer rules (ADR-0016).
    CheckArch,
    /// `check --full` or `check --affected [--base <rev>]`.
    Check(Mode),
}

const USAGE: &str =
    "Usage: cargo xtask check-arch | check --full | check --affected [--base <rev>]";

/// Reads the command line (without the program name).
pub fn parse_args(args: &[String]) -> Result<Command, String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["check-arch"] => Ok(Command::CheckArch),
        ["check", "--full"] => Ok(Command::Check(Mode::Full)),
        ["check", "--affected"] => Ok(Command::Check(Mode::Affected {
            base: "HEAD".into(),
        })),
        ["check", "--affected", "--base", base] => Ok(Command::Check(Mode::Affected {
            base: (*base).into(),
        })),
        _ => Err(USAGE.into()),
    }
}

/// One step of a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// A cargo command; `title` names it in the output.
    Cargo {
        /// Short name shown in the output.
        title: String,
        /// Arguments of `cargo`.
        args: Vec<String>,
    },
    /// The layer rules (ADR-0016), checked in this process.
    Arch,
}

impl Step {
    fn cargo(title: impl Into<String>, args: &[&str]) -> Step {
        Step::Cargo {
            title: title.into(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        }
    }

    fn title(&self) -> &str {
        match self {
            Step::Cargo { title, .. } => title,
            Step::Arch => "check-arch",
        }
    }
}

/// Builds with optional features switched off: package and the features left on (all others,
/// including the defaults, are off). The workspace build unifies features, so code behind
/// `#[cfg(not(feature = …))]` – the "this build has no …" errors and their tests – and the
/// `required-features` of integration tests are only compiled and run here.
pub const FEATURE_MATRIX: &[(&str, &[&str])] = &[
    ("grauwolf-host", &[]),
    ("grauwolf-host", &["builtin"]),
    ("grauwolf-server", &[]),
    ("grauwolf-server", &["service"]),
    ("grauwolf", &[]),
];

/// Lints (all targets) and fast tests of one build of [`FEATURE_MATRIX`].
fn feature_build_steps(package: &str, features: &[&str]) -> [Step; 2] {
    let joined = features.join(",");
    let mut selection = vec!["-p", package, "--no-default-features"];
    let label = if features.is_empty() {
        format!("features {package}")
    } else {
        selection.extend(["--features", joined.as_str()]);
        format!("features {package} +{joined}")
    };
    let clippy = [
        &["clippy"][..],
        &selection,
        &["--all-targets", "--", "-D", "warnings"],
    ]
    .concat();
    let test = [&["test"][..], &selection].concat();
    [
        Step::cargo(format!("{label} clippy"), &clippy),
        Step::cargo(format!("{label} test"), &test),
    ]
}

/// The steps of `check --full`: format, lints, all tests including the slow tier, lints and
/// fast tests of every build of the [`FEATURE_MATRIX`], layer rules.
pub fn full_plan() -> Vec<Step> {
    let matrix = FEATURE_MATRIX
        .iter()
        .flat_map(|(package, features)| feature_build_steps(package, features));
    let mut steps = vec![
        Step::cargo("fmt", &["fmt", "--all", "--check"]),
        Step::cargo(
            "clippy",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        ),
        Step::cargo("test", &["test", "--workspace", "--", "--include-ignored"]),
    ];
    steps.extend(matrix);
    steps.push(Step::Arch);
    steps
}

/// The steps of `check --affected`: format, then lints and the fast tier of the tests for the
/// selected packages only (the whole workspace when workspace-wide files changed; no packages
/// when only documents changed), then the layer rules.
pub fn affected_plan(selection: &Selection) -> Vec<Step> {
    let scope: Vec<&str> = match selection {
        Selection::All(_) => vec!["--workspace"],
        Selection::Packages(packages) => packages
            .keys()
            .flat_map(|name| ["-p", name.as_str()])
            .collect(),
    };
    let mut steps = vec![Step::cargo("fmt", &["fmt", "--all", "--check"])];
    if !scope.is_empty() {
        let clippy = [
            &["clippy"][..],
            &scope,
            &["--all-targets", "--", "-D", "warnings"],
        ]
        .concat();
        let test = [&["test"][..], &scope].concat();
        steps.push(Step::cargo("clippy", &clippy));
        steps.push(Step::cargo("test", &test));
    }
    steps.push(Step::Arch);
    steps
}

/// The plan for `check --affected`: reads the workspace and the changes since `base` (the
/// working tree and the index are included, as are untracked files), prints which packages are
/// selected and why, and returns the workspace root with the steps.
pub fn affected(base: &str) -> Result<(PathBuf, Vec<Step>), String> {
    let metadata = crate::arch::cargo_metadata()?;
    let root = PathBuf::from(metadata["workspace_root"].as_str().unwrap_or_default());
    let diff = git(&root, &["diff", "--name-only", base])?;
    let status = git(&root, &["status", "--porcelain", "-uall"])?;
    let changed = changed_paths(&diff, &status);
    let selection = select(&members_from_metadata(&metadata), &changed);
    println!("{} changed files; {}", changed.len(), selection.summary());
    Ok((root, affected_plan(&selection)))
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

/// Runs the steps in order and stops at the first failure. Prints one line per step and the
/// output of the failed step. Returns whether every step passed.
pub fn run(steps: &[Step], workspace_root: &Path) -> bool {
    for step in steps {
        print!("{:<11} ", step.title());
        let _ = std::io::stdout().flush();
        let started = Instant::now();
        match execute(step, workspace_root) {
            Ok(()) => println!("ok ({}s)", started.elapsed().as_secs()),
            Err(output) => {
                println!("FAILED ({}s)", started.elapsed().as_secs());
                eprintln!("{output}");
                return false;
            }
        }
    }
    true
}

/// Runs one step; the error is what the step printed.
fn execute(step: &Step, workspace_root: &Path) -> Result<(), String> {
    match step {
        Step::Cargo { args, .. } => {
            let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
            let output = std::process::Command::new(cargo)
                .args(args)
                .current_dir(workspace_root)
                .output()
                .map_err(|error| format!("cannot run cargo: {error}"))?;
            if output.status.success() {
                Ok(())
            } else {
                Err(format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ))
            }
        }
        Step::Arch => {
            let (_, violations) = crate::arch::workspace_violations()?;
            if violations.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "Architecture violations (ADR-0016):\n  - {}",
                    violations.join("\n  - ")
                ))
            }
        }
    }
}
