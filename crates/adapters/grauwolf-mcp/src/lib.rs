//! Client for the Model Context Protocol (E09, F09.1, ADR-0036).
//!
//! Speaks JSON-RPC 2.0 (`initialize`, `notifications/initialized`, `tools/list`, `tools/call`)
//! over stdio (line by line), Streamable HTTP (ADR-0051) or the older HTTP+SSE transport
//! (ADR-0112).

mod server;
mod sse;
#[cfg(feature = "test-sse-server")]
pub mod testing;

pub use server::{McpPrompt, McpPromptArgument, McpResource, McpServer};

use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use grauwolf_prompt::{Message, ToolDescriptor};
pub use grauwolf_spiffe::{AuthMode, ClientAuth};
use grauwolf_tools::{DynTool, ToolError};
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, Lines};

use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use sse::SseTransport;

/// Protocol version the client offers.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Error while exchanging messages with an MCP server.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    /// Reading or writing failed.
    #[error("MCP connection broken: {0}")]
    Io(#[from] std::io::Error),
    /// The server closed the connection.
    #[error("MCP server closed the connection")]
    Closed,
    /// The server sent something that is not valid JSON-RPC.
    #[error("invalid answer from the MCP server: {0}")]
    Protocol(String),
    /// The server reports a JSON-RPC error.
    #[error("MCP server reports error {code}: {message}")]
    Rpc { code: i64, message: String },
    /// The tool failed (`isError`); contains its text.
    #[error("MCP tool failed: {0}")]
    Tool(String),
    /// The HTTP request failed or the server responded with an error status.
    #[error("MCP over HTTP: {0}")]
    Http(String),
    /// The server sent nothing over stdio within the read timeout (ADR-0065).
    #[error(
        "timeout: no answer from the MCP server within {seconds} s; the call \
         was sent and may have been executed"
    )]
    Timeout { seconds: f32 },
}

/// A resource of an MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    /// Identifier used to read it.
    pub uri: String,
    /// Anzeigename.
    pub name: String,
    /// Description, if present.
    pub description: String,
    /// MIME-Typ, falls bekannt.
    pub mime_type: Option<String>,
}

/// A prompt template of an MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTemplate {
    /// Name under which it is fetched.
    pub name: String,
    /// Description, if present.
    pub description: String,
    /// Namen ihrer Argumente.
    pub arguments: Vec<String>,
}

/// Read and write side of the connection; one request at a time.
struct Connection {
    lines: Lines<BufReader<Box<dyn AsyncRead + Send + Unpin>>>,
    writer: Box<dyn AsyncWrite + Send + Unpin>,
    next_id: u64,
    /// Maximum time without a line from the server (ADR-0065).
    read_timeout: Duration,
}

/// Maximum time to establish the connection to an MCP server over HTTP (ADR-0065).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum time without data from the MCP server over HTTP (ADR-0065).
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(600);

/// Streamable HTTP (ADR-0051): one `POST` per message, session tracked via `Mcp-Session-Id`.
struct HttpTransport {
    client: reqwest::Client,
    url: String,
    headers: Vec<(String, String)>,
    auth: Option<Arc<ClientAuth>>,
    /// Session and negotiated protocol version after `initialize`.
    session: Mutex<(Option<String>, Option<String>)>,
    next_id: std::sync::atomic::AtomicU64,
    /// Read timeout, for error messages (ADR-0065).
    read_timeout: Duration,
}

/// Transport to the server.
enum Transport {
    Stdio(Mutex<Connection>),
    Http(HttpTransport),
    Sse(SseTransport),
}

/// Verbundener MCP-Client.
pub struct McpClient {
    transport: Transport,
    server_name: String,
    /// Server process for the stdio transport; only kept so that it ends with the client
    /// (`kill_on_drop`).
    _process: Option<Child>,
}

impl McpClient {
    /// Connects over `reader`/`writer` and performs the handshake; read timeout
    /// [`DEFAULT_READ_TIMEOUT`].
    pub async fn connect(
        reader: impl AsyncRead + Send + Unpin + 'static,
        writer: impl AsyncWrite + Send + Unpin + 'static,
    ) -> Result<Self, McpError> {
        Self::connect_with_read_timeout(reader, writer, DEFAULT_READ_TIMEOUT).await
    }

