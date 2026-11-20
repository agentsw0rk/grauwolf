//! LlmClient adapter for the native Gemini API (E02, F02.5) with its own DTOs (ADR-0083).

use futures_util::future::BoxFuture;
use futures_util::stream::{self, BoxStream, StreamExt};
use grauwolf_llm::{Completion, LlmClient, LlmError, StreamFrame, StructuredOutput};
use grauwolf_llm_http::HttpClient;
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};
use grauwolf_rag::{Embedder, Embedding, RagError};

mod dto;

/// Base URL of the Gemini API.
pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";

/// Client for Gemini's `generateContent`.
#[derive(Debug, Clone)]
pub struct GeminiClient {
    api_key: String,
    http: HttpClient,
}

impl GeminiClient {
    /// Client with `api_key` against [`DEFAULT_BASE_URL`].
    pub fn new(api_key: impl Into<String>) -> Self {
        let api_key = api_key.into();
        Self {
            http: http_client(DEFAULT_BASE_URL, &api_key),
            api_key,
        }
    }

    /// Uses the API at `base_url`, e.g. a test server.
    pub fn with_base_url(self, base_url: impl Into<String>) -> Self {
        Self {
            http: http_client(base_url, &self.api_key),
            ..self
        }
    }

    async fn send(
        &self,
        model: &LlModel,
        request: &dto::GenerateRequest,
    ) -> Result<Completion, LlmError> {
        let response: dto::GenerateResponse = self
            .http
            .post_json(&model_path(model, "generateContent"), request)
            .await
            .map_err(LlmError::from)?;
        let usage = response.usage_metadata.map(dto::UsageMetadata::usage);
        let model = response.model_version.clone();
        Ok(Completion {
            model,
            message: response.into_message()?,
            usage,
        })
    }
}

fn http_client(base_url: impl Into<String>, api_key: &str) -> HttpClient {
    HttpClient::new(base_url).with_header("x-goog-api-key", api_key)
}

/// Path of the method `method` for `model`; a `models/` prefix in the ID is dropped.
fn model_path(model: &LlModel, method: &str) -> String {
    format!("/v1beta/models/{}:{method}", model_id(&model.id))
}

fn model_id(id: &str) -> &str {
    id.strip_prefix("models/").unwrap_or(id)
}

/// Embeddings via `batchEmbedContents` (ADR-0038, ADR-0083).
#[derive(Debug, Clone)]
pub struct GeminiEmbedder {
    api_key: String,
    http: HttpClient,
    model: String,
}

impl GeminiEmbedder {
    /// Embedder for `model` (e.g. `gemini-embedding-001`) against [`DEFAULT_BASE_URL`].
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        let api_key = api_key.into();
        Self {
            http: http_client(DEFAULT_BASE_URL, &api_key),
            api_key,
            model: model_id(&model.into()).to_owned(),
        }
    }

    /// Uses the API at `base_url`, e.g. a test server.
    pub fn with_base_url(self, base_url: impl Into<String>) -> Self {
        Self {
            http: http_client(base_url, &self.api_key),
            ..self
        }
    }
}

impl Embedder for GeminiEmbedder {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Embedding>, RagError>> {
        Box::pin(async move {
            let request = dto::EmbedRequest::new(&self.model, texts);
            let response: dto::EmbedResponse = self
                .http
                .post_json(
                    &format!("/v1beta/models/{}:batchEmbedContents", self.model),
                    &request,
                )
                .await
                .map_err(|error| RagError::Embedding(Box::new(error)))?;
            Ok(response
                .embeddings
                .into_iter()
                .map(|embedding| Embedding(embedding.values))
                .collect())
        })
    }
}

impl LlmClient for GeminiClient {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async move { Ok(self.execute_with_usage(prompt, model, tools).await?.message) })
    }

    /// With token usage from `usageMetadata` (ADR-0067).
    fn execute_with_usage<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async move {
            let request =
                dto::GenerateRequest::new(prompt, tools).with_temperature(model.temperature);
            self.send(model, &request).await
        })
    }

    /// Real streaming via `streamGenerateContent?alt=sse` (ADR-0033).
    fn execute_streaming<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxStream<'a, Result<StreamFrame, LlmError>> {
        let request = dto::GenerateRequest::new(prompt, tools).with_temperature(model.temperature);
        let path = format!("{}?alt=sse", model_path(model, "streamGenerateContent"));
        stream::once(async move {
            match self.http.post_sse(&path, &request).await {
                Ok(events) => dto::frames_of_events(events),
                Err(error) => stream::once(async { Err(LlmError::from(error)) }).boxed(),
            }
        })
        .flatten()
        .boxed()
    }

    /// Native structured output via `responseJsonSchema` (ADR-0031).
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
            let request = dto::GenerateRequest {
                generation_config: Some(dto::json_schema_config(&output.schema)),
                ..dto::GenerateRequest::new(prompt, &[])
            }
            .with_temperature(model.temperature);
            self.send(model, &request).await
        })
    }
}
