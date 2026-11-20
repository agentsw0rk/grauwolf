//! In-memory MCP server for the HTTP+SSE transport of protocol 2024-11-05 (feature
//! `test-sse-server`, ADR-0112): `GET /sse` opens the event stream and announces the endpoint
//! `/messages?session=<id>`; client messages arrive by `POST` (answered with `202`), their
//! answers go out on the stream.
//!
//! Tools: `echo { text }` (also sends a stray response with an unknown ID and a notification
//! first), `reverse { text }`, `fail` (`isError`), `ask_back` (sends a request to the client and
//! reports its reply), `hang` (never answers). `tools/list` has two pages.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

/// How the server deviates from the plain behavior.
#[derive(Debug, Clone, Default)]
pub struct SseBehavior {
    /// Port to listen on; 0 picks a free one.
    pub port: u16,
    /// End lines of events with `\r\n`.
    pub crlf: bool,
    /// Split every message over several `data:` lines (pretty-printed JSON).
    pub multi_line: bool,
    /// Announce this endpoint (e.g. one on another origin) instead of its own.
    pub endpoint: Option<String>,
    /// Header that every request must carry, otherwise `401` with a body that must not be
    /// repeated.
    pub require_header: Option<(String, String)>,
    /// Status for every `POST` instead of `202`, with a body that must not be repeated.
    pub post_status: Option<u16>,
    /// Close the stream without announcing an endpoint.
    pub no_endpoint: bool,
    /// Offer only the tool `reverse`, on one page (for live checks).
    pub only_reverse: bool,
}

#[derive(Default)]
struct Recorded {
    connections: usize,
    /// Messages of the client, in order of arrival.
    posts: Vec<Value>,
    /// The headers of each message.
    post_headers: Vec<HeaderMap>,
}

struct Shared {
    behavior: SseBehavior,
    /// Open streams by session.
    sessions: Mutex<HashMap<String, mpsc::UnboundedSender<String>>>,
    next_session: AtomicU64,
    recorded: Mutex<Recorded>,
    /// Requests to the client waiting for its reply, by ID.
    waiting: Mutex<HashMap<String, oneshot::Sender<Value>>>,
}

/// A running server on `127.0.0.1`; stops when dropped.
pub struct SseTestServer {
    addr: SocketAddr,
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for SseTestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl SseTestServer {
    /// Server with the plain behavior.
    pub async fn start() -> Self {
        Self::start_with(SseBehavior::default()).await
    }

    /// Server with `behavior`.
    pub async fn start_with(behavior: SseBehavior) -> Self {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", behavior.port))
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("address");
        let shared = Arc::new(Shared {
            behavior,
            sessions: Mutex::default(),
            next_session: AtomicU64::new(1),
            recorded: Mutex::default(),
            waiting: Mutex::default(),
        });
        let app = Router::new()
            .route("/sse", get(open_stream))
            .route("/messages", post(receive))
            .with_state(Arc::clone(&shared));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        Self { addr, shared, task }
    }

    /// `host:port` of the server.
    pub fn host(&self) -> String {
        self.addr.to_string()
    }

    /// URL of the event stream.
    pub fn url(&self) -> String {
        format!("http://{}/sse", self.addr)
    }

    /// Number of streams opened so far.
    pub fn connections(&self) -> usize {
        self.shared.recorded.lock().expect("recorded").connections
    }

    /// The messages the client has sent, in order of arrival.
    pub fn posts(&self) -> Vec<Value> {
        self.shared.recorded.lock().expect("recorded").posts.clone()
    }

    /// The value of the header `name` of each message the client has sent.
    pub fn post_header(&self, name: &str) -> Vec<Option<String>> {
        let recorded = self.shared.recorded.lock().expect("recorded");
        recorded
            .post_headers
            .iter()
            .map(|headers| {
                headers
                    .get(name)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned)
            })
            .collect()
    }

    /// Ends all open streams (the sessions are gone afterwards).
    pub fn drop_streams(&self) {
        self.shared.sessions.lock().expect("sessions").clear();
    }
}

impl Shared {
    /// One event as the server writes it, with a comment and an `id:` line around the data.
    fn event(&self, name: &str, data: &str) -> String {
        let nl = if self.behavior.crlf { "\r\n" } else { "\n" };
        let mut event = format!(": keep-alive{nl}id: 1{nl}event: {name}{nl}");
        for line in data.lines() {
            event.push_str(&format!("data: {line}{nl}"));
        }
        event.push_str(nl);
        event
    }

    fn message(&self, message: &Value) -> String {
        let data = if self.behavior.multi_line {
            serde_json::to_string_pretty(message)
        } else {
            serde_json::to_string(message)
        };
        self.event("message", &data.expect("json"))
    }

    /// The `401` to send if a required header is missing.
    fn rejection(&self, headers: &HeaderMap) -> Option<Response> {
        let (name, value) = self.behavior.require_header.as_ref()?;
        let sent = headers.get(name).and_then(|sent| sent.to_str().ok());
        (sent != Some(value)).then(|| reply(StatusCode::UNAUTHORIZED, "secret-body"))
    }

    fn send(&self, session: &str, event: String) {
        if let Some(stream) = self.sessions.lock().expect("sessions").get(session) {
            let _ = stream.send(event);
        }
    }
}

