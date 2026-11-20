//! Fallback models on errors (F02.10, ADR-0088).

use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::AgentContext;
use grauwolf_agents::events::{AgentEvent, Feature};
use grauwolf_llm::{Completion, LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor, Usage};

/// Always answers with an error, like a provider that is down.
struct Down;

impl LlmClient for Down {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async { Err(LlmError::Client("provider unreachable".into())) })
    }
}

/// Answers with the model's name and 1000 input tokens.
struct Up;

impl LlmClient for Up {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async move { Ok(Message::assistant(format!("Answer from {}", model.id))) })
    }

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
                usage: Some(Usage {
                    input_tokens: 1000,
                    output_tokens: 0,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                }),
            })
        })
    }
}

#[derive(Default)]
struct Recorder(Mutex<Vec<AgentEvent>>);

impl Feature for Recorder {
    fn on_event(&self, event: &AgentEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

fn context(model: LlModel) -> (AgentContext, Arc<Recorder>) {
    let executor = PromptExecutor::builder()
        .client("down", Arc::new(Down))
        .client("up", Arc::new(Up))
        .build();
    let prices: grauwolf_prompt::Prices = serde_json::from_value(serde_json::json!({
        "currency": "USD",
        "models": {
            "down:primary": { "input": 1.0, "output": 1.0 },
            "up:backup": { "input": 2.0, "output": 2.0 }
        }
    }))
    .unwrap();
    let recorder = Arc::new(Recorder::default());
    let ctx = AgentContext::new(
        Arc::new(executor),
        model,
        Prompt::builder("p").user("Hello").build(),
    )
    .with_prices(Arc::new(prices))
    .install(recorder.clone());
    (ctx, recorder)
}

#[test]
fn falls_back_to_the_next_model_and_prices_its_answer() {
    let primary = LlModel::new("down", "primary").with_fallbacks(vec![
        LlModel::new("down", "second"),
        LlModel::new("up", "backup"),
    ]);
    let (mut ctx, recorder) = context(primary.clone());

    let answer = block_on(ctx.request_llm()).unwrap();

    assert_eq!(answer, Message::assistant("Answer from backup"));
    assert_eq!(ctx.model(), &primary, "the active model stays");
    assert_eq!(ctx.cost(), Some(0.002));
    let fallbacks: Vec<(String, String)> = recorder
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            AgentEvent::LlmFallback { from, to, error } => {
                assert!(error.contains("unreachable"), "{error}");
                Some((from.id.clone(), to.id.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        fallbacks,
        [
            ("primary".to_owned(), "second".to_owned()),
            ("second".to_owned(), "backup".to_owned())
        ]
    );
}

#[test]
fn fails_as_before_without_fallbacks() {
    let (mut ctx, _) = context(LlModel::new("down", "primary"));

    let error = block_on(ctx.request_llm()).unwrap_err();

    assert!(error.to_string().contains("unreachable"), "{error}");
}

/// Answers with a JSON object that understands every kind of request.
struct JsonUp;

impl LlmClient for JsonUp {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async {
            Ok(Message::assistant(
                r#"{"model":"backup","harmful":false,"categories":[],"facts":["Fact"],"durable":[]}"#,
            ))
        })
    }
}

/// Every kind of request falls back: streaming, structured, several answers, compression,
/// facts and moderation (ADR-0088).
#[test]
fn every_kind_of_request_falls_back() {
    let executor = Arc::new(
        PromptExecutor::builder()
            .client("down", Arc::new(Down))
            .client("up", Arc::new(JsonUp))
            .build(),
    );
    let model = LlModel::new("down", "primary").with_fallbacks(vec![LlModel::new("up", "backup")]);
    let recorder = Arc::new(Recorder::default());
    let mut ctx = AgentContext::new(
        executor,
        model.clone(),
        Prompt::builder("p").user("Hello").build(),
    )
    .install(recorder.clone());
    let output = grauwolf_llm::StructuredOutput {
        name: "Answer".into(),
        schema: serde_json::json!({ "type": "object", "properties": { "model": { "type": "string" } } }),
    };

    let streamed = block_on(ctx.request_llm_streaming()).unwrap();
    let structured = block_on(ctx.request_llm_structured(&output, 0)).unwrap();
    let choices = block_on(ctx.request_llm_choices(2)).unwrap();
    let facts = block_on(ctx.extract_facts("My name is Anna.", "Hello Anna.", &[])).unwrap();
    let moderation = block_on(ctx.moderate("Hello", None)).unwrap();
    block_on(ctx.compress_history(grauwolf_agents::HistoryCompression::Summarize)).unwrap();

    assert!(format!("{streamed:?}").contains("backup"), "{streamed:?}");
    assert_eq!(structured["model"], "backup");
    assert_eq!(choices.len(), 2);
    assert_eq!(facts, ["Fact"]);
    assert!(!moderation.harmful);
    let fallbacks = recorder
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|event| matches!(event, AgentEvent::LlmFallback { .. }))
        .count();
    assert_eq!(fallbacks, 6);
}
