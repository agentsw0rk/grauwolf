//! Fact requests: extracting durable facts from an exchange and listing the facts of a
//! history compression (F05.15, ADR-0069, ADR-0076, ADR-0079). Free functions over the
//! [`LlmGateway`]; the history stays unchanged, and the usage counts toward the run.

use std::sync::Arc;

use grauwolf_graph::NodeError;
use grauwolf_llm::{LlmError, StructuredOutput};
use grauwolf_prompt::Prompt;
use serde_json::Value;

use crate::llm_gateway::LlmGateway;

/// Addition to the compression request when durable facts go into memory (ADR-0079).
pub(crate) const DURABLE_FACTS: &str = "`durable` is a subset of `facts`: copy every fact from \
    `facts` verbatim that says something about the user – who they are, where they live, what \
    they like, own or plan, whom they know. Leave out only open questions, daily events \
    (e.g. the weather) and statements about the assistant.";

/// Instruction of the compression request with [`crate::HistoryCompression::Facts`].
pub(crate) const FACT_COMPRESSION: &str = "You shorten the history of a conversation so that it can \
    continue without it. Record every piece of information the user gave about themselves, their \
    plans, intentions, appointments, places, things and wishes, plus tool results, \
    decisions and open questions – one complete, self-contained sentence each, with all \
    details (names, numbers, times), about the user in the third person \
    (e.g. \"The user is called …\"). Better one fact too many than one too few. An \
    earlier list of facts is carried over completely into your list. Leave out only pleasantries, \
    guesses of the assistant and repetitions.";

/// Instruction of the fact request of [`crate::AgentContext::extract_facts`] (ADR-0069).
const FACT_EXTRACTION: &str = "You maintain the long-term memory of an assistant. From the \
    following exchange, name only durable facts about the user (name, preferences, plans, \
    relationships) as complete, self-contained sentences. No questions, no daily events, \
    nothing about the assistant. Do not name again what is listed under \"Already known\" – \
    not even in different words. If there is nothing new and durable, return an empty list.";

/// Asks the active model, with its own structured request, for durable facts from `input`
/// and `output` that are not yet in `known` (ADR-0069). The caller checks the budget.
pub(crate) async fn extract_facts(
    gateway: &LlmGateway<'_>,
    input: &str,
    output: &str,
    known: &[String],
) -> Result<Vec<String>, NodeError> {
    let mut exchange = format!("User message:\n{input}\n\nAssistant answer:\n{output}");
    if !known.is_empty() {
        let known: Vec<String> = known.iter().map(|fact| format!("- {fact}")).collect();
        exchange.push_str(&format!("\n\nAlready known:\n{}", known.join("\n")));
    }
    Ok(request_facts(gateway, FACT_EXTRACTION, exchange).await?)
}

/// Asks the active model for a structured list `{ facts }` about `text`, with
/// `instruction` as the system prompt.
async fn request_facts(
    gateway: &LlmGateway<'_>,
    instruction: &str,
    text: String,
) -> Result<Vec<String>, LlmError> {
    Ok(request_fact_lists(gateway, instruction, text, false)
        .await?
        .0)
}

/// Like [`request_facts`]; with `durable`, also the list `durable` of the durable facts about
/// the user (ADR-0079).
pub(crate) async fn request_fact_lists(
    gateway: &LlmGateway<'_>,
    instruction: &str,
    text: String,
    durable: bool,
) -> Result<(Vec<String>, Vec<String>), LlmError> {
    let instruction = if durable {
        format!("{instruction} {DURABLE_FACTS}")
    } else {
        instruction.to_owned()
    };
    let prompt = Prompt::builder("facts")
        .system(instruction)
        .user(text)
        .build();
    let list = serde_json::json!({ "type": "array", "items": { "type": "string" } });
    let mut properties = serde_json::json!({ "facts": list });
    let mut required = vec!["facts"];
    if durable {
        properties["durable"] = list;
        required.push("durable");
    }
    let schema = StructuredOutput {
        name: "Facts".into(),
        schema: serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required
        }),
    };
    let executor = Arc::clone(gateway.executor);
    let (prompt, schema_ref) = (&prompt, &schema);
    let (completion, answered) = gateway
        .with_fallbacks(gateway.model.model(), |model| {
            let executor = Arc::clone(&executor);
            async move {
                executor
                    .execute_structured_with_usage(prompt, &model, schema_ref)
                    .await
            }
        })
        .await;
    let completion = completion?;
    if let Some(usage) = completion.usage {
        let _ = gateway.accounting.add_usage_for(&answered, usage);
    }
    let value = crate::structured::parse_answer(&completion.message, &schema)
        .map_err(|error| LlmError::Client(Box::new(error)))?;
    let strings = |key: &str| -> Vec<String> {
        value[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    };
    Ok((strings("facts"), strings("durable")))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use futures_executor::block_on;
    use futures_util::future::BoxFuture;
    use grauwolf_llm::{LlmClient, PromptExecutor};
    use grauwolf_prompt::ToolDescriptor;

    use grauwolf_prompt::{LlModel, Message};

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

    #[test]
    fn extracting_facts_returns_the_list_and_names_what_is_already_known() {
        let parts = Parts::new(r#"{"facts": ["The user is called Ada."]}"#);
        let known = vec!["The user likes tea.".to_owned()];
        let facts = block_on(extract_facts(
            &parts.gateway(),
            "I am Ada",
            "Hello Ada",
            &known,
        ))
        .unwrap();
        assert_eq!(facts, ["The user is called Ada."]);
        let prompts = parts.client.prompts.lock().unwrap();
        let text = format!("{:?}", prompts[0].messages);
        assert!(text.contains("Already known:\\n- The user likes tea."));
    }

    #[test]
    fn durable_facts_are_only_asked_for_on_request() {
        let parts = Parts::new(r#"{"facts": ["a", "b"], "durable": ["b"]}"#);
        let (facts, durable) = block_on(request_fact_lists(
            &parts.gateway(),
            "Shorten.",
            "text".into(),
            true,
        ))
        .unwrap();
        assert_eq!(
            (facts, durable),
            (vec!["a".into(), "b".into()], vec!["b".into()])
        );
        let (_, durable) = block_on(request_fact_lists(
            &parts.gateway(),
            "Shorten.",
            "text".into(),
            false,
        ))
        .unwrap();
        assert!(durable.is_empty());
        let prompts = parts.client.prompts.lock().unwrap();
        assert!(format!("{:?}", prompts[0].messages).contains("`durable` is a subset"));
        assert!(!format!("{:?}", prompts[1].messages).contains("`durable` is a subset"));
    }

    #[test]
    fn an_answer_that_is_not_json_is_an_error() {
        let parts = Parts::new("no idea");
        assert!(block_on(extract_facts(&parts.gateway(), "a", "b", &[])).is_err());
    }
}
