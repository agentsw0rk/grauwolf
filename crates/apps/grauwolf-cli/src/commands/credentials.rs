//! `grauwolf credentials check <VAR>`: resolves a variable through the credential sources and
//! reports where it came from, never the value (ADR-0110).

use std::process::ExitCode;
use std::sync::Arc;

use grauwolf_host::config::{Env, ProcessEnv};

use crate::cli::CredentialsCommand;
use crate::output::{Io, errln, outln, report};

/// Runs a `credentials` subcommand.
pub fn run(io: &Io, command: CredentialsCommand) -> ExitCode {
    match command {
        CredentialsCommand::Check {
            variables,
            all,
            config,
        } => {
            let base: Arc<dyn Env> = match config {
                Some(path) => Arc::new(move |name: &str| {
                    if name == "GRAUWOLF_CREDENTIALS" {
                        Some(path.display().to_string())
                    } else {
                        ProcessEnv.var(name)
                    }
                }),
                None => Arc::new(ProcessEnv),
            };
            // `--all` is the empty list; clap requires names without it.
            let names = if all { Vec::new() } else { variables };
            match grauwolf_host::credentials::check_many(base, None, &names) {
                Ok(results) => {
                    let mut code = ExitCode::SUCCESS;
                    for (_, result) in results {
                        match result {
                            Ok(message) => outln!(io, "{message}"),
                            Err(message) => {
                                errln!(io, "{message}");
                                code = ExitCode::FAILURE;
                            }
                        }
                    }
                    code
                }
                Err(message) => report(io, Err(message)),
            }
        }
    }
}
