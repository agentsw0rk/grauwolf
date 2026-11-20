//! Nodes of kind `goap` in the interpreter (ADR-0070).

use grauwolf_spec::AgentSpecExt;
use std::sync::Arc;

use futures_executor::block_on;
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::PromptExecutor;
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use serde_json::json;

fn spec() -> AgentSpec {
    let step = |name: &str| {
        json!({ "graph": {
            "nodes": [ { "id": name, "kind": "llm_request" } ],
            "edges": [ { "from": "start", "to": name },
                       { "from": name, "to": "finish", "when": { "kind": "on_assistant_message" } } ]
        } })
    };
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "baker",
            "model": { "provider": "mock", "id": "m" },
            "subgraphs": { "buy": step("buy"), "knead": step("knead"), "bake": step("bake") },
            "graph": {
                "nodes": [ { "id": "baker", "kind": "goap", "params": {
                    "state": { "has_flour": false, "dough_ready": false, "bread_baked": false },
                    "actions": [
                        { "name": "bake", "subgraph": "bake", "pre": { "dough_ready": true },
                          "effects": { "bread_baked": true } },
                        { "name": "buy", "subgraph": "buy", "effects": { "has_flour": true } },
                        { "name": "knead", "subgraph": "knead", "pre": { "has_flour": true },
                          "effects": { "dough_ready": true } }
                    ],
                    "goals": [ { "name": "bread", "facts": { "bread_baked": true } } ]
                } } ],
                "edges": [ { "from": "start", "to": "baker" }, { "from": "baker", "to": "finish" } ]
            }
        })
        .to_string(),
    )
    .unwrap()
}

#[test]
fn runs_the_cheapest_actions_to_the_goal_as_subgraphs() {
    let mock = MockLlmClient::new("?")
        .answer_when_contains("Action: buy", "Bought flour.")
        .answer_when_contains("Action: knead", "Dough kneaded.")
        .answer_when_contains("Action: bake", "Bread baked.");
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(mock))
            .build(),
    ))
    .load(&spec())
    .unwrap();

    let output = block_on(agent.run("Bake a loaf of bread.")).unwrap();

    assert_eq!(output, "Bread baked.");
}

/// Addendum to ADR-0070: an action can run a sub-agent from `agent_tools` as a step, with an
/// input template, a success check and the answer in a fact; planner input and output are facts.
#[test]
fn runs_an_action_with_a_sub_agent_as_executor_and_delivers_the_output_fact() {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "polisher",
            "model": { "provider": "ollama", "id": "m" },
            "agent_tools": { "translator": { "spec": "echo.json", "description": "Translates" } },
            "subgraphs": { "polish": { "graph": {
                "nodes": [ { "id": "polish", "kind": "llm_request" } ],
                "edges": [ { "from": "start", "to": "polish" },
                           { "from": "polish", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
            } } },
            "graph": {
                "nodes": [ { "id": "work", "kind": "goap", "params": {
                    "state": { "request": "", "result": "", "translated": false, "done": false },
                    "input_fact": "request",
                    "output_fact": "result",
                    "actions": [
                        { "name": "translate", "agent": "translator",
                          "input": "Translate: {{request}}", "output_fact": "result",
                          "success_contains": "Hello", "effects": { "translated": true } },
                        { "name": "polish", "subgraph": "polish", "pre": { "translated": true },
                          "input": "Polish: {{result}}", "output_fact": "result",
                          "effects": { "done": true } }
                    ],
                    "goals": [ { "name": "done", "facts": { "done": true } } ]
                } } ],
                "edges": [ { "from": "start", "to": "work" }, { "from": "work", "to": "finish" } ]
            }
        })
        .to_string(),
    )
    .unwrap();
    let translator = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client(
                "ollama",
                Arc::new(MockLlmClient::new("?").answer_when_contains("Translate: Hola", "Hello")),
            )
            .build(),
    ))
    .load(&echo())
    .unwrap();
    let main = MockLlmClient::new("?").answer_when_contains("Polish: Hello", "Hello, friend!");
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("ollama", Arc::new(main))
            .build(),
    ))
    .with_tools(vec![
        Arc::new(translator).into_tool("translator", "Translates"),
    ])
    .load(&spec)
    .unwrap();

    let output = block_on(agent.run("Hola")).unwrap();

    assert_eq!(output, "Hello, friend!");
}

fn echo() -> AgentSpec {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/specs/echo.json");
    AgentSpec::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}
