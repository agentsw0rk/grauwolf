//! The `grauwolf` binary: parses the command line and hands it to the library.

use clap::Parser;
use grauwolf_cli::{Cli, Io, dispatch};

fn main() -> std::process::ExitCode {
    dispatch(Cli::parse(), &Io::stdio())
}
