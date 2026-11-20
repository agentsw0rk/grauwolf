//! Checkpoint runs with runtime handles (ADR-0077).

use grauwolf_spec::AgentSpecExt;
use std::sync::Arc;

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::checkpoints::MemoryCheckpointStore;
use grauwolf_agents::events::ToolDecision;
use grauwolf_interpreter::{InterpretedAgent, Interpreter, RunExtras, ToolApprover};
use grauwolf_llm::{LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use grauwolf_tools::{DynTool, ToolError};
use serde_json::{Value, json};

/// Writes text in uppercase; only enters the run via `RunExtras.tools`.
struct Upper;

impl DynTool for Upper {
    fn name(&self) -> &str {
        "upper"
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "upper".into(),
            description: "Writes text in uppercase".into(),
            parameters: json!({ "type": "object" }),
        }
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        let text = args["text"].as_str().unwrap_or_default().to_uppercase();
        Box::pin(async move { Ok(text) })
    }
}

/// Agent with a tool loop; `upper` requires approval.
fn agent() -> InterpretedAgent {
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains("loudly", "upper", json!({ "text": "hello" }))
        .answer_when_contains("HELLO", "Done");
    agent_with(Arc::new(mock))
}

/// The same agent with a different LLM client.
fn agent_with(client: Arc<dyn LlmClient>) -> InterpretedAgent {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "shouter",
            "model": { "provider": "mock", "id": "m" },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    Interpreter::new(Arc::new(
        PromptExecutor::builder().client("mock", client).build(),
    ))
    .with_tool_approval(["upper"])
    .load(&spec)
    .unwrap()
}

fn approving() -> ToolApprover {
    Arc::new(|_call| Box::pin(async { ToolDecision::Proceed }))
}

#[test]
fn offers_run_tools_and_asks_the_approver_in_checkpointed_runs() {
    let store = Arc::new(MemoryCheckpointStore::default());
    let extras = RunExtras {
        tools: vec![Arc::new(Upper) as Arc<dyn DynTool>],
        approver: Some(approving()),
        ..RunExtras::default()
    };

    let output =
        block_on(agent().run_checkpointed_with(json!("Say it loudly"), store, "run-1", extras))
            .unwrap();

    assert_eq!(output, json!("Done"));
}

/// LLM that fails immediately.
struct Down;

impl LlmClient for Down {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async { Err(LlmError::Client("Connection lost".into())) })
    }
}

#[test]
fn resumes_with_run_tools_and_the_approver_of_the_resuming_run() {
    let store = Arc::new(MemoryCheckpointStore::default());
    let crashed = block_on(agent_with(Arc::new(Down)).run_checkpointed(
        json!("Say it loudly"),
        store.clone(),
        "run-2",
    ));
    assert!(crashed.is_err());
    let extras = RunExtras {
        tools: vec![Arc::new(Upper) as Arc<dyn DynTool>],
        approver: Some(approving()),
        ..RunExtras::default()
    };

    let output = block_on(agent().resume_with(store, "run-2", extras)).unwrap();

    assert_eq!(output, json!("Done"));
}
