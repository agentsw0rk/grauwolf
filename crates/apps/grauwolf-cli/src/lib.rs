//! The `grauwolf` command line, as a library so that the commands can run in-process
//! (ADR-0108): `grauwolf spec validate|schema|graph|build|run|chat|serve|mcp|create|resume|generate`,
//! `grauwolf tools …` (delegated to grauwolf-tools-app), `grauwolf login|logout` and `grauwolf token`.

use std::process::ExitCode;

use clap::Parser;
use output::{err, out};
mod cli;
mod commands;
mod errors;
mod options;
mod output;
pub use cli::{Cli, Command};
pub use commands::spec::SpecCommand;
pub use commands::tools::ToolsCommand;
pub use options::{BuildOptions, CacheOptions, RunOptions};
pub use output::{Captured, Io};

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
                err!(io, "{error}");
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
    match cli.command {
        Command::Credentials { command } => commands::credentials::run(io, command),
        Command::Identity { command } => commands::identity::run(io, command),
        Command::Login(args) => commands::login::run(io, args),
        Command::Call(args) => commands::call::run(io, args),
        Command::Logout(args) => commands::login::logout(io, args),
        Command::Token { user } => commands::token::run(io, user),
        Command::Version { json } => commands::version::run(io, json),
        Command::Tools { command } => commands::tools::run(io, command),
        Command::Spec { command } => commands::spec::run(io, command),
        Command::Harness { command } => commands::harness::run(io, command),
    }
}
