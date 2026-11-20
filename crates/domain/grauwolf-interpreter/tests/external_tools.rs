//! External tools, e.g. from MCP servers (ADR-0036).

use grauwolf_spec::AgentSpecExt;
use std::sync::Arc;

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_interpreter::{Interpreter, InterpreterError, RunExtras};
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::ToolDescriptor;
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use grauwolf_tools::{DynTool, ToolError};
use serde_json::{Value, json};

/// Stands in for an MCP tool: writes text in uppercase.
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

fn spec() -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "shouter",
            "model": { "provider": "mock", "id": "m" },
            "mcp_servers": { "text": { "command": "text-server", "tools": ["upper"] } },
            "graph": {
                "nodes": [ { "id": "call_llm", "kind": "llm_request" },
                           { "id": "execute_tool", "kind": "execute_tool" },
                           { "id": "send_result", "kind": "llm_send_tool_result" } ],
                "edges": [ { "from": "start", "to": "call_llm" },
                           { "from": "call_llm", "to": "execute_tool", "when": { "kind": "on_tool_call" } },
                           { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } },
                           { "from": "execute_tool", "to": "send_result" },
                           { "from": "send_result", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
            }
        })
        .to_string(),
    )
    .unwrap()
}

fn interpreter() -> Interpreter {
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains("loudly", "upper", json!({ "text": "hello" }))
        .answer_when_contains("HELLO", "Finished: HELLO");
    Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(mock))
            .build(),
    ))
}

#[test]
fn uses_external_tools_declared_by_mcp_servers() {
    let agent = interpreter()
        .with_tools(vec![Arc::new(Upper) as Arc<dyn DynTool>])
        .load(&spec())
        .unwrap();

    assert_eq!(
        block_on(agent.run("Say it loudly")).unwrap(),
        "Finished: HELLO"
    );
}

#[test]
fn reports_declared_mcp_tools_that_were_not_provided() {
    let error = interpreter().load(&spec()).err().unwrap();

    assert!(
        matches!(&error, InterpreterError::MissingExternalTool { tool } if tool == "upper"),
        "{error}"
    );
}

#[test]
fn requires_the_builtin_tools_of_the_spec() {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "filer",
            "model": { "provider": "mock", "id": "m" },
            "builtin_tools": { "tools": ["say_to_user"] },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();

    let error = interpreter().load(&spec).err().unwrap();

    assert!(
        matches!(&error, InterpreterError::MissingExternalTool { tool } if tool == "say_to_user"),
        "{error}"
    );
}

#[test]
fn requires_the_search_tool_of_the_knowledge_section() {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "librarian",
            "model": { "provider": "mock", "id": "m" },
            "knowledge": { "sources": ["docs"], "embedding_model": { "provider": "ollama", "id": "e" } },
            "graph": {
                "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
                "edges": [ { "from": "start", "to": "call_llm" },
                           { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
            }
        })
        .to_string(),
    )
    .unwrap();

    let error = interpreter().load(&spec).err().unwrap();

    assert!(
        matches!(&error, InterpreterError::MissingExternalTool { tool } if tool == "search_knowledge"),
        "{error}"
    );
}

/// Replaces `upper` for a single run.
struct Replacement;

impl DynTool for Replacement {
    fn name(&self) -> &str {
        "upper"
    }

    fn descriptor(&self) -> ToolDescriptor {
        Upper.descriptor()
    }

    fn call_json(&self, _args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async { Ok("REPLACED".to_owned()) })
    }
}

#[test]
fn replaces_tools_of_the_same_name_for_one_run_only() {
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains("loudly", "upper", json!({ "text": "hello" }))
        .answer_when_contains("REPLACED", "Finished: REPLACED")
        .answer_when_contains("HELLO", "Finished: HELLO");
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(mock))
            .build(),
    ))
    .with_tools(vec![Arc::new(Upper) as Arc<dyn DynTool>])
    .load(&spec())
    .unwrap();

    let replaced = block_on(agent.run_with(
        json!("Say it loudly"),
        RunExtras {
            tools: vec![Arc::new(Replacement) as Arc<dyn DynTool>],
            ..RunExtras::default()
        },
    ))
    .unwrap();
    let normal = block_on(agent.run("Say it loudly")).unwrap();

    assert_eq!(replaced, json!("Finished: REPLACED"));
    assert_eq!(normal, "Finished: HELLO");
}
