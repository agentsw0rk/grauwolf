//! `agent build --harness`: the Kaniko build of a harness's image in the cluster (ADR-0129 B4),
//! the same flow as the studio's, without the studio.

use std::path::PathBuf;
use std::time::Duration;

use grauwolf_k8s::{BuildFinished, BuildRequest, kinds};
use serde_json::json;

use crate::agent::{MANAGED_BY, MANAGER};
use crate::cli::{BuildCommand, BuildSecretCommand};
use crate::io::Io;
use crate::report;
use crate::services::Services;

pub struct BuildOptions {
    pub harness: PathBuf,
    pub project: Option<PathBuf>,
    pub namespace: String,
    pub kaniko_image: String,
    pub registry_secret: Option<String>,
    pub git_secret: Option<String>,
    pub timeout: u64,
}

/// Runs the build; the exit code is 0 when the image was pushed, 1 when Kaniko failed or the time
/// was up. Everything that can be checked is checked before the cluster is touched.
pub async fn run(
    services: &Services,
    io: &mut Io<'_>,
    options: BuildOptions,
) -> Result<i32, String> {
    let harness = crate::harness::read(&options.harness)?;
    let folder = options
        .project
        .unwrap_or_else(|| PathBuf::from("build").join(&harness.name));
    let source = grauwolf_git::build_source(&folder)?;
    let repository = grauwolf_k8s::kaniko_repository(&source.remote)?;
    let destination = match harness.image.clone() {
        Some(image) => image,
        None => grauwolf_agent_tree::harness_registry(&options.harness)?
            .map(|registry| harness.default_image(&registry, env!("CARGO_PKG_VERSION")))
            .ok_or(
                "no image: set `image` in the harness file or `[harness] registry` in grauwolf.toml",
            )?,
    };
    let request = BuildRequest {
        name: harness.name.clone(),
        namespace: options.namespace.clone(),
        kaniko_image: options.kaniko_image,
        repository,
        git_ref: source.branch,
        commit: source.commit,
        sub_path: source.path,
        destination: destination.clone(),
        git_secret: options.git_secret,
        registry_secret: options.registry_secret,
    };
    ensure_namespace(services, &options.namespace).await?;
    for name in [&request.registry_secret, &request.git_secret]
        .into_iter()
        .flatten()
    {
        let found = services
            .cluster
            .get(&kinds::SECRET, Some(&request.namespace), name)
            .await
            .map_err(|error| format!("secret {name}: {error}"))?;
        if found.is_none() {
            return Err(format!(
                "secret {name} not found in namespace {}: `grauwolf-k8-ops build secret …` makes it",
                request.namespace
            ));
        }
    }
    let build = grauwolf_k8s::run_build_with(
        &services.cluster,
        &request,
        Duration::from_secs(options.timeout),
        |line| io.line(line),
    )
    .await?;
    Ok(match build.finished {
        BuildFinished::Succeeded => {
            io.line(&format!("built and pushed {destination}"));
            0
        }
        BuildFinished::Failed => {
            io.line(&format!(
                "build of {destination} failed (see the log above)"
            ));
            1
        }
        BuildFinished::TimedOut => {
            io.line(&format!(
                "build of {destination} not finished within {} seconds; the Job is deleted",
                options.timeout
            ));
            1
        }
    })
}

/// The build namespace: ours by its label, not at the restricted Pod Security level (Kaniko runs
/// as root).
async fn ensure_namespace(services: &Services, name: &str) -> Result<(), String> {
    let namespace = json!({
        "apiVersion": "v1",
        "kind": "Namespace",
        "metadata": { "name": name, "labels": { MANAGED_BY: MANAGER } },
    });
    services
        .cluster
        .ensure(&kinds::NAMESPACE, None, &namespace)
        .await
        .map_err(|error| format!("namespace {name}: {error}"))?;
    Ok(())
}

/// `build secret registry|git`: the token comes from stdin or a hidden prompt and is never
/// printed.
pub async fn run_secret(
    services: &Services,
    io: &mut Io<'_>,
    command: BuildCommand,
) -> Result<(), String> {
    let BuildCommand::Secret { command } = command;
    let (name, namespace, secret) = match command {
        BuildSecretCommand::Registry {
            name,
            server,
            username,
            namespace,
        } => {
            let token = io.read_secret(&format!("Token for {server} (hidden): "))?;
            let secret =
                grauwolf_k8s::registry_secret(&name, &namespace, &server, &username, &token);
            (name, namespace, secret)
        }
        BuildSecretCommand::Git {
            name,
            username,
            namespace,
        } => {
            let token = io.read_secret(&format!("Git token for {username} (hidden): "))?;
            let secret = json!({
                "apiVersion": "v1",
                "kind": "Secret",
                "metadata": { "name": name, "namespace": namespace },
                "type": "Opaque",
                "stringData": { "username": username, "password": token },
            });
            (name, namespace, secret)
        }
    };
    ensure_namespace(services, &namespace).await?;
    let outcome = services
        .cluster
        .ensure(&kinds::SECRET, Some(&namespace), &secret)
        .await
        .map_err(|error| format!("secret {namespace}/{name}: {error}"))?;
    io.line(&format!(
        "secret {namespace}/{name}: {}",
        report::k8s(outcome)
    ));
    Ok(())
}
