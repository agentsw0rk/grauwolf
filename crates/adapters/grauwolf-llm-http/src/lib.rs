//! Shared HTTP base for LLM provider adapters (reqwest, ADR-0008).
//!
//! Contains no provider DTOs; the adapter crates bring their own (ADR-0005).

use std::time::Duration;

use futures_util::stream::{self, BoxStream, Stream, StreamExt};
use reqwest::Response;

use grauwolf_llm::LlmError;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Error of an HTTP request to a provider.
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// The request could not be made or the response could not be read.
    #[error("HTTP request to {url} failed: {source}")]
    Transport {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    /// The provider answered with an error status; `body` contains its message.
    #[error("{url} answers with status {status}: {body}")]
    Status {
        url: String,
        status: u16,
        body: String,
        /// Wait time requested by the server (`retry-after` in seconds).
        retry_after: Option<Duration>,
    },
}

/// Statuses for which a retry may succeed (timeout, conflict, rate limit, server error,
/// overload).
const TRANSIENT_STATUS: &[u16] = &[408, 409, 429, 500, 502, 503, 504, 529];

/// Classifies HTTP errors for the retry policy (ADR-0026): the statuses from
/// `TRANSIENT_STATUS` as well as connection errors and timeouts are temporary.
impl From<HttpError> for LlmError {
    fn from(error: HttpError) -> Self {
        let (transient, retry_after) = match &error {
            HttpError::Status {
                status,
                retry_after,
                ..
            } => (TRANSIENT_STATUS.contains(status), *retry_after),
            HttpError::Transport { source, .. } => {
                (source.is_connect() || source.is_timeout(), None)
            }
        };
        if transient {
            LlmError::Transient {
                source: Box::new(error),
                retry_after,
            }
        } else {
            LlmError::Client(Box::new(error))
        }
    }
}

/// Maximum duration for establishing a connection (ADR-0065).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum duration without received data, before the response headers as well as between
/// parts of a stream (ADR-0065); long thinking phases before the first output fit in.
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(600);

/// JSON client for a provider's API with a fixed base URL.
#[derive(Clone)]
pub struct HttpClient {
    base_url: String,
    http: reqwest::Client,
    /// Additional headers of every request, e.g. authentication.
    headers: Vec<(String, String)>,
    /// Computes further headers from each finished request, e.g. a signature.
    signer: Option<Signer>,
}

/// The finished request that a [`Signer`] sees.
#[derive(Debug, Clone, Copy)]
pub struct SigningRequest<'a> {
    /// HTTP method, e.g. `POST`.
    pub method: &'a str,
    /// Full URL as sent (scheme, host, port, path).
    pub url: &'a str,
    /// The serialized body.
    pub body: &'a [u8],
}

