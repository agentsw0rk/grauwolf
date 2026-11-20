//! Fallback models from the spec (ADR-0088).

use grauwolf_spec::AgentSpecExt;
use std::sync::Arc;

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::{LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use serde_json::json;

/// A provider that is down.
struct Down;

impl LlmClient for Down {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async { Err(LlmError::Client("provider unreachable".into())) })
    }
}

#[test]
fn answers_with_the_fallback_model_from_the_spec() {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "robust",
            "model": { "provider": "down", "id": "a", "fallbacks": [ { "provider": "up", "id": "b" } ] },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("down", Arc::new(Down))
            .client("up", Arc::new(MockLlmClient::new("Here is the answer.")))
            .build(),
    ))
    .load(&spec)
    .unwrap();

    let answer = block_on(agent.run("Question")).unwrap();

    assert_eq!(answer, "Here is the answer.");
}

#[test]
fn switches_to_a_model_with_fallbacks() {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "wechselnd",
            "model": { "provider": "up", "id": "start" },
            "graph": {
                "nodes": [
                    { "id": "switch", "kind": "set_model",
                      "params": { "provider": "down", "id": "a", "value": "string",
                                  "fallbacks": [ { "provider": "up", "id": "b" } ] } },
                    { "id": "call_llm", "kind": "llm_request" }
                ],
                "edges": [
                    { "from": "start", "to": "switch" },
                    { "from": "switch", "to": "call_llm" },
                    { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } }
                ]
            }
        })
        .to_string(),
    )
    .unwrap();
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("down", Arc::new(Down))
            .client(
                "up",
                Arc::new(MockLlmClient::new("From the fallback model.")),
            )
            .build(),
    ))
    .load(&spec)
    .unwrap();

    let answer = block_on(agent.run("Question")).unwrap();

    assert_eq!(answer, "From the fallback model.");
}
