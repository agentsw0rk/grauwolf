//! LlmClient adapter for the Amazon Bedrock Runtime Converse API (E14, F14.18, ADR-0105) with
//! its own JSON handling (ADR-0005).

use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::future::BoxFuture;
use futures_util::stream::{self, BoxStream, StreamExt};
use grauwolf_llm::{Completion, LlmClient, LlmError, StreamFrame, StructuredOutput};
use grauwolf_llm_http::{HttpClient, Signer};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolChoice, ToolDescriptor};
use serde_json::Value;

mod dto;
pub mod eventstream;
mod frames;
pub mod sigv4;

pub use sigv4::Credentials;

/// Region without any setting (the default of AgentWeaver, too).
pub const DEFAULT_REGION: &str = "us-west-2";

/// Service name in the SigV4 credential scope.
const SIGNING_SERVICE: &str = "bedrock";

/// Models that reject a forced tool choice (`any`/`tool`) with a `ValidationException`, also with
/// a prefix or suffix, e.g. `eu.anthropic.claude-opus-5-5-v1:0` (the same as ADR-0063 for the
/// Anthropic API).
const UNFORCED_TOOL_CHOICE_MODELS: [&str; 4] = [
    "claude-fable-5-1",
    "claude-mythos-5-1",
    "claude-opus-5-5",
    "claude-sonnet-5-5",
];

/// How requests are authenticated.
#[derive(Clone)]
enum Auth {
    /// AWS access keys, requests signed with SigV4.
    Sigv4(Credentials),
    /// A Bedrock API key as bearer token.
    Bearer(String),
}

/// Client for the Bedrock Runtime Converse API.
#[derive(Clone)]
pub struct BedrockClient {
    auth: Auth,
    region: String,
    base_url: String,
    http: HttpClient,
    max_tokens: Option<u32>,
}

/// Shows neither credentials nor tokens.
impl std::fmt::Debug for BedrockClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BedrockClient")
            .field("region", &self.region)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

/// Endpoint of Bedrock Runtime in `region`.
pub fn default_base_url(region: &str) -> String {
    format!("https://bedrock-runtime.{region}.amazonaws.com")
}

impl BedrockClient {
    /// Client in `region` that signs requests with `credentials` (SigV4).
    pub fn new(credentials: Credentials, region: impl Into<String>) -> Self {
        Self::build(Auth::Sigv4(credentials), region.into())
    }

    /// Client in `region` that authenticates with a Bedrock API key (`Authorization: Bearer`).
    pub fn with_api_key(api_key: impl Into<String>, region: impl Into<String>) -> Self {
        Self::build(Auth::Bearer(api_key.into()), region.into())
    }

    fn build(auth: Auth, region: String) -> Self {
        let base_url = default_base_url(&region);
        Self {
            http: http_client(&base_url, &auth, &region),
            auth,
            region,
            base_url,
            max_tokens: None,
        }
    }

    /// Uses the endpoint at `base_url`, e.g. a test server, a VPC endpoint or a proxy.
    pub fn with_base_url(self, base_url: impl Into<String>) -> Self {
        let base_url = base_url.into();
        Self {
            http: http_client(&base_url, &self.auth, &self.region),
            base_url,
            ..self
        }
    }

    /// Upper limit for the length of a response in tokens (`inferenceConfig.maxTokens`); without
    /// it the default of the model applies.
    pub fn with_max_tokens(self, max_tokens: u32) -> Self {
        Self {
            max_tokens: Some(max_tokens),
            ..self
        }
    }

    pub(crate) fn path(model: &str, operation: &str) -> String {
        format!("/model/{}/{operation}", sigv4::uri_encode(model))
    }

    async fn converse(&self, model: &str, body: &Value) -> Result<Completion, LlmError> {
        let response: Value = self
            .http
            .post_json(&Self::path(model, "converse"), body)
            .await
            .map_err(dto::llm_error)?;
        // The Converse API does not report which model answered.
        let reply = dto::reply(&response)?;
        Ok(Completion {
            message: reply.message,
            usage: reply.usage,
            model: None,
        })
    }
}

