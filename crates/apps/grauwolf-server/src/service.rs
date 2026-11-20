//! A generated crate as an HTTP service (ADR-0093, feature `service`): the entry point
//! `run_app_with` of the crate is wrapped as a [`ServedAgent`] and served with the routes of
//! `spec serve` – `/run`, `/run/stream`, `/health`, A2A and MCP.

use grauwolf_spec::AgentSpecExt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use crate::a2a::AgentCardInfo;
use crate::{ServedAgent, ServiceOptions, Users};
use futures_util::future::BoxFuture;
use grauwolf_agents::events::Feature;
use grauwolf_agents::memory::{ChatHistoryStore, MemoryChatHistoryStore};
use grauwolf_interpreter::RunExtras;
use grauwolf_spec::AgentSpec;
use serde_json::Value;

use crate::chat_api::ChatApi;
use crate::spiffe::SpiffeInbound;
use grauwolf_host::app::RunOptions;
use grauwolf_host::config::{
    ChatApiConfig, Env, ProcessEnv, ServiceConfig as HostServiceConfig, SpiffeServiceConfig,
};
use grauwolf_host::memory::FileChatHistoryStore;
use grauwolf_host::provisioning::{ProvisionOptions, ProvisionedAgent};

/// Port of the service if `SERVER_PORT` is not set.
pub const DEFAULT_PORT: u16 = grauwolf_host::config::DEFAULT_SERVER_PORT;

/// Environment variable with the shared token (as with `spec serve`).
pub const TOKEN_VARIABLE: &str = "GRAUWOLF_SERVER_TOKEN";

/// The provisioning entry point of a generated crate as the service sees it: base directory and
/// the options of the agent (environment, observers); called once at start (ADR-0114).
pub type ProvisionApp = Arc<
    dyn Fn(PathBuf, ProvisionOptions) -> BoxFuture<'static, Result<ProvisionedAgent, String>>
        + Send
        + Sync,
>;

/// The run entry point of a generated crate: the provisioned agent, the input as JSON and the
/// options of the run; the output as JSON. Called for every request.
pub type RunApp = Arc<
    dyn Fn(Arc<ProvisionedAgent>, Value, RunOptions) -> BoxFuture<'static, Result<Value, String>>
        + Send
        + Sync,
>;

/// A generated agent behind [`ServedAgent`]. It hands to `run_provisioned` what the request
/// brings, like the interpreter does (ADR-0119): the user (long-term memory, key-value store),
/// the events of a run (`/run/stream`), the conversation, attachments, the approver and the
/// frame sink (text deltas). Tools per request and the memory of the request are ignored: the
/// provisioned agent has its own, per user.
pub struct GeneratedAgent {
    input_schema: Option<Value>,
    provisioned: Arc<ProvisionedAgent>,
    run: RunApp,
    /// Observes the events of every run (chat API, ADR-0094).
    observer: Option<Arc<dyn Feature>>,
    /// Records the spans of every run (`GRAUWOLF_OTEL`, ADR-0123).
    tracing: Option<Arc<dyn Feature>>,
    /// Writes the events of every run as JSON lines (`GRAUWOLF_EVENTS`).
    events: Option<Arc<dyn Feature>>,
}

impl GeneratedAgent {
    /// Wraps `run` for the agent of `spec`, provisioned once as `provisioned`.
    pub fn new(spec: &AgentSpec, provisioned: Arc<ProvisionedAgent>, run: RunApp) -> Self {
        Self {
            input_schema: spec.types.get(&spec.input).cloned(),
            provisioned,
            run,
            observer: None,
            tracing: None,
            events: None,
        }
    }

    /// Lets `log` see the events of every run (`GRAUWOLF_EVENTS`, see [`stderr_event_log`]).
    pub fn logged_by(mut self, log: Arc<dyn Feature>) -> Self {
        self.events = Some(log);
        self
    }

    /// Records every run as the agent `spec_name` with `tracing` (ADR-0123); the run continues
    /// the `traceparent` of its caller.
    pub fn traced_by(mut self, tracing: &ServiceTracing, spec_name: &str) -> Self {
        self.tracing = Some(tracing.feature(spec_name));
        self
    }

    /// Lets the observations of `chat_api` see the events of every run, under the name of the
    /// agent `spec_name` (ADR-0094). The tool lists of its MCP servers and its sub-agents are
    /// observed through [`provision_options`].
    pub fn observed_by(mut self, chat_api: &ChatApi, spec_name: &str) -> Self {
        self.observer = Some(chat_api.observations().feature_for(spec_name));
        self
    }
}

