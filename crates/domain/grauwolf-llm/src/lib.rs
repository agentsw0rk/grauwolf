//! LLM clients and prompt executor (E02): port `LlmClient` and routing by provider – without HTTP.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use futures_util::stream::{self, BoxStream, StreamExt};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};

mod cache;
mod moderation;
mod retry;
mod streaming;

pub use cache::{MemoryPromptCache, PromptCache};
pub use moderation::{Moderation, classify as classify_moderation};
pub use retry::{Clock, RetryPolicy};
pub use streaming::{StreamFrame, aggregate};

/// Error of an LLM request.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    /// No client is registered for the provider.
    #[error("no client registered for provider '{provider}'")]
    UnknownProvider { provider: String },
    /// Temporary error (overload, rate limit, connection); a retry may succeed. `retry_after`
    /// is the wait time requested by the provider.
    #[error("LLM request failed temporarily: {source}")]
    Transient {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
        retry_after: Option<std::time::Duration>,
    },
    /// The client reports an error.
    #[error("LLM client failed: {0}")]
    Client(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// The run budget is exhausted; no further request is made (ADR-0072).
    #[error("run budget exhausted: {reason}")]
    BudgetExceeded { reason: String },
}

/// Requested shape of a structured answer (ADR-0031).
#[derive(Debug, Clone, PartialEq)]
pub struct StructuredOutput {
    /// Name of the type, e.g. `Ticket`.
    pub name: String,
    /// JSON schema of the type from the spec.
    pub schema: serde_json::Value,
}

/// Answer of a call with its token usage, if the provider reports it (ADR-0067).
#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    /// The answer.
    pub message: Message,
    /// Tokens used; `None` if unknown.
    pub usage: Option<grauwolf_prompt::Usage>,
    /// The model the provider reports to have answered (it can be more precise than the
    /// requested one, e.g. a dated version); `None` if the provider does not say (ADR-0094).
    pub model: Option<String>,
}

/// Several alternative answers with their combined token usage (ADR-0042, ADR-0067).
#[derive(Debug, Clone, PartialEq)]
pub struct Choices {
    /// The answers.
    pub messages: Vec<Message>,
    /// Tokens used by all answers together; `None` if unknown.
    pub usage: Option<grauwolf_prompt::Usage>,
}

/// Port to an LLM provider; implementations live in `crates/adapters`.
pub trait LlmClient: Send + Sync {
    /// Sends `prompt` to `model`, offers `tools` and returns the answer.
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>>;

