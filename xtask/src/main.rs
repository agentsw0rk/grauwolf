//! `cargo xtask <command>` – development tools.

use std::process::ExitCode;

use xtask::check::{Command, Mode, full_plan, run};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match xtask::check::parse_args(&args) {
        Ok(Command::CheckArch) => check_arch(),
        Ok(Command::Check(Mode::Full)) => match xtask::arch::workspace_root() {
            Ok(root) => exit_code(run(&full_plan(), &root)),
            Err(error) => fail(&error),
        },
        Ok(Command::Check(Mode::Affected { base })) => match xtask::check::affected(&base) {
            Ok((root, plan)) => exit_code(run(&plan, &root)),
            Err(error) => fail(&error),
        },
        Err(usage) => fail(&usage),
    }
}

fn check_arch() -> ExitCode {
    match xtask::arch::workspace_violations() {
        Ok((count, violations)) => {
            if violations.is_empty() {
                println!("Architecture ok: {count} crates checked.");
                ExitCode::SUCCESS
            } else {
                eprintln!("Architecture violations (ADR-0016):");
                for violation in violations {
                    eprintln!("  - {violation}");
                }
                ExitCode::FAILURE
            }
        }
        Err(error) => fail(&error),
    }
}

fn exit_code(success: bool) -> ExitCode {
    if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("{message}");
    ExitCode::FAILURE
}
