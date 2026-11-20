//! The tool commands of grauwolf as a library (ADR-0118): `build`, `pack`, `push`, `pull`,
//! `inspect`, `lock`, `keygen` and `validate`. The program `grauwolf-tools` runs them, and
//! `grauwolf tools …` delegates to the same code, so both behave the same.
//!
//! The crate builds only on domain and adapter crates – no host, server or CLI – which keeps the
//! program small and quick to build (ADR-0118; `cargo xtask check-arch` guards it).

use std::process::ExitCode;

use clap::Parser;

mod build;
pub mod catalog;
mod errors;
mod lock;
pub mod new;
mod options;
mod output;
mod tools;
pub mod validate;

pub use build::Format;
pub use lock::{LockSummary, Locker, lock_spec_tree};
pub use options::{BuildOptions, CacheOptions, DEFAULT_CACHE, tool_builder};
pub use output::{Captured, Io, report};
pub use tools::{ToolsCommand, run as run_tools};

/// The `grauwolf-tools` command line.
#[derive(Parser)]
#[command(
    name = "grauwolf-tools",
    version,
    about = "grauwolf-tools – build, pack, validate and distribute tool packages"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: ToolsCommand,
}

/// Parses `args` (the first is the program name) and runs the command, writing to `io`.
/// Usage errors, `--help` and `--version` go to `io` as well.
pub fn run<I, T>(args: I, io: &Io) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    match Cli::try_parse_from(args) {
        Ok(cli) => dispatch(cli, io),
        Err(error) => {
            if error.use_stderr() {
                errln!(io, "{error}");
                ExitCode::from(2)
            } else {
                out!(io, "{error}");
                ExitCode::SUCCESS
            }
        }
    }
}

/// Runs a parsed command line.
pub fn dispatch(cli: Cli, io: &Io) -> ExitCode {
    run_tools(io, cli.command, &lock_spec_tree)
}
