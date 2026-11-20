//! One trace across agents (ADR-0123): the calls out of a traced run carry `traceparent`, and
//! the service continues an incoming one. Spans go to an in-memory exporter.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_a2a::{A2aClient, AuthMode, ClientAuth, DEFAULT_READ_TIMEOUT};
use grauwolf_agents::events::{Feature, TraceParent};
use grauwolf_agents::memory::{ChatHistoryStore, MemoryChatHistoryStore};
use grauwolf_interpreter::{InterpretedAgent, Interpreter, RunExtras};
use grauwolf_llm::PromptExecutor;
use grauwolf_mcp::{McpClient, McpServer, PROTOCOL_VERSION};
use grauwolf_otel::OtelFeature;
use grauwolf_server::a2a::AgentCardInfo;
use grauwolf_server::{
    CallerMap, ChatApiOptions, ServedAgent, ServiceOptions, SpiffeInbound, Users,
};
use grauwolf_spec::{AgentSpec, AgentSpecExt};
use grauwolf_spiffe::testing::{FakeIdentity, TestCa};
use grauwolf_test::MockLlmClient;
use grauwolf_tools::DynTool;
use opentelemetry::global::BoxedTracer;
use opentelemetry::trace::TracerProvider;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A provider that keeps its spans in memory.
fn provider() -> (InMemorySpanExporter, SdkTracerProvider) {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    (exporter, provider)
}

fn feature(provider: &SdkTracerProvider, agent: &str) -> Arc<dyn Feature> {
    Arc::new(OtelFeature::new(
        BoxedTracer::new(Box::new(provider.tracer("test"))),
        agent,
    ))
}

fn span<'a>(spans: &'a [SpanData], name: &str) -> &'a SpanData {
    spans
        .iter()
        .find(|span| span.name == name)
        .unwrap_or_else(|| panic!("span {name} missing in {:?}", names(spans)))
}

fn names(spans: &[SpanData]) -> Vec<String> {
    spans.iter().map(|span| span.name.to_string()).collect()
}

/// The coordinator: asks its one external tool `tool` for anything that mentions it.
fn coordinator(provider: &SdkTracerProvider, tool: Arc<dyn DynTool>) -> InterpretedAgent {
    let name = tool.name().to_owned();
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "coordinator",
            "model": { "provider": "mock", "id": "m" },
            "mcp_servers": { "remote": { "command": "unused", "tools": [name] } },
            "graph": {
                "nodes": [ { "id": "call_llm", "kind": "llm_request" },
                           { "id": "execute_tool", "kind": "execute_tool" },
                           { "id": "send_result", "kind": "llm_send_tool_result" } ],
                "edges": [ { "from": "start", "to": "call_llm" },
                           { "from": "call_llm", "to": "execute_tool", "when": { "kind": "on_tool_call" } },
                           { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } },
                           { "from": "execute_tool", "to": "send_result" },
                           { "from": "send_result", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
            }
        })
        .to_string(),
    )
    .unwrap();
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains("Ask", &name, json!({ "request": "Review this." }))
        .answer_when_contains("Looks good", "Done.");
    Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(mock))
            .build(),
    ))
    .with_tools(vec![tool])
    .with_feature(feature(provider, "coordinator"))
    .load(&spec)
    .unwrap()
}

/// `00-<trace id>-<span id>-<flags>` as (trace id, span id).
fn ids(traceparent: &str) -> (String, String) {
    let parts: Vec<_> = traceparent.split('-').collect();
    assert_eq!(parts.len(), 4, "{traceparent}");
    assert_eq!(parts[0], "00");
    (parts[1].to_owned(), parts[2].to_owned())
}

async fn run(agent: InterpretedAgent) {
    assert_eq!(agent.run("Ask the reviewer").await.unwrap(), "Done.");
}

/// A remote A2A agent that answers every message with "Looks good."
async fn remote_a2a() -> MockServer {
    let server = MockServer::start().await;
    let url = server.uri();
    Mock::given(method("GET"))
        .and(path("/.well-known/agent-card.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "reviewer", "description": "Reviews",
            "supportedInterfaces": [ { "protocolBinding": "JSONRPC", "url": format!("{url}/a2a") } ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/a2a"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 1,
            "result": { "message": { "parts": [ { "text": "Looks good." } ] } }
        })))
        .mount(&server)
        .await;
    server
}

