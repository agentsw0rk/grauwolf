//! File store for execution states (ADR-0034).

use grauwolf_agents::checkpoints::{
    CheckpointStore, ContextSnapshot, EXECUTION_STATE_VERSION, ExecutionState,
};
use grauwolf_graph::Checkpoint;
use grauwolf_host::checkpoints::FileCheckpointStore;
use grauwolf_prompt::{LlModel, Prompt};
use serde_json::json;

fn state(input: &str) -> ExecutionState {
    ExecutionState {
        version: EXECUTION_STATE_VERSION,
        checkpoint: Checkpoint {
            path: vec!["solve".into(), "call_llm".into()],
            input: json!(input),
        },
        context: ContextSnapshot {
            prompt: Prompt::builder("p").user("Hello").build(),
            model: LlModel::new("ollama", "m"),
            storage: Default::default(),
            self_tools: Vec::new(),
            usage: None,
            cost: None,
            tool_calls: 0,
        },
    }
}

fn store(name: &str) -> FileCheckpointStore {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target/checkpoint-tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    FileCheckpointStore::new(dir)
}

#[tokio::test]
async fn keeps_the_latest_state_of_each_run_in_a_file() {
    let store = store("latest");

    store.save("run-1", &state("old")).await.unwrap();
    store.save("run-1", &state("new")).await.unwrap();

    assert_eq!(store.load("run-1").await.unwrap(), Some(state("new")));
    assert_eq!(store.load("run-2").await.unwrap(), None);
}

#[tokio::test]
async fn rejects_run_ids_that_could_leave_the_directory() {
    let store = store("ids");

    let error = store.save("../escape", &state("x")).await.unwrap_err();

    assert!(error.to_string().contains("run ID"), "{error}");
}
