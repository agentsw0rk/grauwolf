//! `grauwolf spec schema`: the JSON schema of the spec format (F13.2).

use std::process::ExitCode;

use crate::output::{Io, outln};

pub fn run(io: &Io) -> ExitCode {
    outln!(io, "{:#}", grauwolf_host::spec_schema());
    ExitCode::SUCCESS
}
