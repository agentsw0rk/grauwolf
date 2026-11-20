//! `grauwolf-k8-ops` (ADR-0120): sets up SPIRE and OpenBao for grauwolf agents on Kubernetes.
//! The commands are functions over [`Services`] (a cluster and an OpenBao client) and [`Io`], so
//! tests run them against a fake API server; `main` only connects and prints.

mod agent;
mod agent_files;
mod build;
mod caller_spec;
mod callers;
mod check;
mod cli;
mod consumers;
mod deploy;
mod doctor;
mod egress;
mod harness;
mod hcl;
mod identity;
mod io;
mod oidc;
mod openbao_setup;
mod report;
mod restore;
mod schedule;
mod secret;
mod services;
mod spire;
mod trust_domain;
mod x509;

pub use cli::{
    AgentCommand, BuildCommand, BuildSecretCommand, Cli, Command, IdentityCommand, OidcCommand,
    OpenbaoCommand, SecretCommand, Settings, TrustDomainCommand,
};
pub use egress::Resolver;
pub use hcl::{trust_domain_of_hcl, trust_domain_of_yaml};
pub use io::Io;
pub use services::{AnyTransport, ProxyTransport, Services};

use clap::Parser;

/// The whole program: parse `args`, connect, run. The exit code (0 ok, 1 failure, 2 usage).
pub async fn run(args: impl IntoIterator<Item = String>, io: &mut Io<'_>) -> i32 {
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let text = error.to_string();
            if code == 0 {
                io.line(&text);
            } else {
                io.error(&text);
            }
            return code;
        }
    };
    let services = match Services::connect(cli.settings.clone()).await {
        Ok(services) => services,
        Err(message) => {
            io.error(&format!("error: {message}"));
            return 1;
        }
    };
    execute(cli.command, &services, io).await
}

/// Runs one command; prints its result to `io` and returns the exit code.
pub async fn execute(command: Command, services: &Services, io: &mut Io<'_>) -> i32 {
    let result = match command {
        Command::Doctor { json } => doctor::run(services, io, json).await,
        Command::Oidc {
            command: OidcCommand::Install,
        } => oidc::install(services, io).await.map(|()| 0),
        Command::Openbao {
            command:
                OpenbaoCommand::Setup {
                    jwks_url,
                    bound_issuer,
                },
        } => openbao_setup::setup(services, io, jwks_url, bound_issuer)
            .await
            .map(|()| 0),
        Command::Openbao {
            command: OpenbaoCommand::Token { ttl, renewable },
        } => openbao_setup::token(services, io, &ttl, renewable)
            .await
            .map(|()| 0),
        Command::Agent { command } => agent::run(services, io, command).await,
        Command::Build { command } => build::run_secret(services, io, command).await.map(|()| 0),
        Command::Secret { command } => secret::run(services, io, command).await.map(|()| 0),
        Command::TrustDomain { command } => trust_domain::run(services, io, command).await,
        Command::Identity { command } => identity::run(services, io, command).await,
        Command::Schedule { command } => schedule::run(services, io, command).await,
    };
    match result {
        Ok(code) => code,
        Err(message) => {
            io.error(&format!("error: {message}"));
            1
        }
    }
}

/// The policy `grauwolf-k8-ops` for the settings (tests check what it allows).
pub fn openbao_policy(settings: &Settings) -> String {
    openbao_setup::policy(settings)
}

/// The check Job of `agent check` (tests look at its security settings).
pub fn check_job(
    name: &str,
    namespace: &str,
    agent: &str,
    image: &str,
    variables: &[String],
    timeout: u64,
) -> serde_json::Value {
    check::job_object(name, namespace, agent, image, variables, timeout)
}
