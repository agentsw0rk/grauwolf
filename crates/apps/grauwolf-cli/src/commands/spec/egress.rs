//! `grauwolf spec egress <file>`: every destination the agent tree may reach (ADR-0110, addendum B).

use std::path::Path;
use std::process::ExitCode;

use clap::ValueEnum;
use grauwolf_egress::Options;

use crate::output::{Io, errln, out, report};

/// Output format of `spec egress`.
#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum EgressFormat {
    /// One line per destination.
    Text,
    /// A JSON array.
    Json,
}

/// `grauwolf spec egress <file> [--format text|json] [--otel]`.
pub fn run(io: &Io, file: &Path, format: EgressFormat, otel: bool) -> ExitCode {
    let tree = match grauwolf_host::agent_tree::read(file) {
        Ok(tree) => tree,
        Err(failure) => {
            for warning in &failure.warnings {
                errln!(io, "{warning}");
            }
            return report(io, Err(failure.to_string()));
        }
    };
    let credentials = grauwolf_egress::credentials_file(&tree);
    let destinations = grauwolf_egress::derive_tree(
        &tree,
        &Options {
            credentials_toml: credentials.as_deref(),
            otel,
        },
    );
    match format {
        EgressFormat::Text => out!(io, "{}", grauwolf_egress::to_text(&destinations)),
        EgressFormat::Json => out!(io, "{}", grauwolf_egress::to_json(&destinations)),
    }
    ExitCode::SUCCESS
}
