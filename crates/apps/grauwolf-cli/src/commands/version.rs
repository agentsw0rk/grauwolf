//! `grauwolf version [--json]`: the version and the builder image that goes with it (ADR-0096).

use std::process::ExitCode;

use crate::output::{Io, outln};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Prints `grauwolf <version>`, or with `json` the version and the default builder image.
pub fn run(io: &Io, json: bool) -> ExitCode {
    if json {
        let report = serde_json::json!({
            "version": VERSION,
            "builder_image": grauwolf_generator::default_builder_image(VERSION),
        });
        outln!(io, "{report}");
    } else {
        outln!(io, "grauwolf {VERSION}");
    }
    ExitCode::SUCCESS
}