/// Computes headers (name, value) for a finished request, e.g. a request signature (AWS SigV4
/// with Bedrock). Runs once per request, after the body is serialized.
pub type Signer =
    std::sync::Arc<dyn Fn(&SigningRequest<'_>) -> Vec<(String, String)> + Send + Sync>;

/// Shows only the header names: values often contain API keys.
impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpClient")
            .field("base_url", &self.base_url)
            .field("signed", &self.signer.is_some())
            .field(
                "headers",
                &self
                    .headers
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// reqwest client with [`CONNECT_TIMEOUT`] and the read timeout `read_timeout`.
fn reqwest_client(read_timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(read_timeout)
        .build()
        .expect("a reqwest client with timeouts can always be built")
}

impl HttpClient {
    /// Client for the API at `base_url`, e.g. `http://localhost:11434`.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            http: reqwest_client(DEFAULT_READ_TIMEOUT),
            headers: Vec::new(),
            signer: None,
        }
    }

    /// Adds the headers that `signer` computes to every POST request.
    pub fn with_signer(self, signer: Signer) -> Self {
        Self {
            signer: Some(signer),
            ..self
        }
    }

    /// Waits at most `timeout` for data before the request counts as temporarily failed
    /// (default [`DEFAULT_READ_TIMEOUT`]).
    pub fn with_read_timeout(self, timeout: Duration) -> Self {
        Self {
            http: reqwest_client(timeout),
            ..self
        }
    }

    /// Sends `Authorization: Bearer <token>` with every request.
    pub fn with_bearer_token(self, token: impl std::fmt::Display) -> Self {
        self.with_header("authorization", format!("Bearer {token}"))
    }

    /// Sends the header `name: value` with every request.
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Sends `body` as JSON via POST to `path` and reads the response as JSON.
    pub async fn post_json<B, R>(&self, path: &str, body: &B) -> Result<R, HttpError>
    where
        B: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let (url, response) = self.send(path, body).await?;
        response
            .json()
            .await
            .map_err(|source| HttpError::Transport { url, source })
    }

    /// Reads the JSON answer of a GET request to `path`.
    pub async fn get_json<R>(&self, path: &str) -> Result<R, HttpError>
    where
        R: DeserializeOwned,
    {
        let url = format!("{}{path}", self.base_url);
        let request = self
            .headers
            .iter()
            .fold(self.http.get(&url), |request, (name, value)| {
                request.header(name, value)
            });
        let response = match request.send().await {
            Ok(response) => response,
            Err(source) => return Err(HttpError::Transport { url, source }),
        };
        let status = response.status();
        if !status.is_success() {
            return Err(HttpError::Status {
                url,
                status: status.as_u16(),
                body: response.text().await.unwrap_or_default(),
                retry_after: None,
            });
        }
        response
            .json()
            .await
            .map_err(|source| HttpError::Transport { url, source })
    }

    /// Sends `body` as JSON via POST and returns the response line by line, without empty
    /// lines (e.g. NDJSON with Ollama). An error status is reported before the first item.
    pub async fn post_lines<B>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<BoxStream<'static, Result<String, HttpError>>, HttpError>
    where
        B: Serialize + ?Sized,
    {
        let (url, response) = self.send(path, body).await?;
        Ok(lines(url, response)
            .filter(|line| std::future::ready(!matches!(line, Ok(text) if text.is_empty())))
            .boxed())
    }

    /// Sends `body` as JSON via POST and returns the response as server-sent events.
    /// An error status is reported before the first item.
    pub async fn post_sse<B>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<BoxStream<'static, Result<SseEvent, HttpError>>, HttpError>
    where
        B: Serialize + ?Sized,
    {
        let (url, response) = self.send(path, body).await?;
        Ok(sse_events(lines(url, response)).boxed())
    }

    /// Sends `body` as JSON via POST and returns the response body as it arrives, in chunks
    /// without any interpretation (e.g. the binary event stream of Bedrock). An error status is
    /// reported before the first chunk.
    pub async fn post_chunks<B>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<BoxStream<'static, Result<Vec<u8>, HttpError>>, HttpError>
    where
        B: Serialize + ?Sized,
    {
        let (url, response) = self.send(path, body).await?;
        Ok(response
            .bytes_stream()
            .map(move |chunk| {
                chunk
                    .map(|bytes| bytes.to_vec())
                    .map_err(|source| HttpError::Transport {
                        url: url.clone(),
                        source,
                    })
            })
            .boxed())
    }

    /// Sends the request; an error status becomes [`HttpError::Status`].
    async fn send<B>(&self, path: &str, body: &B) -> Result<(String, Response), HttpError>
    where
        B: Serialize + ?Sized,
    {
        let url = format!("{}{path}", self.base_url);
        let request = self
            .headers
            .iter()
            .fold(self.http.post(&url), |request, (name, value)| {
                request.header(name, value)
            });
        let mut request = match request.json(body).build() {
            Ok(request) => request,
            Err(source) => return Err(HttpError::Transport { url, source }),
        };
        if let Some(signer) = &self.signer {
            let bytes = request
                .body()
                .and_then(|body| body.as_bytes())
                .unwrap_or_default();
            let signed = signer(&SigningRequest {
                method: "POST",
                url: &url,
                body: bytes,
            });
            for (name, value) in signed {
                if let (Ok(name), Ok(value)) = (
                    reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                    reqwest::header::HeaderValue::from_str(&value),
                ) {
                    request.headers_mut().insert(name, value);
                }
            }
        }
        let response = match self.http.execute(request).await {
            Ok(response) => response,
            Err(source) => return Err(HttpError::Transport { url, source }),
        };
        let status = response.status();
        if !status.is_success() {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok()?.trim().parse().ok())
                .map(Duration::from_secs);
            return Err(HttpError::Status {
                url,
                status: status.as_u16(),
                body: response.text().await.unwrap_or_default(),
                retry_after,
            });
        }
        Ok((url, response))
    }
}

