//! Tools that require approval (ADR-0061).

use grauwolf_spec::AgentSpecExt;
use std::sync::Arc;

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::events::ToolDecision;
use grauwolf_interpreter::{InterpretedAgent, Interpreter, RunExtras, ToolApprover};
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::ToolDescriptor;
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use grauwolf_tools::{DynTool, ToolError};
use serde_json::{Value, json};

/// Writes text in uppercase.
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

/// Agent that calls `upper` for "loudly" and returns the tool result; `upper` requires
/// approval.
fn agent() -> InterpretedAgent {
    let spec = AgentSpec::from_json(
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
    .unwrap();
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains("loudly", "upper", json!({ "text": "hello" }))
        .answer_when_contains("HELLO", "Done")
        .answer_when_contains("approval", "Rejected");
    Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(mock))
            .build(),
    ))
    .with_tools(vec![Arc::new(Upper) as Arc<dyn DynTool>])
    .with_tool_approval(["upper"])
    .load(&spec)
    .unwrap()
}

#[test]
fn denies_tools_that_need_approval_when_nobody_can_approve() {
    let output = block_on(agent().run("Say it loudly")).unwrap();

    assert_eq!(output, "Rejected");
}

/// Approver that only allows calls with the argument `text = "hello"`.
fn approver(seen: Arc<std::sync::Mutex<Vec<String>>>) -> ToolApprover {
    Arc::new(move |call| {
        seen.lock().unwrap().push(call.tool.clone());
        let decision = if call.args["text"] == "hello" {
            ToolDecision::Proceed
        } else {
            ToolDecision::Deny("Approval denied".into())
        };
        Box::pin(async move { decision })
    })
}

#[test]
fn runs_tools_that_need_approval_once_the_approver_agrees() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));

    let output = block_on(agent().run_with(
        json!("Say it loudly"),
        RunExtras {
            approver: Some(approver(Arc::clone(&seen))),
            ..RunExtras::default()
        },
    ))
    .unwrap();

    assert_eq!(output, json!("Done"));
    assert_eq!(*seen.lock().unwrap(), ["upper"]);
}

/// Stands in for the built-in command tool (ADR-0099).
struct RunCommand;

impl DynTool for RunCommand {
    fn name(&self) -> &str {
        "run_command"
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "run_command".into(),
            description: "Runs a command".into(),
            parameters: json!({ "type": "object" }),
        }
    }

    fn call_json(&self, _args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async { Ok("RAN".to_owned()) })
    }
}

fn command_agent(command: Value) -> InterpretedAgent {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "runner",
            "model": { "provider": "mock", "id": "m" },
            "builtin_tools": { "tools": ["run_command"], "root": ".", "command": command },
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
    .unwrap();
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains("run it", "run_command", json!({ "command": "echo hi" }))
        .answer_when_contains("RAN", "Done")
        .answer_when_contains("approval", "Rejected");
    Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(mock))
            .build(),
    ))
    .with_tools(vec![Arc::new(RunCommand) as Arc<dyn DynTool>])
    .load(&spec)
    .unwrap()
}

const SHELL: fn() -> Value =
    || json!({ "allowed": ["*"], "shell": true, "i_understand_shell": true });

#[test]
fn shell_mode_needs_an_approval_for_every_call() {
    let refused = block_on(command_agent(SHELL()).run("run it")).unwrap();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let approver: ToolApprover = {
        let seen = Arc::clone(&seen);
        Arc::new(move |call| {
            seen.lock().unwrap().push(call.args["command"].to_string());
            Box::pin(async { ToolDecision::Proceed })
        })
    };
    let approved = block_on(command_agent(SHELL()).run_with(
        json!("run it"),
        RunExtras {
            approver: Some(approver),
            ..RunExtras::default()
        },
    ))
    .unwrap();

    assert_eq!(refused, "Rejected");
    assert_eq!(approved, json!("Done"));
    assert_eq!(*seen.lock().unwrap(), ["\"echo hi\""]);
}

#[test]
fn allowlist_mode_and_approve_each_call_false_need_no_approval() {
    let allowlist = json!({ "allowed": ["echo"] });
    let mut open = SHELL();
    open["approve_each_call"] = json!(false);

    assert_eq!(
        block_on(command_agent(allowlist).run("run it")).unwrap(),
        "Done"
    );
    assert_eq!(block_on(command_agent(open).run("run it")).unwrap(), "Done");
}
