//! Saving progress within long nodes (ADR-0052).

use std::sync::Arc;

use futures_executor::block_on;
use grauwolf_agents::AgentContext;
use grauwolf_agents::checkpoints::{CheckpointStore, MemoryCheckpointStore};
use grauwolf_agents::observer::AgentObserver;
use grauwolf_graph::{Checkpoint, CheckpointObserver};
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::{LlModel, Prompt};
use serde_json::json;

fn context() -> AgentContext {
    AgentContext::new(
        Arc::new(PromptExecutor::builder().build()),
        LlModel::new("mock", "m"),
        Prompt::builder("p").build(),
    )
}

#[test]
fn saves_progress_as_a_new_state_of_the_running_checkpoint() {
    let store = Arc::new(MemoryCheckpointStore::default());
    let observer = AgentObserver::new().with_checkpoints(store.clone(), "run");
    let mut ctx = context();
    ctx.set_progress_saver(observer.progress_saver());
    let checkpoint = Checkpoint {
        path: vec!["solve".into()],
        input: json!("Task"),
    };

    block_on(observer.checkpoint(&ctx, &checkpoint)).unwrap();
    ctx.store("grauwolf.plan.solve", &2).unwrap();
    block_on(ctx.save_progress()).unwrap();
    // Forks of parallel branches save nothing.
    let mut fork = ctx.fork();
    fork.store("grauwolf.plan.solve", &99).unwrap();
    block_on(fork.save_progress()).unwrap();

    let state = block_on(store.load("run")).unwrap().unwrap();
    assert_eq!(state.checkpoint, checkpoint);
    assert_eq!(state.context.storage["grauwolf.plan.solve"], json!(2));
    // Without checkpoints, `save_progress` does nothing.
    block_on(context().save_progress()).unwrap();
}
