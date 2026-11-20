//! The `grauwolf-tools` binary: parses the command line and hands it to the library.

use clap::Parser;
use grauwolf_tools_app::{Cli, Io, dispatch};

fn main() -> std::process::ExitCode {
    dispatch(Cli::parse(), &Io::stdio())
}
