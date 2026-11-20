//! History compression (F05.14, ADR-0037).

use std::sync::Arc;

use grauwolf_llm::LlmError;
use grauwolf_prompt::Message;

use crate::conversation::Conversation;
use crate::events::AgentEvent;
use crate::facts::{FACT_COMPRESSION, request_fact_lists};
use crate::llm_gateway::LlmGateway;
use crate::memory::FactMemory;

/// How the history is shortened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryCompression {
    /// The LLM summarizes the history; the summary replaces it.
    Summarize,
    /// Only the last n messages remain (after the system prompt).
    KeepLast(usize),
    /// The last n messages remain; the older ones are replaced by a list of the facts the
    /// LLM extracts from them (F05.15, ADR-0076).
    Facts(usize),
}

/// Start of the message that replaces the older messages with [`HistoryCompression::Facts`].
pub const FACTS_HEADER: &str = "Facts from the conversation so far:";

/// Message with the facts of a shortening, one line per fact.
pub(crate) fn facts_message(facts: &[String]) -> String {
    let lines: Vec<String> = facts.iter().map(|fact| format!("- {fact}")).collect();
    format!("{FACTS_HEADER}\n{}", lines.join("\n"))
}

/// History as text for the fact request; an earlier fact list is kept verbatim.
pub(crate) fn transcript(messages: &[Message]) -> String {
    let mut lines = Vec::new();
    for message in messages {
        match message {
            Message::User { content, .. } if content.starts_with(FACTS_HEADER) => {
                lines.push(content.clone());
            }
            Message::User { content, .. } => lines.push(format!("User: {content}")),
            Message::Assistant(answer) => {
                let text = answer.text();
                if !text.is_empty() {
                    lines.push(format!("Assistant: {text}"));
                }
                for call in answer.tool_calls() {
                    lines.push(format!("Assistant calls {} with {}", call.tool, call.args));
                }
            }
            Message::ToolResult(result) => {
                lines.push(format!("Result of {}: {}", result.tool, result.content));
            }
            Message::System { .. } => {}
        }
    }
    lines.join("\n")
}

/// Number of leading system messages; they are kept by every compression.
pub(crate) fn leading_system(messages: &[Message]) -> usize {
    messages
        .iter()
        .take_while(|message| matches!(message, Message::System { .. }))
        .count()
}

/// Index from which the last `keep` messages of `rest` are kept. The kept part never starts
/// with a tool result without its call, and a pending tool call at the end is kept.
pub(crate) fn keep_from(rest: &[Message], keep: usize) -> usize {
    let pending_call = matches!(rest.last(), Some(Message::Assistant(answer)) if answer.tool_calls().next().is_some());
    let keep = if pending_call { keep.max(1) } else { keep };
    let mut start = rest.len().saturating_sub(keep);
    while start > 0 && start < rest.len() && matches!(rest[start], Message::ToolResult(_)) {
        start -= 1;
    }
    start
}

