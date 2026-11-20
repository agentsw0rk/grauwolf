//! Agents as an HTTP service (ADR-0053): `GET /health`, `POST /run`, `POST /run/stream` (SSE).

pub mod a2a;
mod approvals;
mod auth;
pub mod chat_api;
pub mod identity;
pub mod observations;
#[cfg(feature = "service")]
pub mod service;
pub mod spiffe;

pub use approvals::DEFAULT_APPROVAL_TIMEOUT;
pub use auth::{Users, new_user_token, token_hash, with_auth, with_spiffe_auth};
pub use identity::identity_router;
pub use spiffe::{CallerMap, CallerSpiffe, PeerSpiffe, SpiffeGate, SpiffeInbound};

use std::convert::Infallible;
use std::sync::Arc;

use auth::{Caller, Delegated};
use axum::extract::{FromRequestParts, State};
use axum::http::{HeaderMap, StatusCode, request::Parts};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use futures_util::Stream;
use futures_util::future::BoxFuture;
use grauwolf_agents::events::{AgentEvent, Feature, TraceParent};
use grauwolf_agents::memory::{ChatHistoryStore, FactMemory, conversation_key};
use grauwolf_host::provisioning::ProvisionedAgent;
use grauwolf_interpreter::{InterpretedAgent, RunExtras};
use grauwolf_llm::StreamFrame;
use grauwolf_mcp::McpServer;
use grauwolf_prompt::ToolDescriptor;
use grauwolf_prompt::{Attachment, AttachmentData};
use grauwolf_tools::{DynTool, ToolError};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// Builds the tools of a user that replace same-named tools of the agent for their runs,
/// e.g. their long-term memory (ADR-0059).
pub type UserTools = Arc<dyn Fn(&str) -> Vec<Arc<dyn DynTool>> + Send + Sync>;

/// Loads tools before every request that only come into being at run time, e.g. from a tools
/// directory (ADR-0074); they are added to the user's tools.
pub type RunTools = Arc<dyn Fn() -> BoxFuture<'static, Vec<Arc<dyn DynTool>>> + Send + Sync>;

/// An agent that the service can serve: interpreted ([`InterpretedAgent`]) or generated and
/// wrapped by the facade (ADR-0093). The routes need nothing else from it.
pub trait ServedAgent: Send + Sync {
    /// JSON schema of the input if it is a named type; `None` for text input (this decides
    /// the argument of the MCP tool).
    fn input_schema(&self) -> Option<Value>;

    /// Runs the agent once. `extras` are the handles of the request (conversation, tools,
    /// memory, approver, frame sink, attachments); `user` is the user of the run. An agent
    /// may ignore what it does not support. Errors are reported as text.
    fn run(
        &self,
        input: Value,
        extras: RunExtras,
        user: Option<String>,
    ) -> BoxFuture<'_, Result<Value, String>>;
}

impl ServedAgent for InterpretedAgent {
    fn input_schema(&self) -> Option<Value> {
        InterpretedAgent::input_schema(self).cloned()
    }

    fn run(
        &self,
        input: Value,
        extras: RunExtras,
        _user: Option<String>,
    ) -> BoxFuture<'_, Result<Value, String>> {
        Box::pin(async move {
            self.run_with(input, extras)
                .await
                .map_err(|error| error.to_string())
        })
    }
}

/// A served agent as a tool for MCP: text input as `{ "request": "<text>" }`, a named type as
/// its schema; structured outputs come back as JSON text.
struct ServedTool {
    descriptor: ToolDescriptor,
    agent: Arc<dyn ServedAgent>,
    json_input: bool,
    options: ServiceOptions,
    user: Option<String>,
    /// SPIFFE ID of the caller (ADR-0109).
    caller: Option<String>,
    /// Trace context of the caller (ADR-0123).
    trace_parent: Option<TraceParent>,
    /// The person the call acts for (ADR-0111).
    delegated: Option<Delegated>,
}

impl DynTool for ServedTool {
    fn name(&self) -> &str {
        &self.descriptor.name
    }

