//! Observer for agent runs: node events and optional checkpoints (ADR-0007, ADR-0034).

use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_graph::{Checkpoint, CheckpointObserver, NodeError};
use serde_json::Value;

use crate::AgentContext;
use crate::checkpoints::{CheckpointStore, Checkpointing, ProgressSaver};
use crate::events::AgentEvent;

/// Reports the start and end of each node as an [`AgentEvent`] to the context's features and
/// optionally also saves checkpoints.
#[derive(Default)]
pub struct AgentObserver {
    checkpointing: Option<Checkpointing>,
}

impl AgentObserver {
    /// Observer for node events only.
    pub fn new() -> Self {
        Self::default()
    }

    /// Save hook for [`AgentContext::save_progress`]; none without checkpoints (ADR-0052).
    pub fn progress_saver(&self) -> Option<ProgressSaver> {
        self.checkpointing
            .as_ref()
            .map(Checkpointing::progress_saver)
    }

    /// Additionally saves the run state of run `run_id` to `store` before each node.
    pub fn with_checkpoints(mut self, store: Arc<dyn CheckpointStore>, run_id: &str) -> Self {
        self.checkpointing = Some(Checkpointing::new(store, run_id));
        self
    }
}

impl CheckpointObserver<AgentContext> for AgentObserver {
    fn checkpoint<'a>(
        &'a self,
        ctx: &'a AgentContext,
        checkpoint: &'a Checkpoint,
    ) -> BoxFuture<'a, Result<(), NodeError>> {
        Box::pin(async move {
            ctx.emit(AgentEvent::NodeStarting {
                path: checkpoint.path.clone(),
                input: checkpoint.input.clone(),
            });
            match &self.checkpointing {
                Some(checkpointing) => checkpointing.checkpoint(ctx, checkpoint).await,
                None => Ok(()),
            }
        })
    }

    fn node_completed<'a>(
        &'a self,
        ctx: &'a AgentContext,
        path: &'a [String],
        output: &'a Value,
    ) -> BoxFuture<'a, Result<(), NodeError>> {
        ctx.emit(AgentEvent::NodeCompleted {
            path: path.to_vec(),
            output: output.clone(),
        });
        Box::pin(async { Ok(()) })
    }
}
