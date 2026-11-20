//! Events of interpreted agents (F06.3, ADR-0007).

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use grauwolf_agents::events::{AgentEvent, Feature};
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::PromptExecutor;
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use serde_json::json;

#[derive(Default)]
struct Recorder(Mutex<Vec<String>>);

impl Feature for Recorder {
    fn on_event(&self, event: &AgentEvent) {
        let name = serde_json::to_value(event).unwrap()["event"]
            .as_str()
            .unwrap()
            .to_owned();
        self.0.lock().unwrap().push(name);
    }
}

fn echo() -> AgentSpec {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/specs/echo.json");
    AgentSpec::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn reports_agent_node_and_llm_events_of_a_run() {
    let recorder = Arc::new(Recorder::default());
    let executor = PromptExecutor::builder()
        .client(
            "ollama",
            Arc::new(MockLlmClient::new("?").answer_when_contains("Hello", "Hello")),
        )
        .build();
    let agent = Interpreter::new(Arc::new(executor))
        .with_feature(recorder.clone())
        .load(&echo())
        .unwrap();

    block_on(agent.run_json(json!("Hello"))).unwrap();

    assert_eq!(
        *recorder.0.lock().unwrap(),
        [
            "agent_starting",
            "node_starting",
            "llm_call_starting",
            "llm_call_completed",
            "node_completed",
            "agent_completed",
        ]
    );
}

#[test]
fn reports_failed_runs() {
    let recorder = Arc::new(Recorder::default());
    let agent = Interpreter::new(Arc::new(PromptExecutor::builder().build()))
        .with_feature(recorder.clone())
        .load(&echo())
        .unwrap();

    assert!(block_on(agent.run_json(json!("Hello"))).is_err());

    assert_eq!(
        recorder.0.lock().unwrap().last().map(String::as_str),
        Some("agent_failed")
    );
}