/// A server-sent event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// Name from the `event:` line, if present.
    pub event: Option<String>,
    /// Content of the `data:` lines, joined with line breaks.
    pub data: String,
}

/// All lines of the response body (including empty ones), without line endings.
fn lines(url: String, response: Response) -> impl Stream<Item = Result<String, HttpError>> {
    let state = (response.bytes_stream().boxed(), Vec::<u8>::new(), false);
    stream::unfold(state, move |(mut bytes, mut buffer, mut done)| {
        let url = url.clone();
        async move {
            loop {
                if let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
                    let line: Vec<u8> = buffer.drain(..=end).collect();
                    return Some((Ok(text_of(&line[..end])), (bytes, buffer, done)));
                }
                if done {
                    if buffer.is_empty() {
                        return None;
                    }
                    let line = text_of(&std::mem::take(&mut buffer));
                    return Some((Ok(line), (bytes, buffer, done)));
                }
                match bytes.next().await {
                    Some(Ok(chunk)) => buffer.extend_from_slice(&chunk),
                    Some(Err(source)) => {
                        return Some((
                            Err(HttpError::Transport { url, source }),
                            (bytes, buffer, true),
                        ));
                    }
                    None => done = true,
                }
            }
        }
    })
}

fn text_of(line: &[u8]) -> String {
    String::from_utf8_lossy(line)
        .trim_end_matches('\r')
        .to_owned()
}

/// Combines lines into events according to the SSE format; comments (`:`) are dropped.
fn sse_events(
    lines: impl Stream<Item = Result<String, HttpError>> + Send + 'static,
) -> impl Stream<Item = Result<SseEvent, HttpError>> {
    let state = (lines.boxed(), None::<String>, Vec::<String>::new(), false);
    stream::unfold(
        state,
        |(mut lines, mut event, mut data, finished)| async move {
            if finished {
                return None;
            }
            loop {
                match lines.next().await {
                    Some(Ok(line)) if line.is_empty() => {
                        if event.is_some() || !data.is_empty() {
                            let ready = SseEvent {
                                event: event.take(),
                                data: std::mem::take(&mut data).join("\n"),
                            };
                            return Some((Ok(ready), (lines, event, data, false)));
                        }
                    }
                    Some(Ok(line)) => {
                        let (field, value) = line.split_once(':').unwrap_or((&line, ""));
                        let value = value.strip_prefix(' ').unwrap_or(value).to_owned();
                        match field {
                            "event" => event = Some(value),
                            "data" => data.push(value),
                            _ => {}
                        }
                    }
                    Some(Err(error)) => return Some((Err(error), (lines, event, data, true))),
                    None => {
                        if event.is_none() && data.is_empty() {
                            return None;
                        }
                        let last = SseEvent {
                            event: event.take(),
                            data: std::mem::take(&mut data).join("\n"),
                        };
                        return Some((Ok(last), (lines, event, data, true)));
                    }
                }
            }
        },
    )
}
