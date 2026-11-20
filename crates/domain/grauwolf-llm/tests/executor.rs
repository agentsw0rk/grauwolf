//! Routing of the PromptExecutor by provider (F02.10).

use std::sync::Arc;

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_llm::{LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};

/// Client that always answers with its name.
struct NamedClient(&'static str);

impl LlmClient for NamedClient {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async move { Ok(Message::assistant(self.0)) })
    }
}

#[test]
fn routes_requests_to_the_client_of_the_model_provider() {
    let executor = PromptExecutor::builder()
        .client("openai", Arc::new(NamedClient("openai-client")))
        .client("ollama", Arc::new(NamedClient("ollama-client")))
        .build();
    let prompt = Prompt::builder("p").user("Hello").build();

    let answer =
        block_on(executor.execute(&prompt, &LlModel::new("ollama", "llama3.2"), &[])).unwrap();

    assert_eq!(answer, Message::assistant("ollama-client"));
}

#[test]
fn fails_for_providers_without_a_client() {
    let executor = PromptExecutor::builder()
        .client("openai", Arc::new(NamedClient("openai-client")))
        .build();
    let prompt = Prompt::builder("p").user("Hello").build();

    let error =
        block_on(executor.execute(&prompt, &LlModel::new("anthropic", "claude"), &[])).unwrap_err();

    assert!(
        matches!(&error, LlmError::UnknownProvider { provider } if provider == "anthropic"),
        "unexpected error: {error}"
    );
}

/// Client that answers with the names of the offered tools.
struct ToolEcho;

impl LlmClient for ToolEcho {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
        let answer = Message::assistant(names.join(","));
        Box::pin(async move { Ok(answer) })
    }
}

#[test]
fn passes_the_offered_tools_to_the_client() {
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(ToolEcho))
        .build();
    let prompt = Prompt::builder("p").user("Hello").build();
    let tools = [
        ToolDescriptor {
            name: "add".into(),
            description: "Adds".into(),
            parameters: serde_json::json!({}),
        },
        ToolDescriptor {
            name: "mul".into(),
            description: "Multiplies".into(),
            parameters: serde_json::json!({}),
        },
    ];

    let answer = block_on(executor.execute(&prompt, &LlModel::new("mock", "m"), &tools)).unwrap();

    assert_eq!(answer, Message::assistant("add,mul"));
}

/// Client that records the last received prompt and the offered tools.
#[derive(Default)]
struct RecordingClient {
    seen: std::sync::Mutex<Option<(Prompt, usize)>>,
}

impl LlmClient for RecordingClient {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        _model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        *self.seen.lock().unwrap() = Some((prompt.clone(), tools.len()));
        Box::pin(async { Ok(Message::assistant("{}")) })
    }
}

#[test]
fn structured_requests_put_the_schema_into_the_prompt_by_default() {
    let client = Arc::new(RecordingClient::default());
    let executor = PromptExecutor::builder()
        .client("ollama", client.clone())
        .build();
    let prompt = Prompt::builder("p").user("Bug in login").build();
    let output = grauwolf_llm::StructuredOutput {
        name: "Ticket".into(),
        schema: serde_json::json!({ "type": "object", "properties": { "title": { "type": "string" } } }),
    };

    let answer = block_on(executor.execute_structured(
        &prompt,
        &LlModel::new("ollama", "llama3.2"),
        &output,
    ))
    .unwrap();

    assert_eq!(answer, Message::assistant("{}"));
    let (sent, tools) = client.seen.lock().unwrap().clone().unwrap();
    assert_eq!(tools, 0);
    assert_eq!(&sent.messages[..1], &prompt.messages[..]);
    match sent.messages.last() {
        Some(Message::System { content }) => {
            assert!(content.contains("Ticket"), "{content}");
            assert!(content.contains(&output.schema.to_string()), "{content}");
        }
        other => panic!("system instruction expected: {other:?}"),
    }
}

/// Counts the requests and answers with their number.
#[derive(Default)]
struct CountingClient(std::sync::atomic::AtomicUsize);

impl LlmClient for CountingClient {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        let number = self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        Box::pin(async move { Ok(Message::assistant(format!("Answer {number}"))) })
    }
}

#[test]
fn requests_several_choices_one_by_one_by_default() {
    let executor = PromptExecutor::builder()
        .client("ollama", Arc::new(CountingClient::default()))
        .build();
    let prompt = Prompt::builder("p").user("Hello").build();

    let choices =
        block_on(executor.execute_choices(&prompt, &LlModel::new("ollama", "m"), &[], 3)).unwrap();

    assert_eq!(
        choices.messages,
        ["Answer 1", "Answer 2", "Answer 3"].map(Message::assistant)
    );
    assert_eq!(choices.usage, None);
}

/// Reports 10 input and 2 output tokens per request (ADR-0067).
struct MeteredClient;

impl LlmClient for MeteredClient {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async { Ok(Message::assistant("Answer")) })
    }

    fn execute_with_usage<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<grauwolf_llm::Completion, LlmError>> {
        Box::pin(async move {
            Ok(grauwolf_llm::Completion {
                model: None,
                message: self.execute(prompt, model, tools).await?,
                usage: Some(grauwolf_prompt::Usage {
                    input_tokens: 10,
                    output_tokens: 2,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                }),
            })
        })
    }
}

#[test]
fn sums_the_usage_of_several_choices() {
    let executor = PromptExecutor::builder()
        .client("ollama", Arc::new(MeteredClient))
        .build();
    let prompt = Prompt::builder("p").user("Hello").build();

    let choices =
        block_on(executor.execute_choices(&prompt, &LlModel::new("ollama", "m"), &[], 3)).unwrap();

    let usage = choices.usage.unwrap();
    assert_eq!((usage.input_tokens, usage.output_tokens), (30, 6));
}

/// Client that can force tool calls only for models with `old` in their name (ADR-0063).
struct PickyClient;

impl LlmClient for PickyClient {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async { Ok(Message::assistant("?")) })
    }

    fn forces_tool_choice(&self, model: &LlModel) -> bool {
        model.id.contains("old")
    }
}

#[test]
fn asks_the_client_of_the_provider_whether_it_can_force_tool_calls() {
    let executor = PromptExecutor::builder()
        .client("picky", Arc::new(PickyClient))
        .client("ollama", Arc::new(NamedClient("ollama-client")))
        .build();

    assert!(executor.forces_tool_choice(&LlModel::new("picky", "old-model")));
    assert!(!executor.forces_tool_choice(&LlModel::new("picky", "new-model")));
    assert!(executor.forces_tool_choice(&LlModel::new("ollama", "llama3.2")));
    assert!(executor.forces_tool_choice(&LlModel::new("unknown", "x")));
}