    /// Like [`McpClient::connect`], but waits at most `read_timeout` for each line of the
    /// Servers (ADR-0065).
    pub async fn connect_with_read_timeout(
        reader: impl AsyncRead + Send + Unpin + 'static,
        writer: impl AsyncWrite + Send + Unpin + 'static,
        read_timeout: Duration,
    ) -> Result<Self, McpError> {
        let reader: Box<dyn AsyncRead + Send + Unpin> = Box::new(reader);
        Self::initialize(Transport::Stdio(Mutex::new(Connection {
            lines: BufReader::new(reader).lines(),
            writer: Box::new(writer),
            next_id: 1,
            read_timeout,
        })))
        .await
    }

    /// Connects over Streamable HTTP to the endpoint `url`; `headers` are sent with every
    /// request, e.g. `Authorization` (ADR-0051). Read timeout [`DEFAULT_READ_TIMEOUT`].
    pub async fn connect_http(url: &str, headers: &[(String, String)]) -> Result<Self, McpError> {
        Self::connect_http_with_read_timeout(url, headers, DEFAULT_READ_TIMEOUT).await
    }

    /// Like [`McpClient::connect_http`], but waits at most `read_timeout` for data from the
    /// Servers (ADR-0065).
    pub async fn connect_http_with_read_timeout(
        url: &str,
        headers: &[(String, String)],
        read_timeout: Duration,
    ) -> Result<Self, McpError> {
        Self::connect_http_with_auth(url, headers, read_timeout, None).await
    }

    /// Like [`McpClient::connect_http_with_read_timeout`], with a workload identity (ADR-0109):
    /// mTLS (the URL must be `https://`) and/or a fresh JWT-SVID as bearer for every request.
    pub async fn connect_http_with_auth(
        url: &str,
        headers: &[(String, String)],
        read_timeout: Duration,
        auth: Option<Arc<ClientAuth>>,
    ) -> Result<Self, McpError> {
        let client = http_client(
            reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .read_timeout(read_timeout),
            url,
            auth.as_deref(),
        )?;
        Self::initialize(Transport::Http(HttpTransport {
            client,
            url: url.to_owned(),
            headers: headers.to_vec(),
            auth,
            session: Mutex::new((None, None)),
            next_id: std::sync::atomic::AtomicU64::new(1),
            read_timeout,
        }))
        .await
    }

    /// Connects over the older HTTP+SSE transport (protocol 2024-11-05, ADR-0112): `GET url`
    /// opens the event stream, whose first event names the endpoint for the messages
    /// (same origin as `url`, otherwise the connection is refused); `headers` are sent with
    /// every request. Read timeout [`DEFAULT_READ_TIMEOUT`] per call.
    ///
    /// If the stream is lost, pending calls fail; the next call reconnects once (new stream,
    /// new handshake).
    pub async fn connect_sse(url: &str, headers: &[(String, String)]) -> Result<Self, McpError> {
        Self::connect_sse_with_read_timeout(url, headers, DEFAULT_READ_TIMEOUT).await
    }

    /// Like [`McpClient::connect_sse`], but waits at most `read_timeout` for the answer to
    /// each call (ADR-0065).
    pub async fn connect_sse_with_read_timeout(
        url: &str,
        headers: &[(String, String)],
        read_timeout: Duration,
    ) -> Result<Self, McpError> {
        Self::connect_sse_with_auth(url, headers, read_timeout, None).await
    }

    /// Like [`McpClient::connect_sse_with_read_timeout`], with a workload identity (ADR-0109).
    pub async fn connect_sse_with_auth(
        url: &str,
        headers: &[(String, String)],
        read_timeout: Duration,
        auth: Option<Arc<ClientAuth>>,
    ) -> Result<Self, McpError> {
        Self::initialize(Transport::Sse(SseTransport::new(
            url,
            headers,
            read_timeout,
            auth,
        )?))
        .await
    }

