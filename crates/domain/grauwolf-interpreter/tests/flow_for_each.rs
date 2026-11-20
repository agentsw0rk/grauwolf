//! The `for_each` node in the interpreter: a flow over a list (F14.13, ADR-0101).

use grauwolf_spec::AgentSpecExt;
use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use grauwolf_agents::events::{AgentEvent, Feature};
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::PromptExecutor;
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use serde_json::{Value, json};

/// A sub-agent that answers like `mock`.
fn sub_agent(name: &str, mock: MockLlmClient) -> Arc<dyn grauwolf_tools::DynTool> {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": name,
            "model": { "provider": "ollama", "id": "m" },
            "graph": {
                "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
                "edges": [
                    { "from": "start", "to": "call_llm" },
                    { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } }
                ]
            }
        })
        .to_string(),
    )
    .unwrap();
    let interpreter = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("ollama", Arc::new(mock))
            .build(),
    ));
    Arc::new(interpreter.load(&spec).unwrap()).into_tool(name, name)
}

/// researcher → for_each (writer) → summarizer, with the given `for_each` parameters.
fn flow(each: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "flow",
            "model": { "provider": "ollama", "id": "m" },
            "system_prompt": "Shout the text.",
            "agent_tools": {
                "researcher": { "spec": "researcher.json", "description": "Finds topics" },
                "writer": { "spec": "writer.json", "description": "Writes" },
                "summarizer": { "spec": "summarizer.json", "description": "Summarizes" }
            },
            "subgraphs": {
                "shout": {
                    "input": "string", "output": "string",
                    "graph": {
                        "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
                        "edges": [
                            { "from": "start", "to": "call_llm" },
                            { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } }
                        ]
                    }
                }
            },
            "graph": {
                "nodes": [
                    { "id": "research", "kind": "agent", "params": { "agent": "researcher" } },
                    { "id": "write_all", "kind": "for_each", "params": each },
                    { "id": "summarize", "kind": "agent", "params": { "agent": "summarizer" } }
                ],
                "edges": [
                    { "from": "start", "to": "research" },
                    { "from": "research", "to": "write_all" },
                    { "from": "write_all", "to": "summarize" },
                    { "from": "summarize", "to": "finish" }
                ]
            }
        })
        .to_string(),
    )
    .unwrap()
}

#[derive(Default)]
struct Recorder(Mutex<Vec<AgentEvent>>);

impl Feature for Recorder {
    fn on_event(&self, event: &AgentEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

fn agents() -> Vec<Arc<dyn grauwolf_tools::DynTool>> {
    vec![
        sub_agent(
            "researcher",
            MockLlmClient::new("?")
                .answer_when_contains("Research", r#"["alpha", "beta", "gamma"]"#),
        ),
        sub_agent(
            "writer",
            MockLlmClient::new("?")
                .answer_when_contains("alpha", "Article alpha")
                .answer_when_contains("beta", "Article beta")
                .answer_when_contains("gamma", "Article gamma"),
        ),
        sub_agent(
            "summarizer",
            MockLlmClient::new("?").answer_when_contains(
                r#"["Article alpha","Article beta","Article gamma"]"#,
                "Summary of three articles",
            ),
        ),
    ]
}

#[test]
fn a_researcher_a_writer_per_topic_and_a_summarizer_make_a_flow() {
    let recorder = Arc::new(Recorder::default());
    let each = json!({ "input": "string", "output": "string", "body": { "agent": "writer" }, "max_parallel": 2 });
    let agent = Interpreter::new(Arc::new(PromptExecutor::builder().build()))
        .with_tools(agents())
        .with_feature(recorder.clone())
        .load(&flow(each))
        .unwrap();

    let answer = block_on(agent.run("Research three topics")).unwrap();

    assert_eq!(answer, "Summary of three articles");
    let events = recorder.0.lock().unwrap();
    let completed = events
        .iter()
        .filter(|event| matches!(event, AgentEvent::ForEachItemCompleted { node, .. } if node == "write_all"))
        .count();
    assert_eq!(completed, 3);
}

#[test]
fn a_subgraph_body_runs_on_the_model_of_the_spec_for_each_element() {
    let mock = MockLlmClient::new("?")
        .answer_when_contains("alpha", "ALPHA")
        .answer_when_contains("beta", "BETA")
        .answer_when_contains("gamma", "GAMMA");
    let each = json!({ "input": "string", "output": "string", "body": { "subgraph": "shout" } });
    let mut tools = agents();
    // The summarizer sees the shouted list.
    tools[2] = sub_agent(
        "summarizer",
        MockLlmClient::new("?").answer_when_contains(r#"["ALPHA","BETA","GAMMA"]"#, "Loud summary"),
    );
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("ollama", Arc::new(mock))
            .build(),
    ))
    .with_tools(tools)
    .load(&flow(each))
    .unwrap();

    let answer = block_on(agent.run("Research three topics")).unwrap();

    assert_eq!(answer, "Loud summary");
}
