//! Compression by tokens instead of by messages (F06.13, ADR-0087).

use grauwolf_spec::AgentSpecExt;
use std::sync::Arc;

use futures_executor::block_on;
use grauwolf_agents::memory::{ChatHistoryStore, MemoryChatHistoryStore};
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::Message;
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use serde_json::json;

#[test]
fn compresses_only_when_the_history_has_more_tokens_than_allowed() {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "chatter",
            "model": { "provider": "mock", "id": "m" },
            "graph": {
                "nodes": [
                    { "id": "compress", "kind": "compress_history",
                      "params": { "value": "string", "strategy": "keep_last", "keep": 0 } },
                    { "id": "call_llm", "kind": "llm_request" }
                ],
                "edges": [
                    { "from": "start", "to": "compress", "when": { "kind": "on_history_tokens_above", "params": { "tokens": 100 } } },
                    { "from": "start", "to": "call_llm" },
                    { "from": "compress", "to": "call_llm" },
                    { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } }
                ]
            }
        })
        .to_string(),
    )
    .unwrap();
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(MockLlmClient::new("Sure.")))
            .build(),
    ))
    .load(&spec)
    .unwrap();
    let store = Arc::new(MemoryChatHistoryStore::default());
    // Same number of messages, but once just under 10 tokens and once over 250 tokens.
    let short = [Message::user("Short note."), Message::assistant("Ok.")];
    let long = [
        Message::user("Long note. ".repeat(80)),
        Message::assistant("Ok."),
    ];
    block_on(store.save("kurz", &short)).unwrap();
    block_on(store.save("lang", &long)).unwrap();

    block_on(agent.run_conversation(json!("Continue?"), store.clone(), "kurz")).unwrap();
    block_on(agent.run_conversation(json!("Continue?"), store.clone(), "lang")).unwrap();

    let kept = block_on(store.load("kurz")).unwrap();
    let compressed = block_on(store.load("lang")).unwrap();
    assert_eq!(kept.len(), 4, "{kept:?}");
    assert_eq!(
        compressed,
        vec![Message::user("Continue?"), Message::assistant("Sure.")]
    );
}