    fn descriptor(&self) -> ToolDescriptor {
        self.descriptor.clone()
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let input = if self.json_input {
                args
            } else {
                let request = args["request"]
                    .as_str()
                    .ok_or_else(|| ToolError::InvalidArgs {
                        tool: self.descriptor.name.clone(),
                        reason: "missing field 'request' (text)".into(),
                    })?;
                Value::String(request.to_owned())
            };
            let mut extras = self.options.extras_for(self.user.as_deref()).await;
            extras.caller = self.caller.clone();
            extras.trace_parent = self.trace_parent.clone();
            let output = run_agent(
                &*self.agent,
                input,
                extras,
                self.user.clone(),
                self.delegated.as_ref(),
            )
            .await;
            match output {
                Ok(Value::String(text)) => Ok(text),
                Ok(other) => Ok(other.to_string()),
                Err(error) => Err(ToolError::Execution(error)),
            }
        })
    }
}

/// Loaded agent, storage for conversations and tools per user.
#[derive(Clone)]
struct Service {
    agent: Arc<dyn ServedAgent>,
    history: Arc<dyn ChatHistoryStore>,
    /// Tools and memory per run (ADR-0059, ADR-0069, ADR-0074).
    handles: ServiceOptions,
    /// Pending approvals of tool calls (ADR-0061).
    approvals: Arc<approvals::Approvals>,
}

/// Body of `/run` and `/run/stream`.
#[derive(Deserialize)]
struct RunRequest {
    /// Input: text or JSON according to the input type of the spec.
    input: Value,
    /// Continues this conversation (ADR-0037).
    #[serde(default)]
    conversation_id: Option<String>,
    /// User of the run; selects their tools (ADR-0059). The service trusts this value.
    #[serde(default)]
    user_id: Option<String>,
    /// Attachments of the input (ADR-0081).
    #[serde(default)]
    attachments: Vec<RequestAttachment>,
}

/// Attachment in a request: content as Base64 in `data` or a reference in `url`.
#[derive(Deserialize)]
struct RequestAttachment {
    media_type: String,
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

impl RequestAttachment {
    fn to_attachment(&self) -> Result<Attachment, &'static str> {
        let data = match (&self.data, &self.url) {
            (Some(data), None) => AttachmentData::Base64(data.clone()),
            (None, Some(url)) => AttachmentData::Url(url.clone()),
            _ => return Err("give exactly one of data and url"),
        };
        Ok(Attachment {
            media_type: self.media_type.clone(),
            data,
        })
    }
}

/// A memory that cannot be reached; every use reports why.
struct NoMemory(String);

impl FactMemory for NoMemory {
    fn recall<'a>(
        &'a self,
        _: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, grauwolf_agents::memory::MemoryError>> {
        Box::pin(async move { Err(self.0.clone().into()) })
    }

    fn remember<'a>(
        &'a self,
        _: &'a [String],
    ) -> BoxFuture<'a, Result<(), grauwolf_agents::memory::MemoryError>> {
        Box::pin(async move { Err(self.0.clone().into()) })
    }
}

/// Memory of a user (without a user: the shared area) for automatic recall
/// and storing (ADR-0069).
pub type UserFacts = Arc<dyn Fn(Option<&str>) -> Arc<dyn FactMemory> + Send + Sync>;

/// What a service can do besides the agent and the conversation store.
#[derive(Clone, Default)]
pub struct ServiceOptions {
    /// Tools per user, e.g. their memory (ADR-0059).
    pub user_tools: Option<UserTools>,
    /// Tools that are reloaded before every request (ADR-0074).
    pub run_tools: Option<RunTools>,
    /// Memory per user for automatic recall/storing, if the spec requires it
    /// (ADR-0069).
    pub facts: Option<UserFacts>,
    /// Switches on `POST /api/chat` and `GET /api/signature` (ADR-0094).
    pub chat_api: Option<ChatApiOptions>,
}

/// What the chat API needs besides the agent (ADR-0094).
#[derive(Clone)]
pub struct ChatApiOptions {
    /// Content of the signature file, `null` in the response if there is none.
    pub signature: Option<Value>,
    /// Produces the `observations` object of `GET /api/signature` when asked.
    pub observations: Arc<dyn Fn() -> BoxFuture<'static, Value> + Send + Sync>,
}

