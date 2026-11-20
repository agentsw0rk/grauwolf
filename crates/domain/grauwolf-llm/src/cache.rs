//! Answer cache for LLM requests (F02.13, ADR-0086).

use std::collections::HashMap;
use std::sync::Mutex;

use futures_util::future::BoxFuture;
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor, Usage};
use serde_json::{Value, json};

/// Port for an answer cache. The implementation handles errors itself: a cache must never
/// make a request fail.
pub trait PromptCache: Send + Sync {
    /// The answer stored under `key`, if any.
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Option<Message>>;

    /// Stores `message` under `key`.
    fn put<'a>(&'a self, key: &'a str, message: &'a Message) -> BoxFuture<'a, ()>;
}

/// In-memory [`PromptCache`], for the lifetime of a process.
#[derive(Default)]
pub struct MemoryPromptCache(Mutex<HashMap<String, Message>>);

impl PromptCache for MemoryPromptCache {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Option<Message>> {
        let found = self.0.lock().expect("cache lock").get(key).cloned();
        Box::pin(async move { found })
    }

    fn put<'a>(&'a self, key: &'a str, message: &'a Message) -> BoxFuture<'a, ()> {
        self.0
            .lock()
            .expect("cache lock")
            .insert(key.to_owned(), message.clone());
        Box::pin(async {})
    }
}

/// Key of a request: kind, provider, model, temperature and think (if set), whole prompt, tools and schema as canonical
/// JSON – any change to them yields a new key.
pub(crate) fn key(
    kind: &str,
    model: &LlModel,
    prompt: &Prompt,
    tools: &[ToolDescriptor],
    schema: Option<&Value>,
) -> String {
    let mut key = json!({
        "kind": kind,
        "provider": model.provider,
        "model": model.id,
        "prompt": prompt,
        "tools": tools,
        "schema": schema,
    });
    // Only when set, so keys of models without sampling settings stay as they were (ADR-0097).
    if let Some(temperature) = model.temperature {
        key["temperature"] = json!(temperature);
    }
    if let Some(think) = model.think {
        key["think"] = json!(think);
    }
    key.to_string()
}

/// Usage of an answer from the cache: nothing was consumed.
pub(crate) fn no_usage() -> Usage {
    Usage {
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: None,
        cache_write_tokens: None,
    }
}