/// The `traceparent` headers of the requests to `path` (none without the header).
async fn traceparents(
    server: &MockServer,
    request_path: &str,
    only_method: &str,
) -> Vec<Option<String>> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.url.path() == request_path)
        .filter(|request| {
            only_method.is_empty()
                || serde_json::from_slice::<Value>(&request.body)
                    .is_ok_and(|body| body["method"] == only_method)
        })
        .map(|request| {
            request
                .headers
                .get("traceparent")
                .map(|value| value.to_str().unwrap().to_owned())
        })
        .collect()
}

#[tokio::test]
async fn a_traced_run_sends_its_trace_to_a_remote_a2a_agent() {
    let (exporter, provider) = provider();
    let remote = remote_a2a().await;
    let client = Arc::new(A2aClient::connect(&remote.uri(), &[]).await.unwrap());

    run(coordinator(&provider, client.tool("reviewer", None))).await;

    let spans = exporter.get_finished_spans().unwrap();
    let agent = span(&spans, "invoke_agent coordinator");
    let call = span(&spans, "execute_tool reviewer");
    let sent = traceparents(&remote, "/a2a", "SendMessage").await;
    assert_eq!(sent.len(), 1);
    let (trace, parent) = ids(sent[0].as_deref().expect("traceparent"));
    assert_eq!(trace, agent.span_context.trace_id().to_string());
    assert_eq!(parent, call.span_context.span_id().to_string());
}

#[tokio::test]
async fn a_run_without_tracing_sends_no_traceparent_to_an_a2a_agent() {
    let remote = remote_a2a().await;
    let client = Arc::new(A2aClient::connect(&remote.uri(), &[]).await.unwrap());

    // Without a tracing feature, nothing continues a trace.
    let answer = client
        .tool("reviewer", None)
        .call_json(json!({ "request": "Review this." }))
        .await
        .unwrap();

    assert_eq!(answer, "Looks good.");
    assert_eq!(traceparents(&remote, "/a2a", "SendMessage").await, [None],);
}

fn rpc(id: u64, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// A remote MCP server over Streamable HTTP with the tool `reviewer`.
async fn remote_mcp() -> MockServer {
    let server = MockServer::start().await;
    let respond = |method_name: &str, response: ResponseTemplate| {
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(body_partial_json(json!({ "method": method_name })))
            .respond_with(response)
    };
    respond(
        "initialize",
        ResponseTemplate::new(200).set_body_json(rpc(
            1,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "remote", "version": "1" }
            }),
        )),
    )
    .mount(&server)
    .await;
    respond("notifications/initialized", ResponseTemplate::new(202))
        .mount(&server)
        .await;
    respond(
        "tools/list",
        ResponseTemplate::new(200).set_body_json(rpc(
            2,
            json!({ "tools": [ { "name": "reviewer", "description": "Reviews",
                "inputSchema": { "type": "object" } } ] }),
        )),
    )
    .mount(&server)
    .await;
    respond(
        "tools/call",
        ResponseTemplate::new(200).set_body_json(rpc(
            3,
            json!({ "content": [ { "type": "text", "text": "Looks good." } ] }),
        )),
    )
    .mount(&server)
    .await;
    server
}

#[tokio::test]
async fn a_traced_run_sends_its_trace_to_a_remote_mcp_server() {
    let (exporter, provider) = provider();
    let remote = remote_mcp().await;
    let client = Arc::new(
        McpClient::connect_http(&format!("{}/mcp", remote.uri()), &[])
            .await
            .unwrap(),
    );
    let tools = client.tools().await.unwrap();

    run(coordinator(&provider, tools.into_iter().next().unwrap())).await;

    let spans = exporter.get_finished_spans().unwrap();
    let agent = span(&spans, "invoke_agent coordinator");
    let call = span(&spans, "execute_tool reviewer");
    let sent = traceparents(&remote, "/mcp", "tools/call").await;
    assert_eq!(sent.len(), 1);
    let (trace, parent) = ids(sent[0].as_deref().expect("traceparent"));
    assert_eq!(trace, agent.span_context.trace_id().to_string());
    assert_eq!(parent, call.span_context.span_id().to_string());
    // Only the call carries it: the handshake and the listing happened before the run.
    for request in ["initialize", "tools/list"] {
        assert_eq!(traceparents(&remote, "/mcp", request).await, [None]);
    }
}