impl ServiceOptions {
    /// The handles of a provisioned agent (ADR-0114): memory tools and facts, key-value store and
    /// sub-agents per user, the tools directory reloaded per request. A user for whom the
    /// agent cannot be provisioned (an invalid user ID) gets no memory.
    pub fn provisioned(agent: Arc<ProvisionedAgent>) -> Self {
        let facts = agent.has_automatic_memory().then(|| {
            let agent = Arc::clone(&agent);
            Arc::new(move |user: Option<&str>| -> Arc<dyn FactMemory> {
                match agent.run(user).map(|run| run.facts()) {
                    Ok(Some(facts)) => facts,
                    Ok(None) => Arc::new(NoMemory("this agent has no memory".to_owned())),
                    Err(error) => Arc::new(NoMemory(error)),
                }
            }) as UserFacts
        });
        let user_tools = Arc::clone(&agent);
        let directory = Arc::clone(&agent);
        Self {
            user_tools: Some(Arc::new(move |user: &str| {
                user_tools
                    .run(Some(user))
                    .map(|run| run.user_tools())
                    .unwrap_or_default()
            })),
            run_tools: Some(Arc::new(move || {
                let directory = Arc::clone(&directory);
                Box::pin(async move {
                    match directory.run(None) {
                        Ok(run) => run.directory_tools().await,
                        Err(_) => Vec::new(),
                    }
                })
            })),
            facts,
            ..Self::default()
        }
    }

    /// Handles of a run for `user` (without: the shared area): the user's tools, freshly
    /// loaded run-time tools and memory – the same for `/run`, A2A and MCP (ADR-0089).
    pub async fn extras_for(&self, user: Option<&str>) -> RunExtras {
        let mut tools = match (&self.user_tools, user) {
            (Some(user_tools), Some(user)) => user_tools(user),
            _ => Vec::new(),
        };
        if let Some(run_tools) = &self.run_tools {
            tools.extend(run_tools().await);
        }
        RunExtras {
            tools,
            memory: self.facts.as_ref().map(|facts| facts(user)),
            ..RunExtras::default()
        }
    }
}

/// Router for `agent`; conversations live in `history`. Token protection is added with
/// [`with_token`] or [`with_auth`].
pub fn router(agent: Arc<InterpretedAgent>, history: Arc<dyn ChatHistoryStore>) -> Router {
    router_with(agent, history, ServiceOptions::default())
}

/// Like [`router`]; requests with `user_id` additionally run with the tools from `user_tools`
/// (ADR-0059), requests without one with those of the agent.
pub fn router_with_user_tools(
    agent: Arc<InterpretedAgent>,
    history: Arc<dyn ChatHistoryStore>,
    user_tools: UserTools,
) -> Router {
    router_with(
        agent,
        history,
        ServiceOptions {
            user_tools: Some(user_tools),
            ..ServiceOptions::default()
        },
    )
}

/// Router for `agent` with all options from `options`.
pub fn router_with(
    agent: Arc<InterpretedAgent>,
    history: Arc<dyn ChatHistoryStore>,
    options: ServiceOptions,
) -> Router {
    router_for(agent, history, options)
}

/// Like [`router_with`] for any [`ServedAgent`], e.g. a generated one (ADR-0093).
pub fn router_for(
    agent: Arc<dyn ServedAgent>,
    history: Arc<dyn ChatHistoryStore>,
    options: ServiceOptions,
) -> Router {
    service_router(Service {
        agent,
        history,
        handles: options,
        approvals: Arc::default(),
    })
}

fn service_router(service: Service) -> Router {
    let approvals = Router::new()
        .route("/approvals/{id}", post(approvals::decide))
        .with_state(Arc::clone(&service.approvals));
    let mut router = Router::new()
        .route("/run", post(run))
        .route("/run/stream", post(run_stream))
        .route("/health", get(|| async { "ok" }));
    if service.handles.chat_api.is_some() {
        router = router
            .route("/api/chat", post(chat))
            .route("/api/signature", get(signature));
    }
    router.with_state(service).merge(approvals)
}