fn reply(status: StatusCode, body: &'static str) -> Response {
    Response::builder()
        .status(status)
        .body(Body::from(body))
        .expect("response")
}

async fn open_stream(State(shared): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    if let Some(response) = shared.rejection(&headers) {
        return response;
    }
    shared.recorded.lock().expect("recorded").connections += 1;
    let (sender, mut receiver) = mpsc::unbounded_channel::<String>();
    if !shared.behavior.no_endpoint {
        let session = shared
            .next_session
            .fetch_add(1, Ordering::Relaxed)
            .to_string();
        let endpoint = shared
            .behavior
            .endpoint
            .clone()
            .unwrap_or_else(|| format!("/messages?session={session}"));
        let _ = sender.send(shared.event("endpoint", &endpoint));
        shared
            .sessions
            .lock()
            .expect("sessions")
            .insert(session, sender);
    }
    let stream = futures_util::stream::poll_fn(move |cx| {
        receiver
            .poll_recv(cx)
            .map(|event| event.map(|event| Ok::<_, Infallible>(Bytes::from(event))))
    });
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(stream))
        .expect("response")
}

async fn receive(
    State(shared): State<Arc<Shared>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    if let Some(response) = shared.rejection(&headers) {
        return response;
    }
    if let Some(status) = shared.behavior.post_status {
        return reply(StatusCode::from_u16(status).expect("status"), "secret-body");
    }
    let Ok(message) = serde_json::from_str::<Value>(&body) else {
        return reply(StatusCode::BAD_REQUEST, "not json");
    };
    {
        let mut recorded = shared.recorded.lock().expect("recorded");
        recorded.posts.push(message.clone());
        recorded.post_headers.push(headers.clone());
    }
    let session = query.get("session").cloned().unwrap_or_default();
    tokio::spawn(handle(shared, session, message));
    reply(StatusCode::ACCEPTED, "Accepted")
}

/// Answers a client message on the stream of `session`.
async fn handle(shared: Arc<Shared>, session: String, message: Value) {
    let Some(method) = message["method"].as_str() else {
        // A reply of the client to a request of the server.
        let id = message["id"].as_str().unwrap_or_default().to_owned();
        if let Some(waiting) = shared.waiting.lock().expect("waiting").remove(&id) {
            let _ = waiting.send(message);
        }
        return;
    };
    let Some(id) = message.get("id").cloned() else {
        return;
    };
    let params = &message["params"];
    let answer = |result: Value| json!({ "jsonrpc": "2.0", "id": id, "result": result });
    let response = match method {
        "initialize" => answer(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "sse-test", "version": "1" }
        })),
        "tools/list" => answer(tools_list(&shared.behavior, params["cursor"].as_str())),
        "tools/call" => match call_tool(&shared, &session, params).await {
            Some(result) => answer(result),
            None => return,
        },
        other => json!({
            "jsonrpc": "2.0", "id": id,
            "error": { "code": -32601, "message": format!("unknown method {other}") }
        }),
    };
    shared.send(&session, shared.message(&response));
}

fn tool(name: &str, description: &str) -> Value {
    json!({
        "name": name, "description": description,
        "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } } }
    })
}

fn tools_list(behavior: &SseBehavior, cursor: Option<&str>) -> Value {
    if behavior.only_reverse {
        return json!({ "tools": [tool("reverse", "Reverses the text")] });
    }
    match cursor {
        None => json!({
            "tools": [tool("echo", "Repeats the text"), tool("reverse", "Reverses the text")],
            "nextCursor": "page2"
        }),
        Some(_) => json!({
            "tools": [
                tool("fail", "Always fails"),
                tool("ask_back", "Asks the client for its roots"),
                tool("hang", "Never answers")
            ]
        }),
    }
}

fn text(text: impl Into<String>, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }], "isError": is_error })
}

/// Result of a tool call; `None` if the call is never answered.
async fn call_tool(shared: &Arc<Shared>, session: &str, params: &Value) -> Option<Value> {
    let argument = params["arguments"]["text"].as_str().unwrap_or_default();
    match params["name"].as_str()? {
        "echo" => {
            // Noise before the answer: an answer nobody waits for and a notification.
            shared.send(
                session,
                shared.message(&json!({ "jsonrpc": "2.0", "id": 9999, "result": {} })),
            );
            shared.send(
                session,
                shared.message(&json!({
                    "jsonrpc": "2.0", "method": "notifications/message",
                    "params": { "level": "info", "data": "echoing" }
                })),
            );
            Some(text(format!("echo: {argument}"), false))
        }
        "reverse" => Some(text(argument.chars().rev().collect::<String>(), false)),
        "fail" => Some(text("boom", true)),
        "ask_back" => {
            let (sender, receiver) = oneshot::channel();
            shared
                .waiting
                .lock()
                .expect("waiting")
                .insert("server-1".into(), sender);
            shared.send(
                session,
                shared.message(&json!({
                    "jsonrpc": "2.0", "id": "server-1", "method": "roots/list"
                })),
            );
            let reply = receiver.await.ok()?;
            Some(text(
                format!("client answered with error {}", reply["error"]["code"]),
                false,
            ))
        }
        _ => None,
    }
}
