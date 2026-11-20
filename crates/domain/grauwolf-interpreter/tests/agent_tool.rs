//! Agents as tools of other agents (F03.12, ADR-0040).

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;
use std::sync::Arc;

use futures_executor::block_on;
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::PromptExecutor;
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use serde_json::json;

fn example(name: &str) -> AgentSpec {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/specs")
        .join(name);
    AgentSpec::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn interpreter(mock: MockLlmClient) -> Interpreter {
    Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("ollama", Arc::new(mock))
            .build(),
    ))
}

#[test]
fn offers_a_text_agent_as_tool_with_a_request_argument() {
    let agent = interpreter(MockLlmClient::new("?").answer_when_contains("Hola", "Hello"))
        .load(&example("echo.json"))
        .unwrap();

    let tool = Arc::new(agent).into_tool("translator", "Translates into English");
    let descriptor = tool.descriptor();
    let answer = block_on(tool.call_json(json!({ "request": "Hola" }))).unwrap();

    assert_eq!(descriptor.name, "translator");
    assert_eq!(descriptor.description, "Translates into English");
    assert_eq!(
        descriptor.parameters,
        json!({
            "type": "object",
            "properties": { "request": { "type": "string", "description": "Request to the agent" } },
            "required": ["request"]
        })
    );
    assert_eq!(answer, "Hello");
}

#[test]
fn offers_a_typed_agent_with_its_input_schema_and_returns_json_text() {
    let spec = example("relay.json");
    let agent = interpreter(MockLlmClient::new("?")).load(&spec).unwrap();

    let tool = Arc::new(agent).into_tool("relay", "Forwards tickets");
    let answer =
        block_on(tool.call_json(json!({ "title": "Bug", "priority": 2, "tags": [] }))).unwrap();
    let invalid = block_on(tool.call_json(json!({ "title": "Bug" }))).unwrap_err();

    assert_eq!(tool.descriptor().parameters, spec.types["Ticket"]);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&answer).unwrap(),
        json!({ "title": "Bug", "priority": 2, "tags": [] })
    );
    assert!(invalid.to_string().contains("priority"), "{invalid}");
}

#[test]
fn coordinator_delegates_to_a_sub_agent_declared_in_agent_tools() {
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains("Translate:", "translator", json!({ "request": "Hola" }))
        .answer_when_contains("Hola", "Hello")
        .answer_when_contains("Hello", "In English: Hello");
    let translator = Arc::new(
        interpreter(mock.clone())
            .load(&example("echo.json"))
            .unwrap(),
    );
    let coordinator = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "coordinator",
            "model": { "provider": "ollama", "id": "m" },
            "agent_tools": { "translator": { "spec": "echo.json", "description": "Translates" } },
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
    let missing = interpreter(mock.clone()).load(&coordinator).err().unwrap();

    let agent = interpreter(mock)
        .with_tools(vec![translator.into_tool("translator", "Translates")])
        .load(&coordinator)
        .unwrap();

    assert!(matches!(
        missing,
        grauwolf_interpreter::InterpreterError::MissingExternalTool { .. }
    ));
    assert_eq!(
        block_on(agent.run("Translate: Hola")).unwrap(),
        "In English: Hello"
    );
}
