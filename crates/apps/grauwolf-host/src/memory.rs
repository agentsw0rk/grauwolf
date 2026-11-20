//! File adapter for the port `ChatHistoryStore` (F07.2, ADR-0037).

use std::path::PathBuf;

use futures_util::future::BoxFuture;
use grauwolf_agents::memory::ChatHistoryStore;
use grauwolf_graph::NodeError;
use grauwolf_prompt::Message;

use crate::checkpoints::safe_file_name;

/// Stores the history of each conversation as `<directory>/<conversation-id>.json`, conversations
/// of a user under `<directory>/users/<hex>/` (ADR-0060).
#[derive(Debug, Clone)]
pub struct FileChatHistoryStore {
    dir: PathBuf,
}

impl FileChatHistoryStore {
    /// Store in the directory `dir`; it is created on the first save.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// File for the key `key`: `<dir>/<id>.json`, for conversations of a user
    /// (`users/<hex>/<id>`, see `conversation_key`) `<dir>/users/<hex>/<id>.json` (ADR-0060).
    fn path(&self, key: &str) -> Result<PathBuf, NodeError> {
        let Some(scoped) = key.strip_prefix("users/") else {
            return Ok(self.dir.join(safe_file_name(key, "conversation ID")?));
        };
        let user_dir = scoped.split_once('/').and_then(|(hex, id)| {
            let hex_valid = !hex.is_empty()
                && hex.len() % 2 == 0
                && hex
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
            hex_valid.then_some((hex, id))
        });
        let (hex, id) = user_dir.ok_or_else(|| format!("invalid conversation key '{key}'"))?;
        Ok(self
            .dir
            .join("users")
            .join(hex)
            .join(safe_file_name(id, "conversation ID")?))
    }
}

impl ChatHistoryStore for FileChatHistoryStore {
    fn load<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<Message>, NodeError>> {
        Box::pin(async move {
            let file = self.path(conversation_id)?;
            match tokio::fs::read(&file).await {
                Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
                Err(error) => Err(error.into()),
            }
        })
    }

    fn save<'a>(
        &'a self,
        conversation_id: &'a str,
        messages: &'a [Message],
    ) -> BoxFuture<'a, Result<(), NodeError>> {
        Box::pin(async move {
            let file = self.path(conversation_id)?;
            if let Some(dir) = file.parent() {
                tokio::fs::create_dir_all(dir).await?;
            }
            let partial = file.with_extension("json.partial");
            tokio::fs::write(&partial, serde_json::to_vec_pretty(messages)?).await?;
            tokio::fs::rename(&partial, &file).await?;
            Ok(())
        })
    }
}

/// Conversation store in the database at `url` (`sqlite://file.db`, `postgres://…`;
/// ADR-0048).
#[cfg(feature = "sql")]
pub async fn sql_chat_history_store(
    url: &str,
) -> Result<std::sync::Arc<dyn ChatHistoryStore>, String> {
    grauwolf_store_sql::SqlChatHistoryStore::connect(url)
        .await
        .map(|store| std::sync::Arc::new(store) as std::sync::Arc<dyn ChatHistoryStore>)
        .map_err(|error| format!("{url}: {error}"))
}
