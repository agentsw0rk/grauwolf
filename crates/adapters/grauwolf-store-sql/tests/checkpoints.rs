//! Checkpoints in SQL (F07.15, ADR-0048), tested against SQLite.

use std::path::PathBuf;

use grauwolf_agents::checkpoints::{
    CheckpointStore, ContextSnapshot, EXECUTION_STATE_VERSION, ExecutionState,
};
use grauwolf_graph::Checkpoint;
use grauwolf_prompt::{LlModel, Prompt};
use grauwolf_store_sql::SqlCheckpointStore;
use serde_json::json;

fn database(name: &str) -> String {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/store-sql-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(format!("{name}.db"));
    let _ = std::fs::remove_file(&file);
    format!("sqlite://{}", file.display())
}

fn state(node: &str) -> ExecutionState {
    ExecutionState {
        version: EXECUTION_STATE_VERSION,
        checkpoint: Checkpoint {
            path: vec!["solve".into(), node.into()],
            input: json!("Topic"),
        },
        context: ContextSnapshot {
            prompt: Prompt::builder("p").user("Topic").build(),
            model: LlModel::new("ollama", "m"),
            storage: Default::default(),
            self_tools: Vec::new(),
            usage: None,
            cost: None,
            tool_calls: 0,
        },
    }
}

#[tokio::test]
async fn keeps_the_latest_state_of_each_run_across_connections() {
    let url = database("checkpoints");
    let store = SqlCheckpointStore::connect(&url).await.unwrap();

    store.save("run-1", &state("draft")).await.unwrap();
    store.save("run-1", &state("summarize")).await.unwrap();
    store.save("run-2", &state("draft")).await.unwrap();
    drop(store);
    let reopened = SqlCheckpointStore::connect(&url).await.unwrap();

    assert_eq!(
        reopened.load("run-1").await.unwrap(),
        Some(state("summarize"))
    );
    assert_eq!(reopened.load("run-2").await.unwrap(), Some(state("draft")));
    assert_eq!(reopened.load("unknown").await.unwrap(), None);
}