/// The options a generated service provisions its agent with: the process environment (this is
/// an entry point), and, with a chat API, observers for the MCP tool lists and the sub-agents
/// (ADR-0094).
pub fn provision_options(chat_api: Option<&ChatApi>) -> ProvisionOptions {
    provision_options_with(Arc::new(ProcessEnv), chat_api)
}

/// Like [`provision_options`] with the environment `env` (the process environment with the
/// credential sources of ADR-0110).
pub fn provision_options_with(env: Arc<dyn Env>, chat_api: Option<&ChatApi>) -> ProvisionOptions {
    provision_options_traced(env, chat_api, None)
}

/// Every event of a run as one JSON line on stderr: what `spec serve --events` does and what
/// `GRAUWOLF_EVENTS=1` switches on. One log is shared by the agent and its sub-agents, so lines
/// never mix.
pub fn stderr_event_log() -> Arc<dyn Feature> {
    Arc::new(grauwolf_host::features::JsonlEventLog::new(
        std::io::stderr(),
    ))
}

/// Like [`provision_options_with`], and with `tracing` each sub-agent reports its own
/// `invoke_agent` span (ADR-0123).
pub fn provision_options_traced(
    env: Arc<dyn Env>,
    chat_api: Option<&ChatApi>,
    tracing: Option<&ServiceTracing>,
) -> ProvisionOptions {
    provision_options_logged(env, chat_api, tracing, None)
}

/// Like [`provision_options_traced`], and with `events` every sub-agent also reports its events
/// to that log (`GRAUWOLF_EVENTS`).
pub fn provision_options_logged(
    env: Arc<dyn Env>,
    chat_api: Option<&ChatApi>,
    tracing: Option<&ServiceTracing>,
    events: Option<Arc<dyn Feature>>,
) -> ProvisionOptions {
    let mut options = ProvisionOptions::new(env);
    let observations = chat_api.map(|chat_api| Arc::clone(chat_api.observations()));
    if let Some(observations) = &observations {
        options = options
            .with_chat_api_mode(true)
            .with_mcp_tools_observer(Some(observations.mcp_tools_observer()));
    }
    if observations.is_none() && tracing.is_none() && events.is_none() {
        return options;
    }
    let tracing = tracing.cloned();
    options.with_agent_features(Some(Arc::new(move |spec: &AgentSpec| {
        let mut features = Vec::new();
        if let Some(tracing) = &tracing {
            features.push(tracing.feature(&spec.name));
        }
        if let Some(observations) = &observations {
            features.push(observations.feature_for(&spec.name));
        }
        features.extend(events.clone());
        features
    })))
}

/// The OpenTelemetry export of a generated service (`GRAUWOLF_OTEL=1`, ADR-0123): the same
/// spans as `spec serve --otel`.
#[derive(Clone)]
pub struct ServiceTracing {
    #[cfg(feature = "otel")]
    export: grauwolf_host::otel::OtelTracing,
    content: bool,
}

impl ServiceTracing {
    /// Starts the export if `config.otel` is set; the service name comes from `env`
    /// (`OTEL_SERVICE_NAME`), the target from `OTEL_EXPORTER_OTLP_*`. Call it outside of an async
    /// context, like [`shutdown`](Self::shutdown).
    #[cfg(feature = "otel")]
    pub fn start(config: &ServiceConfig, env: &dyn Env) -> Result<Option<Self>, String> {
        if !config.otel {
            return Ok(None);
        }
        let telemetry = grauwolf_host::config::TelemetryConfig::from_env(env);
        let export = grauwolf_host::otel::OtelTracing::init_with(None, &telemetry)?;
        Ok(Some(Self {
            export,
            content: config.otel_content,
        }))
    }

    /// Without the feature `otel` the switch is refused.
    #[cfg(not(feature = "otel"))]
    pub fn start(config: &ServiceConfig, _env: &dyn Env) -> Result<Option<Self>, String> {
        if config.otel {
            return Err(
                "GRAUWOLF_OTEL: this build has no OpenTelemetry (feature `otel`)".to_owned(),
            );
        }
        Ok(None)
    }

    /// The feature of the agent `agent_name`.
    #[cfg(feature = "otel")]
    pub fn feature(&self, agent_name: &str) -> Arc<dyn Feature> {
        self.export.feature(agent_name, self.content)
    }