    /// Handshake over `transport`.
    async fn initialize(transport: Transport) -> Result<Self, McpError> {
        let client = Self {
            transport,
            server_name: String::new(),
            _process: None,
        };
        let result = client.request("initialize", initialize_params()).await?;
        client
            .notify("notifications/initialized", json!({}))
            .await?;
        let server_name = result["serverInfo"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        Ok(Self {
            server_name,
            ..client
        })
    }

    /// Starts `command` with `args` as an MCP server and connects over stdin/stdout
    /// (stdio transport). The server's stderr stays visible; the process ends with the client.
    pub async fn spawn(command: &str, args: &[String]) -> Result<Self, McpError> {
        Self::spawn_with_env(command, args, &[]).await
    }

    /// Like [`McpClient::spawn`] with extra environment variables for the child. The child never
    /// inherits the agent's Workload API settings (`SPIFFE_ENDPOINT_SOCKET`,
    /// `SPIFFE_FETCH_TIMEOUT`, ADR-0110): they reach it only if listed in `env`.
    pub async fn spawn_with_env(
        command: &str,
        args: &[String],
        env: &[(String, String)],
    ) -> Result<Self, McpError> {
        let mut child = Command::new(command)
            .args(args)
            .env_remove(grauwolf_spiffe::ENDPOINT_VARIABLE)
            .env_remove(grauwolf_spiffe::FETCH_TIMEOUT_VARIABLE)
            .envs(env.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let stdout = child.stdout.take().ok_or(McpError::Closed)?;
        let stdin = child.stdin.take().ok_or(McpError::Closed)?;
        let client = Self::connect(stdout, stdin).await?;
        Ok(Self {
            _process: Some(child),
            ..client
        })
    }

    /// Server name from the handshake.
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// All tools of the server; follows pagination (`nextCursor`).
    pub async fn list_tools(&self) -> Result<Vec<ToolDescriptor>, McpError> {
        self.pages("tools/list", "tools")
            .await?
            .into_iter()
            .map(|tool| {
                Ok(ToolDescriptor {
                    name: text(&tool, "name")?,
                    description: tool["description"].as_str().unwrap_or_default().to_owned(),
                    parameters: tool
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| json!({ "type": "object" })),
                })
            })
            .collect()
    }

    /// Calls the tool `name` with `arguments`; text contents are joined with line breaks.
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<String, McpError> {
        let result = self
            .request(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
            )
            .await?;
        let content = result["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|part| match part["type"].as_str() {
                Some("text") => part["text"].as_str().map(str::to_owned),
                Some(other) => Some(format!("[{other}-Inhalt]")),
                None => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if result["isError"].as_bool() == Some(true) {
            Err(McpError::Tool(content))
        } else {
            Ok(content)
        }
    }

    /// All resources of the server; follows pagination.
    pub async fn list_resources(&self) -> Result<Vec<Resource>, McpError> {
        let mut resources = Vec::new();
        for page in self.pages("resources/list", "resources").await? {
            resources.push(Resource {
                uri: text(&page, "uri")?,
                name: page["name"].as_str().unwrap_or_default().to_owned(),
                description: page["description"].as_str().unwrap_or_default().to_owned(),
                mime_type: page["mimeType"].as_str().map(str::to_owned),
            });
        }
        Ok(resources)
    }

    /// Content of the resource `uri` as text; binary contents are only described.
    pub async fn read_resource(&self, uri: &str) -> Result<String, McpError> {
        let result = self
            .request("resources/read", json!({ "uri": uri }))
            .await?;
        Ok(result["contents"]
            .as_array()
            .into_iter()
            .flatten()
            .map(
                |content| match (content["text"].as_str(), content["blob"].as_str()) {
                    (Some(text), _) => text.to_owned(),
                    (None, Some(blob)) => format!(
                        "[binary content {}, {} Base64 characters]",
                        content["uri"].as_str().unwrap_or(uri),
                        blob.len()
                    ),
                    _ => String::new(),
                },
            )
            .collect::<Vec<_>>()
            .join("\n"))
    }

    /// All prompt templates of the server; follows pagination.
    pub async fn list_prompts(&self) -> Result<Vec<PromptTemplate>, McpError> {
        let mut prompts = Vec::new();
        for page in self.pages("prompts/list", "prompts").await? {
            prompts.push(PromptTemplate {
                name: text(&page, "name")?,
                description: page["description"].as_str().unwrap_or_default().to_owned(),
                arguments: page["arguments"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|argument| argument["name"].as_str().map(str::to_owned))
                    .collect(),
            });
        }
        Ok(prompts)
    }

    /// The messages of the prompt template `name` with `arguments`; text contents only.
    pub async fn get_prompt(&self, name: &str, arguments: Value) -> Result<Vec<Message>, McpError> {
        let result = self
            .request(
                "prompts/get",
                json!({ "name": name, "arguments": arguments }),
            )
            .await?;
        result["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|message| {
                let content = message["content"]["text"].as_str().unwrap_or_default();
                match message["role"].as_str() {
                    Some("user") => Ok(Message::user(content)),
                    Some("assistant") => Ok(Message::assistant(content)),
                    other => Err(McpError::Protocol(format!(
                        "unknown role {other:?} in {message}"
                    ))),
                }
            })
            .collect()
    }

    /// All entries under `field` across all pages of `method` (`nextCursor`).
    async fn pages(&self, method: &str, field: &str) -> Result<Vec<Value>, McpError> {
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(cursor) => json!({ "cursor": cursor }),
                None => json!({}),
            };
            let result = self.request(method, params).await?;
            items.extend(result[field].as_array().into_iter().flatten().cloned());
            match result["nextCursor"].as_str() {
                Some(next) => cursor = Some(next.to_owned()),
                None => return Ok(items),
            }
        }
    }

    /// Resources as two tools: `<prefix>_list_resources` (list with URI, name,
    /// description, type) and `<prefix>_read_resource { uri }`.
    pub fn resource_tools(self: &Arc<Self>, prefix: &str) -> Vec<Arc<dyn DynTool>> {
        [false, true]
            .into_iter()
            .map(|read| {
                Arc::new(ResourceTool {
                    client: Arc::clone(self),
                    name: format!(
                        "{prefix}_{}",
                        if read {
                            "read_resource"
                        } else {
                            "list_resources"
                        }
                    ),
                    read,
                }) as Arc<dyn DynTool>
            })
            .collect()
    }

    /// All tools of the server as [`DynTool`] for a `ToolRegistry`.
    pub async fn tools(self: &Arc<Self>) -> Result<Vec<Arc<dyn DynTool>>, McpError> {
        Ok(self
            .list_tools()
            .await?
            .into_iter()
            .map(|descriptor| {
                Arc::new(McpTool {
                    client: Arc::clone(self),
                    descriptor,
                }) as Arc<dyn DynTool>
            })
            .collect())
    }

    /// Sends a request and waits for the response with the same ID.
    async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        let connection = match &self.transport {
            Transport::Stdio(connection) => connection,
            Transport::Http(http) => return http.request(method, params).await,
            Transport::Sse(sse) => return sse.request(method, params).await,
        };
        let mut connection = connection.lock().await;
        let id = connection.next_id;
        connection.next_id += 1;
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        send(&mut connection, &message).await?;
        loop {
            let read_timeout = connection.read_timeout;
            let line = tokio::time::timeout(read_timeout, connection.lines.next_line())
                .await
                .map_err(|_| McpError::Timeout {
                    seconds: read_timeout.as_secs_f32(),
                })??
                .ok_or(McpError::Closed)?;
            if line.trim().is_empty() {
                continue;
            }
            let response: Value = serde_json::from_str(&line)
                .map_err(|error| McpError::Protocol(format!("{error}: {line}")))?;
            // Skip notifications and server requests without a matching ID.
            if response.get("id") != Some(&json!(id)) {
                continue;
            }
            return result_of(response);
        }
    }

    /// Sends a notification (without a response).
    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        let connection = match &self.transport {
            Transport::Stdio(connection) => connection,
            Transport::Http(http) => return http.notify(method, params).await,
            Transport::Sse(sse) => return sse.notify(method, params).await,
        };
        let mut connection = connection.lock().await;
        send(
            &mut connection,
            &json!({ "jsonrpc": "2.0", "method": method, "params": params }),
        )
        .await
    }
}

