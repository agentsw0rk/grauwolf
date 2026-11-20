//! Tools from resolved packages (ADR-0107) run in the sandbox like spec tools.

use grauwolf_spec::AgentSpecExt;
use std::sync::Arc;

use futures_executor::block_on;
use grauwolf_interpreter::{Interpreter, InterpreterError};
use grauwolf_llm::PromptExecutor;
use grauwolf_sandbox::SandboxRuntime;
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use grauwolf_tools::{ResolvedTool, ToolArtifact};
use serde_json::json;

/// Tool ABI v1 module that always answers `pong`.
const PONG: &str = r#"(module
  (memory (export "memory") 1)
  (data (i32.const 1024) "{\"ok\":\"pong\"}")
  (func (export "gw_alloc") (param i32) (result i32) (i32.const 2048))
  (func (export "gw_call") (param i32 i32) (result i64)
    (i64.or (i64.shl (i64.const 1024) (i64.const 32)) (i64.const 13))))"#;

fn package_tool() -> ResolvedTool {
    ResolvedTool {
        name: "ping".into(),
        description: "Answers pong".into(),
        args: json!({ "type": "object" }),
        artifact: ToolArtifact {
            name: "ping".into(),
            bytes: PONG.as_bytes().to_vec(),
            hash: "test".into(),
        },
        limits: Default::default(),
    }
}

fn spec() -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "tool_packages": [{ "oci": "registry.example/test/ping:1", "tools": ["ping"] }],
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap()
}

fn executor() -> Arc<PromptExecutor> {
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains("ping please", "ping", json!({}))
        .answer_when_contains("pong", "The tool said pong.");
    Arc::new(
        PromptExecutor::builder()
            .client("ollama", Arc::new(mock))
            .build(),
    )
}

#[test]
fn offers_package_tools_to_the_model() {
    let agent = Interpreter::new(executor())
        .with_tool_runtime(Arc::new(SandboxRuntime::new().unwrap()))
        .with_package_tools(vec![package_tool()])
        .load(&spec())
        .unwrap();

    assert_eq!(
        block_on(agent.run("ping please")).unwrap(),
        "The tool said pong."
    );
}

#[test]
fn package_tools_need_a_tool_runtime() {
    let error = Interpreter::new(executor())
        .with_package_tools(vec![package_tool()])
        .load(&spec())
        .err()
        .unwrap();
    assert!(
        matches!(error, InterpreterError::MissingToolRuntime),
        "{error}"
    );
}
