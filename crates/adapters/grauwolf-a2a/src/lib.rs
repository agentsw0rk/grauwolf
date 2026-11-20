//! A2A client (ADR-0055): delegate work to remote agents over A2A 1.0 (JSON-RPC).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use grauwolf_prompt::ToolDescriptor;
pub use grauwolf_spiffe::{AuthMode, ClientAuth};
use grauwolf_tools::{DynTool, ToolError};
use serde_json::{Value, json};

/// Protocol version the client speaks.
pub const PROTOCOL_VERSION: &str = "1.0";

/// Error while exchanging messages with an A2A agent.
#[derive(Debug, thiserror::Error)]
pub enum A2aError {
    /// HTTP request failed or error status.
    #[error("A2A over HTTP: {0}")]
    Http(String),
    /// Response does not match the protocol.
    #[error("invalid A2A answer: {0}")]
    Protocol(String),
    /// The agent reports a JSON-RPC error.
    #[error("A2A agent reports error {code}: {message}")]
    Rpc { code: i64, message: String },
    /// The task ended unsuccessfully (`FAILED`, `REJECTED`, `CANCELED`, …).
    #[error("A2A task ended with {state}: {message}")]
    Task { state: String, message: String },
}

/// A fresh JWT-SVID if the identity works in `jwt` mode.
async fn bearer(auth: Option<&ClientAuth>) -> Result<Option<String>, A2aError> {
    match auth {
        Some(auth) => auth
            .bearer()
            .await
            .map_err(|error| A2aError::Http(format!("workload identity: {error}"))),
        None => Ok(None),
    }
}

/// Maximum time to establish the connection to an A2A agent (ADR-0065).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum time without data from the A2A agent (ADR-0065). `SendMessage` only responds after the
/// agent's entire run; for long tasks, raise the value via
/// [`A2aClient::connect_with_read_timeout`].
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(600);

/// Connection to an A2A agent via its JSON-RPC interface.
pub struct A2aClient {
    http: reqwest::Client,
    endpoint: String,
    headers: Vec<(String, String)>,
    name: String,
    description: String,
    next_id: AtomicU64,
    /// Read timeout, for error messages (ADR-0065).
    read_timeout: Duration,
    /// The agent card offers `SendStreamingMessage` (`capabilities.streaming`, ADR-0089).
    streaming: bool,
    /// Workload identity towards the agent (ADR-0109).
    auth: Option<Arc<ClientAuth>>,
}

impl A2aClient {
    /// Loads the agent card from `url` (the card itself or the agent's base URL);
    /// `headers` are sent with every request, e.g. `Authorization`. Read timeout
    /// [`DEFAULT_READ_TIMEOUT`].
    pub async fn connect(url: &str, headers: &[(String, String)]) -> Result<Self, A2aError> {
        Self::connect_with_read_timeout(url, headers, DEFAULT_READ_TIMEOUT).await
    }

    /// Like [`A2aClient::connect`], but waits at most `read_timeout` for data from the agent
    /// (ADR-0065).
    pub async fn connect_with_read_timeout(
        url: &str,
        headers: &[(String, String)],
        read_timeout: Duration,
    ) -> Result<Self, A2aError> {
        Self::connect_with_auth(url, headers, read_timeout, None).await
    }

    /// Like [`A2aClient::connect_with_read_timeout`], with a workload identity (ADR-0109): mTLS
    /// (the URL must be `https://`) and/or a JWT-SVID as bearer, fresh for every request.
    pub async fn connect_with_auth(
        url: &str,
        headers: &[(String, String)],
        read_timeout: Duration,
        auth: Option<Arc<ClientAuth>>,
    ) -> Result<Self, A2aError> {
        let card_url = if url.ends_with(".json") {
            url.to_owned()
        } else {
            format!("{}/.well-known/agent-card.json", url.trim_end_matches('/'))
        };
        let mut builder = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(read_timeout);
        if let Some(auth) = &auth
            && let Some(tls) = auth
                .tls_config()
                .map_err(|e| A2aError::Http(e.to_string()))?
        {
            if !card_url.starts_with("https://") {
                return Err(A2aError::Http(format!(
                    "{card_url}: `auth.spiffe` needs an https:// URL"
                )));
            }
            builder = builder.use_preconfigured_tls(tls);
        }
        let http = builder
            .build()
            .map_err(|error| A2aError::Http(error.to_string()))?;
        let mut request = http.get(&card_url);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        if let Some(token) = bearer(auth.as_deref()).await? {
            request = request.bearer_auth(token);
        }
        let card: Value = read_json(request.send().await, &card_url, read_timeout).await?;
        let endpoint = card["supportedInterfaces"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|interface| interface["protocolBinding"] == "JSONRPC")
            .and_then(|interface| interface["url"].as_str())
            .ok_or_else(|| {
                A2aError::Protocol(format!(
                    "{card_url}: no JSON-RPC interface in the agent card"
                ))
            })?
            .to_owned();
        Ok(Self {
            http,
            endpoint,
            headers: headers.to_vec(),
            name: card["name"].as_str().unwrap_or_default().to_owned(),
            description: card["description"].as_str().unwrap_or_default().to_owned(),
            next_id: AtomicU64::new(1),
            read_timeout,
            streaming: card["capabilities"]["streaming"] == true,
            auth,
        })
    }