    /// The feature of the agent `agent_name`.
    #[cfg(not(feature = "otel"))]
    pub fn feature(&self, _agent_name: &str) -> Arc<dyn Feature> {
        unreachable!(
            "a build without the feature `otel` never starts tracing ({})",
            self.content
        )
    }

    /// Sends the spans still pending; blocks.
    #[cfg(feature = "otel")]
    pub fn shutdown(self) -> Result<(), String> {
        self.export.shutdown()
    }

    /// Nothing to send without the feature `otel`.
    #[cfg(not(feature = "otel"))]
    pub fn shutdown(self) -> Result<(), String> {
        Ok(())
    }
}

impl ServedAgent for GeneratedAgent {
    fn input_schema(&self) -> Option<Value> {
        self.input_schema.clone()
    }

    fn run(
        &self,
        input: Value,
        extras: RunExtras,
        user: Option<String>,
    ) -> BoxFuture<'_, Result<Value, String>> {
        let features: Vec<Arc<dyn Feature>> = extras
            .feature
            .into_iter()
            .chain(self.tracing.clone())
            .chain(self.observer.clone())
            .chain(self.events.clone())
            .collect();
        let options = RunOptions {
            user,
            features,
            conversation: extras.conversation,
            attachments: extras.attachments,
            approver: extras.approver,
            frame_sink: extras.frame_sink,
            caller: extras.caller,
            trace_parent: extras.trace_parent,
            on_behalf_of: extras.on_behalf_of,
            ..RunOptions::default()
        };
        (self.run)(Arc::clone(&self.provisioned), input, options)
    }
}

/// Settings of the service, normally from the environment ([`ServiceConfig::from_env`]).
#[derive(Clone)]
pub struct ServiceConfig {
    /// Address the service binds to (`0.0.0.0`).
    pub bind: IpAddr,
    /// Port of the service (`SERVER_PORT`, default [`DEFAULT_PORT`]).
    pub port: u16,
    /// Shared token (`GRAUWOLF_SERVER_TOKEN`).
    pub token: Option<String>,
    /// User tokens, read from the file in `GRAUWOLF_USERS_FILE` (`{ "<user>": "sha256:<hex>" }`).
    pub users: Option<Users>,
    /// Allows access without any token (`GRAUWOLF_ALLOW_ANONYMOUS=true`); without it the
    /// service refuses to start without a token on a non-local address.
    pub allow_anonymous: bool,
    /// Directory for the paths of the spec: knowledge, long-term memory, tools
    /// (`GRAUWOLF_DATA_DIR`, default the working directory).
    pub data_dir: PathBuf,
    /// Directory of the conversation histories (`GRAUWOLF_CONVERSATIONS_DIR`, relative to
    /// `data_dir`); without it and without `conversations_db` they stay in memory.
    pub conversations_dir: Option<PathBuf>,
    /// Database of the conversation histories (`GRAUWOLF_CONVERSATIONS_DB`, a secret; excludes
    /// the directory; needs the feature `sql`).
    pub conversations_db: Option<String>,
    /// Additional port for A2A (`A2A_PORT`); A2A is always also on the main port.
    pub a2a_port: Option<u16>,
    /// Path prefix of the A2A routes on that port (`A2A_BASE_PATH`, default none).
    pub a2a_base_path: String,
    /// Public URL of the additional A2A port for its agent card (`GRAUWOLF_A2A_PUBLIC_URL`;
    /// default `http://localhost:<A2A_PORT>`).
    pub a2a_public_url: Option<String>,
    /// Public URL of the service for the agent card (`GRAUWOLF_PUBLIC_URL`; default
    /// `http://localhost:<port>`).
    pub public_url: Option<String>,
    /// Chat API (`POST /api/chat`, `GET /api/signature`; `GRAUWOLF_CHAT_API=1` or `true`,
    /// ADR-0094); read from the files in `data_dir`.
    pub chat_api: Option<Arc<ChatApi>>,
    /// Inbound SPIFFE settings (`GRAUWOLF_SPIFFE=1`, ADR-0109); `None` without it.
    pub spiffe: Option<SpiffeServiceConfig>,
    /// The started SPIFFE identity and callers (made by [`main`] from `spiffe`); with it
    /// [`serve`] adds the mTLS port and the HTTP port accepts JWT-SVIDs.
    pub spiffe_inbound: Option<Arc<SpiffeInbound>>,
    /// Export the spans of the service (`GRAUWOLF_OTEL=1` or `true`, ADR-0123); [`main`] starts
    /// the export, a build without the feature `otel` refuses it.
    pub otel: bool,
    /// Also record contents in the spans (`GRAUWOLF_OTEL_CONTENT=1` or `true`).
    pub otel_content: bool,
    /// Write every event of every run as a JSON line to stderr (`GRAUWOLF_EVENTS=1` or `true`),
    /// as `spec serve --events`; [`main`] attaches [`stderr_event_log`].
    pub events: bool,
}

