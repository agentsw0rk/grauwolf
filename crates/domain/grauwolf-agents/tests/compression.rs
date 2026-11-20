//! History compression (F05.14, ADR-0037).

use std::sync::Arc;

use futures_executor::block_on;
use grauwolf_agents::{AgentContext, HistoryCompression};
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::{LlModel, Message, Prompt};
use grauwolf_test::MockLlmClient;
use serde_json::json;

fn context(mock: MockLlmClient, messages: Vec<Message>) -> AgentContext {
    let mut prompt = Prompt::builder("p").system("Be brief.").build();
    prompt.messages.extend(messages);
    AgentContext::new(
        Arc::new(
            PromptExecutor::builder()
                .client("mock", Arc::new(mock))
                .build(),
        ),
        LlModel::new("mock", "m"),
        prompt,
    )
}

/// Question, tool call, tool result, answer, new question.
fn history() -> Vec<Message> {
    vec![
        Message::user("What is 2+3?"),
        Message::tool_call("c1", "add", json!({ "a": 2, "b": 3 })),
        Message::tool_result("c1", "add", "5"),
        Message::assistant("5"),
        Message::user("And 4+5?"),
    ]
}

#[test]
fn keeps_the_system_prompt_and_the_last_messages() {
    let mut ctx = context(MockLlmClient::new("?"), history());

    block_on(ctx.compress_history(HistoryCompression::KeepLast(2))).unwrap();

    assert_eq!(
        ctx.prompt().messages,
        [
            Message::system("Be brief."),
            Message::assistant("5"),
            Message::user("And 4+5?")
        ]
    );
}

#[test]
fn never_starts_with_a_tool_result_without_its_call() {
    let mut ctx = context(MockLlmClient::new("?"), history());

    block_on(ctx.compress_history(HistoryCompression::KeepLast(3))).unwrap();

    assert_eq!(
        ctx.prompt().messages[1..],
        history()[1..],
        "the tool call for the result must be kept"
    );
}

#[test]
fn replaces_the_history_with_a_summary_of_the_llm() {
    let mock = MockLlmClient::new("?").answer_when_contains(
        "Summarize the conversation so far",
        "User asked about 2+3 (=5) and now about 4+5.",
    );
    let mut ctx = context(mock, history());

    block_on(ctx.compress_history(HistoryCompression::Summarize)).unwrap();

    assert_eq!(
        ctx.prompt().messages,
        [
            Message::system("Be brief."),
            Message::user(
                "Summary of the conversation so far:\nUser asked about 2+3 (=5) and now about 4+5."
            ),
        ]
    );
}

#[test]
fn keeps_a_pending_tool_call_after_the_summary() {
    let mock = MockLlmClient::new("?")
        .answer_when_contains("Summarize the conversation so far", "Short summary");
    let mut messages = history();
    messages.push(Message::tool_call("c2", "add", json!({ "a": 4, "b": 5 })));
    let mut ctx = context(mock, messages);

    block_on(ctx.compress_history(HistoryCompression::Summarize)).unwrap();

    assert_eq!(
        ctx.prompt().messages[1..],
        [
            Message::user("Summary of the conversation so far:\nShort summary"),
            Message::tool_call("c2", "add", json!({ "a": 4, "b": 5 })),
        ]
    );
}

#[test]
fn replaces_older_messages_by_facts_and_keeps_the_last_ones() {
    let mock = MockLlmClient::new("?")
        .answer_when_contains("Result of add: 5", r#"{ "facts": ["2+3 is 5."] }"#);
    let mut ctx = context(mock, history());

    block_on(ctx.compress_history(HistoryCompression::Facts(2))).unwrap();

    assert_eq!(
        ctx.prompt().messages,
        [
            Message::system("Be brief."),
            Message::user("Facts from the conversation so far:\n- 2+3 is 5."),
            Message::assistant("5"),
            Message::user("And 4+5?")
        ]
    );
}

#[test]
fn merges_the_facts_of_an_earlier_compression_into_one_list() {
    let mock = MockLlmClient::new("?").answer_when_contains(
        "- Anna drinks tea.\nUser: What is 2+3?",
        r#"{ "facts": ["Anna drinks tea.", "2+3 is 5."] }"#,
    );
    let mut messages = vec![Message::user(
        "Facts from the conversation so far:\n- Anna drinks tea.",
    )];
    messages.extend(history());
    let mut ctx = context(mock, messages);

    block_on(ctx.compress_history(HistoryCompression::Facts(2))).unwrap();

    assert_eq!(
        ctx.prompt().messages[1],
        Message::user("Facts from the conversation so far:\n- Anna drinks tea.\n- 2+3 is 5.")
    );
    assert_eq!(ctx.prompt().messages.len(), 4);
}

/// Memory that remembers stored facts.
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

/// ADR-0079: with a memory, the durable facts of the compression go into long-term memory.
#[test]
fn remembers_the_durable_facts_of_a_compression() {
    let mock = MockLlmClient::new("?").answer_when_contains(
        "Result of add: 5",
        r#"{ "facts": ["2+3 is 5.", "The user is called Anna."], "durable": ["The user is called Anna."] }"#,
    );
    let memory = Arc::new(Remembering::default());
    let events = Arc::new(Events::default());
    let mut ctx = context(mock, history())
        .with_fact_memory(memory.clone())
        .install(events.clone());

    block_on(ctx.compress_history(HistoryCompression::Facts(2))).unwrap();

    assert_eq!(*memory.0.lock().unwrap(), ["The user is called Anna."]);
    assert_eq!(
        ctx.prompt().messages[1],
        Message::user(
            "Facts from the conversation so far:\n- 2+3 is 5.\n- The user is called Anna."
        )
    );
    assert!(events.0.lock().unwrap().contains(
        &grauwolf_agents::events::AgentEvent::MemoryRemembered {
            facts: vec!["The user is called Anna.".into()]
        }
    ));
}

#[derive(Default)]
struct Events(std::sync::Mutex<Vec<grauwolf_agents::events::AgentEvent>>);

impl grauwolf_agents::events::Feature for Events {
    fn on_event(&self, event: &grauwolf_agents::events::AgentEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}
