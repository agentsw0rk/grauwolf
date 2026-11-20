//! LlmClient adapter for OpenAI Chat Completions (E02, F02.2) with its own DTOs (ADR-0005).
//!
//! Also works with OpenAI-compatible gateways (`with_base_url`).

use futures_util::future::BoxFuture;
use futures_util::stream::{self, BoxStream, StreamExt};
use grauwolf_llm::{
    Choices, Completion, LlmClient, LlmError, Moderation, StreamFrame, StructuredOutput,
};
use grauwolf_llm_http::HttpClient;
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};

mod dto;

/// Base URL of the OpenAI API.
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// Client for the OpenAI Chat Completions API.
#[derive(Debug, Clone)]
pub struct OpenAiClient {
    api_key: String,
    http: HttpClient,
}

impl OpenAiClient {
    /// Client with `api_key` against [`DEFAULT_BASE_URL`].
    pub fn new(api_key: impl Into<String>) -> Self {
        let api_key = api_key.into();
        Self {
            http: HttpClient::new(DEFAULT_BASE_URL).with_bearer_token(&api_key),
            api_key,
        }
    }

    /// Uses the API at `base_url`, e.g. a compatible gateway or a test server.
    pub fn with_base_url(self, base_url: impl Into<String>) -> Self {
        Self {
            http: HttpClient::new(base_url).with_bearer_token(&self.api_key),
            api_key: self.api_key,
        }
    }

    async fn complete(&self, request: &dto::ChatRequest<'_>) -> Result<Completion, LlmError> {
        let response: dto::ChatResponse = self
            .http
            .post_json("/chat/completions", request)
            .await
            .map_err(LlmError::from)?;
        let usage = response.usage.map(dto::ResponseUsage::usage);
        let model = response.model.clone();
        Ok(Completion {
            model,
            message: response.into_message()?,
            usage,
        })
    }
}

impl LlmClient for OpenAiClient {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async move { Ok(self.execute_with_usage(prompt, model, tools).await?.message) })
    }

    /// With token usage from the response's `usage` (ADR-0067).
    fn execute_with_usage<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async move {
            let request = dto::ChatRequest {
                model: &model.id,
                temperature: model.temperature,
                messages: dto::messages(prompt)?,
                tools: tools.iter().map(dto::Tool::from).collect(),
                tool_choice: dto::tool_choice(&prompt.params.tool_choice, !tools.is_empty()),
                n: None,
                response_format: None,
                stream: None,
                stream_options: None,
            };
            self.complete(&request).await
        })
    }

    /// Multiple response variants natively via `n` (ADR-0042).
    fn execute_choices<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
        n: usize,
    ) -> BoxFuture<'a, Result<Choices, LlmError>> {
        Box::pin(async move {
            let request = dto::ChatRequest {
                model: &model.id,
                temperature: model.temperature,
                messages: dto::messages(prompt)?,
                tools: tools.iter().map(dto::Tool::from).collect(),
                tool_choice: dto::tool_choice(&prompt.params.tool_choice, !tools.is_empty()),
                n: Some(n),
                response_format: None,
                stream: None,
                stream_options: None,
            };
            let response: dto::ChatResponse = self
                .http
                .post_json("/chat/completions", &request)
                .await
                .map_err(LlmError::from)?;
            // `usage` applies to all responses together (ADR-0067).
            let usage = response.usage.map(dto::ResponseUsage::usage);
            Ok(Choices {
                messages: response.into_messages()?,
                usage,
            })
        })
    }

    /// Real streaming via Server-Sent Events (`stream: true`, ADR-0033).
    fn execute_streaming<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxStream<'a, Result<StreamFrame, LlmError>> {
        let messages = match dto::messages(prompt) {
            Ok(messages) => messages,
            Err(error) => return stream::once(async move { Err(error) }).boxed(),
        };
        let request = dto::ChatRequest {
            model: &model.id,
            temperature: model.temperature,
            messages,
            tools: tools.iter().map(dto::Tool::from).collect(),
            tool_choice: dto::tool_choice(&prompt.params.tool_choice, !tools.is_empty()),
            n: None,
            response_format: None,
            stream: Some(true),
            stream_options: Some(serde_json::json!({ "include_usage": true })),
        };
        stream::once(async move {
            match self.http.post_sse("/chat/completions", &request).await {
                Ok(events) => dto::frames_of_events(events),
                Err(error) => stream::once(async { Err(LlmError::from(error)) }).boxed(),
            }
        })
        .flatten()
        .boxed()
    }

    /// With a moderation model (`…moderation…`) natively via `/moderations`, otherwise as
    /// usual via the chat model – compatible gateways often do not know `/moderations`
    /// (ADR-0066).
    fn moderate<'a>(
        &'a self,
        text: &'a str,
        model: &'a LlModel,
    ) -> BoxFuture<'a, Result<Moderation, LlmError>> {
        Box::pin(async move {
            if !model.id.contains("moderation") {
                return grauwolf_llm::classify_moderation(self, text, model).await;
            }
            let response: serde_json::Value = self
                .http
                .post_json(
                    "/moderations",
                    &serde_json::json!({ "model": model.id, "input": text }),
                )
                .await
                .map_err(LlmError::from)?;
            let result = &response["results"][0];
            let harmful = result["flagged"].as_bool().ok_or_else(|| {
                LlmError::Client(format!("moderation without `flagged`: {response}").into())
            })?;
            let categories = result["categories"]
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(_, flagged)| flagged.as_bool() == Some(true))
                .map(|(category, _)| category.clone())
                .collect();
            // `/moderations` does not count tokens (ADR-0067).
            Ok(Moderation {
                harmful,
                categories,
                usage: None,
            })
        })
    }

    /// Native structured output via `response_format` with `json_schema`.
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
            let request = dto::ChatRequest {
                model: &model.id,
                temperature: model.temperature,
                messages: dto::messages(prompt)?,
                tools: Vec::new(),
                tool_choice: None,
                n: None,
                response_format: Some(dto::ResponseFormat {
                    kind: "json_schema",
                    json_schema: dto::JsonSchemaFormat {
                        name: &output.name,
                        schema: &output.schema,
                    },
                }),
                stream: None,
                stream_options: None,
            };
            self.complete(&request).await
        })
    }
}