impl ServiceConfig {
    /// Settings from the process environment.
    pub fn from_env() -> Result<Self, String> {
        Self::from_env_with(|name| ProcessEnv.var(name))
    }

    /// Settings from `env`, which returns the value of a variable (the section
    /// [`grauwolf_host::config::ServiceConfig`], plus the users file and the chat API).
    pub fn from_env_with(env: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let settings = HostServiceConfig::from_lookup(&env)?;
        let users = settings
            .users_file
            .map(|path| {
                let text = std::fs::read_to_string(&path)
                    .map_err(|error| format!("GRAUWOLF_USERS_FILE {path}: {error}"))?;
                Users::from_json(&text).map_err(|error| format!("{path}: {error}"))
            })
            .transpose()?;
        let spiffe = SpiffeServiceConfig::from_lookup(&env)?;
        spiffe.check_identity()?;
        let chat_api = ChatApiConfig::from_lookup(&env)
            .enabled
            .then(|| ChatApi::from_dir(&settings.data_dir, |name| env(name)));
        Ok(Self {
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: settings.port,
            token: settings.token,
            users,
            allow_anonymous: settings.allow_anonymous,
            conversations_dir: settings
                .conversations_dir
                .map(|dir| settings.data_dir.join(dir)),
            conversations_db: settings.conversations_db,
            data_dir: settings.data_dir,
            a2a_port: settings.a2a_port,
            a2a_base_path: settings.a2a_base_path,
            a2a_public_url: settings.a2a_public_url,
            public_url: settings.public_url,
            chat_api,
            spiffe: Some(spiffe).filter(|spiffe| spiffe.enabled),
            spiffe_inbound: None,
            otel: settings.otel,
            otel_content: settings.otel_content,
            events: settings.events,
        })
    }

    /// Fails if the service would be reachable from outside without any token, as `spec serve`
    /// does.
    pub fn check_access(&self) -> Result<(), String> {
        if self.token.is_none()
            && self.users.is_none()
            && !self.allow_anonymous
            && !self.bind.is_loopback()
        {
            return Err(format!(
                "the service is reachable from outside: set a token in {TOKEN_VARIABLE}, pass \
                 GRAUWOLF_USERS_FILE or set GRAUWOLF_ALLOW_ANONYMOUS=true"
            ));
        }
        Ok(())
    }

    fn public_url(&self) -> String {
        self.public_url
            .clone()
            .unwrap_or_else(|| format!("http://localhost:{}", self.port))
    }

    fn secure(&self, router: axum::Router) -> axum::Router {
        let gate = self
            .spiffe_inbound
            .as_ref()
            .map(|inbound| Arc::clone(&inbound.gate));
        if self.token.is_none() && self.users.is_none() && gate.is_none() {
            router
        } else {
            crate::with_spiffe_auth(
                router,
                self.token.clone(),
                self.users.clone().unwrap_or_default(),
                gate,
            )
        }
    }
}

/// The store of the conversations: the database, the directory, or the memory of the process
/// (ADR-0060, ADR-0119).
pub async fn history_store(config: &ServiceConfig) -> Result<Arc<dyn ChatHistoryStore>, String> {
    if let Some(url) = &config.conversations_db {
        #[cfg(feature = "sql")]
        return grauwolf_host::memory::sql_chat_history_store(url)
            .await
            .map_err(|_| "GRAUWOLF_CONVERSATIONS_DB: cannot open the database".to_owned());
        #[cfg(not(feature = "sql"))]
        {
            let _ = url;
            return Err(
                "GRAUWOLF_CONVERSATIONS_DB needs a build with the feature `sql`".to_owned(),
            );
        }
    }
    Ok(match &config.conversations_dir {
        Some(dir) => Arc::new(FileChatHistoryStore::new(dir)),
        None => Arc::new(MemoryChatHistoryStore::default()),
    })
}

