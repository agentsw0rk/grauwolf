//! `grauwolf harness …`: the harness file of ADR-0128 – the boundary of a generated, deployable
//! project around a root spec and its local sub-agents.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Subcommand;

use crate::options::BuildOptions;
use crate::output::{Io, errln, outln, report};

#[derive(Subcommand)]
pub enum HarnessCommand {
    /// Generates the harness's project into `<out-dir>/<name>/`: the service crate of the root
    /// spec with its `Dockerfile` and `app/`, as `spec generate --dockerfile` writes it.
    Generate {
        /// Path to the harness file (`<name>.harness.json`).
        file: PathBuf,
        /// Folder of the generated projects; the project goes into a folder named after the
        /// harness.
        #[arg(long, default_value = "build")]
        out_dir: PathBuf,
        #[command(flatten)]
        build: BuildOptions,
    },
}

pub fn run(io: &Io, command: HarnessCommand) -> ExitCode {
    match command {
        HarnessCommand::Generate {
            file,
            out_dir,
            build,
        } => generate_project(io, &file, &out_dir, &build),
    }
}

/// Writes the harness's project into `<out_dir>/<name>/`, replacing an earlier generation
/// (`grauwolf_host::project::generate_harness`).
fn generate_project(io: &Io, file: &Path, out_dir: &Path, build: &BuildOptions) -> ExitCode {
    report(
        io,
        (|| {
            let written =
                crate::errors::runtime()?.block_on(grauwolf_host::project::generate_harness(
                    file,
                    out_dir,
                    &build.tool_builder(),
                    build.locked,
                ))?;
            for warning in &written.warnings {
                errln!(io, "{warning}");
            }
            outln!(
                io,
                "{}: project written to {}",
                file.display(),
                written.project.display()
            );
            Ok(())
        })(),
    )
}