    /// Like [`LlmClient::execute`], but also returns the token usage (ADR-0067). Default:
    /// `execute` with unknown usage; adapters that know the numbers override this.
    fn execute_with_usage<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async move {
            Ok(Completion {
                model: None,
                message: self.execute(prompt, model, tools).await?,
                usage: None,
            })
        })
    }

    /// Streams the answer as frames (ADR-0033). Default: call `execute_with_usage` and emit
    /// the finished answer with its usage in one piece; adapters override this with real
    /// streaming.
    fn execute_streaming<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxStream<'a, Result<StreamFrame, LlmError>> {
        stream::once(self.execute_with_usage(prompt, model, tools))
            .flat_map(|result| {
                let frames: Vec<Result<StreamFrame, LlmError>> = match result {
                    Ok(completion) => {
                        streaming::frames_of(completion.message, completion.usage, completion.model)
                            .into_iter()
                            .map(Ok)
                            .collect()
                    }
                    Err(error) => vec![Err(error)],
                };
                stream::iter(frames)
            })
            .boxed()
    }

    /// Checks `text` for harmful content (ADR-0066). Default: a dedicated structured
    /// classification request; providers with their own moderation API override this.
    fn moderate<'a>(
        &'a self,
        text: &'a str,
        model: &'a LlModel,
    ) -> BoxFuture<'a, Result<Moderation, LlmError>> {
        Box::pin(moderation::classify(self, text, model))
    }

    /// Can the provider force a tool call for `model` (`tool_choice` `Required` or
    /// `Tool(name)`)? Without it, the agent steers via an instruction in the history (ADR-0063).
    fn forces_tool_choice(&self, _model: &LlModel) -> bool {
        true
    }

    /// Returns `n` alternative answers to the same prompt with usage (ADR-0042,
    /// ADR-0067). Default: `n` single requests via `execute_with_usage`, usage summed up
    /// (unknown as soon as one request reports none); adapters with native support (e.g.
    /// OpenAI `n`) override this.
    fn execute_choices<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
        n: usize,
    ) -> BoxFuture<'a, Result<Choices, LlmError>> {
        Box::pin(async move {
            let mut messages = Vec::with_capacity(n);
            let mut usage = Some(None);
            for _ in 0..n {
                let completion = self.execute_with_usage(prompt, model, tools).await?;
                messages.push(completion.message);
                usage = match (usage, completion.usage) {
                    (Some(total), Some(one)) => Some(Some(
                        total.map_or(one, |total: grauwolf_prompt::Usage| total + one),
                    )),
                    _ => None,
                };
            }
            Ok(Choices {
                messages,
                usage: usage.flatten(),
            })
        })
    }

    /// Like [`LlmClient::execute_structured`], but also returns the token usage (ADR-0067).
    /// Default: `execute_structured` with unknown usage – this keeps a native path intact even
    /// if an adapter only overrides `execute_structured`.
    fn execute_structured_with_usage<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        output: &'a StructuredOutput,
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async move {
            Ok(Completion {
                model: None,
                message: self.execute_structured(prompt, model, output).await?,
                usage: None,
            })
        })
    }

    /// Requests an answer as JSON according to `output.schema`, without tools.
    ///
    /// The default is the *manual* strategy: the schema is a system instruction at the end
    /// of the prompt. Adapters override this if the provider supports structured output
    /// natively.
    fn execute_structured<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        output: &'a StructuredOutput,
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async move {
            let mut prompt = prompt.clone();
            prompt.messages.push(Message::system(format!(
                "Answer with nothing but a JSON value of type {} according to this JSON schema, without any further text:\n{}",
                output.name, output.schema
            )));
            self.execute(&prompt, model, &[]).await
        })
    }
}

/// Forwards requests to the client of the provider named in the model.
#[derive(Clone, Default)]
pub struct PromptExecutor {
    clients: HashMap<String, Arc<dyn LlmClient>>,
    /// Without a clock there are no retries: the domain cannot wait by itself (ADR-0026).
    retry: Option<(RetryPolicy, Arc<dyn Clock>)>,
    /// Answers to identical requests come from here instead of the provider (ADR-0086).
    cache: Option<Arc<dyn PromptCache>>,
}

impl PromptExecutor {
    /// Starts building an executor.
    pub fn builder() -> PromptExecutorBuilder {
        PromptExecutorBuilder::default()
    }

    /// Sends `prompt` to `model` via the client of `model.provider` and offers `tools`.
    pub async fn execute(
        &self,
        prompt: &Prompt,
        model: &LlModel,
        tools: &[ToolDescriptor],
    ) -> Result<Message, LlmError> {
        let key = self.cache_key("execute", model, prompt, tools, None);
        if let Some(message) = self.cached(&key).await {
            return Ok(message);
        }
        let client = self.client(model)?;
        let message = self
            .with_retries(|| client.execute(prompt, model, tools))
            .await?;
        self.store(&key, &message).await;
        Ok(message)
    }

    /// Can the client of `model.provider` force tool calls (ADR-0063)? Without a client
    /// `true`; the request then fails with `UnknownProvider` anyway.
    pub fn forces_tool_choice(&self, model: &LlModel) -> bool {
        match self.client(model) {
            Ok(client) => client.forces_tool_choice(model),
            Err(_) => true,
        }
    }

    /// Like [`PromptExecutor::execute`], but also returns the token usage (ADR-0067).
    pub async fn execute_with_usage(
        &self,
        prompt: &Prompt,
        model: &LlModel,
        tools: &[ToolDescriptor],
    ) -> Result<Completion, LlmError> {
        let key = self.cache_key("execute", model, prompt, tools, None);
        if let Some(message) = self.cached(&key).await {
            return Ok(Completion {
                model: None,
                message,
                usage: Some(cache::no_usage()),
            });
        }
        let client = self.client(model)?;
        let completion = self
            .with_retries(|| client.execute_with_usage(prompt, model, tools))
            .await?;
        self.store(&key, &completion.message).await;
        Ok(completion)
    }