/// The routes of the service on the main port: `/run`, `/run/stream`, `/health`, approvals,
/// A2A and MCP (`/mcp`), protected by the tokens of `config`.
pub async fn router(
    agent: Arc<dyn ServedAgent>,
    spec: &AgentSpec,
    config: &ServiceConfig,
) -> Result<axum::Router, String> {
    let history = history_store(config).await?;
    Ok(routers_with_history(agent, spec, config, history)
        .await?
        .main)
}

/// The routes of the main port and, with `config.spiffe_inbound`, of the mTLS port (ADR-0109).
pub struct ServiceRouters {
    /// The routes of the main port.
    pub main: axum::Router,
    /// The routes of the mTLS port: the same as on the main port, but the A2A agent card names
    /// the mTLS URL, because an A2A client sends its messages to the endpoint of the card.
    pub mtls: Option<axum::Router>,
}

/// Like [`router`] with the conversation store `history`, for both ports.
pub async fn routers_with_history(
    agent: Arc<dyn ServedAgent>,
    spec: &AgentSpec,
    config: &ServiceConfig,
    history: Arc<dyn ChatHistoryStore>,
) -> Result<ServiceRouters, String> {
    let options = ServiceOptions {
        chat_api: config.chat_api.as_ref().map(|chat_api| chat_api.options()),
        ..ServiceOptions::default()
    };
    let mcp = grauwolf_host::mcp::agent_server(spec, &config.data_dir, Vec::new()).await?;
    let core = crate::router_for(Arc::clone(&agent), Arc::clone(&history), options.clone()).merge(
        crate::mcp_router_for(mcp, Arc::clone(&agent), description(spec), options.clone()),
    );
    // A2A once per port: the agent card of each names its own URL.
    let a2a = |base_url: &str| {
        crate::a2a::router_for(
            Arc::clone(&agent),
            Arc::clone(&history),
            AgentCardInfo::from_spec(spec, base_url),
            options.clone(),
        )
    };
    let mtls = config.spiffe_inbound.as_ref().map(|inbound| {
        let url = config
            .spiffe
            .as_ref()
            .and_then(|spiffe| spiffe.public_url.clone())
            .unwrap_or_else(|| format!("https://localhost:{}", inbound.port));
        config.secure(core.clone().merge(a2a(&url)))
    });
    Ok(ServiceRouters {
        main: config.secure(core.merge(a2a(&config.public_url()))),
        mtls,
    })
}

/// A2A alone, for the additional port, below `config.a2a_base_path`.
fn a2a_router(
    agent: Arc<dyn ServedAgent>,
    spec: &AgentSpec,
    config: &ServiceConfig,
    port: u16,
    history: Arc<dyn ChatHistoryStore>,
) -> axum::Router {
    let base = config
        .a2a_public_url
        .clone()
        .unwrap_or_else(|| format!("http://localhost:{port}"));
    let info = AgentCardInfo::from_spec(spec, &format!("{base}{}", config.a2a_base_path));
    let router = crate::a2a::router_for(agent, history, info, ServiceOptions::default());
    let router = if config.a2a_base_path.is_empty() {
        router
    } else {
        axum::Router::new().nest(&config.a2a_base_path, router)
    };
    config.secure(router)
}

fn description(spec: &AgentSpec) -> String {
    spec.system_prompt
        .clone()
        .unwrap_or_else(|| format!("grauwolf agent {}", spec.name))
}

/// Serves `agent` until SIGTERM or SIGINT; running requests are finished first.
pub async fn serve(
    agent: Arc<dyn ServedAgent>,
    spec: &AgentSpec,
    config: ServiceConfig,
) -> Result<(), String> {
    config.check_access()?;
    let main = tokio::net::TcpListener::bind(SocketAddr::new(config.bind, config.port))
        .await
        .map_err(|error| format!("port {}: {error}", config.port))?;
    let history = history_store(&config).await?;
    let ServiceRouters {
        main: router,
        mtls: mtls_router,
    } = routers_with_history(Arc::clone(&agent), spec, &config, Arc::clone(&history)).await?;
    eprintln!(
        "grauwolf service for {} on http://{}",
        spec.name,
        main.local_addr().map_err(|error| error.to_string())?
    );
    let shutdown = shutdown_signal();
    // The second listener (ADR-0109): the same routes over mTLS, client certificates required.
    let mtls_server = async {
        let (Some(inbound), Some(mtls_router)) = (&config.spiffe_inbound, mtls_router) else {
            return Ok(());
        };
        let listener = tokio::net::TcpListener::bind(SocketAddr::new(config.bind, inbound.port))
            .await
            .map_err(|error| format!("SPIFFE port {}: {error}", inbound.port))?;
        eprintln!(
            "grauwolf mTLS (SPIFFE) for {} on port {}",
            spec.name, inbound.port
        );
        inbound.serve(listener, mtls_router, shutdown.clone()).await
    };
    let main_server = axum::serve(main, router).with_graceful_shutdown(shutdown.clone());
    let a2a_server = async {
        let Some(port) = config.a2a_port else {
            return Ok(());
        };
        let listener = tokio::net::TcpListener::bind(SocketAddr::new(config.bind, port))
            .await
            .map_err(|error| format!("A2A port {port}: {error}"))?;
        eprintln!(
            "grauwolf A2A for {} on port {port}{}",
            spec.name, config.a2a_base_path
        );
        axum::serve(listener, a2a_router(agent, spec, &config, port, history))
            .with_graceful_shutdown(shutdown.clone())
            .await
            .map_err(|error| error.to_string())
    };
    let (main_result, a2a_result, mtls_result) = tokio::join!(
        async { main_server.await.map_err(|error| error.to_string()) },
        a2a_server,
        mtls_server
    );
    main_result.and(a2a_result).and(mtls_result)
}

