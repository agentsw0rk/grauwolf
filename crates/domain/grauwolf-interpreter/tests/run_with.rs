//! Per-run runtime handles (ADR-0053): feature, frame sink, conversation.

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use grauwolf_agents::events::{AgentEvent, Feature};
use grauwolf_agents::memory::{ChatHistoryStore, MemoryChatHistoryStore};
use grauwolf_interpreter::{Interpreter, RunExtras};
use grauwolf_llm::{PromptExecutor, StreamFrame};
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use serde_json::json;

fn example(name: &str) -> AgentSpec {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/specs")
        .join(name);
    AgentSpec::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

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

#[test]
fn installs_a_feature_a_frame_sink_and_a_conversation_for_one_run_only() {
    let executor = PromptExecutor::builder()
        .client(
            "ollama",
            Arc::new(MockLlmClient::new("?").answer_when_contains("Hello", "Hello world")),
        )
        .build();
    let agent = Interpreter::new(Arc::new(executor))
        .load(&example("streaming_echo.json"))
        .unwrap();
    let recorder = Arc::new(Recorder::default());
    let frames = Arc::new(Mutex::new(String::new()));
    let sink_frames = Arc::clone(&frames);
    let history = Arc::new(MemoryChatHistoryStore::default());

    let output = block_on(agent.run_with(
        json!("Hello"),
        RunExtras {
            feature: Some(recorder.clone()),
            frame_sink: Some(Arc::new(move |frame: &StreamFrame| {
                if let StreamFrame::TextDelta { text } = frame {
                    sink_frames.lock().unwrap().push_str(text);
                }
            })),
            conversation: Some((history.clone(), "anna".into())),
            tools: Vec::new(),
            approver: None,
            memory: None,
            attachments: Vec::new(),
            caller: None,
            trace_parent: None,
            on_behalf_of: None,
        },
    ))
    .unwrap();
    let events_after_first = recorder.0.lock().unwrap().len();
    block_on(agent.run_json(json!("Hello"))).unwrap();

    assert_eq!(output, json!("Hello world"));
    assert_eq!(*frames.lock().unwrap(), "Hello world");
    assert_eq!(
        recorder.0.lock().unwrap().first().map(String::as_str),
        Some("agent_starting")
    );
    assert_eq!(
        recorder.0.lock().unwrap().len(),
        events_after_first,
        "Feature applied only to one run"
    );
    assert_eq!(block_on(history.load("anna")).unwrap().len(), 2);
}