#[tokio::test]
async fn an_agent_node_sends_the_trace_of_its_node_to_a_remote_a2a_agent() {
    let (exporter, provider) = provider();
    let remote = remote_a2a().await;
    let client = Arc::new(A2aClient::connect(&remote.uri(), &[]).await.unwrap());
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "flow",
            "model": { "provider": "mock", "id": "m" },
            "agent_tools": { "reviewer": { "spec": "echo.json", "description": "Reviews" } },
            "graph": {
                "nodes": [ { "id": "review", "kind": "agent", "params": { "agent": "reviewer" } } ],
                "edges": [ { "from": "start", "to": "review" },
                           { "from": "review", "to": "finish" } ]
            }
        })
        .to_string(),
    )
    .unwrap();
    let agent = Interpreter::new(Arc::new(PromptExecutor::builder().build()))
        .with_tools(vec![client.tool("reviewer", None)])
        .with_feature(feature(&provider, "flow"))
        .load(&spec)
        .unwrap();

    assert_eq!(agent.run("Review this.").await.unwrap(), "Looks good.");

    let spans = exporter.get_finished_spans().unwrap();
    let node = span(&spans, "node review");
    let sent = traceparents(&remote, "/a2a", "SendMessage").await;
    let (trace, parent) = ids(sent[0].as_deref().expect("traceparent"));
    assert_eq!(trace, node.span_context.trace_id().to_string());
    assert_eq!(parent, node.span_context.span_id().to_string());
}

/// Remembers the trace context of each run it gets.
#[derive(Default)]
struct Remember(std::sync::Mutex<Vec<Option<TraceParent>>>);

impl ServedAgent for Remember {
    fn input_schema(&self) -> Option<Value> {
        None
    }

    fn run(
        &self,
        _input: Value,
        extras: RunExtras,
        _user: Option<String>,
    ) -> BoxFuture<'_, Result<Value, String>> {
        self.0.lock().unwrap().push(extras.trace_parent);
        Box::pin(async { Ok(json!("ok")) })
    }
}

const TRACEPARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

/// All routes of a service around `agent`; returns the base URL.
async fn serve_all(agent: Arc<dyn ServedAgent>) -> String {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "reviewer",
            "model": { "provider": "mock", "id": "m" },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let history = Arc::new(MemoryChatHistoryStore::default());
    let options = ServiceOptions {
        chat_api: Some(ChatApiOptions {
            signature: None,
            observations: Arc::new(|| Box::pin(async { json!({}) })),
        }),
        ..ServiceOptions::default()
    };
    let router = grauwolf_server::router_for(agent.clone(), history.clone(), options.clone())
        .merge(grauwolf_server::a2a::router_for(
            agent.clone(),
            history,
            AgentCardInfo::from_spec(&spec, &base),
            options.clone(),
        ))
        .merge(grauwolf_server::mcp_router_for(
            McpServer::new("reviewer", Vec::new()),
            agent,
            "Reviews".into(),
            options,
        ));
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    base
}

/// POST of `body` to `route` with `headers`; the answer must be a success.
async fn post(base: &str, route: &str, headers: &[(&str, &str)], body: Value) -> String {
    let mut request = reqwest::Client::new()
        .post(format!("{base}{route}"))
        .json(&body);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = request.send().await.unwrap();
    assert!(response.status().is_success(), "{route}: {response:?}");
    response.text().await.unwrap()
}

/// One request to each way into the service, with `headers`.
async fn call_every_route(base: &str, headers: &[(&str, &str)]) {
    post(base, "/run", headers, json!({ "input": "hi" })).await;
    post(base, "/run/stream", headers, json!({ "input": "hi" })).await;
    post(base, "/api/chat", headers, json!({ "message": "hi" })).await;
    let message = json!({ "jsonrpc": "2.0", "id": 1, "method": "SendMessage",
                          "params": { "message": { "parts": [ { "text": "hi" } ] } } });
    post(base, "/a2a", headers, message).await;
    let call = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                       "params": { "name": "reviewer", "arguments": { "request": "hi" } } });
    post(base, "/mcp", headers, call).await;
}

#[tokio::test]
async fn every_route_hands_the_incoming_trace_context_to_the_run() {
    let remember = Arc::new(Remember::default());
    let base = serve_all(remember.clone()).await;

    call_every_route(
        &base,
        &[("traceparent", TRACEPARENT), ("tracestate", "vendor=1")],
    )
    .await;

    let expected = Some(TraceParent {
        traceparent: TRACEPARENT.to_owned(),
        tracestate: Some("vendor=1".to_owned()),
    });
    assert_eq!(*remember.0.lock().unwrap(), vec![expected; 5]);
}

#[tokio::test]
async fn without_a_traceparent_the_run_gets_no_trace_context() {
    let remember = Arc::new(Remember::default());
    let base = serve_all(remember.clone()).await;

    call_every_route(&base, &[("tracestate", "vendor=1")]).await;

    assert_eq!(*remember.0.lock().unwrap(), vec![None; 5]);
}

const REVIEWER: &str = "spiffe://agentswork.de/ns/reviewer/sa/reviewer";
const COORDINATOR: &str = "spiffe://agentswork.de/ns/coordinator/sa/coordinator";