/// Parameters of `initialize`.
fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": { "name": "grauwolf", "version": env!("CARGO_PKG_VERSION") }
    })
}

/// Error while sending or reading over HTTP; the message says whether the call may have run
/// (not if the connection failed, possibly if it timed out) (ADR-0065). `url` names the server.
fn http_error(error: &reqwest::Error, url: &str, read_timeout: Duration) -> McpError {
    McpError::Http(if let Some(reason) = grauwolf_spiffe::tls_refusal(error) {
        format!("TLS handshake with {url} failed: {reason}; the call was not executed")
    } else if error.is_connect() {
        format!(
            "no connection to {url} (refused or given up after {} s); the call was \
             not executed",
            CONNECT_TIMEOUT.as_secs()
        )
    } else if error.is_timeout() {
        format!(
            "timeout: no answer from {url} within {} s; the call was \
             sent and may have been executed",
            read_timeout.as_secs_f32()
        )
    } else {
        error.to_string()
    })
}

/// `result` of a JSON-RPC response, or its error.
fn result_of(response: Value) -> Result<Value, McpError> {
    if let Some(error) = response.get("error") {
        return Err(McpError::Rpc {
            code: error["code"].as_i64().unwrap_or_default(),
            message: error["message"].as_str().unwrap_or_default().to_owned(),
        });
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| McpError::Protocol(format!("answer without result: {response}")))
}

