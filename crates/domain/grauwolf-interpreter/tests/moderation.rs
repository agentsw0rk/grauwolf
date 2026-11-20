//! Node kind `llm_moderate` (F05.17, ADR-0066) in the interpreter.

use grauwolf_spec::AgentSpecExt;
use std::sync::Arc;

use futures_executor::block_on;
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::PromptExecutor;
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use serde_json::json;

fn spec() -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "guarded",
            "model": { "provider": "mock", "id": "m" },
            "types": { "Moderation": {
                "type": "object",
                "properties": {
                    "harmful": { "type": "boolean" },
                    "categories": { "type": "array", "items": { "type": "string" } },
                    "text": { "type": "string" },
                    "message": { "type": "string" }
                },
                "required": ["harmful", "categories", "text", "message"]
            } },
            "graph": {
                "nodes": [
                    { "id": "moderate", "kind": "llm_moderate", "params": { "type": "Moderation" } },
                    { "id": "refuse", "kind": "get_field", "params": { "type": "Moderation", "field": "message" } },
                    { "id": "pass", "kind": "get_field", "params": { "type": "Moderation", "field": "text" } },
                    { "id": "call_llm", "kind": "llm_request" }
                ],
                "edges": [
                    { "from": "start", "to": "moderate" },
                    { "from": "moderate", "to": "refuse",
                      "when": { "kind": "on_field", "params": { "field": "harmful", "equals": true } } },
                    { "from": "moderate", "to": "pass" },
                    { "from": "refuse", "to": "finish" },
                    { "from": "pass", "to": "call_llm" },
                    { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } }
                ]
            }
        })
        .to_string(),
    )
    .unwrap()
}

#[test]
fn refuses_harmful_requests_before_the_model_sees_them() {
    let mock = MockLlmClient::new("The model would have answered.")
        .answer_when_contains("hurt", r#"{ "harmful": true, "categories": ["violence"] }"#);
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(mock))
            .build(),
    ))
    .load(&spec())
    .unwrap();

    let harmful = block_on(agent.run("I want to hurt someone.")).unwrap();

    assert!(
        harmful.starts_with("I cannot process this request"),
        "{harmful}"
    );
    assert!(harmful.contains("violence"), "{harmful}");
}

/// Recognizes moderation requests by their system instruction: "hurt" is harmful, everything
/// else harmless; otherwise it answers with a recipe.
struct Moderated;

impl grauwolf_llm::LlmClient for Moderated {
    fn execute<'a>(
        &'a self,
        prompt: &'a grauwolf_prompt::Prompt,
        _model: &'a grauwolf_prompt::LlModel,
        _tools: &'a [grauwolf_prompt::ToolDescriptor],
    ) -> futures_util::future::BoxFuture<'a, Result<grauwolf_prompt::Message, grauwolf_llm::LlmError>>
    {
        let moderation = prompt.messages.iter().any(|message| {
            matches!(message, grauwolf_prompt::Message::System { content } if content.contains("harmful content"))
        });
        let text = format!("{:?}", prompt.messages);
        let answer = match (moderation, text.contains("hurt")) {
            (true, true) => r#"{ "harmful": true, "categories": ["violence"] }"#,
            (true, false) => r#"{ "harmful": false, "categories": [] }"#,
            (false, _) => "Sure: flour, water, salt, yeast.",
        };
        Box::pin(async move { Ok(grauwolf_prompt::Message::assistant(answer)) })
    }
}

/// Convenient way (ADR-0066): `single_run` with `moderate: true` checks the input first.
#[test]
fn single_run_with_moderation_checks_the_input_before_the_model_answers() {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "guarded",
            "model": { "provider": "mock", "id": "m" },
            "strategy": { "kind": "single_run", "params": { "moderate": true } }
        })
        .to_string(),
    )
    .unwrap();
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(Moderated))
            .build(),
    ))
    .load(&spec)
    .unwrap();

    let harmful = block_on(agent.run("I want to hurt someone.")).unwrap();
    let harmless = block_on(agent.run("How do I bake bread?")).unwrap();

    assert!(harmful.contains("violence"), "{harmful}");
    assert_eq!(harmless, "Sure: flour, water, salt, yeast.");
}
