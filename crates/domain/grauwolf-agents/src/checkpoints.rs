//! Run state, checkpoints and resuming (ADR-0020, ADR-0034).

use std::collections::BTreeMap;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_graph::{Checkpoint, CheckpointObserver, NodeError};
use grauwolf_prompt::{LlModel, Prompt};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AgentContext;

/// Version of the [`ExecutionState`] format.
pub const EXECUTION_STATE_VERSION: u32 = 1;

/// Serializable part of the [`AgentContext`]: history and model. Tools, executor and
/// frame sink are runtime handles and are passed in again when resuming; the engine restores
/// tool subsets when it enters the subgraphs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextSnapshot {
    /// History up to the checkpoint.
    pub prompt: Prompt,
    /// Active model.
    pub model: LlModel,
    /// Agent storage (ADR-0044); missing in older checkpoints.
    #[serde(default)]
    pub storage: BTreeMap<String, Value>,
    /// Definitions of self-built tools (ADR-0077); missing in older checkpoints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub self_tools: Vec<grauwolf_tools::NewTool>,
    /// Token total of the run so far (ADR-0067); missing in older checkpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<grauwolf_prompt::Usage>,
    /// Cost of the run so far (ADR-0071); missing in older checkpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    /// Tool calls executed so far (ADR-0078); missing in older checkpoints.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tool_calls: u64,
}

fn is_zero(count: &u64) -> bool {
    *count == 0
}

/// Complete run state at a checkpoint (ADR-0020).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionState {
    /// Format version, see [`EXECUTION_STATE_VERSION`].
    pub version: u32,
    /// Position in the graph and input of the next node.
    pub checkpoint: Checkpoint,
    /// State of the context at this point.
    pub context: ContextSnapshot,
}

/// Port for storing run states; adapters, e.g. for files, live outside the domain.
pub trait CheckpointStore: Send + Sync {
    /// Saves `state` as the latest state of run `run_id`.
    fn save<'a>(
        &'a self,
        run_id: &'a str,
        state: &'a ExecutionState,
    ) -> BoxFuture<'a, Result<(), NodeError>>;

    /// Loads the latest state of run `run_id`, if there is one.
    fn load<'a>(
        &'a self,
        run_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<ExecutionState>, NodeError>>;
}

/// Saves the run state of run `run_id` to a [`CheckpointStore`] before each node.
pub struct Checkpointing {
    store: Arc<dyn CheckpointStore>,
    run_id: String,
    /// The most recently saved checkpoint; progress is saved under it (ADR-0052).
    current: Arc<std::sync::Mutex<Option<Checkpoint>>>,
}

/// Saves a context as the new state of the current checkpoint (ADR-0052).
pub type ProgressSaver =
    Arc<dyn Fn(ContextSnapshot) -> BoxFuture<'static, Result<(), NodeError>> + Send + Sync>;

impl Checkpointing {
    /// Checkpoints of run `run_id` go to `store`.
    pub fn new(store: Arc<dyn CheckpointStore>, run_id: impl Into<String>) -> Self {
        Self {
            store,
            run_id: run_id.into(),
            current: Arc::default(),
        }
    }

    /// Save hook for [`AgentContext::save_progress`]: writes the context under the most
    /// recently reported checkpoint; before the first checkpoint it does nothing.
    pub fn progress_saver(&self) -> ProgressSaver {
        let (store, run_id, current) = (
            Arc::clone(&self.store),
            self.run_id.clone(),
            Arc::clone(&self.current),
        );
        Arc::new(move |context| {
            let (store, run_id) = (Arc::clone(&store), run_id.clone());
            let checkpoint = current.lock().expect("checkpoint").clone();
            Box::pin(async move {
                let Some(checkpoint) = checkpoint else {
                    return Ok(());
                };
                let state = ExecutionState {
                    version: EXECUTION_STATE_VERSION,
                    checkpoint,
                    context,
                };
                store.save(&run_id, &state).await
            })
        })
    }
}

impl CheckpointObserver<AgentContext> for Checkpointing {
    fn checkpoint<'a>(
        &'a self,
        ctx: &'a AgentContext,
        checkpoint: &'a Checkpoint,
    ) -> BoxFuture<'a, Result<(), NodeError>> {
        *self.current.lock().expect("checkpoint") = Some(checkpoint.clone());
        Box::pin(async move {
            let state = ExecutionState {
                version: EXECUTION_STATE_VERSION,
                checkpoint: checkpoint.clone(),
                context: ctx.snapshot(),
            };
            self.store.save(&self.run_id, &state).await
        })
    }
}

/// In-memory [`CheckpointStore`], e.g. for tests or short-lived runs.
#[derive(Debug, Default)]
pub struct MemoryCheckpointStore(
    std::sync::Mutex<std::collections::HashMap<String, ExecutionState>>,
);

impl CheckpointStore for MemoryCheckpointStore {
    fn save<'a>(
        &'a self,
        run_id: &'a str,
        state: &'a ExecutionState,
    ) -> BoxFuture<'a, Result<(), NodeError>> {
        self.0
            .lock()
            .expect("checkpoint store")
            .insert(run_id.to_owned(), state.clone());
        Box::pin(async { Ok(()) })
    }

    fn load<'a>(
        &'a self,
        run_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<ExecutionState>, NodeError>> {
        let state = self
            .0
            .lock()
            .expect("checkpoint store")
            .get(run_id)
            .cloned();
        Box::pin(async move { Ok(state) })
    }
}
