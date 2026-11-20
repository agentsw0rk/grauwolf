//! Code in graphs: `code` nodes, `on_code` conditions and edge transformations (ADR-0102).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use grauwolf_agents::checkpoints::MemoryCheckpointStore;
use grauwolf_agents::events::{AgentEvent, Feature};
use grauwolf_interpreter::{Interpreter, InterpreterError};
use grauwolf_llm::PromptExecutor;
use grauwolf_sandbox::SandboxRuntime;
use grauwolf_spec::{AgentSpec, AgentSpecExt};
use grauwolf_test::MockLlmClient;
use grauwolf_toolbuild::ToolBuilder;
use grauwolf_tools::ToolArtifact;
use serde_json::{Value, json};

#[derive(Default)]
struct Recorder(Mutex<Vec<AgentEvent>>);

impl Feature for Recorder {
    fn on_event(&self, event: &AgentEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

/// An LLM judges a review; code scores the answer, `on_code` routes it and a transformation
/// reshapes it into the reply.
fn review_router() -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "review_router",
            "model": { "provider": "ollama", "id": "m" },
            "types": {
                "Verdict": { "type": "object", "properties": {
                    "text": { "type": "string" }, "positive": { "type": "boolean" }, "words": { "type": "integer" }
                }, "required": ["text", "positive", "words"] },
                "Reply": { "type": "object", "properties": {
                    "subject": { "type": "string" }, "body": { "type": "string" }
                }, "required": ["subject", "body"] }
            },
            "input": "string", "output": "Reply",
            "graph": {
                "nodes": [
                    { "id": "judge", "kind": "llm_request" },
                    { "id": "score", "kind": "code", "params": {
                        "input": "string", "output": "Verdict",
                        "code": "if input.contains(\"FAIL\") { return Err(\"cannot score\".to_owned()); }\nlet positive = input.to_lowercase().contains(\"great\");\nOk(Verdict { text: input.trim().to_owned(), positive, words: input.split_whitespace().count() as i64 })"
                    } }
                ],
                "edges": [
                    { "from": "start", "to": "judge" },
                    { "from": "judge", "to": "score", "when": { "kind": "on_assistant_message" } },
                    { "from": "score", "to": "finish",
                      "when": { "kind": "on_code", "params": { "input": "Verdict", "code": "value.positive" } },
                      "transform": { "output": "Reply", "code": "Reply { subject: \"Thanks\".to_owned(), body: value.text }" } },
                    { "from": "score", "to": "finish",
                      "transform": { "output": "Reply", "code": "Reply { subject: format!(\"Looking into it ({} words)\", value.words), body: value.text }" } }
                ]
            }
        })
        .to_string(),
    )
    .unwrap()
}

fn artifacts(spec: &AgentSpec) -> Vec<ToolArtifact> {
    let builder = ToolBuilder::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/toolbuild-tests"),
    );
    builder.prepare().unwrap();
    spec.code_pieces()
        .iter()
        .map(|piece| builder.build_code(piece).unwrap())
        .collect()
}

fn interpreter(mock: MockLlmClient) -> Interpreter {
    Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("ollama", Arc::new(mock))
            .build(),
    ))
    .with_tool_runtime(Arc::new(SandboxRuntime::new().unwrap()))
}

#[test]
fn code_in_a_spec_needs_its_built_modules() {
    let spec = review_router();
    let plain = Interpreter::new(Arc::new(PromptExecutor::builder().build()));

    let error = plain.load(&spec).err().unwrap();

    assert!(
        matches!(&error, InterpreterError::MissingArtifact { tool } if tool == "code_main_score"),
        "{error}"
    );
}

#[test]
#[ignore = "slow: builds code for WASM with cargo"]
fn code_routes_and_reshapes_the_answer_of_the_llm() {
    let spec = review_router();
    let artifacts = artifacts(&spec);
    let ask = |answer: &str| {
        let agent = interpreter(MockLlmClient::new(answer.to_owned()))
            .load_with_artifacts(&spec, &artifacts)
            .unwrap();
        block_on(agent.run_json(json!("The review")))
    };

    assert_eq!(
        ask("This is great").unwrap(),
        json!({ "subject": "Thanks", "body": "This is great" })
    );
    assert_eq!(
        ask("Not for me").unwrap(),
        json!({ "subject": "Looking into it (3 words)", "body": "Not for me" })
    );
}

#[test]
#[ignore = "slow: builds code for WASM with cargo"]
fn the_events_show_the_path_and_the_code_node_failing_fails_the_run() {
    let spec = review_router();
    let artifacts = artifacts(&spec);
    let recorder = Arc::new(Recorder::default());
    let agent = interpreter(MockLlmClient::new("great"))
        .with_feature(recorder.clone())
        .load_with_artifacts(&spec, &artifacts)
        .unwrap();

    block_on(agent.run_json(json!("x"))).unwrap();

    let completed: Vec<(Vec<String>, Value)> = recorder
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            AgentEvent::NodeCompleted { path, output } => Some((path.clone(), output.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(completed[1].0, ["score"]);
    assert_eq!(
        completed[1].1,
        json!({ "text": "great", "positive": true, "words": 1 })
    );

    let failing = interpreter(MockLlmClient::new("FAIL"))
        .load_with_artifacts(&spec, &artifacts)
        .unwrap();
    let error = block_on(failing.run_json(json!("x"))).unwrap_err();
    assert!(error.to_string().contains("cannot score"), "{error}");
}

#[test]
#[ignore = "slow: builds code for WASM with cargo"]
fn a_checkpointed_run_gives_the_same_answer() {
    let spec = review_router();
    let artifacts = artifacts(&spec);
    let agent = interpreter(MockLlmClient::new("great"))
        .load_with_artifacts(&spec, &artifacts)
        .unwrap();

    let answer = block_on(agent.run_checkpointed(
        json!("x"),
        Arc::new(MemoryCheckpointStore::default()),
        "run-1",
    ))
    .unwrap();

    assert_eq!(answer, json!({ "subject": "Thanks", "body": "great" }));
}