/// Embedding model of the OpenAI API (`/embeddings`, F08.2); also for compatible endpoints such as
/// Ollamas `/v1`.
#[derive(Debug, Clone)]
pub struct OpenAiEmbedder {
    api_key: String,
    model: String,
    http: HttpClient,
}

impl OpenAiEmbedder {
    /// Embedder for `model` (e.g. `text-embedding-3-small`) against [`DEFAULT_BASE_URL`].
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        let api_key = api_key.into();
        Self {
            http: HttpClient::new(DEFAULT_BASE_URL).with_bearer_token(&api_key),
            api_key,
            model: model.into(),
        }
    }

    /// Other endpoint, e.g. a compatible gateway.
    pub fn with_base_url(self, base_url: impl Into<String>) -> Self {
        Self {
            http: HttpClient::new(base_url).with_bearer_token(&self.api_key),
            ..self
        }
    }
}

#[derive(serde::Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(serde::Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(serde::Deserialize)]
struct EmbeddingData {
    index: usize,
    embedding: Vec<f32>,
}

impl grauwolf_rag::Embedder for OpenAiEmbedder {
    fn embed<'a>(
        &'a self,
        texts: &'a [String],
    ) -> BoxFuture<'a, Result<Vec<grauwolf_rag::Embedding>, grauwolf_rag::RagError>> {
        Box::pin(async move {
            let mut response: EmbeddingResponse = self
                .http
                .post_json(
                    "/embeddings",
                    &EmbeddingRequest {
                        model: &self.model,
                        input: texts,
                    },
                )
                .await
                .map_err(|error| grauwolf_rag::RagError::Embedding(Box::new(error)))?;
            // The API gives the order via `index`.
            response.data.sort_by_key(|data| data.index);
            Ok(response
                .data
                .into_iter()
                .map(|data| grauwolf_rag::Embedding(data.embedding))
                .collect())
        })
    }
}
