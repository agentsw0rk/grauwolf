//! Chat memory: histories per conversation ID across several runs (F07.1, F07.2, ADR-0037);
//! automatic recall and storing in the long-term memory for the interpreter and generated code
//! (ADR-0069, ADR-0084).

use std::collections::HashMap;
use std::sync::Mutex;

use futures_util::future::BoxFuture;
use grauwolf_graph::NodeError;
use grauwolf_prompt::Message;
use serde_json::Value;

use crate::AgentContext;
use crate::events::AgentEvent;

/// Maximum length of a user ID in bytes.
const MAX_USER_LEN: usize = 128;

/// Checks that `user` can name a user's area in the stores (conversations, memory, key-value
/// store): 1 to 128 bytes (ADR-0060). Services refuse other IDs at the entry.
pub fn check_user_id(user: &str) -> Result<(), String> {
    if user.is_empty() || user.len() > MAX_USER_LEN {
        return Err(format!(
            "invalid user ID: 1 to {MAX_USER_LEN} bytes allowed"
        ));
    }
    Ok(())
}

/// Storage key of conversation `id` (ADR-0060): without a user `id`, with a user
/// `users/<hex(user)>/<id>`. `id` may only consist of letters, digits, `-` and `_`; this way
/// no key without a user equals one with a user. User IDs are arbitrary text
/// of 1 to 128 bytes.
pub fn conversation_key(user: Option<&str>, id: &str) -> Result<String, String> {
    let valid = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        return Err(format!(
            "invalid conversation ID '{id}': allowed are letters, digits, '-' and '_'"
        ));
    }
    let Some(user) = user else {
        return Ok(id.to_owned());
    };
    check_user_id(user)?;
    let hex: String = user.bytes().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("users/{hex}/{id}"))
}

/// Error of a long-term memory (ADR-0069).
pub type MemoryError = Box<dyn std::error::Error + Send + Sync>;

/// Long-term memory of a run for automatic recall and storing (ADR-0069); belongs to
/// exactly one scope (user or shared).
pub trait FactMemory: Send + Sync {
    /// The most relevant facts for `query`.
    fn recall<'a>(&'a self, query: &'a str) -> BoxFuture<'a, Result<Vec<String>, MemoryError>>;

    /// Stores `facts`; already known ones are kept only once.
    fn remember<'a>(&'a self, facts: &'a [String]) -> BoxFuture<'a, Result<(), MemoryError>>;
}

/// Appends the most relevant facts for the input `input` as a system message (ADR-0069);
/// errors do not end the run.
pub async fn recall_facts(ctx: &mut AgentContext, memory: &dyn FactMemory, input: &str) {
    match memory.recall(input).await {
        Ok(facts) if facts.is_empty() => {}
        Ok(facts) => {
            let list: Vec<String> = facts.iter().map(|fact| format!("- {fact}")).collect();
            ctx.append(Message::system(format!(
                "Known facts from long-term memory:\n{}",
                list.join("\n")
            )));
            ctx.emit(AgentEvent::MemoryRecalled { facts });
        }
        Err(error) => ctx.emit(AgentEvent::MemoryFailed {
            error: error.to_string(),
        }),
    }
}

/// Stores lasting facts from input and answer (ADR-0069); errors do not end the run.
pub async fn remember_facts(
    ctx: &AgentContext,
    memory: &dyn FactMemory,
    input: &str,
    output: &str,
) {
    // Pass along what is known so that only new facts are stored – not even reworded ones.
    let known = match memory.recall(input).await {
        Ok(known) => known,
        Err(error) => {
            ctx.emit(AgentEvent::MemoryFailed {
                error: error.to_string(),
            });
            Vec::new()
        }
    };
    let facts = match ctx.extract_facts(input, output, &known).await {
        Ok(facts) => facts,
        Err(error) => {
            return ctx.emit(AgentEvent::MemoryFailed {
                error: error.to_string(),
            });
        }
    };
    if facts.is_empty() {
        return;
    }
    match memory.remember(&facts).await {
        Ok(()) => ctx.emit(AgentEvent::MemoryRemembered { facts }),
        Err(error) => ctx.emit(AgentEvent::MemoryFailed {
            error: error.to_string(),
        }),
    }
}

/// Text of a value: strings unchanged, otherwise JSON.
pub fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Port for loading and saving chat histories (without system prompt).
pub trait ChatHistoryStore: Send + Sync {
    /// History of conversation `conversation_id`; empty if there is none yet.
    fn load<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<Message>, NodeError>>;

    /// Replaces the history of conversation `conversation_id`.
    fn save<'a>(
        &'a self,
        conversation_id: &'a str,
        messages: &'a [Message],
    ) -> BoxFuture<'a, Result<(), NodeError>>;
}

/// In-memory [`ChatHistoryStore`], e.g. for tests or a process with many users.
#[derive(Debug, Default)]
pub struct MemoryChatHistoryStore(Mutex<HashMap<String, Vec<Message>>>);

impl ChatHistoryStore for MemoryChatHistoryStore {
    fn load<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<Message>, NodeError>> {
        let messages = self
            .0
            .lock()
            .expect("chat store")
            .get(conversation_id)
            .cloned()
            .unwrap_or_default();
        Box::pin(async move { Ok(messages) })
    }

    fn save<'a>(
        &'a self,
        conversation_id: &'a str,
        messages: &'a [Message],
    ) -> BoxFuture<'a, Result<(), NodeError>> {
        self.0
            .lock()
            .expect("chat store")
            .insert(conversation_id.to_owned(), messages.to_vec());
        Box::pin(async { Ok(()) })
    }
}