/// The reviewer as a service behind the SPIFFE gate on a plain port (JWT-SVIDs), traced into
/// `provider`. Returns its base URL and the CA of the test identities.
async fn traced_reviewer(provider: &SdkTracerProvider) -> (String, TestCa) {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "reviewer",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let executor = PromptExecutor::builder()
        .client("ollama", Arc::new(MockLlmClient::new("Looks good.")))
        .build();
    let agent = Arc::new(
        Interpreter::new(Arc::new(executor))
            .with_feature(feature(provider, "reviewer"))
            .load(&spec)
            .unwrap(),
    );
    let ca = TestCa::new("agentswork.de");
    let callers = CallerMap::from_json(&json!({ COORDINATOR: "coordinator" }).to_string()).unwrap();
    let inbound = SpiffeInbound::with_identity(
        0,
        Arc::new(FakeIdentity::with_ca(&ca, REVIEWER)),
        callers,
        None,
        &["agentswork.de".to_owned()],
        false,
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let history = Arc::new(MemoryChatHistoryStore::default()) as Arc<dyn ChatHistoryStore>;
    let router = grauwolf_server::with_spiffe_auth(
        grauwolf_server::a2a::router(agent, history, AgentCardInfo::from_spec(&spec, &base)),
        None,
        Users::default(),
        Some(Arc::clone(&inbound.gate)),
    );
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (base, ca)
}

/// ADR-0123 end to end: the coordinator and the reviewer, both traced into one exporter, make
/// one trace; the reviewer still knows who called.
#[tokio::test]
async fn a_call_from_one_traced_agent_to_another_is_one_trace_and_names_the_caller() {
    let (exporter, provider) = provider();
    let (base, ca) = traced_reviewer(&provider).await;
    let auth = ClientAuth::new(
        Arc::new(FakeIdentity::with_ca(&ca, COORDINATOR)),
        AuthMode::Jwt,
        None,
        Some(REVIEWER.to_owned()),
    )
    .unwrap();
    let client = Arc::new(
        A2aClient::connect_with_auth(&base, &[], DEFAULT_READ_TIMEOUT, Some(Arc::new(auth)))
            .await
            .unwrap(),
    );

    run(coordinator(&provider, client.tool("reviewer", None))).await;

    let spans = exporter.get_finished_spans().unwrap();
    let coordinator = span(&spans, "invoke_agent coordinator");
    let call = span(&spans, "execute_tool reviewer");
    let reviewer = span(&spans, "invoke_agent reviewer");
    assert_eq!(
        reviewer.span_context.trace_id(),
        coordinator.span_context.trace_id()
    );
    assert_eq!(reviewer.parent_span_id, call.span_context.span_id());
    let caller = reviewer
        .attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == "grauwolf.caller")
        .map(|attribute| attribute.value.to_string());
    assert_eq!(caller.as_deref(), Some(COORDINATOR));
}

/// A sub-agent in the same process has its own tracing feature, but it runs inside the call of
/// its caller and so continues the caller's trace.
#[tokio::test]
async fn an_agent_in_the_same_process_continues_the_trace_of_the_call_that_runs_it() {
    let (exporter, provider) = provider();
    let sub_spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "translator",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let sub = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("ollama", Arc::new(MockLlmClient::new("Hello")))
            .build(),
    ))
    .with_feature(feature(&provider, "translator"))
    .load(&sub_spec)
    .unwrap();
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "flow",
            "model": { "provider": "mock", "id": "m" },
            "agent_tools": { "translator": { "spec": "echo.json", "description": "Translates" } },
            "graph": {
                "nodes": [ { "id": "translate", "kind": "agent", "params": { "agent": "translator" } } ],
                "edges": [ { "from": "start", "to": "translate" },
                           { "from": "translate", "to": "finish" } ]
            }
        })
        .to_string(),
    )
    .unwrap();
    let agent = Interpreter::new(Arc::new(PromptExecutor::builder().build()))
        .with_tools(vec![Arc::new(sub).into_tool("translator", "Translates")])
        .with_feature(feature(&provider, "flow"))
        .load(&spec)
        .unwrap();

    assert_eq!(agent.run("Hola").await.unwrap(), "Hello");

    let spans = exporter.get_finished_spans().unwrap();
    let outer = span(&spans, "invoke_agent flow");
    let node = span(&spans, "node translate");
    let inner = span(&spans, "invoke_agent translator");
    assert_eq!(inner.span_context.trace_id(), outer.span_context.trace_id());
    assert_eq!(inner.parent_span_id, node.span_context.span_id());
}