    /// Returns `n` alternative answers via the client of `model.provider` (ADR-0042).
    pub async fn execute_choices(
        &self,
        prompt: &Prompt,
        model: &LlModel,
        tools: &[ToolDescriptor],
        n: usize,
    ) -> Result<Choices, LlmError> {
        let client = self.client(model)?;
        self.with_retries(|| client.execute_choices(prompt, model, tools, n))
            .await
    }

    /// Checks `text` for harmful content via the client of `model.provider` (ADR-0066).
    pub async fn moderate(&self, text: &str, model: &LlModel) -> Result<Moderation, LlmError> {
        let client = self.client(model)?;
        self.with_retries(|| client.moderate(text, model)).await
    }

    /// Like [`PromptExecutor::execute_structured`], but also returns the token usage (ADR-0067).
    pub async fn execute_structured_with_usage(
        &self,
        prompt: &Prompt,
        model: &LlModel,
        output: &StructuredOutput,
    ) -> Result<Completion, LlmError> {
        let key = self.cache_key("structured", model, prompt, &[], Some(&output.schema));
        if let Some(message) = self.cached(&key).await {
            return Ok(Completion {
                model: None,
                message,
                usage: Some(cache::no_usage()),
            });
        }
        let client = self.client(model)?;
        let completion = self
            .with_retries(|| client.execute_structured_with_usage(prompt, model, output))
            .await?;
        self.store(&key, &completion.message).await;
        Ok(completion)
    }

    /// Requests a structured answer via the client of `model.provider` (ADR-0031).
    pub async fn execute_structured(
        &self,
        prompt: &Prompt,
        model: &LlModel,
        output: &StructuredOutput,
    ) -> Result<Message, LlmError> {
        let key = self.cache_key("structured", model, prompt, &[], Some(&output.schema));
        if let Some(message) = self.cached(&key).await {
            return Ok(message);
        }
        let client = self.client(model)?;
        let message = self
            .with_retries(|| client.execute_structured(prompt, model, output))
            .await?;
        self.store(&key, &message).await;
        Ok(message)
    }

