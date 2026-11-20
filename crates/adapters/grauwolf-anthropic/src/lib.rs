//! LlmClient adapter for the Anthropic Messages API (E02, F02.3) with its own DTOs (ADR-0005).

use futures_util::future::BoxFuture;
use futures_util::stream::{self, BoxStream, StreamExt};
use grauwolf_llm::{Completion, LlmClient, LlmError, StreamFrame, StructuredOutput};
use grauwolf_llm_http::HttpClient;
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};

mod dto;

/// Base URL of the Anthropic API.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// API version the adapter speaks (header `anthropic-version`).
pub const API_VERSION: &str = "2026-10-21";

/// Default for `max_tokens` in requests without streaming.
pub const DEFAULT_MAX_TOKENS: u32 = 16_000;

/// Models that reject `tool_choice` `any`/`tool` with `400` (ADR-0063); also with a prefix or
/// suffix, e.g. `anthropic.claude-opus-5-5` on Bedrock.
const UNFORCED_TOOL_CHOICE_MODELS: [&str; 4] = [
    "claude-fable-5-1",
    "claude-mythos-5-1",
    "claude-opus-5-5",
    "claude-sonnet-5-5",
];

/// Client for the Anthropic Messages API.
#[derive(Debug, Clone)]
pub struct AnthropicClient {
    api_key: String,
    http: HttpClient,
    max_tokens: u32,
}

impl AnthropicClient {
    /// Client with `api_key` against [`DEFAULT_BASE_URL`].
    pub fn new(api_key: impl Into<String>) -> Self {
        let api_key = api_key.into();
        Self {
            http: http_client(DEFAULT_BASE_URL, &api_key),
            api_key,
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }

    /// Uses the API at `base_url`, e.g. a test server.
    pub fn with_base_url(self, base_url: impl Into<String>) -> Self {
        Self {
            http: http_client(base_url, &self.api_key),
            ..self
        }
    }

    /// Upper limit for the length of a response in tokens.
    pub fn with_max_tokens(self, max_tokens: u32) -> Self {
        Self { max_tokens, ..self }
    }

    async fn send(&self, request: &dto::MessagesRequest<'_>) -> Result<Completion, LlmError> {
        let response: dto::MessagesResponse = self
            .http
            .post_json("/v1/messages", request)
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

fn http_client(base_url: impl Into<String>, api_key: &str) -> HttpClient {
    HttpClient::new(base_url)
        .with_header("x-api-key", api_key)
        .with_header("anthropic-version", API_VERSION)
}

impl LlmClient for AnthropicClient {
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
            dto::check_attachments(prompt)?;
            let request = dto::MessagesRequest {
                temperature: model.temperature,
                ..dto::MessagesRequest::new(&model.id, self.max_tokens, prompt, tools)
            };
            self.send(&request).await
        })
    }

    /// Real streaming via Server-Sent Events (`stream: true`, ADR-0033).
    fn execute_streaming<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxStream<'a, Result<StreamFrame, LlmError>> {
        if let Err(error) = dto::check_attachments(prompt) {
            return stream::once(async move { Err(error) }).boxed();
        }
        let request = dto::MessagesRequest {
            stream: Some(true),
            temperature: model.temperature,
            ..dto::MessagesRequest::new(&model.id, self.max_tokens, prompt, tools)
        };
        stream::once(async move {
            match self.http.post_sse("/v1/messages", &request).await {
                Ok(events) => dto::frames_of_events(events),
                Err(error) => stream::once(async { Err(LlmError::from(error)) }).boxed(),
            }
        })
        .flatten()
        .boxed()
    }

    /// Newer models can no longer force tool calls (ADR-0063).
    fn forces_tool_choice(&self, model: &LlModel) -> bool {
        !UNFORCED_TOOL_CHOICE_MODELS
            .iter()
            .any(|unforced| model.id.contains(unforced))
    }

    /// Native structured output via `output_config.format` (`json_schema`).
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
            let request = dto::MessagesRequest {
                output_config: Some(dto::OutputConfig::json_schema(&output.schema)),
                temperature: model.temperature,
                ..dto::MessagesRequest::new(&model.id, self.max_tokens, prompt, &[])
            };
            self.send(&request).await
        })
    }
}
