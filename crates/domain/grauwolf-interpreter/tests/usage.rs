//! Token usage of a run (ADR-0067).

use grauwolf_spec::AgentSpecExt;
use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::events::{AgentEvent, Feature};
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::{Completion, LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor, Usage};
use grauwolf_spec::AgentSpec;
use serde_json::json;

/// Answers with text and reports 10 input and 2 output tokens.
struct Counting;

impl LlmClient for Counting {
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
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async move {
            Ok(Completion {
                model: None,
                message: self.execute(prompt, model, tools).await?,
                usage: Some(Usage {
                    input_tokens: 10,
                    output_tokens: 2,
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

#[test]
fn reports_the_total_usage_of_a_run_when_it_completes() {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "twice",
            "model": { "provider": "mock", "id": "m" },
            "graph": {
                "nodes": [ { "id": "first", "kind": "llm_request" },
                           { "id": "again", "kind": "llm_continue" } ],
                "edges": [ { "from": "start", "to": "first" },
                           { "from": "first", "to": "again", "when": { "kind": "on_assistant_message" } },
                           { "from": "again", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
            }
        })
        .to_string(),
    )
    .unwrap();
    let recorder = Arc::new(Recorder::default());
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(Counting))
            .build(),
    ))
    .with_feature(recorder.clone())
    .load(&spec)
    .unwrap();

    block_on(agent.run("Hello")).unwrap();

    let events = recorder.0.lock().unwrap();
    assert_eq!(
        events.last(),
        Some(&AgentEvent::AgentCompleted {
            output: json!("Hello"),
            usage: Some(Usage {
                input_tokens: 20,
                output_tokens: 4,
                cache_read_tokens: None,
                cache_write_tokens: None,
            }),
            cost: None
        })
    );
}

/// Costs (ADR-0071): total of the run in `agent_completed`.
#[test]
fn reports_the_total_cost_of_a_run_with_a_price_table() {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "twice",
            "model": { "provider": "mock", "id": "m" },
            "graph": {
                "nodes": [ { "id": "first", "kind": "llm_request" },
                           { "id": "again", "kind": "llm_continue" } ],
                "edges": [ { "from": "start", "to": "first" },
                           { "from": "first", "to": "again", "when": { "kind": "on_assistant_message" } },
                           { "from": "again", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
            }
        })
        .to_string(),
    )
    .unwrap();
    let prices: grauwolf_prompt::Prices = serde_json::from_value(json!({
        "currency": "EUR", "models": { "mock:m": { "input": 2.0, "output": 8.0 } }
    }))
    .unwrap();
    let recorder = Arc::new(Recorder::default());
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(Counting))
            .build(),
    ))
    .with_prices(Arc::new(prices))
    .with_feature(recorder.clone())
    .load(&spec)
    .unwrap();

    block_on(agent.run("Hello")).unwrap();

    let events = recorder.0.lock().unwrap();
    let Some(AgentEvent::AgentCompleted { cost, .. }) = events.last() else {
        panic!("{events:?}");
    };
    // Two calls with 10 input and 2 output tokens each.
    let expected = 2.0 * (10.0 * 2.0 + 2.0 * 8.0) / 1e6;
    assert!((cost.unwrap() - expected).abs() < 1e-12);
}
