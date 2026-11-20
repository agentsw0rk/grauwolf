//! The model the provider reports on `llm_call_completed` (ADR-0094).

use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::AgentContext;
use grauwolf_agents::events::{AgentEvent, Feature};
use grauwolf_llm::{Completion, LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};

/// Reports a dated model version for every answer.
struct Dated;

impl LlmClient for Dated {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async { Ok(Message::assistant("Hello")) })
    }

    fn execute_with_usage<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async {
            Ok(Completion {
                message: Message::assistant("Hello"),
                usage: None,
                model: Some("m-2026-11-01".into()),
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

#[test]
fn llm_call_completed_carries_the_model_the_provider_reported() {
    let recorder = Arc::new(Recorder::default());
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(Dated))
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("Hello").build(),
    )
    .install(recorder.clone());

    block_on(ctx.request_llm()).unwrap();

    let events = recorder.0.lock().unwrap();
    let json = serde_json::to_value(events.last().unwrap()).unwrap();
    assert_eq!(json["event"], "llm_call_completed");
    assert_eq!(json["model"], "m-2026-11-01");
}

#[test]
fn streamed_llm_call_completed_carries_the_model_the_provider_reported() {
    let recorder = Arc::new(Recorder::default());
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(Dated))
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("Hello").build(),
    )
    .install(recorder.clone());

    block_on(ctx.request_llm_streaming()).unwrap();

    let events = recorder.0.lock().unwrap();
    let json = serde_json::to_value(events.last().unwrap()).unwrap();
    assert_eq!(json["event"], "llm_call_completed");
    assert_eq!(json["model"], "m-2026-11-01");
}
