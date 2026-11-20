//! `grauwolf identity serve`: the identity service of a trust domain (ADR-0111). It verifies
//! people's Entra tokens, issues Txn-Tokens signed by OpenBao Transit and publishes their keys;
//! its callers are workloads with a SPIFFE identity (mTLS port or JWT-SVID). `grauwolf identity
//! grant …` (stage 3) is in `grant.rs`.

use std::net::{IpAddr, Ipv4Addr};
use std::process::ExitCode;
use std::sync::Arc;

use grauwolf_host::config::{Config, Env, ProcessEnv, SpiffeServiceConfig};

use crate::cli::IdentityCommand;
use crate::output::{Io, report};

/// Runs an `identity` subcommand.
pub fn run(io: &Io, command: IdentityCommand) -> ExitCode {
    match command {
        IdentityCommand::Serve { port, mtls_port } => report(io, serve(port, mtls_port)),
        IdentityCommand::Grant { command } => crate::commands::grant::run(io, command),
    }
}

fn serve(port: Option<u16>, mtls_port: Option<u16>) -> Result<(), String> {
    let env: Arc<dyn Env> = Arc::new(ProcessEnv);
    let config = Config::from_env(&*env)?;
    let spiffe = SpiffeServiceConfig::from_env(&*env)?;
    let runtime = crate::errors::runtime()?;
    runtime.block_on(async {
        let identity = grauwolf_host::spiffe::workload(&*env).await?.ok_or(
            "grauwolf identity serve needs a workload identity: SPIFFE_ENDPOINT_SOCKET is not set",
        )?;
        let service = grauwolf_host::identity::service(&config.identity, Arc::clone(&identity))?;
        let mut spiffe = spiffe;
        if let Some(mtls_port) = mtls_port {
            spiffe.port = mtls_port;
        }
        grauwolf_server::identity::serve_identity(
            Arc::new(service),
            identity,
            &spiffe,
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port.unwrap_or(config.service.port),
        )
        .await
    })
}
