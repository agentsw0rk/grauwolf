//! Response cache for LLM requests as files (ADR-0086).

use std::path::{Path, PathBuf};

use futures_util::future::BoxFuture;
use grauwolf_llm::PromptCache;
use grauwolf_prompt::Message;
use serde::{Deserialize, Serialize};

/// [`PromptCache`] with one file per request under `<dir>/<fnv1a>.json`; the file contains the
/// whole key so that a checksum collision never yields a wrong answer.
#[derive(Debug, Clone)]
pub struct FilePromptCache {
    dir: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    key: String,
    message: Message,
}

impl FilePromptCache {
    /// Cache in the directory `dir`; it is created on the first store.
    pub fn new(dir: impl AsRef<Path>) -> Self {
        Self {
            dir: dir.as_ref().to_owned(),
        }
    }

    fn file(&self, key: &str) -> PathBuf {
        self.dir.join(format!(
            "{:016x}.json",
            crate::knowledge::fnv1a(key.as_bytes())
        ))
    }
}

impl PromptCache for FilePromptCache {
    /// Missing, unreadable or foreign entries count as a miss.
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Option<Message>> {
        Box::pin(async move {
            let text = tokio::fs::read_to_string(self.file(key)).await.ok()?;
            let entry: Entry = serde_json::from_str(&text).ok()?;
            (entry.key == key).then_some(entry.message)
        })
    }

    /// Writes via a temporary file; errors are lost – the cache never slows down a run.
    fn put<'a>(&'a self, key: &'a str, message: &'a Message) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let entry = Entry {
                key: key.to_owned(),
                message: message.clone(),
            };
            let Ok(text) = serde_json::to_string(&entry) else {
                return;
            };
            let file = self.file(key);
            let partial = file.with_extension("json.partial");
            let written = async {
                tokio::fs::create_dir_all(&self.dir).await?;
                tokio::fs::write(&partial, text).await?;
                tokio::fs::rename(&partial, &file).await
            };
            let _ = written.await;
        })
    }
}
