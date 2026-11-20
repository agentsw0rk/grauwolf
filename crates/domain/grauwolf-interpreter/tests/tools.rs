//! Interpreter with spec tools in the sandbox (F13.10, ADR-0024).

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;
use std::sync::Arc;

use futures_executor::block_on;
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::PromptExecutor;
use grauwolf_sandbox::SandboxRuntime;
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use grauwolf_toolbuild::ToolBuilder;
use serde_json::json;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

#[test]
fn runs_spec_tools_inside_the_sandbox() {
    let spec = AgentSpec::from_json(
        &std::fs::read_to_string(workspace().join("examples/specs/calculator.json")).unwrap(),
    )
    .unwrap();
    let builder = ToolBuilder::new(workspace().join("target/toolbuild-tests"));
    builder.prepare().unwrap();
    let artifacts: Vec<_> = spec
        .tools
        .iter()
        .map(|tool| builder.build(tool).unwrap())
        .collect();
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains("2+3", "add", json!({ "a": 2, "b": 3 }))
        .answer_when_contains("5", "The result is 5.");
    let executor = PromptExecutor::builder()
        .client("ollama", Arc::new(mock))
        .build();

    let agent = Interpreter::new(Arc::new(executor))
        .with_tool_runtime(Arc::new(SandboxRuntime::new().unwrap()))
        .load_with_artifacts(&spec, &artifacts)
        .unwrap();

    assert_eq!(
        block_on(agent.run("What is 2+3?")).unwrap(),
        "The result is 5."
    );
}
