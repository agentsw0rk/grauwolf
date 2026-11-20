//! Typed events and features (F06.1, F06.3, ADR-0007).

use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::events::{AgentEvent, Feature};
use grauwolf_agents::{AgentContext, conditions, nodes};
use grauwolf_graph::TypedGraph;
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::{LlModel, Prompt};
use grauwolf_test::MockLlmClient;
use grauwolf_tools::{Tool, ToolError, ToolRegistry};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize, JsonSchema)]
struct AddArgs {
    a: f64,
    b: f64,
}

struct Add;

impl Tool for Add {
    type Args = AddArgs;
    type Output = f64;

    fn name(&self) -> &str {
        "add"
    }

    fn description(&self) -> &str {
        "Adds two numbers"
    }

    fn call(&self, args: AddArgs) -> BoxFuture<'_, Result<f64, ToolError>> {
        Box::pin(async move { Ok(args.a + args.b) })
    }
}

/// Feature that collects all events.
#[derive(Default)]
struct Recorder(Mutex<Vec<AgentEvent>>);

impl Feature for Recorder {
    fn on_event(&self, event: &AgentEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

#[test]
fn reports_llm_and_tool_calls_to_installed_features() {
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains("2+3", "add", json!({ "a": 2, "b": 3 }))
        .answer_when_contains("5.0", "The result is 5.");
    let recorder = Arc::new(Recorder::default());
    let mut ctx = AgentContext::new(
        Arc::new(
            PromptExecutor::builder()
                .client("mock", Arc::new(mock))
                .build(),
        ),
        LlModel::new("mock", "m"),
        Prompt::builder("calc").build(),
    )
    .with_tools(ToolRegistry::builder().tool(Add).build())
    .install(recorder.clone());
    let graph = TypedGraph::<AgentContext, String, String>::build(|g| {
        let call_llm = g.node("call_llm", nodes::llm_request);
        let execute_tool = g.node("execute_tool", nodes::execute_tool);
        let send_result = g.node("send_result", nodes::llm_send_tool_result);
        g.edge(&g.start(), &call_llm);
        g.edge_when(&call_llm, &execute_tool, conditions::on_tool_call);
        g.edge_when(&call_llm, &g.finish(), conditions::on_assistant_message);
        g.edge(&execute_tool, &send_result);
        g.edge_when(&send_result, &g.finish(), conditions::on_assistant_message);
    })
    .unwrap();

    block_on(graph.run(&mut ctx, "What is 2+3?".to_string())).unwrap();

    let names: Vec<String> = recorder
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|event| {
            serde_json::to_value(event).unwrap()["event"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        names,
        [
            "llm_call_starting",
            "llm_call_completed",
            "tool_call_starting",
            "tool_call_completed",
            "llm_call_starting",
            "llm_call_completed",
        ]
    );
    assert_eq!(
        serde_json::to_value(&recorder.0.lock().unwrap()[3]).unwrap(),
        json!({ "event": "tool_call_completed", "result": { "id": "add", "tool": "add", "content": "5.0" } })
    );
}

#[test]
fn reports_node_events_and_saves_checkpoints_through_one_observer() {
    use grauwolf_agents::checkpoints::{CheckpointStore, MemoryCheckpointStore};
    use grauwolf_agents::observer::AgentObserver;

    let mock = MockLlmClient::new("?").answer_when_contains("Hello", "Hi");
    let recorder = Arc::new(Recorder::default());
    let store = Arc::new(MemoryCheckpointStore::default());
    let mut ctx = AgentContext::new(
        Arc::new(
            PromptExecutor::builder()
                .client("mock", Arc::new(mock))
                .build(),
        ),
        LlModel::new("mock", "m"),
        Prompt::builder("echo").build(),
    )
    .install(recorder.clone());
    let graph = TypedGraph::<AgentContext, String, String>::build(|g| {
        let call_llm = g.node("call_llm", nodes::llm_request);
        g.edge(&g.start(), &call_llm);
        g.edge_when(&call_llm, &g.finish(), conditions::on_assistant_message);
    })
    .unwrap();
    let observer = AgentObserver::new().with_checkpoints(store.clone(), "run-1");

    block_on(graph.run_observed(&mut ctx, "Hello".to_string(), &observer)).unwrap();

    let events = recorder.0.lock().unwrap();
    assert_eq!(
        events.first(),
        Some(&AgentEvent::NodeStarting {
            path: vec!["call_llm".into()],
            input: json!("Hello"),
        })
    );
    assert!(
        matches!(events.last(), Some(AgentEvent::NodeCompleted { path, .. }) if path == &["call_llm"]),
        "{events:?}"
    );
    assert!(block_on(store.load("run-1")).unwrap().is_some());
}
