//! `grauwolf spec serve`: the agent as an HTTP service with A2A and MCP, until Ctrl+C
//! (ADR-0053, ADR-0054, ADR-0056, ADR-0068, ADR-0094).

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use grauwolf_agents::memory::ChatHistoryStore;
use grauwolf_host::config::{ProcessEnv, SpiffeServiceConfig};
use grauwolf_host::memory::FileChatHistoryStore;
use grauwolf_host::otel::OtelTracing;
use grauwolf_server::SpiffeInbound;

use super::handles::agent_description;
use super::loader::service_loader;
use crate::options::{RunOptions, Usage};
use crate::output::{Io, errln, report};

/// Environment variable with the token for `spec serve`.
const SERVER_TOKEN: &str = "GRAUWOLF_SERVER_TOKEN";

/// `grauwolf spec serve <file>`: loads the agent once and serves requests until Ctrl+C.
#[allow(clippy::too_many_arguments)]
pub fn run(
    io: &Io,
    file: &Path,
    addr: &str,
    public_url: Option<&str>,
    users: Option<&Path>,
    chat_api: bool,
    spiffe: &super::SpiffeFlags,
    options: &RunOptions,
) -> ExitCode {
    if let Err(error) = options.check_effect(Usage::Serve) {
        return report(io, Err(error));
    }
    let tracing = match options
        .otel
        .then(|| {
            OtelTracing::init_with(
                None,
                &grauwolf_host::config::TelemetryConfig::from_env(
                    &grauwolf_host::config::ProcessEnv,
                ),
            )
        })
        .transpose()
    {
        Ok(tracing) => tracing,
        Err(error) => return report(io, Err(error)),
    };
    let result = (|| {
        let runtime = crate::errors::runtime()?;
        runtime.block_on(async {
            // `--confirm-tools` asks in the service via `/run/stream` instead of on the console
            // (ADR-0061).
            // Before loading: it switches on the `OLLAMA_BASE_URL` fallback and sees the MCP tools.
            // The credential sources of the spec (ADR-0110) apply to everything read below.
            let env = grauwolf_host::credentials::env_for(Arc::new(ProcessEnv), Some(file))?;
            let chat_api = chat_api.then(|| {
                let env = Arc::clone(&env);
                grauwolf_server::chat_api::ChatApi::from_dir(
                    file.parent().unwrap_or(Path::new(".")),
                    move |name| env.var(name),
                )
            });
            let observations = chat_api.as_ref().map(|chat_api| chat_api.observations());
            let (spec, agent, provisioned) = service_loader(io, options, tracing.as_ref(), observations)?
                .with_credential_env(Arc::clone(&env))
                .load_provisioned(file, None)
                .await?;
            let history: Arc<dyn ChatHistoryStore> = match &options.conversations_db {
                Some(url) => grauwolf_host::memory::sql_chat_history_store(url).await?,
                None => Arc::new(FileChatHistoryStore::new(options.conversations_dir())),
            };
            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .map_err(|error| format!("{addr}: {error}"))?;
            let bound = listener.local_addr().map_err(|error| error.to_string())?;
            // Access via a token from the environment; without a token only locally (ADR-0053).
            let token = env.var(SERVER_TOKEN).filter(|token| !token.is_empty());
            // User tokens (ADR-0068): runs belong to the user of the token.
            let users = users
                .map(|path| {
                    let text = std::fs::read_to_string(path)
                        .map_err(|error| format!("{}: {error}", path.display()))?;
                    grauwolf_server::Users::from_json(&text)
                        .map_err(|error| format!("{}: {error}", path.display()))
                })
                .transpose()?;
            // Callers with a SPIFFE identity (ADR-0109): flags win over the environment.
            let mut spiffe_config = SpiffeServiceConfig::from_env(&*env)?;
            spiffe_config.enabled |= spiffe.spiffe;
            if let Some(port) = spiffe.spiffe_port {
                spiffe_config.port = port;
            }
            if let Some(file) = &spiffe.spiffe_users {
                spiffe_config.users_file = Some(file.display().to_string());
            }
            spiffe_config.check_identity()?;
            let anonymous = token.is_none() && users.is_none();
            let inbound = if spiffe_config.enabled {
                Some(Arc::new(
                    SpiffeInbound::start(&spiffe_config, &*env, anonymous && bound.ip().is_loopback())
                        .await?,
                ))
            } else {
                None
            };
            if token.is_none() && users.is_none() && inbound.is_none() && !bound.ip().is_loopback() {
                return Err(format!(
                    "{bound} is reachable from outside: set a token in {SERVER_TOKEN}, pass --users or bind to 127.0.0.1"
                ));
            }
            errln!(io, "grauwolf service for {} on http://{bound}", spec.name);
            let agent = Arc::new(agent);
            let base_url = public_url.map_or_else(|| format!("http://{bound}"), str::to_owned);
            // HTTP interface, A2A (ADR-0054) and MCP (ADR-0056) on the same service.
            // Resources (spec, knowledge) and prompt of the agent (ADR-0089).
            let mcp = grauwolf_host::mcp::agent_server(
                &spec,
                file.parent().unwrap_or(Path::new(".")),
                Vec::new(),
            )
            .await?;
            let mut options = grauwolf_server::ServiceOptions::provisioned(provisioned);
            options.chat_api = chat_api.as_ref().map(|chat_api| chat_api.options());
            let (mcp_agent, mcp_options) = (Arc::clone(&agent), options.clone());
            let router = grauwolf_server::router_with(
                Arc::clone(&agent),
                Arc::clone(&history),
                options.clone(),
            );
            // MCP likewise with the handles of the user from the token (ADR-0089).
            let router = router.merge(grauwolf_server::mcp_router_with(
                mcp,
                Arc::clone(&mcp_agent),
                agent_description(&spec),
                mcp_options,
            ));
            // A2A with the same tools and the same memory as `/run` (ADR-0089), once per port:
            // the agent card of each names its own URL.
            let a2a = |url: &str| {
                grauwolf_server::a2a::router_with(
                    Arc::clone(&mcp_agent),
                    Arc::clone(&history),
                    grauwolf_server::a2a::AgentCardInfo::from_spec(&spec, url),
                    options.clone(),
                )
            };
            let gate = inbound.as_ref().map(|inbound| Arc::clone(&inbound.gate));
            let secure = |router: axum::Router| match (&token, &users, &gate) {
                (None, None, None) => router,
                (token, users, gate) => grauwolf_server::with_spiffe_auth(
                    router,
                    token.clone(),
                    users.clone().unwrap_or_default(),
                    gate.clone(),
                ),
            };
            let mtls_router = inbound.as_ref().map(|inbound| {
                let url = spiffe_config
                    .public_url
                    .clone()
                    .unwrap_or_else(|| format!("https://localhost:{}", inbound.port));
                secure(router.clone().merge(a2a(&url)))
            });
            let router = secure(router.merge(a2a(&base_url)));
            // The second listener: the same router over mTLS, client certificates required.
            let mtls = match inbound {
                Some(inbound) => {
                    let secure = tokio::net::TcpListener::bind((bound.ip(), inbound.port))
                        .await
                        .map_err(|error| format!("SPIFFE port {}: {error}", inbound.port))?;
                    errln!(io, "grauwolf mTLS (SPIFFE) on port {}", inbound.port);
                    let router = mtls_router.expect("the mTLS routes exist with the inbound");
                    Some(tokio::spawn(async move {
                        inbound.serve(secure, router, std::future::pending()).await
                    }))
                }
                None => None,
            };
            let served = axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await
                .map_err(|error| error.to_string());
            if let Some(mtls) = mtls {
                mtls.abort();
            }
            served
        })
    })();
    let flushed = tracing.map_or(Ok(()), OtelTracing::shutdown);
    report(io, result.and(flushed))
}