    /// Name from the agent card.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Description from the agent card.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Does the agent offer streaming according to its card?
    pub fn streams(&self) -> bool {
        self.streaming
    }

    /// Like [`A2aClient::send`], via `SendStreamingMessage`: `on_text` receives each text chunk
    /// as it arrives, and the read timeout applies between two events instead of for the
    /// entire run (ADR-0089).
    pub async fn send_streaming(
        &self,
        text: &str,
        on_text: impl Fn(&str) + Send,
    ) -> Result<String, A2aError> {
        let response = self
            .post(&self.message("SendStreamingMessage", text))
            .await?
            .header("Accept", "text/event-stream")
            .send()
            .await
            .map_err(|error| transport_error(&self.endpoint, self.read_timeout, &error))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(A2aError::Http(format!(
                "{} answers with {status}: {body}",
                self.endpoint
            )));
        }
        let mut answer = StreamedAnswer::default();
        let mut bytes = response.bytes_stream();
        let mut buffer = String::new();
        while let Some(chunk) = bytes.next().await {
            let chunk = chunk
                .map_err(|error| transport_error(&self.endpoint, self.read_timeout, &error))?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            // An event ends with a blank line; its `data:` lines form the JSON.
            while let Some(end) = buffer.find("\n\n") {
                let event: String = buffer.drain(..end + 2).collect();
                let data: String = event
                    .lines()
                    .filter_map(|line| line.strip_prefix("data:"))
                    .map(str::trim_start)
                    .collect::<Vec<_>>()
                    .join("\n");
                if data.is_empty() {
                    continue;
                }
                let message: Value = serde_json::from_str(&data).map_err(|error| {
                    A2aError::Protocol(format!("{}: {error}: {data}", self.endpoint))
                })?;
                if let Some(done) = answer.apply(&message, &on_text)? {
                    return Ok(done);
                }
            }
        }
        Err(A2aError::Protocol(format!(
            "{}: stream ended without a final state",
            self.endpoint
        )))
    }

    /// JSON-RPC request `method` with `text` as the new message.
    fn message(&self, method: &str, text: &str) -> Value {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": { "message": {
                "messageId": format!("grauwolf-{id}"),
                "role": "ROLE_USER",
                "parts": [ { "text": text } ]
            } }
        })
    }

    /// POST to the endpoint with the protocol version and the client's headers.
    async fn post(&self, body: &Value) -> Result<reqwest::RequestBuilder, A2aError> {
        let mut request = self
            .http
            .post(&self.endpoint)
            .header("A2A-Version", PROTOCOL_VERSION)
            .json(body);
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        // The trace of the calling run continues in the agent (ADR-0123).
        for (name, value) in grauwolf_tools::current_trace_headers() {
            request = request.header(name, value);
        }
        if let Some(token) = bearer(self.auth.as_deref()).await? {
            request = request.bearer_auth(token);
        }
        // The person the run acts for (ADR-0111): only to agents with a workload identity.
        if self.auth.is_some()
            && let Some((name, token)) = grauwolf_tools::delegation_header()
                .await
                .map_err(A2aError::Http)?
        {
            request = request.header(name, token);
        }
        Ok(request)
    }

    /// Sends `text` as a new message and returns the response as text; a follow-up question
    /// (`TASK_STATE_INPUT_REQUIRED`) comes back as text, unsuccessful tasks as errors.
    pub async fn send(&self, text: &str) -> Result<String, A2aError> {
        let request = self.post(&self.message("SendMessage", text)).await?;
        let response: Value =
            read_json(request.send().await, &self.endpoint, self.read_timeout).await?;
        if let Some(error) = response.get("error") {
            return Err(A2aError::Rpc {
                code: error["code"].as_i64().unwrap_or_default(),
                message: error["message"].as_str().unwrap_or_default().to_owned(),
            });
        }
        let result = &response["result"];
        if let Some(message) = result.get("message") {
            return Ok(parts_text(&message["parts"]));
        }
        let task = result
            .get("task")
            .ok_or_else(|| A2aError::Protocol(format!("neither task nor message: {response}")))?;
        let state = task["status"]["state"].as_str().unwrap_or_default();
        let status_text = parts_text(&task["status"]["message"]["parts"]);
        match state {
            "TASK_STATE_COMPLETED" => Ok(task["artifacts"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|artifact| parts_text(&artifact["parts"]))
                .collect::<Vec<_>>()
                .join("\n")),
            "TASK_STATE_INPUT_REQUIRED" | "TASK_STATE_AUTH_REQUIRED" => {
                Ok(format!("Question from the agent: {status_text}"))
            }
            other => Err(A2aError::Task {
                state: other.to_owned(),
                message: status_text,
            }),
        }
    }

    /// The agent as tool `name`; without `description`, the one from the agent card is used.
    pub fn tool(self: &Arc<Self>, name: &str, description: Option<&str>) -> Arc<dyn DynTool> {
        Arc::new(A2aTool {
            client: Arc::clone(self),
            descriptor: ToolDescriptor {
                name: name.to_owned(),
                description: description.unwrap_or(&self.description).to_owned(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "request": { "type": "string", "description": "Request to the agent" }
                    },
                    "required": ["request"]
                }),
            },
        })
    }
}