/// Router with `POST /mcp` for `server` (Streamable HTTP, stateless, ADR-0056): responses as
/// JSON, notifications with `202 Accepted`.
pub fn mcp_router(server: McpServer) -> Router {
    Router::new()
        .route("/mcp", post(mcp))
        .with_state(Arc::new(server))
}

/// Like [`mcp_router`] with `agent` as the only tool (name of the server, `description`); every
/// call runs with the handles from `options` for the user of the token – the user's tools,
/// tools directory, memory (ADR-0089).
pub fn mcp_router_with(
    server: McpServer,
    agent: Arc<InterpretedAgent>,
    description: String,
    options: ServiceOptions,
) -> Router {
    mcp_router_for(server, agent, description, options)
}

/// Like [`mcp_router_with`] for any [`ServedAgent`] (ADR-0093).
pub fn mcp_router_for(
    server: McpServer,
    agent: Arc<dyn ServedAgent>,
    description: String,
    options: ServiceOptions,
) -> Router {
    Router::new()
        .route("/mcp", post(agent_mcp))
        .with_state(Arc::new(AgentMcp {
            server,
            agent,
            description,
            options,
        }))
}

/// State of [`mcp_router_with`].
struct AgentMcp {
    server: McpServer,
    agent: Arc<dyn ServedAgent>,
    description: String,
    options: ServiceOptions,
}

async fn agent_mcp(
    State(mcp): State<Arc<AgentMcp>>,
    caller: Option<Extension<Caller>>,
    spiffe: Option<Extension<CallerSpiffe>>,
    delegation: Option<Extension<Delegated>>,
    InboundTrace(trace_parent): InboundTrace,
    Json(message): Json<Value>,
) -> Response {
    let user = match caller {
        Some(Extension(Caller::User(user))) => Some(user),
        _ => None,
    };
    let schema = mcp.agent.input_schema();
    let tool: Arc<dyn DynTool> = Arc::new(ServedTool {
        descriptor: ToolDescriptor {
            name: mcp.server.name().to_owned(),
            description: mcp.description.clone(),
            parameters: schema.clone().unwrap_or_else(|| {
                json!({
                    "type": "object",
                    "properties": {
                        "request": { "type": "string", "description": "Request to the agent" }
                    },
                    "required": ["request"]
                })
            }),
        },
        agent: Arc::clone(&mcp.agent),
        json_input: schema.is_some(),
        options: mcp.options.clone(),
        user,
        caller: spiffe_id(spiffe),
        trace_parent,
        delegated: delegated(delegation),
    });
    let server = mcp.server.clone().with_tools(vec![tool]);
    match server.handle(message).await {
        Some(response) => Json(response).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

async fn mcp(State(server): State<Arc<McpServer>>, Json(message): Json<Value>) -> Response {
    match server.handle(message).await {
        Some(response) => Json(response).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// Requires `Authorization: Bearer <token>` for `router`, otherwise `401`; `/health` and
/// `/.well-known/…` stay open (ADR-0053, ADR-0054). Short form of [`with_auth`] without
/// user tokens.
pub fn with_token(router: Router, token: impl Into<String>) -> Router {
    with_auth(router, Some(token.into()), Users::default())
}

impl Service {
    /// Handles of the run; the conversation belongs to the user of the request (ADR-0060). Invalid
    /// IDs and attachments without exactly one of `data` and `url` yield `400`. The run-time tools
    /// are reloaded here (ADR-0074).
    async fn extras(&self, request: &RunRequest) -> Result<RunExtras, (StatusCode, Json<Value>)> {
        let conversation = request
            .conversation_id
            .as_deref()
            .map(|id| conversation_key(request.user_id.as_deref(), id))
            .transpose()
            .map_err(|error| (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))))?;
        let attachments = request
            .attachments
            .iter()
            .enumerate()
            .map(|(index, attachment)| {
                attachment.to_attachment().map_err(|error| {
                    let error = format!("attachments/{index}: {error}");
                    (StatusCode::BAD_REQUEST, Json(json!({ "error": error })))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RunExtras {
            conversation: conversation.map(|key| (Arc::clone(&self.history), key)),
            attachments,
            ..self.handles.extras_for(request.user_id.as_deref()).await
        })
    }
}

/// With a user token, the run belongs to the user of the token; a different `user_id` yields
/// `403` (ADR-0068).
fn as_caller(
    mut request: RunRequest,
    caller: Option<Extension<Caller>>,
) -> Result<RunRequest, (StatusCode, Json<Value>)> {
    if let Some(Extension(Caller::User(user))) = caller {
        if request
            .user_id
            .as_ref()
            .is_some_and(|claimed| claimed != &user)
        {
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "user_id does not match the token" })),
            ));
        }
        request.user_id = Some(user);
    }
    if let Some(user) = &request.user_id {
        grauwolf_agents::memory::check_user_id(user)
            .map_err(|error| (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))))?;
    }
    Ok(request)
}