fn http_client(base_url: &str, auth: &Auth, region: &str) -> HttpClient {
    let client = HttpClient::new(base_url);
    match auth {
        Auth::Bearer(token) => client.with_bearer_token(token),
        Auth::Sigv4(credentials) => client.with_signer(signer(credentials.clone(), region.into())),
    }
}

/// Signs each request for `host`, `x-amz-date` and, with temporary credentials,
/// `x-amz-security-token`.
fn signer(credentials: Credentials, region: String) -> Signer {
    std::sync::Arc::new(move |request| {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        sign_request(&credentials, &region, request, seconds)
    })
}

/// Headers (`x-amz-date`, `authorization`, `x-amz-security-token`) for `request` at `seconds`.
fn sign_request(
    credentials: &Credentials,
    region: &str,
    request: &grauwolf_llm_http::SigningRequest<'_>,
    seconds: u64,
) -> Vec<(String, String)> {
    let without_scheme = request
        .url
        .split_once("://")
        .map_or(request.url, |(_, rest)| rest);
    let (host, target) = without_scheme
        .find('/')
        .map_or((without_scheme, "/"), |at| without_scheme.split_at(at));
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let amz_date = sigv4::amz_date(seconds);
    let mut headers = vec![("host", host), ("x-amz-date", amz_date.as_str())];
    if let Some(token) = &credentials.session_token {
        headers.push(("x-amz-security-token", token));
    }
    let signed = sigv4::sign(
        credentials,
        sigv4::Scope {
            region,
            service: SIGNING_SERVICE,
        },
        &sigv4::RequestParts {
            method: request.method,
            path,
            query,
            headers: &headers,
            body: request.body,
        },
        &amz_date,
    );
    let mut added = vec![
        ("x-amz-date".to_owned(), signed.amz_date),
        ("authorization".to_owned(), signed.authorization),
    ];
    if let Some(token) = &credentials.session_token {
        added.push(("x-amz-security-token".to_owned(), token.clone()));
    }
    added
}

impl LlmClient for BedrockClient {
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
            let body = dto::request(
                prompt,
                &dto::RequestOptions {
                    tools,
                    tool_choice: None,
                    temperature: model.temperature,
                    max_tokens: self.max_tokens,
                },
            )?;
            self.converse(&model.id, &body).await
        })
    }

    /// Real streaming via `converse-stream` (ADR-0033): the binary event stream is decoded into
    /// frames; usage comes from the closing `metadata` event, the model is not reported.
    fn execute_streaming<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxStream<'a, Result<StreamFrame, LlmError>> {
        let body = match dto::request(
            prompt,
            &dto::RequestOptions {
                tools,
                tool_choice: None,
                temperature: model.temperature,
                max_tokens: self.max_tokens,
            },
        ) {
            Ok(body) => body,
            Err(error) => return stream::once(async move { Err(error) }).boxed(),
        };
        stream::once(async move {
            let path = Self::path(&model.id, "converse-stream");
            match self.http.post_chunks(&path, &body).await {
                Ok(chunks) => frames::frames_of_chunks(chunks),
                Err(error) => stream::once(async { Err(dto::llm_error(error)) }).boxed(),
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

    /// Structured output via a forced tool whose input schema is the schema of the answer
    /// (ADR-0105, like the tool strategy of ADR-0031). Models that cannot force a tool get the
    /// schema as an instruction instead.
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
            if !self.forces_tool_choice(model) {
                let mut prompt = prompt.clone();
                prompt.messages.push(Message::system(format!(
                    "Answer with nothing but a JSON value of type {} according to this JSON schema, without any further text:\n{}",
                    output.name, output.schema
                )));
                return self.execute_with_usage(&prompt, model, &[]).await;
            }
            let tool = dto::structured_tool(output);
            let choice = ToolChoice::Tool(tool.name.clone());
            let body = dto::request(
                prompt,
                &dto::RequestOptions {
                    tools: std::slice::from_ref(&tool),
                    tool_choice: Some(&choice),
                    temperature: model.temperature,
                    max_tokens: self.max_tokens,
                },
            )?;
            let mut completion = self.converse(&model.id, &body).await?;
            completion.message =
                dto::structured_answer(completion.message, &tool.name, &output.schema)?;
            Ok(completion)
        })
    }
}
