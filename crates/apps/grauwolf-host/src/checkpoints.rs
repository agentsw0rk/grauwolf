//! File adapter for the port `CheckpointStore` (ADR-0034).

use std::path::PathBuf;

use futures_util::future::BoxFuture;
use grauwolf_agents::checkpoints::{CheckpointStore, ExecutionState};
use grauwolf_graph::NodeError;

/// Stores the latest execution state of each run as `<directory>/<run-id>.json`.
#[derive(Debug, Clone)]
pub struct FileCheckpointStore {
    dir: PathBuf,
}

impl FileCheckpointStore {
    /// Store in the directory `dir`; it is created on the first save.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// File of the run.
    fn file(&self, run_id: &str) -> Result<PathBuf, NodeError> {
        Ok(self.dir.join(safe_file_name(run_id, "run ID")?))
    }
}

impl CheckpointStore for FileCheckpointStore {
    fn save<'a>(
        &'a self,
        run_id: &'a str,
        state: &'a ExecutionState,
    ) -> BoxFuture<'a, Result<(), NodeError>> {
        Box::pin(async move {
            let file = self.file(run_id)?;
            tokio::fs::create_dir_all(&self.dir).await?;
            // Via a temporary file and `rename`: an abort never leaves half-written states.
            let partial = file.with_extension("json.partial");
            tokio::fs::write(&partial, serde_json::to_vec_pretty(state)?).await?;
            tokio::fs::rename(&partial, &file).await?;
            Ok(())
        })
    }

    fn load<'a>(
        &'a self,
        run_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<ExecutionState>, NodeError>> {
        Box::pin(async move {
            let file = self.file(run_id)?;
            match tokio::fs::read(&file).await {
                Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error.into()),
            }
        })
    }
}

/// `<id>.json`, provided `id` consists only of letters, digits, `-` and `_` – so an ID cannot
/// leave the directory. `what` names the ID in the error message.
pub(crate) fn safe_file_name(id: &str, what: &str) -> Result<String, NodeError> {
    let valid = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid {
        Ok(format!("{id}.json"))
    } else {
        Err(format!("invalid {what} '{id}': allowed are letters, digits, '-' and '_'").into())
    }
}

/// Checkpoint store in the database at `url` (`sqlite://file.db`, `postgres://…`;
/// F07.15, ADR-0048).
#[cfg(feature = "sql")]
pub async fn sql_checkpoint_store(
    url: &str,
) -> Result<std::sync::Arc<dyn CheckpointStore>, String> {
    grauwolf_store_sql::SqlCheckpointStore::connect(url)
        .await
        .map(|store| std::sync::Arc::new(store) as std::sync::Arc<dyn CheckpointStore>)
        .map_err(|error| format!("{url}: {error}"))
}
