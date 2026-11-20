//! Resuming interpreted agents after an interruption (ADR-0020, ADR-0034).

use grauwolf_spec::AgentSpecExt;
use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::checkpoints::{CheckpointStore, MemoryCheckpointStore};
use grauwolf_interpreter::{Interpreter, InterpreterError};
use grauwolf_llm::{LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use serde_json::json;

/// Subgraph `draft` writes an answer; the main graph summarizes it.
fn spec() -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "two_steps",
            "model": { "provider": "mock", "id": "m" },
            "subgraphs": { "draft": { "graph": {
                "nodes": [ { "id": "write", "kind": "llm_request" } ],
                "edges": [ { "from": "start", "to": "write" },
                           { "from": "write", "to": "finish", "when": { "kind": "on_assistant_message" } } ] } } },
            "graph": {
                "nodes": [ { "id": "draft", "kind": "subgraph", "params": { "subgraph": "draft" } },
                           { "id": "summarize", "kind": "llm_request" } ],
                "edges": [ { "from": "start", "to": "draft" },
                           { "from": "draft", "to": "summarize" },
                           { "from": "summarize", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
            }
        })
        .to_string(),
    )
    .unwrap()
}

/// LLM that answers once and then fails; counts its calls.
#[derive(Default)]
struct OnceThenDown(Mutex<usize>);

impl LlmClient for OnceThenDown {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        let mut calls = self.0.lock().unwrap();
        *calls += 1;
        let answer = match *calls {
            1 => Ok(Message::assistant("Draft")),
            _ => Err(LlmError::Client("Connection lost".into())),
        };
        Box::pin(async move { answer })
    }
}

fn interpreter(client: Arc<dyn LlmClient>) -> Interpreter {
    Interpreter::new(Arc::new(
        PromptExecutor::builder().client("mock", client).build(),
    ))
}

#[test]
fn resumes_a_crashed_run_from_its_last_checkpoint() {
    let store = Arc::new(MemoryCheckpointStore::default());
    let first = interpreter(Arc::new(OnceThenDown::default()))
        .load(&spec())
        .unwrap();

    let crashed = block_on(first.run_checkpointed(json!("Topic"), store.clone(), "run-1"));
    assert!(crashed.is_err());
    let saved = block_on(store.load("run-1")).unwrap().unwrap();
    assert_eq!(saved.checkpoint.path, ["summarize"]);

    let working = MockLlmClient::new("?").answer_when_contains("Draft", "Summary");
    let second = interpreter(Arc::new(working)).load(&spec()).unwrap();
    let output = block_on(second.resume(store.clone(), "run-1")).unwrap();

    assert_eq!(output, json!("Summary"));
}

#[test]
fn reports_runs_without_a_checkpoint() {
    let agent = interpreter(Arc::new(OnceThenDown::default()))
        .load(&spec())
        .unwrap();

    let error = block_on(agent.resume(Arc::new(MemoryCheckpointStore::default()), "unbekannt"))
        .unwrap_err();

    assert!(
        matches!(error, InterpreterError::NoCheckpoint { .. }),
        "{error}"
    );
}
