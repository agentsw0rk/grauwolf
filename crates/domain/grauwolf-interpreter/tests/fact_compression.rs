//! History compression with facts in specs (F05.15, ADR-0076).

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
fn compresses_a_long_conversation_into_facts_before_answering() {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "chatter",
            "model": { "provider": "mock", "id": "m" },
            "graph": {
                "nodes": [
                    { "id": "compress", "kind": "compress_history",
                      "params": { "value": "string", "strategy": "facts", "keep": 0 } },
                    { "id": "call_llm", "kind": "llm_request" }
                ],
                "edges": [
                    { "from": "start", "to": "compress", "when": { "kind": "on_history_longer", "params": { "messages": 1 } } },
                    { "from": "start", "to": "call_llm" },
                    { "from": "compress", "to": "call_llm" },
                    { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } }
                ]
            }
        })
        .to_string(),
    )
    .unwrap();
    let mock = MockLlmClient::new("?")
        .answer_when_contains(
            "User: My name is Anna.",
            r#"{ "facts": ["The user is called Anna."] }"#,
        )
        .answer_when_contains(
            "The user is called Anna.\nWhat is my name?",
            "You are called Anna.",
        );
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(mock))
            .build(),
    ))
    .load(&spec)
    .unwrap();
    let store = Arc::new(MemoryChatHistoryStore::default());
    block_on(store.save(
        "chat",
        &[
            Message::user("My name is Anna."),
            Message::assistant("Hello Anna!"),
        ],
    ))
    .unwrap();

    let output =
        block_on(agent.run_conversation(json!("What is my name?"), store.clone(), "chat")).unwrap();

    assert_eq!(output, json!("You are called Anna."));
    let saved = block_on(store.load("chat")).unwrap();
    assert_eq!(
        saved[0],
        Message::user("Facts from the conversation so far:\n- The user is called Anna.")
    );
}

/// Memory that records the stored facts.
#[derive(Default)]
struct Remembering(std::sync::Mutex<Vec<String>>);

impl grauwolf_agents::memory::FactMemory for Remembering {
    fn recall<'a>(
        &'a self,
        _query: &'a str,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<Vec<String>, grauwolf_agents::memory::MemoryError>,
    > {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn remember<'a>(
        &'a self,
        facts: &'a [String],
    ) -> futures_util::future::BoxFuture<'a, Result<(), grauwolf_agents::memory::MemoryError>> {
        self.0.lock().unwrap().extend(facts.iter().cloned());
        Box::pin(async { Ok(()) })
    }
}

/// ADR-0079: With `remember_compressed`, the `chat` strategy stores lasting facts from the
/// compression in the run's memory.
#[test]
fn remembers_durable_facts_of_a_chat_compression() {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "plauderer",
            "model": { "provider": "mock", "id": "m" },
            "long_term_memory": {
                "store": "m.json",
                "embedding_model": { "provider": "mock", "id": "e" },
                "tools": false,
                "remember_compressed": true
            },
            "strategy": { "kind": "chat", "params": { "max_messages": 1, "keep": 0 } }
        })
        .to_string(),
    )
    .unwrap();
    let mock = MockLlmClient::new("Sure.").answer_when_contains(
        "User: My name is Anna.",
        r#"{ "facts": ["The user is called Anna."], "durable": ["The user is called Anna."] }"#,
    );
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(mock))
            .build(),
    ))
    .load(&spec)
    .unwrap();
    let store = Arc::new(MemoryChatHistoryStore::default());
    block_on(store.save(
        "chat",
        &[
            Message::user("My name is Anna."),
            Message::assistant("Hello Anna!"),
        ],
    ))
    .unwrap();
    let memory = Arc::new(Remembering::default());

    block_on(agent.run_with(
        json!("How are you?"),
        grauwolf_interpreter::RunExtras {
            conversation: Some((store, "chat".into())),
            memory: Some(memory.clone()),
            ..Default::default()
        },
    ))
    .unwrap();

    assert_eq!(*memory.0.lock().unwrap(), ["The user is called Anna."]);
}