/// A fresh JWT-SVID if the identity works in `jwt` mode.
pub(crate) async fn bearer(auth: Option<&ClientAuth>) -> Result<Option<String>, McpError> {
    match auth {
        Some(auth) => auth
            .bearer()
            .await
            .map_err(|error| McpError::Http(format!("workload identity: {error}"))),
        None => Ok(None),
    }
}

/// The HTTP client; with a TLS configuration of the identity (mTLS) the URL must be `https://`.
pub(crate) fn http_client(
    mut builder: reqwest::ClientBuilder,
    url: &str,
    auth: Option<&ClientAuth>,
) -> Result<reqwest::Client, McpError> {
    if let Some(auth) = auth
        && let Some(tls) = auth
            .tls_config()
            .map_err(|error| McpError::Http(format!("workload identity: {error}")))?
    {
        if !url.starts_with("https://") {
            return Err(McpError::Http(format!(
                "{url}: `auth.spiffe` needs an https:// URL"
            )));
        }
        builder = builder.use_preconfigured_tls(tls);
    }
    builder
        .build()
        .map_err(|error| McpError::Http(error.to_string()))
}

impl HttpTransport {
    /// `POST` with the fixed headers, plus session and protocol version once known.
    async fn post(&self, message: &Value) -> Result<reqwest::Response, McpError> {
        let mut request = self
            .client
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .json(message);
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        // The trace of the calling run continues in the server (ADR-0123).
        for (name, value) in grauwolf_tools::current_trace_headers() {
            request = request.header(name, value);
        }
        if let Some(token) = bearer(self.auth.as_deref()).await? {
            request = request.bearer_auth(token);
        }
        // The person the run acts for (ADR-0111): only to servers with a workload identity.
        if self.auth.is_some()
            && let Some((name, token)) = grauwolf_tools::delegation_header()
                .await
                .map_err(McpError::Http)?
        {
            request = request.header(name, token);
        }
        let (session, version) = self.session.lock().await.clone();
        if let Some(session) = session {
            request = request.header("Mcp-Session-Id", session);
        }
        if let Some(version) = version {
            request = request.header("MCP-Protocol-Version", version);
        }
        let response = request
            .send()
            .await
            .map_err(|error| self.transport_error(&error))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(McpError::Http(format!(
                "{} answers with {status}: {body}",
                self.url
            )));
        }
        Ok(response)
    }

    fn transport_error(&self, error: &reqwest::Error) -> McpError {
        http_error(error, &self.url, self.read_timeout)
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let response = self.post(&message).await?;
        if let Some(session) = response
            .headers()
            .get("Mcp-Session-Id")
            .and_then(|value| value.to_str().ok())
        {
            self.session.lock().await.0 = Some(session.to_owned());
        }
        let is_stream = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));
        let body = response
            .text()
            .await
            .map_err(|error| self.transport_error(&error))?;
        let messages: Vec<Value> = if is_stream {
            sse_data(&body)
                .iter()
                .map(|data| serde_json::from_str(data))
                .collect::<Result<_, _>>()
                .map_err(|error| McpError::Protocol(format!("{error}: {body}")))?
        } else {
            vec![
                serde_json::from_str(&body)
                    .map_err(|error| McpError::Protocol(format!("{error}: {body}")))?,
            ]
        };
        // Skip notifications and server requests without a matching ID.
        let response = messages
            .into_iter()
            .find(|message| message.get("id") == Some(&json!(id)))
            .ok_or_else(|| McpError::Protocol(format!("no answer to {method}: {body}")))?;
        let result = result_of(response)?;
        if method == "initialize" {
            let version = result["protocolVersion"].as_str().map(str::to_owned);
            self.session.lock().await.1 = version;
        }
        Ok(result)
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        self.post(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .await
            .map(drop)
    }
}

