//! The older HTTP+SSE transport of protocol version 2024-11-05 (ADR-0112): `GET` opens an
//! event stream whose first event `endpoint` names the URI for client messages (`POST`);
//! server messages arrive as `message` events.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use reqwest::Url;
use serde_json::{Value, json};
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;

use grauwolf_spiffe::ClientAuth;

use crate::{CONNECT_TIMEOUT, McpError, http_error, initialize_params, result_of};

type Answer = Result<Value, McpError>;

/// What stays the same across reconnects.
struct Settings {
    client: reqwest::Client,
    url: Url,
    headers: Vec<(String, String)>,
    auth: Option<Arc<ClientAuth>>,
    /// Maximum time to wait for the answer to one call (ADR-0065).
    read_timeout: Duration,
}

impl Settings {
    /// `server` names where a failed request went: origin only, the path may carry a session.
    fn error(&self, error: &reqwest::Error, server: &str) -> McpError {
        http_error(error, server, self.read_timeout)
    }

    async fn with_headers(
        &self,
        mut request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, McpError> {
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        if let Some(token) = crate::bearer(self.auth.as_deref()).await? {
            request = request.bearer_auth(token);
        }
        Ok(request)
    }
}

/// One event stream with the endpoint it announced.
struct Link {
    settings: Arc<Settings>,
    endpoint: Url,
    /// Calls waiting for their answer; `None` once the stream is gone.
    pending: StdMutex<Option<HashMap<u64, oneshot::Sender<Answer>>>>,
}

/// A connection: the stream is read by a task that ends with the connection.
struct Connection {
    link: Arc<Link>,
    reader: JoinHandle<()>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// The transport; reconnects once per call after the stream was lost.
pub(crate) struct SseTransport {
    settings: Arc<Settings>,
    connection: Mutex<Option<Arc<Connection>>>,
    next_id: AtomicU64,
}

impl SseTransport {
    pub(crate) fn new(
        url: &str,
        headers: &[(String, String)],
        read_timeout: Duration,
        auth: Option<Arc<ClientAuth>>,
    ) -> Result<Self, McpError> {
        let url = Url::parse(url)
            .map_err(|error| McpError::Http(format!("invalid URL of the MCP server: {error}")))?;
        let client = crate::http_client(
            reqwest::Client::builder().connect_timeout(CONNECT_TIMEOUT),
            url.as_str(),
            auth.as_deref(),
        )?;
        Ok(Self {
            settings: Arc::new(Settings {
                client,
                url,
                headers: headers.to_vec(),
                auth,
                read_timeout,
            }),
            connection: Mutex::new(None),
            next_id: AtomicU64::new(1),
        })
    }

    pub(crate) async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        let connection = self.connection(method).await?;
        connection.exchange(self.next_id(), method, params).await
    }