/// Completes on SIGTERM or SIGINT (Ctrl+C).
pub(crate) fn shutdown_signal() -> impl std::future::Future<Output = ()> + Clone {
    use futures_util::FutureExt;
    async {
        let interrupt = async {
            let _ = tokio::signal::ctrl_c().await;
        };
        #[cfg(unix)]
        {
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
            let terminated = async {
                match terminate.as_mut() {
                    Some(signal) => {
                        signal.recv().await;
                    }
                    None => std::future::pending().await,
                }
            };
            tokio::select! { () = interrupt => {}, () = terminated => {} }
        }
        #[cfg(not(unix))]
        interrupt.await;
    }
    .shared()
}

/// `main` of a generated service: reads `spec_json` and the environment, serves the agent
/// `run` and returns the exit code (errors go to stderr).
pub fn main(spec_json: &str, provision: ProvisionApp, run: RunApp) -> ExitCode {
    let mut tracing = None;
    let result = (|| {
        let spec = AgentSpec::from_json(spec_json).map_err(|error| error.to_string())?;
        // Credential sources (ADR-0110): `GRAUWOLF_CREDENTIALS`, else a `grauwolf.toml` in the
        // data directory; every variable below is read through them.
        let data_dir = ProcessEnv
            .var("GRAUWOLF_DATA_DIR")
            .unwrap_or_else(|| ".".to_owned());
        let env = grauwolf_host::credentials::env_for(
            Arc::new(ProcessEnv),
            Some(&std::path::Path::new(&data_dir).join("agent.json")),
        )?;
        let mut config = ServiceConfig::from_env_with(|name| env.var(name))?;
        // Outside of the runtime: the export sets up its own threads (ADR-0123).
        tracing = ServiceTracing::start(&config, &*env)?;
        let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
        // Waits for the workload identity up to SPIFFE_FETCH_TIMEOUT; without a socket the
        // start fails here with a clear message (ADR-0109).
        if let Some(settings) = &config.spiffe {
            let anonymous = config.allow_anonymous;
            config.spiffe_inbound = Some(Arc::new(
                runtime.block_on(SpiffeInbound::start(settings, &*env, anonymous))?,
            ));
        }
        let events = config.events.then(stderr_event_log);
        // Executor, MCP servers, knowledge and sub-agents are set up once, here (ADR-0114).
        let provisioned = runtime.block_on(provision(
            config.data_dir.clone(),
            provision_options_logged(
                Arc::clone(&env),
                config.chat_api.as_deref(),
                tracing.as_ref(),
                events.clone(),
            ),
        ))?;
        let mut agent = GeneratedAgent::new(&spec, Arc::new(provisioned), run);
        if let Some(chat_api) = &config.chat_api {
            agent = agent.observed_by(chat_api, &spec.name);
        }
        if let Some(tracing) = &tracing {
            agent = agent.traced_by(tracing, &spec.name);
        }
        if let Some(events) = events {
            agent = agent.logged_by(events);
        }
        let agent = Arc::new(agent);
        runtime.block_on(serve(agent, &spec, config))
    })();
    // Ctrl+C and SIGTERM end `serve`; the pending spans are sent before the process ends.
    let flushed = tracing.map_or(Ok(()), ServiceTracing::shutdown);
    let result = result.and(flushed);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}