/// Contents of the `data:` lines per event of an SSE body.
fn sse_data(body: &str) -> Vec<String> {
    body.split("\n\n")
        .filter_map(|event| {
            let data: Vec<&str> = event
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(str::trim_start)
                .collect();
            (!data.is_empty()).then(|| data.join("\n"))
        })
        .collect()
}

async fn send(connection: &mut Connection, message: &Value) -> Result<(), McpError> {
    let mut line = message.to_string();
    line.push('\n');
    connection.writer.write_all(line.as_bytes()).await?;
    connection.writer.flush().await?;
    Ok(())
}

fn text(value: &Value, key: &str) -> Result<String, McpError> {
    value[key]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| McpError::Protocol(format!("field '{key}' is missing in {value}")))
}

/// A tool of an MCP server as a grauwolf tool.
struct McpTool {
    client: Arc<McpClient>,
    descriptor: ToolDescriptor,
}

impl DynTool for McpTool {
    fn name(&self) -> &str {
        &self.descriptor.name
    }

    fn descriptor(&self) -> ToolDescriptor {
        self.descriptor.clone()
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            self.client
                .call_tool(&self.descriptor.name, args)
                .await
                .map_err(|error| match error {
                    McpError::Tool(text) => ToolError::Execution(text),
                    other => ToolError::Execution(other.to_string()),
                })
        })
    }
}

/// Listing or reading the resources of an MCP server as a grauwolf tool.
struct ResourceTool {
    client: Arc<McpClient>,
    name: String,
    read: bool,
}

impl DynTool for ResourceTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn descriptor(&self) -> ToolDescriptor {
        let (description, parameters) = if self.read {
            (
                "Reads a resource of the MCP server by its URI.",
                json!({
                    "type": "object",
                    "properties": { "uri": { "type": "string" } },
                    "required": ["uri"]
                }),
            )
        } else {
            (
                "Lists the resources of the MCP server with URI, name, description and type.",
                json!({ "type": "object", "properties": {} }),
            )
        };
        ToolDescriptor {
            name: self.name.clone(),
            description: description.into(),
            parameters,
        }
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let failed = |error: McpError| ToolError::Execution(error.to_string());
            if self.read {
                let uri = args["uri"].as_str().ok_or_else(|| ToolError::InvalidArgs {
                    tool: self.name.clone(),
                    reason: "missing field 'uri' (text)".into(),
                })?;
                return self.client.read_resource(uri).await.map_err(failed);
            }
            let resources = self.client.list_resources().await.map_err(failed)?;
            if resources.is_empty() {
                return Ok("No resources.".into());
            }
            Ok(resources
                .iter()
                .map(|resource| {
                    let mut line = format!("{} – {}", resource.uri, resource.name);
                    if !resource.description.is_empty() {
                        line.push_str(&format!(": {}", resource.description));
                    }
                    if let Some(mime) = &resource.mime_type {
                        line.push_str(&format!(" ({mime})"));
                    }
                    line
                })
                .collect::<Vec<_>>()
                .join("\n"))
        })
    }
}
