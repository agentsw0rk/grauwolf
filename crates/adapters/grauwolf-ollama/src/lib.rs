//! LlmClient adapter for Ollama (E02, F02.4) with its own DTOs (ADR-0005).

use futures_util::future::BoxFuture;
use futures_util::stream::{self, BoxStream, StreamExt};
use grauwolf_llm::{Completion, LlmClient, LlmError, StreamFrame, StructuredOutput};
use grauwolf_llm_http::HttpClient;
use grauwolf_prompt::{LlModel, Message, Prompt, ToolChoice, ToolDescriptor};
use grauwolf_rag::{Embedder, Embedding, RagError};
use serde::{Deserialize, Serialize};

mod dto;

/// Default address of a local Ollama server.
pub const DEFAULT_BASE_URL: &str = "http://localhost:11434";

/// Client for the chat API of an Ollama server.
#[derive(Debug, Clone)]
pub struct OllamaClient {
    http: HttpClient,
    /// `think` of the request; `None` leaves it to the model or server.
    think: Option<bool>,
}

impl OllamaClient {
    /// Client for the Ollama server at `base_url`, e.g. [`DEFAULT_BASE_URL`].
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            http: HttpClient::new(base_url),
            think: None,
        }
    }

    /// Turns thinking of reasoning models on or off (`think`). When off, they often
    /// respond many times faster.
    pub fn with_think(mut self, think: bool) -> Self {
        self.think = Some(think);
        self
    }

    /// Digest of the installed model `model` from `/api/tags` (exact name, or `<name>:latest`
    /// for a name without tag; case does not matter); `None` if it is not installed or the
    /// server cannot be reached (ADR-0094).
    pub async fn model_digest(&self, model: &str) -> Option<String> {
        let tags: serde_json::Value = self
            .http
            .clone()
            .with_read_timeout(std::time::Duration::from_secs(2))
            .get_json("/api/tags")
            .await
            .ok()?;
        let wanted = model.to_lowercase();
        let latest = (!wanted.contains(':')).then(|| format!("{wanted}:latest"));
        tags["models"].as_array()?.iter().find_map(|entry| {
            let named = ["name", "model"].iter().any(|key| {
                entry[key]
                    .as_str()
                    .map(str::to_lowercase)
                    .is_some_and(|name| name == wanted || latest.as_deref() == Some(name.as_str()))
            });
            named
                .then(|| entry["digest"].as_str().map(str::to_owned))
                .flatten()
        })
    }

    async fn chat(&self, request: &dto::ChatRequest<'_>) -> Result<Completion, LlmError> {
        let response: dto::ChatResponse = self
            .http
            .post_json("/api/chat", request)
            .await
            .map_err(LlmError::from)?;
        let usage = response.usage();
        let model = response.model.clone();
        Ok(Completion {
            model,
            message: response.message.into_message(),
            usage,
        })
    }
}

/// Chat request with history and tools; the prompt's tool choice is approximated, because Ollama
/// does not know it (ADR-0042): `Tool(name)` offers only that tool, `None` no tools,
/// `Required` and `Tool` append a system instruction.
fn chat_request<'a>(
    model: &'a str,
    prompt: &Prompt,
    tools: &'a [ToolDescriptor],
    think: Option<bool>,
    temperature: Option<f64>,
) -> dto::ChatRequest<'a> {
    let mut messages: Vec<dto::ChatMessage> =
        prompt.messages.iter().map(dto::ChatMessage::from).collect();
    let (offered, instruction): (Vec<&ToolDescriptor>, Option<String>) =
        match &prompt.params.tool_choice {
            _ if tools.is_empty() => (Vec::new(), None),
            ToolChoice::Auto => (tools.iter().collect(), None),
            ToolChoice::None => (Vec::new(), None),
            ToolChoice::Required => (
                tools.iter().collect(),
                Some("Call one of the tools now; do not answer with text.".into()),
            ),
            ToolChoice::Tool(name) => (
                tools.iter().filter(|tool| &tool.name == name).collect(),
                Some(format!("Call the tool {name} now.")),
            ),
        };
    if let Some(instruction) = instruction {
        messages.push(dto::ChatMessage::from(&Message::system(instruction)));
    }
    dto::ChatRequest {
        model,
        stream: false,
        messages,
        tools: offered.into_iter().map(dto::Tool::from).collect(),
        format: None,
        think,
        options: dto::Options::of(temperature),
    }
}

impl LlmClient for OllamaClient {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async move { Ok(self.execute_with_usage(prompt, model, tools).await?.message) })
    }

    /// With token usage from `prompt_eval_count`/`eval_count` (ADR-0067).
    fn execute_with_usage<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async move {
            dto::check_attachments(prompt)?;
            let request = dto::ChatRequest {
                stream: false,
                ..chat_request(
                    &model.id,
                    prompt,
                    tools,
                    model.think.or(self.think),
                    model.temperature,
                )
            };
            self.chat(&request).await
        })
    }

    /// Real streaming via NDJSON (`stream: true`, ADR-0033).
    fn execute_streaming<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxStream<'a, Result<StreamFrame, LlmError>> {
        if let Err(error) = dto::check_attachments(prompt) {
            return stream::once(async move { Err(error) }).boxed();
        }
        let request = dto::ChatRequest {
            stream: true,
            ..chat_request(
                &model.id,
                prompt,
                tools,
                model.think.or(self.think),
                model.temperature,
            )
        };
        stream::once(async move {
            match self.http.post_lines("/api/chat", &request).await {
                Ok(lines) => lines
                    .flat_map(|line| {
                        let frames = match line {
                            Ok(line) => dto::frames_of_chunk(&line),
                            Err(error) => vec![Err(LlmError::from(error))],
                        };
                        stream::iter(frames)
                    })
                    .boxed(),
                Err(error) => stream::once(async { Err(LlmError::from(error)) }).boxed(),
            }
        })
        .flatten()
        .boxed()
    }

    /// Native structured output: Ollama enforces the schema via `format`.
    fn execute_structured<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        output: &'a StructuredOutput,
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async move {
            Ok(self
                .execute_structured_with_usage(prompt, model, output)
                .await?
                .message)
        })
    }

    /// Like `execute_structured`, with token usage (ADR-0067).
    fn execute_structured_with_usage<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        output: &'a StructuredOutput,
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async move {
            dto::check_attachments(prompt)?;
            let request = dto::ChatRequest {
                model: &model.id,
                stream: false,
                messages: prompt.messages.iter().map(dto::ChatMessage::from).collect(),
                tools: Vec::new(),
                format: Some(&output.schema),
                think: model.think.or(self.think),
                options: dto::Options::of(model.temperature),
            };
            self.chat(&request).await
        })
    }
}

/// Embedding model of an Ollama server via `/api/embed` (F08.2).
#[derive(Debug, Clone)]
pub struct OllamaEmbedder {
    http: HttpClient,
    model: String,
}

impl OllamaEmbedder {
    /// Embedder for `model` (e.g. `nomic-embed-text`) on the server at `base_url`.
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            http: HttpClient::new(base_url),
            model: model.into(),
        }
    }
}

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

impl Embedder for OllamaEmbedder {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Embedding>, RagError>> {
        Box::pin(async move {
            let response: EmbedResponse = self
                .http
                .post_json(
                    "/api/embed",
                    &EmbedRequest {
                        model: &self.model,
                        input: texts,
                    },
                )
                .await
                .map_err(|error| RagError::Embedding(Box::new(error)))?;
            Ok(response.embeddings.into_iter().map(Embedding).collect())
        })
    }
}
