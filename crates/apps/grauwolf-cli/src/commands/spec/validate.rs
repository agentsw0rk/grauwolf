//! `grauwolf spec validate <file>`: check errors, unknown providers and warnings over the whole
//! agent tree – the spec and its local sub-agents (ADR-0080, ADR-0115).

use std::path::Path;
use std::process::ExitCode;

use crate::output::{Io, errln, outln, report};

/// `grauwolf spec validate <file>`.
pub fn run(io: &Io, file: &Path) -> ExitCode {
    match grauwolf_host::agent_tree::read(file) {
        Ok(tree) => {
            for warning in tree.warnings() {
                errln!(io, "{warning}");
            }
            outln!(io, "{}: valid", file.display());
            ExitCode::SUCCESS
        }
        Err(failure) => {
            for warning in &failure.warnings {
                errln!(io, "{warning}");
            }
            report(io, Err(failure.to_string()))
        }
    }
}
