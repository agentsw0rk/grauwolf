//! Conversations across several runs (F07.1, ADR-0037).

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;
use std::sync::Arc;

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::memory::{ChatHistoryStore, MemoryChatHistoryStore};
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::{LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};
use grauwolf_spec::AgentSpec;
use serde_json::json;

/// Answers with the number of messages it sees.
struct Counter;

impl LlmClient for Counter {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        let answer = format!("{} messages", prompt.messages.len());
        Box::pin(async move { Ok(Message::assistant(answer)) })
    }
}

fn echo() -> AgentSpec {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/specs/echo.json");
    AgentSpec::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn continues_conversations_with_their_stored_history() {
    let store = Arc::new(MemoryChatHistoryStore::default());
    let executor = PromptExecutor::builder()
        .client("ollama", Arc::new(Counter))
        .build();
    let agent = Interpreter::new(Arc::new(executor)).load(&echo()).unwrap();

    let first = block_on(agent.run_conversation(json!("Hello"), store.clone(), "anna")).unwrap();
    let second =
        block_on(agent.run_conversation(json!("Still there?"), store.clone(), "anna")).unwrap();
    let other = block_on(agent.run_conversation(json!("Hello"), store.clone(), "ben")).unwrap();

    // System prompt + question; the second time, also the first exchange.
    assert_eq!(first, json!("2 messages"));
    assert_eq!(second, json!("4 messages"));
    assert_eq!(other, json!("2 messages"));
    assert_eq!(
        block_on(store.load("anna")).unwrap(),
        [
            Message::user("Hello"),
            Message::assistant("2 messages"),
            Message::user("Still there?"),
            Message::assistant("4 messages"),
        ]
    );
}