async fn run(
    State(service): State<Service>,
    caller: Option<Extension<Caller>>,
    spiffe: Option<Extension<CallerSpiffe>>,
    delegation: Option<Extension<Delegated>>,
    InboundTrace(trace_parent): InboundTrace,
    Json(request): Json<RunRequest>,
) -> (StatusCode, Json<Value>) {
    let request = match as_caller(request, caller) {
        Ok(request) => request,
        Err(rejection) => return rejection,
    };
    let mut extras = match service.extras(&request).await {
        Ok(extras) => extras,
        Err(rejection) => return rejection,
    };
    extras.caller = spiffe_id(spiffe);
    extras.trace_parent = trace_parent;
    let delegated = delegated(delegation);
    match run_agent(
        &*service.agent,
        request.input,
        extras,
        request.user_id,
        delegated.as_ref(),
    )
    .await
    {
        Ok(output) => (StatusCode::OK, Json(json!({ "output": output }))),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": error })),
        ),
    }
}

/// Forwards the events of a run to the SSE channel.
struct Forward(mpsc::UnboundedSender<Event>);

impl Feature for Forward {
    fn on_event(&self, event: &AgentEvent) {
        if let Ok(event) = Event::default().event("agent").json_data(event) {
            let _ = self.0.send(event);
        }
    }
}

async fn run_stream(
    State(service): State<Service>,
    caller: Option<Extension<Caller>>,
    spiffe: Option<Extension<CallerSpiffe>>,
    delegation: Option<Extension<Delegated>>,
    InboundTrace(trace_parent): InboundTrace,
    Json(request): Json<RunRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, Json<Value>)> {
    let request = as_caller(request, caller)?;
    let mut extras = service.extras(&request).await?;
    extras.caller = spiffe_id(spiffe);
    extras.trace_parent = trace_parent;
    let (sender, receiver) = mpsc::unbounded_channel();
    let frames = sender.clone();
    extras.feature = Some(Arc::new(Forward(sender.clone())));
    // Tools that require approval ask via this stream (ADR-0061).
    extras.approver = Some(service.approvals.approver(
        request.user_id.clone(),
        sender.clone(),
        DEFAULT_APPROVAL_TIMEOUT,
    ));
    extras.frame_sink = Some(Arc::new(move |frame: &StreamFrame| {
        if let StreamFrame::TextDelta { text } = frame {
            let _ = frames.send(Event::default().event("text").data(text.clone()));
        }
    }));
    let delegated = delegated(delegation);
    tokio::spawn(async move {
        let last = match run_agent(
            &*service.agent,
            request.input,
            extras,
            request.user_id,
            delegated.as_ref(),
        )
        .await
        {
            Ok(output) => Event::default().event("output").json_data(output),
            Err(error) => Event::default().event("error").json_data(error),
        };
        if let Ok(event) = last {
            let _ = sender.send(event);
        }
    });
    Ok(Sse::new(futures_util::stream::unfold(
        receiver,
        |mut receiver| async move { receiver.recv().await.map(|event| (Ok(event), receiver)) },
    )))
}

