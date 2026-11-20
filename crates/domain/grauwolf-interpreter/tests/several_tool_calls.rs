//! Several tool calls in one answer under the sequential strategy templates (ADR-0041 addendum).

use grauwolf_spec::AgentSpecExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::ToolDescriptor;
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use grauwolf_tools::{DynTool, ToolError};
use serde_json::{Value, json};

/// Counts its calls.
struct Tick(Arc<AtomicUsize>);

impl DynTool for Tick {
    fn name(&self) -> &str {
        "tick"
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "tick".into(),
            description: "Ticks".into(),
            parameters: json!({ "type": "object" }),
        }
    }

    fn call_json(&self, _args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok("tock".into()) })
    }
}

/// Runs `strategy` against `mock`; returns the answer and the number of tool calls.
fn run(strategy: Value, mock: MockLlmClient) -> (Result<String, String>, usize) {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "ticker",
            "model": { "provider": "mock", "id": "m" },
            "mcp_servers": { "clock": { "command": "clock", "tools": ["tick"] } },
            "strategy": strategy
        })
        .to_string(),
    )
    .unwrap();
    let ticks = Arc::new(AtomicUsize::new(0));
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(mock))
            .build(),
    ))
    .with_tools(vec![Arc::new(Tick(Arc::clone(&ticks))) as Arc<dyn DynTool>])
    .load(&spec)
    .unwrap();
    let answer = block_on(agent.run("Tick twice!")).map_err(|error| error.to_string());
    (answer, ticks.load(Ordering::SeqCst))
}

/// A model that answers "Tick twice!" with two tool calls in one answer.
fn two_calls() -> MockLlmClient {
    MockLlmClient::new("?")
        .answer_when_contains("tock", "Done.")
        .call_tools_when_contains("twice", [("tick", json!({})), ("tick", json!({}))])
}

#[test]
fn single_run_runs_two_calls_of_one_answer_and_continues() {
    let (answer, ticks) = run(json!({ "kind": "single_run" }), two_calls());

    assert_eq!(answer, Ok("Done.".to_string()));
    assert_eq!(ticks, 2);
}

#[test]
fn chat_without_parallel_tools_runs_two_calls_of_one_answer() {
    let (answer, ticks) = run(
        json!({ "kind": "chat", "params": { "parallel_tools": false } }),
        two_calls(),
    );

    assert_eq!(answer, Ok("Done.".to_string()));
    assert_eq!(ticks, 2);
}

#[test]
fn verified_runs_two_calls_of_one_answer_and_still_verifies() {
    let mock = MockLlmClient::new("?")
        .answer_when_contains("Answer:\nDone.", r#"{ "correct": true, "feedback": "" }"#)
        .answer_when_contains("tock", "Done.")
        .call_tools_when_contains("twice", [("tick", json!({})), ("tick", json!({}))]);

    let (answer, ticks) = run(json!({ "kind": "verified" }), mock);

    assert_eq!(answer, Ok("Done.".to_string()));
    assert_eq!(ticks, 2);
}

#[test]
fn react_runs_two_calls_of_one_answer_and_reflects_on_the_results() {
    let mock = MockLlmClient::new("?")
        .answer_when_contains("tock", "Reflected.")
        .answer_when_contains("Reflected.", "Final.")
        .call_tools_when_contains("I tick", [("tick", json!({})), ("tick", json!({}))])
        .answer_when_contains("twice", "I tick twice.");

    let (answer, ticks) = run(json!({ "kind": "react" }), mock);

    assert_eq!(answer, Ok("Final.".to_string()));
    assert_eq!(ticks, 2);
}
