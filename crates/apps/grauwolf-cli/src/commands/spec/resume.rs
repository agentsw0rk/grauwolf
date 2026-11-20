//! `grauwolf spec resume`: continues an interrupted run at its last checkpoint (ADR-0020,
//! ADR-0034, ADR-0077).

use std::path::Path;
use std::process::ExitCode;

use super::run::{Start, run as run_spec};
use crate::options::RunOptions;
use crate::output::Io;

pub fn run(
    io: &Io,
    file: &Path,
    run_id: String,
    user: Option<String>,
    options: &RunOptions,
) -> ExitCode {
    run_spec(io, file, Start::Resume { run_id, user }, options)
}
