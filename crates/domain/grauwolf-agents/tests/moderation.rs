//! Node `llm_moderate` (F05.17, ADR-0066): check inputs for harmful content.

use std::sync::Arc;

use futures_executor::block_on;
use grauwolf_agents::structured::Structured;
use grauwolf_agents::{AgentContext, nodes};
use grauwolf_llm::{PromptExecutor, StructuredOutput};
use grauwolf_prompt::{LlModel, Prompt};
use grauwolf_test::MockLlmClient;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Moderation {
    harmful: bool,
    categories: Vec<String>,
    text: String,
    message: String,
}

impl Structured for Moderation {
    fn structured_output() -> StructuredOutput {
        StructuredOutput {
            name: "Moderation".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "harmful": { "type": "boolean" },
                    "categories": { "type": "array", "items": { "type": "string" } },
                    "text": { "type": "string" },
                    "message": { "type": "string" }
                },
                "required": ["harmful", "categories", "text", "message"]
            }),
        }
    }
}

fn context() -> AgentContext {
    let mock = MockLlmClient::new(r#"{ "harmful": false, "categories": [] }"#)
        .answer_when_contains("hurt", r#"{ "harmful": true, "categories": ["violence"] }"#);
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(mock))
        .build();
    AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").system("Be helpful.").build(),
    )
}

#[test]
fn passes_harmless_text_on_and_rejects_harmful_text_without_touching_the_history() {
    let mut ctx = context();
    let node = nodes::llm_moderate::<Moderation>(None);

    let harmless = block_on(node(&mut ctx, "How do I bake bread?".into())).unwrap();
    let harmful = block_on(node(&mut ctx, "I want to hurt someone.".into())).unwrap();

    assert!(!harmless.harmful);
    assert_eq!(harmless.text, "How do I bake bread?");
    assert!(harmful.harmful);
    assert_eq!(harmful.categories, ["violence"]);
    assert!(harmful.message.contains("violence"), "{}", harmful.message);
    assert_eq!(ctx.prompt().messages.len(), 1);
}

/// Records all events.
#[derive(Default)]
struct Recorder(std::sync::Mutex<Vec<grauwolf_agents::events::AgentEvent>>);

impl grauwolf_agents::events::Feature for Recorder {
    fn on_event(&self, event: &grauwolf_agents::events::AgentEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

/// Observability (ADR-0066): moderation reports with its own events.
#[test]
fn reports_moderation_with_its_own_events() {
    use grauwolf_agents::events::AgentEvent;

    let recorder = Arc::new(Recorder::default());
    let ctx = context().install(recorder.clone());

    block_on(nodes::llm_moderate::<Moderation>(None)(
        &mut { ctx },
        "I want to hurt someone.".into(),
    ))
    .unwrap();

    assert_eq!(
        *recorder.0.lock().unwrap(),
        [
            AgentEvent::ModerationStarting {
                model: LlModel::new("mock", "m")
            },
            AgentEvent::ModerationCompleted {
                harmful: true,
                categories: vec!["violence".into()],
                usage: None,
                cost: None,
            }
        ]
    );
}

#[test]
fn reports_a_failed_moderation() {
    use grauwolf_agents::events::AgentEvent;

    let recorder = Arc::new(Recorder::default());
    let ctx = context().install(recorder.clone());
    let model = LlModel::new("missing", "m");

    let result = block_on(nodes::llm_moderate::<Moderation>(Some(model.clone()))(
        &mut { ctx },
        "Hello".into(),
    ));

    assert!(result.is_err());
    let events = recorder.0.lock().unwrap();
    assert_eq!(events[0], AgentEvent::ModerationStarting { model });
    assert!(
        matches!(&events[1], AgentEvent::ModerationFailed { error } if error.contains("missing")),
        "{events:?}"
    );
}

/// Masks a word in moderation requests; records the IDs of the prompts it sees.
#[derive(Default)]
struct Redactor(std::sync::Mutex<Vec<String>>);

impl grauwolf_agents::events::Interceptor for Redactor {
    fn before_llm_call(
        &self,
        prompt: &mut Prompt,
        _tools: &mut Vec<grauwolf_prompt::ToolDescriptor>,
    ) {
        self.0.lock().unwrap().push(prompt.id.clone());
        for message in &mut prompt.messages {
            if let grauwolf_prompt::Message::User { content, .. } = message {
                *content = content.replace("hurt", "[removed]");
            }
        }
    }
}

/// Interceptors (ADR-0047 addendum): the text to check runs through `before_llm_call`.
#[test]
fn lets_interceptors_change_the_text_before_moderating_it() {
    let redactor = Arc::new(Redactor::default());
    let mut ctx = context();
    ctx.intercept(redactor.clone());

    let moderation = block_on(ctx.moderate("I want to hurt someone.", None)).unwrap();

    assert!(!moderation.harmful);
    assert_eq!(
        *redactor.0.lock().unwrap(),
        [grauwolf_agents::MODERATION_PROMPT_ID]
    );
}