/// Runs `agent` once – every route goes through here. With a verified delegation (ADR-0111)
/// the run reports the person as `on_behalf_of` and runs inside the delegation scope, so its
/// internal outgoing calls carry the Txn-Token (renewed when it is about to expire).
pub(crate) async fn run_agent(
    agent: &dyn ServedAgent,
    input: Value,
    mut extras: RunExtras,
    user: Option<String>,
    delegated: Option<&Delegated>,
) -> Result<Value, String> {
    let delegation = delegated.map(|delegated| {
        extras.on_behalf_of = Some(delegated.on_behalf_of());
        delegated.delegation()
    });
    grauwolf_tools::with_delegation(delegation, agent.run(input, extras, user)).await
}

/// The verified delegation of a request (ADR-0111), set by the authentication layer.
fn delegated(extension: Option<Extension<Delegated>>) -> Option<Delegated> {
    extension.map(|Extension(delegated)| delegated)
}

/// The SPIFFE ID of an authenticated workload caller, as text for `agent_starting` (ADR-0109).
fn spiffe_id(caller: Option<Extension<CallerSpiffe>>) -> Option<String> {
    caller.map(|Extension(CallerSpiffe(id))| id.to_string())
}

/// The trace context that a caller sent (ADR-0123): the raw `traceparent` and `tracestate`
/// headers. The tracing feature checks them and ignores what is malformed; without tracing they
/// are not used at all. They link spans and grant nothing, so they are accepted from every
/// caller that passed authentication (this extractor runs inside the authentication layer), and
/// they are never logged.
pub(crate) struct InboundTrace(pub(crate) Option<TraceParent>);

/// `traceparent` and `tracestate` of `headers`, when there is a `traceparent`.
pub(crate) fn trace_parent_of(headers: &HeaderMap) -> Option<TraceParent> {
    let text = |name: &str| {
        let values: Vec<_> = headers
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect();
        (!values.is_empty()).then(|| values.join(","))
    };
    Some(TraceParent {
        traceparent: text("traceparent")?,
        tracestate: text("tracestate"),
    })
}

impl<S: Send + Sync> FromRequestParts<S> for InboundTrace {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Infallible> {
        Ok(InboundTrace(trace_parent_of(&parts.headers)))
    }
}

/// Error answer `{ "error": … }`.
fn failure(status: StatusCode, error: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": error.into() })))
}

/// `POST /api/chat` (ADR-0094): the same run as `/run` without conversation. Text input comes as
/// `{ "message" }` and text output goes out as `{ "response" }`; typed input is the body itself,
/// typed output the answer itself.
async fn chat(
    State(service): State<Service>,
    caller: Option<Extension<Caller>>,
    spiffe: Option<Extension<CallerSpiffe>>,
    delegation: Option<Extension<Delegated>>,
    InboundTrace(trace_parent): InboundTrace,
    body: axum::body::Bytes,
) -> (StatusCode, Json<Value>) {
    let body: Value = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(error) => {
            return failure(
                StatusCode::BAD_REQUEST,
                format!("body is not JSON: {error}"),
            );
        }
    };
    let input = if service.agent.input_schema().is_some() {
        if !body.is_object() {
            return failure(StatusCode::BAD_REQUEST, "the body must be a JSON object");
        }
        body
    } else {
        match body["message"].as_str() {
            Some(message) => Value::String(message.to_owned()),
            None => {
                return failure(
                    StatusCode::BAD_REQUEST,
                    "the body needs a text field 'message'",
                );
            }
        }
    };
    let user = match caller {
        Some(Extension(Caller::User(user))) => Some(user),
        _ => None,
    };
    let mut extras = service.handles.extras_for(user.as_deref()).await;
    extras.caller = spiffe_id(spiffe);
    extras.trace_parent = trace_parent;
    let delegated = delegated(delegation);
    match run_agent(&*service.agent, input, extras, user, delegated.as_ref()).await {
        Ok(Value::String(response)) => (StatusCode::OK, Json(json!({ "response": response }))),
        Ok(output) => (StatusCode::OK, Json(output)),
        Err(error) => failure(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

/// `GET /api/signature` (ADR-0094); open, because it contains no secrets (see `auth::check`).
async fn signature(State(service): State<Service>) -> Json<Value> {
    let Some(chat_api) = &service.handles.chat_api else {
        return Json(Value::Null);
    };
    Json(json!({
        "signature": chat_api.signature,
        "observations": (chat_api.observations)().await,
    }))
}