/// Response of a stream: text per artifact, until a final state arrives.
#[derive(Default)]
struct StreamedAnswer {
    artifacts: Vec<(String, String)>,
}

impl StreamedAnswer {
    /// Processes a JSON-RPC response of the stream; `Some(text)` at the final state.
    fn apply(
        &mut self,
        message: &Value,
        on_text: &impl Fn(&str),
    ) -> Result<Option<String>, A2aError> {
        if let Some(error) = message.get("error") {
            return Err(A2aError::Rpc {
                code: error["code"].as_i64().unwrap_or_default(),
                message: error["message"].as_str().unwrap_or_default().to_owned(),
            });
        }
        let result = &message["result"];
        if let Some(reply) = result.get("message") {
            return Ok(Some(parts_text(&reply["parts"])));
        }
        if let Some(update) = result.get("artifactUpdate") {
            let artifact = &update["artifact"];
            let id = artifact["artifactId"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let text = parts_text(&artifact["parts"]);
            on_text(&text);
            match self.artifacts.iter_mut().find(|(known, _)| *known == id) {
                Some((_, current)) if update["append"] == true => current.push_str(&text),
                Some((_, current)) => *current = text,
                None => self.artifacts.push((id, text)),
            }
            return Ok(None);
        }
        // Final state on the task or on a status message.
        let status = result
            .get("statusUpdate")
            .map(|update| &update["status"])
            .or_else(|| result.get("task").map(|task| &task["status"]));
        let Some(status) = status else {
            return Ok(None);
        };
        let message = parts_text(&status["message"]["parts"]);
        match status["state"].as_str().unwrap_or_default() {
            "TASK_STATE_COMPLETED" => Ok(Some(
                self.artifacts
                    .iter()
                    .map(|(_, text)| text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            )),
            "TASK_STATE_INPUT_REQUIRED" | "TASK_STATE_AUTH_REQUIRED" => {
                Ok(Some(format!("Question from the agent: {message}")))
            }
            "TASK_STATE_FAILED" | "TASK_STATE_CANCELED" | "TASK_STATE_REJECTED" => {
                Err(A2aError::Task {
                    state: status["state"].as_str().unwrap_or_default().to_owned(),
                    message,
                })
            }
            _ => Ok(None),
        }
    }
}

/// Text and `data` parts as text, separated by line breaks.
fn parts_text(parts: &Value) -> String {
    parts
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| match (part.get("text"), part.get("data")) {
            (Some(Value::String(text)), _) => Some(text.clone()),
            (_, Some(data)) => Some(data.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// JSON body of a successful response from `url`.
async fn read_json(
    response: Result<reqwest::Response, reqwest::Error>,
    url: &str,
    read_timeout: Duration,
) -> Result<Value, A2aError> {
    let response = response.map_err(|error| transport_error(url, read_timeout, &error))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| transport_error(url, read_timeout, &error))?;
    if !status.is_success() {
        return Err(A2aError::Http(format!(
            "{url} answers with {status}: {body}"
        )));
    }
    serde_json::from_str(&body)
        .map_err(|error| A2aError::Protocol(format!("{url}: {error}: {body}")))
}

/// Error while sending or reading; the message says whether the call may have run
/// (not if the connection failed, possibly if it timed out) (ADR-0065).
fn transport_error(url: &str, read_timeout: Duration, error: &reqwest::Error) -> A2aError {
    A2aError::Http(if let Some(reason) = grauwolf_spiffe::tls_refusal(error) {
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
        format!("{url}: {error}")
    })
}

/// A remote agent as a grauwolf tool; each call is its own message.
struct A2aTool {
    client: Arc<A2aClient>,
    descriptor: ToolDescriptor,
}

impl DynTool for A2aTool {
    fn name(&self) -> &str {
        &self.descriptor.name
    }

    fn descriptor(&self) -> ToolDescriptor {
        self.descriptor.clone()
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let request = args["request"]
                .as_str()
                .ok_or_else(|| ToolError::InvalidArgs {
                    tool: self.descriptor.name.clone(),
                    reason: "missing field 'request' (text)".into(),
                })?;
            // Streaming if the agent offers it: no read timeout across the whole run.
            let answer = if self.client.streams() {
                self.client.send_streaming(request, |_| {}).await
            } else {
                self.client.send(request).await
            };
            answer.map_err(|error| ToolError::Execution(error.to_string()))
        })
    }
}
