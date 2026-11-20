//! Tools with a state in the agent storage (ADR-0098): the state is part of checkpoints.

use std::sync::Arc;

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::AgentContext;
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::{LlModel, Prompt, ToolCall, ToolDescriptor};
use grauwolf_tools::{DynTool, ToolError, ToolRegistry};
use serde_json::{Value, json};

/// Counts its calls in the agent storage; `fail` leaves the count unchanged.
struct Counter;

impl DynTool for Counter {
    fn name(&self) -> &str {
        "count"
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "count".into(),
            description: "Counts".into(),
            parameters: json!({ "type": "object" }),
        }
    }

    fn call_json(&self, _args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async { Ok("no context".to_owned()) })
    }

    fn state_key(&self) -> Option<&str> {
        Some("test.counter")
    }

    fn call_with_state(&self, args: Value, state: &mut Value) -> Result<String, ToolError> {
        let count = state.as_u64().unwrap_or(0) + 1;
        *state = json!(count);
        if args["fail"] == true {
            return Err(ToolError::Execution("failed".into()));
        }
        Ok(format!("call {count}"))
    }
}

fn context() -> AgentContext {
    AgentContext::new(
        Arc::new(PromptExecutor::builder().build()),
        LlModel::new("mock", "m"),
        Prompt::builder("p").build(),
    )
    .with_tools(ToolRegistry::builder().tool(Counter).build())
}

fn call(args: Value) -> ToolCall {
    ToolCall {
        id: "c".into(),
        tool: "count".into(),
        args,
    }
}

#[test]
fn a_tool_with_state_keeps_it_in_the_agent_storage_and_checkpoints() {
    let ctx = context();

    let first = block_on(ctx.execute_tool(&call(json!({})))).content;
    let second = block_on(ctx.execute_tool(&call(json!({})))).content;
    let restored =
        AgentContext::restore(Arc::new(PromptExecutor::builder().build()), ctx.snapshot())
            .with_tools(ToolRegistry::builder().tool(Counter).build());
    let third = block_on(restored.execute_tool(&call(json!({})))).content;

    assert_eq!((first.as_str(), second.as_str()), ("call 1", "call 2"));
    assert_eq!(third, "call 3");
    assert_eq!(ctx.load::<u64>("test.counter").unwrap(), Some(2));
}

#[test]
fn a_failed_call_leaves_the_state_unchanged() {
    let ctx = context();

    block_on(ctx.execute_tool(&call(json!({}))));
    let failed = block_on(ctx.execute_tool(&call(json!({ "fail": true })))).content;

    assert_eq!(failed, "Error: tool failed: failed");
    assert_eq!(ctx.load::<u64>("test.counter").unwrap(), Some(1));
}
