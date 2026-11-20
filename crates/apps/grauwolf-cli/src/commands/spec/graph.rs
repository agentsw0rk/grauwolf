//! `grauwolf spec graph <file>`: the flow of the spec as a Mermaid diagram (F13.14).

use std::path::Path;
use std::process::ExitCode;

use grauwolf_host::loader::load_spec;

use crate::output::{Io, out, report};

pub fn run(io: &Io, file: &Path) -> ExitCode {
    report(
        io,
        load_spec(file).map(|spec| out!(io, "{}", spec.to_mermaid())),
    )
}