    pub(crate) async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        let connection = self.connection(method).await?;
        connection
            .link
            .post(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .await
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// The live connection. The first one is opened for `initialize` (the caller performs the
    /// handshake); every later one replaces a lost stream and is opened once, with its own
    /// handshake.
    async fn connection(&self, method: &str) -> Result<Arc<Connection>, McpError> {
        let mut slot = self.connection.lock().await;
        if let Some(connection) = slot
            .as_ref()
            .filter(|connection| !connection.link.is_closed())
        {
            return Ok(Arc::clone(connection));
        }
        let connection = Arc::new(Connection::open(&self.settings).await?);
        if method != "initialize" {
            connection
                .exchange(self.next_id(), "initialize", initialize_params())
                .await?;
            connection
                .link
                .post(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized", "params": {} }))
                .await?;
        }
        *slot = Some(Arc::clone(&connection));
        Ok(connection)
    }
}

impl Connection {
    /// Opens the stream and waits for the `endpoint` event.
    async fn open(settings: &Arc<Settings>) -> Result<Self, McpError> {
        let origin = settings.url.origin().ascii_serialization();
        let (link, response, parser, backlog) =
            tokio::time::timeout(CONNECT_TIMEOUT, Self::open_stream(settings, &origin))
                .await
                .map_err(|_| {
                    McpError::Http(format!(
                        "no endpoint event from {origin} within {} s",
                        CONNECT_TIMEOUT.as_secs()
                    ))
                })??;
        let link = Arc::new(link);
        let reader = tokio::spawn(read_events(Arc::clone(&link), response, parser, backlog));
        Ok(Self { link, reader })
    }

    async fn open_stream(
        settings: &Arc<Settings>,
        origin: &str,
    ) -> Result<(Link, reqwest::Response, SseParser, VecDeque<SseEvent>), McpError> {
        let request = settings
            .client
            .get(settings.url.clone())
            .header("Accept", "text/event-stream");
        let mut response = settings
            .with_headers(request)
            .await?
            .send()
            .await
            .map_err(|error| settings.error(&error, origin))?;
        if !response.status().is_success() {
            return Err(McpError::Http(format!(
                "{origin} answers with {}",
                response.status()
            )));
        }
        let mut parser = SseParser::default();
        let mut backlog = VecDeque::new();
        let endpoint = loop {
            let chunk = response
                .chunk()
                .await
                .map_err(|error| settings.error(&error, origin))?
                .ok_or_else(|| {
                    McpError::Http(format!(
                        "{origin} closed the stream before the endpoint event"
                    ))
                })?;
            let mut events = parser.feed(&chunk).into_iter();
            let found = events.by_ref().find(|event| event.name == "endpoint");
            if let Some(event) = found {
                backlog.extend(events);
                break event.data;
            }
        };
        let endpoint = resolve_endpoint(&settings.url, endpoint.trim())?;
        let link = Link {
            settings: Arc::clone(settings),
            endpoint,
            pending: StdMutex::new(Some(HashMap::new())),
        };
        Ok((link, response, parser, backlog))
    }

    /// Sends the request `id` and waits for its answer on the stream.
    async fn exchange(&self, id: u64, method: &str, params: Value) -> Result<Value, McpError> {
        let (sender, receiver) = oneshot::channel();
        self.link.register(id, sender)?;
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        if let Err(error) = self.link.post(&message).await {
            self.link.forget(id);
            return Err(error);
        }
        let read_timeout = self.link.settings.read_timeout;
        match tokio::time::timeout(read_timeout, receiver).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => Err(self.link.lost("the event stream ended")),
            Err(_) => {
                self.link.forget(id);
                Err(McpError::Timeout {
                    seconds: read_timeout.as_secs_f32(),
                })
            }
        }
    }
}

/// The endpoint announced by the server, resolved against `url`; it must have the origin of
/// `url`, so that headers (e.g. tokens) do not go to another host.
fn resolve_endpoint(url: &Url, announced: &str) -> Result<Url, McpError> {
    let endpoint = url
        .join(announced)
        .map_err(|error| McpError::Protocol(format!("invalid endpoint '{announced}': {error}")))?;
    let same_origin = endpoint.scheme() == url.scheme()
        && endpoint.host_str() == url.host_str()
        && endpoint.port_or_known_default() == url.port_or_known_default();
    if same_origin {
        Ok(endpoint)
    } else {
        Err(McpError::Http(format!(
            "{} announces the endpoint {} on another origin; refusing to send requests there",
            url.origin().ascii_serialization(),
            endpoint.origin().ascii_serialization()
        )))
    }
}

impl Link {
    fn origin(&self) -> String {
        self.endpoint.origin().ascii_serialization()
    }

    fn is_closed(&self) -> bool {
        self.pending.lock().expect("pending calls").is_none()
    }

    fn register(&self, id: u64, sender: oneshot::Sender<Answer>) -> Result<(), McpError> {
        match self.pending.lock().expect("pending calls").as_mut() {
            Some(pending) => {
                pending.insert(id, sender);
                Ok(())
            }
            None => Err(McpError::Http(format!(
                "the event stream of {} was lost; the call was not sent",
                self.origin()
            ))),
        }
    }

    fn forget(&self, id: u64) {
        if let Some(pending) = self.pending.lock().expect("pending calls").as_mut() {
            pending.remove(&id);
        }
    }

    fn lost(&self, reason: &str) -> McpError {
        McpError::Http(format!(
            "{}: {reason}; a call that was already sent may have been executed",
            self.origin()
        ))
    }

    /// Fails all pending calls; the stream is gone.
    fn close(&self, reason: &str) {
        let pending = self.pending.lock().expect("pending calls").take();
        for (_, sender) in pending.into_iter().flatten() {
            let _ = sender.send(Err(self.lost(reason)));
        }
    }