    /// Streams the answer via the client of `model.provider` (ADR-0033). Temporary errors
    /// are only retried as long as no frame has been passed on yet.
    pub fn execute_streaming<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxStream<'a, Result<StreamFrame, LlmError>> {
        // Same key as `execute`: hits are replayed as frames, new answers are stored
        // assembled after the last frame (ADR-0086).
        let Some(key) = self.cache_key("execute", model, prompt, tools, None) else {
            return self.stream_from_client(prompt, model, tools);
        };
        stream::once(async move {
            if let Some(message) = self.cache_get(&key).await {
                let frames = streaming::frames_of(message, Some(cache::no_usage()), None);
                return stream::iter(frames.into_iter().map(Ok)).boxed();
            }
            // `None` as soon as an error occurred: then nothing is stored.
            let collected = Arc::new(Mutex::new(Some(Vec::new())));
            let sink = Arc::clone(&collected);
            let frames = self
                .stream_from_client(prompt, model, tools)
                .inspect(move |frame| {
                    let mut frames = sink.lock().expect("frame collection");
                    match frame {
                        Ok(frame) => {
                            if let Some(frames) = frames.as_mut() {
                                frames.push(frame.clone());
                            }
                        }
                        Err(_) => *frames = None,
                    }
                });
            let store = stream::once(async move {
                let frames = collected.lock().expect("frame collection").take();
                if let Some(frames) = frames.filter(|frames| {
                    frames
                        .iter()
                        .any(|frame| matches!(frame, StreamFrame::End { .. }))
                }) {
                    self.cache_put(&key, &aggregate(frames)).await;
                }
            })
            .filter_map(|()| std::future::ready(None));
            frames.chain(store).boxed()
        })
        .flatten()
        .boxed()
    }

    /// Streams via the client of `model.provider` with retries before the first frame.
    fn stream_from_client<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxStream<'a, Result<StreamFrame, LlmError>> {
        let client = match self.client(model) {
            Ok(client) => client,
            Err(error) => return stream::once(async { Err(error) }).boxed(),
        };
        stream::once(async move {
            let mut attempt = 0;
            loop {
                let mut frames = client.execute_streaming(prompt, model, tools);
                match frames.next().await {
                    Some(Err(LlmError::Transient {
                        source,
                        retry_after,
                    })) => {
                        if !self.wait_before_retry(attempt, retry_after).await {
                            let error = LlmError::Transient {
                                source,
                                retry_after,
                            };
                            return stream::once(async { Err(error) }).boxed();
                        }
                        attempt += 1;
                    }
                    Some(first) => return stream::once(async { first }).chain(frames).boxed(),
                    None => return stream::empty().boxed(),
                }
            }
        })
        .flatten()
        .boxed()
    }

    /// Runs `request` and retries temporary errors according to the retry policy.
    async fn with_retries<'a, T>(
        &self,
        request: impl Fn() -> BoxFuture<'a, Result<T, LlmError>>,
    ) -> Result<T, LlmError> {
        let mut attempt = 0;
        loop {
            match request().await {
                Err(LlmError::Transient {
                    source,
                    retry_after,
                }) => {
                    if !self.wait_before_retry(attempt, retry_after).await {
                        return Err(LlmError::Transient {
                            source,
                            retry_after,
                        });
                    }
                    attempt += 1;
                }
                result => return result,
            }
        }
    }

    /// Waits before retry no. `attempt + 1`; `false` if no more are allowed.
    async fn wait_before_retry(&self, attempt: u32, retry_after: Option<Duration>) -> bool {
        let Some((policy, clock)) = &self.retry else {
            return false;
        };
        match policy.next_delay(attempt, retry_after) {
            Some(delay) => {
                clock.sleep(delay).await;
                true
            }
            None => false,
        }
    }

    /// Key of the request, if a cache is configured.
    fn cache_key(
        &self,
        kind: &str,
        model: &LlModel,
        prompt: &Prompt,
        tools: &[ToolDescriptor],
        schema: Option<&serde_json::Value>,
    ) -> Option<String> {
        self.cache
            .as_ref()
            .map(|_| cache::key(kind, model, prompt, tools, schema))
    }

    /// The answer from the cache, if there is one and it knows the answer.
    async fn cached(&self, key: &Option<String>) -> Option<Message> {
        self.cache_get(key.as_deref()?).await
    }

    async fn cache_get(&self, key: &str) -> Option<Message> {
        match &self.cache {
            Some(cache) => cache.get(key).await,
            None => None,
        }
    }

    async fn cache_put(&self, key: &str, message: &Message) {
        if let Some(cache) = &self.cache {
            cache.put(key, message).await;
        }
    }

    /// Stores an answer in the cache, if there is one.
    async fn store(&self, key: &Option<String>, message: &Message) {
        if let Some(key) = key {
            self.cache_put(key, message).await;
        }
    }

    fn client(&self, model: &LlModel) -> Result<&Arc<dyn LlmClient>, LlmError> {
        self.clients
            .get(&model.provider)
            .ok_or_else(|| LlmError::UnknownProvider {
                provider: model.provider.clone(),
            })
    }
}

/// Builds a [`PromptExecutor`].
#[derive(Default)]
pub struct PromptExecutorBuilder {
    executor: PromptExecutor,
}

impl PromptExecutorBuilder {
    /// Registers `client` for the provider `provider`.
    pub fn client(mut self, provider: impl Into<String>, client: Arc<dyn LlmClient>) -> Self {
        self.executor.clients.insert(provider.into(), client);
        self
    }

    /// Retries temporary errors according to `policy`; waiting happens via `clock`.
    pub fn retry(mut self, policy: RetryPolicy, clock: Arc<dyn Clock>) -> Self {
        self.executor.retry = Some((policy, clock));
        self
    }

    /// Answers identical requests from `cache` instead of the provider (ADR-0086).
    pub fn cache(mut self, cache: Arc<dyn PromptCache>) -> Self {
        self.executor.cache = Some(cache);
        self
    }

    /// Finishes building.
    pub fn build(self) -> PromptExecutor {
        self.executor
    }
}
