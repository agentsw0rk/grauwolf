//! Limits per run from the spec (ADR-0078).

use grauwolf_spec::AgentSpecExt;
use std::sync::Arc;

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_interpreter::{Interpreter, InterpreterError};
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::ToolDescriptor;
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use grauwolf_tools::{DynTool, ToolError};
use serde_json::{Value, json};

/// Counts up; the model keeps calling it.
struct Tick;

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
        Box::pin(async { Ok("tock".into()) })
    }
}

fn spec(limits: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "ticker",
            "model": { "provider": "mock", "id": "m" },
            "mcp_servers": { "uhr": { "command": "uhr", "tools": ["tick"] } },
            "limits": limits,
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap()
}

/// A model that keeps ticking until it learns of the limit.
fn interpreter() -> Interpreter {
    let mock = MockLlmClient::new("?")
        .answer_when_contains("limit of", "Enough ticking.")
        .call_tool_when_contains("Tick", "tick", json!({}))
        .call_tool_when_contains("tock", "tick", json!({}));
    Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(mock))
            .build(),
    ))
    .with_tools(vec![Arc::new(Tick) as Arc<dyn DynTool>])
}

#[test]
fn applies_the_limits_of_the_spec() {
    let limited = interpreter()
        .load(&spec(json!({ "max_tool_calls": 3 })))
        .unwrap();
    let no_steps = interpreter()
        .load(&spec(json!({ "max_steps": 2 })))
        .unwrap();
    let without_prices = interpreter()
        .load(&spec(json!({ "max_cost": 0.01 })))
        .err()
        .unwrap();

    assert_eq!(block_on(limited.run("Tick!")).unwrap(), "Enough ticking.");
    let error = block_on(no_steps.run("Tick!")).unwrap_err().to_string();
    assert!(error.contains("2 node executions"), "{error}");
    assert!(
        matches!(without_prices, InterpreterError::CostLimitWithoutPrices),
        "{without_prices}"
    );
}