    /// `POST` of a client message to the endpoint; any 2xx is accepted. Errors name the status,
    /// never the body.
    async fn post(&self, message: &Value) -> Result<(), McpError> {
        let mut request = self.settings.client.post(self.endpoint.clone());
        // The trace of the calling run continues in the server (ADR-0123).
        for (name, value) in grauwolf_tools::current_trace_headers() {
            request = request.header(name, value);
        }
        // The person the run acts for (ADR-0111): only to servers with a workload identity.
        if self.settings.auth.is_some()
            && let Some((name, token)) = grauwolf_tools::delegation_header()
                .await
                .map_err(McpError::Http)?
        {
            request = request.header(name, token);
        }
        let origin = self.origin();
        let response = self
            .settings
            .with_headers(request)
            .await?
            .timeout(self.settings.read_timeout)
            .json(message)
            .send()
            .await
            .map_err(|error| self.settings.error(&error, &origin))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(McpError::Http(format!(
                "{origin} answers with {}",
                response.status()
            )))
        }
    }

    /// Answers a request of the server: `ping` with an empty result, everything else with
    /// "method not found". Sent in a task so that the stream keeps being read.
    fn reply_to_request(self: &Arc<Self>, id: Value, method: &str) {
        let reply = if method == "ping" {
            json!({ "jsonrpc": "2.0", "id": id, "result": {} })
        } else {
            json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32601, "message": format!("method not found: {method}") }
            })
        };
        let link = Arc::clone(self);
        tokio::spawn(async move {
            let _ = link.post(&reply).await;
        });
    }

    /// Handles one event of the stream.
    fn dispatch(self: &Arc<Self>, event: &SseEvent) {
        if event.name != "message" {
            return;
        }
        let Ok(message) = serde_json::from_str::<Value>(&event.data) else {
            return;
        };
        // Notifications and server requests carry a method; responses do not.
        if let Some(method) = message["method"].as_str() {
            if let Some(id) = message.get("id") {
                self.reply_to_request(id.clone(), method);
            }
            return;
        }
        let Some(id) = message["id"].as_u64() else {
            return;
        };
        let sender = self
            .pending
            .lock()
            .expect("pending calls")
            .as_mut()
            .and_then(|pending| pending.remove(&id));
        // An unknown ID is ignored (answer to a call that timed out).
        if let Some(sender) = sender {
            let _ = sender.send(result_of(message));
        }
    }
}

/// Reads the stream until it ends, then fails the pending calls.
async fn read_events(
    link: Arc<Link>,
    mut response: reqwest::Response,
    mut parser: SseParser,
    backlog: VecDeque<SseEvent>,
) {
    for event in &backlog {
        link.dispatch(event);
    }
    let reason = loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                for event in parser.feed(&chunk) {
                    link.dispatch(&event);
                }
            }
            Ok(None) => break "the event stream ended".to_owned(),
            Err(error) => break format!("the event stream failed: {error}"),
        }
    };
    link.close(&reason);
}

/// An event of the stream.
struct SseEvent {
    /// Value of `event:`, `message` by default.
    name: String,
    /// The `data:` lines joined with line breaks.
    data: String,
}

/// Incremental parser of an SSE stream: `event:`, `data:` (several lines are joined with
/// `\n`), comments and other fields (`id:`, `retry:`) are ignored; a blank line ends an
/// event; `\n` and `\r\n` end lines.
#[derive(Default)]
struct SseParser {
    buffer: Vec<u8>,
    name: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=end).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line);
            if line.is_empty() {
                let name = self.name.take().unwrap_or_else(|| "message".to_owned());
                let data = std::mem::take(&mut self.data);
                if !data.is_empty() {
                    events.push(SseEvent {
                        name,
                        data: data.join("\n"),
                    });
                }
            } else if !line.starts_with(':') {
                let (field, value) = line.split_once(':').unwrap_or((&line, ""));
                let value = value.strip_prefix(' ').unwrap_or(value);
                match field {
                    "event" => self.name = Some(value.to_owned()),
                    "data" => self.data.push(value.to_owned()),
                    _ => {}
                }
            }
        }
        events
    }
}