/// Shortens the history in `conversation` by `strategy` (F05.14, ADR-0037); `Summarize` and
/// `Facts` ask the model through `gateway`, `Facts` stores durable facts in `fact_memory`
/// (ADR-0079). The caller checks the budget.
pub(crate) async fn compress(
    conversation: &mut Conversation,
    gateway: &LlmGateway<'_>,
    fact_memory: Option<&Arc<dyn FactMemory>>,
    strategy: HistoryCompression,
) -> Result<(), LlmError> {
    let system = leading_system(&conversation.prompt().messages);
    match strategy {
        HistoryCompression::Summarize => {
            // A pending tool call at the end does not belong in the summary.
            let tail = system + keep_from(&conversation.prompt().messages[system..], 0);
            let mut request = conversation.prompt().clone();
            request.messages.truncate(tail);
            request.messages.push(Message::user(
                "Summarize the conversation so far concisely. Keep all facts, \
                 results and open questions needed for the further work.",
            ));
            let mut no_tools = Vec::new();
            gateway.bus.before_llm_call(&mut request, &mut no_tools);
            gateway.llm_call_starting(&[]);
            let executor = Arc::clone(gateway.executor);
            let request = &request;
            let (result, answered) = gateway
                .with_fallbacks(gateway.model.model(), |model| {
                    let executor = Arc::clone(&executor);
                    async move { executor.execute(request, &model, &[]).await }
                })
                .await;
            let result = gateway.bus.finish(result);
            gateway.llm_call_finished_by(&answered, &result, None, None);
            let summary = match result? {
                Message::Assistant(answer) => answer.text(),
                _ => String::new(),
            };
            let pending: Vec<Message> = conversation.prompt_mut().messages.drain(tail..).collect();
            conversation.prompt_mut().messages.truncate(system);
            conversation
                .prompt_mut()
                .messages
                .push(Message::user(format!(
                    "Summary of the conversation so far:\n{summary}"
                )));
            conversation.prompt_mut().messages.extend(pending);
        }
        HistoryCompression::KeepLast(keep) => {
            let start = system + keep_from(&conversation.prompt().messages[system..], keep);
            conversation.prompt_mut().messages.drain(system..start);
        }
        HistoryCompression::Facts(keep) => {
            let start = system + keep_from(&conversation.prompt().messages[system..], keep);
            if start == system {
                return Ok(());
            }
            let older = transcript(&conversation.prompt().messages[system..start]);
            let (facts, durable) = request_fact_lists(
                gateway,
                FACT_COMPRESSION,
                format!("Conversation so far:\n{older}"),
                fact_memory.is_some(),
            )
            .await?;
            conversation
                .prompt_mut()
                .messages
                .splice(system..start, [Message::user(facts_message(&facts))]);
            if let Some(memory) = fact_memory.cloned()
                && !durable.is_empty()
            {
                // A memory error does not stop the compression (ADR-0079).
                gateway.emit(match memory.remember(&durable).await {
                    Ok(()) => AgentEvent::MemoryRemembered { facts: durable },
                    Err(error) => AgentEvent::MemoryFailed {
                        error: error.to_string(),
                    },
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use futures_executor::block_on;
    use futures_util::future::BoxFuture;
    use grauwolf_llm::{LlmClient, PromptExecutor};
    use grauwolf_prompt::{LlModel, Prompt, ToolDescriptor};

    use super::*;
    use crate::accounting::Accounting;
    use crate::event_bus::EventBus;
    use crate::model_selection::ModelSelection;

    /// Answers every request with `reply` and records the prompts it was asked.
    struct Client {
        reply: String,
        prompts: Mutex<Vec<Prompt>>,
    }

    impl LlmClient for Client {
        fn execute<'a>(
            &'a self,
            prompt: &'a Prompt,
            _model: &'a LlModel,
            _tools: &'a [ToolDescriptor],
        ) -> BoxFuture<'a, Result<Message, LlmError>> {
            self.prompts.lock().unwrap().push(prompt.clone());
            Box::pin(std::future::ready(Ok(Message::assistant(
                self.reply.clone(),
            ))))
        }
    }

    /// The parts a gateway views, with a client that answers `reply`.
    struct Parts {
        client: Arc<Client>,
        executor: Arc<PromptExecutor>,
        model: ModelSelection,
        bus: EventBus,
        accounting: Accounting,
    }

    impl Parts {
        fn new(reply: &str) -> Self {
            let client = Arc::new(Client {
                reply: reply.to_owned(),
                prompts: Mutex::default(),
            });
            Self {
                executor: Arc::new(
                    PromptExecutor::builder()
                        .client("p", client.clone())
                        .build(),
                ),
                client,
                model: ModelSelection::new(LlModel::new("p", "main")),
                bus: EventBus::new(),
                accounting: Accounting::new(),
            }
        }

        fn gateway(&self) -> LlmGateway<'_> {
            LlmGateway {
                executor: &self.executor,
                model: &self.model,
                bus: &self.bus,
                accounting: &self.accounting,
                frame_sink: None,
            }
        }
    }

    fn conversation(count: usize) -> Conversation {
        let mut prompt = Prompt::builder("p").system("Be brief.").build();
        for index in 0..count {
            prompt
                .messages
                .push(Message::user(format!("question {index}")));
            prompt
                .messages
                .push(Message::assistant(format!("answer {index}")));
        }
        Conversation::new(prompt)
    }

    fn texts(conversation: &Conversation) -> Vec<String> {
        conversation
            .prompt()
            .messages
            .iter()
            .map(|message| match message {
                Message::System { content, .. } | Message::User { content, .. } => content.clone(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn keep_last_keeps_the_system_prompt_and_the_last_messages() {
        let parts = Parts::new("unused");
        let mut history = conversation(3);
        block_on(compress(
            &mut history,
            &parts.gateway(),
            None,
            HistoryCompression::KeepLast(2),
        ))
        .unwrap();
        let texts = texts(&history);
        assert_eq!(texts.len(), 3);
        assert_eq!(texts[0], "Be brief.");
        assert_eq!(texts[1], "question 2");
        assert!(parts.client.prompts.lock().unwrap().is_empty());
    }

    #[test]
    fn summarize_replaces_the_history_with_the_summary() {
        let parts = Parts::new("They talked.");
        let mut history = conversation(2);
        block_on(compress(
            &mut history,
            &parts.gateway(),
            None,
            HistoryCompression::Summarize,
        ))
        .unwrap();
        assert_eq!(
            texts(&history),
            [
                "Be brief.",
                "Summary of the conversation so far:\nThey talked."
            ]
        );
    }

    #[test]
    fn facts_replace_the_older_messages_with_a_fact_list() {
        let parts = Parts::new(r#"{"facts": ["The user asked."]}"#);
        let mut history = conversation(3);
        block_on(compress(
            &mut history,
            &parts.gateway(),
            None,
            HistoryCompression::Facts(2),
        ))
        .unwrap();
        let texts = texts(&history);
        assert_eq!(texts.len(), 4);
        assert_eq!(texts[1], facts_message(&["The user asked.".to_owned()]));
        assert_eq!(texts[2], "question 2");
    }

    #[test]
    fn facts_without_older_messages_ask_nothing() {
        let parts = Parts::new("unused");
        let mut history = conversation(1);
        block_on(compress(
            &mut history,
            &parts.gateway(),
            None,
            HistoryCompression::Facts(5),
        ))
        .unwrap();
        assert_eq!(history.prompt().messages.len(), 3);
        assert!(parts.client.prompts.lock().unwrap().is_empty());
    }
}
