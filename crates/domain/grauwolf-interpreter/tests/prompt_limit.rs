//! `limits.max_prompt_tokens` (ADR-0087): requests that are too large are never sent at all.

use grauwolf_spec::AgentSpecExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::{LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};
use grauwolf_spec::AgentSpec;
use serde_json::json;

/// Counts requests and answers with a fixed reply.
#[derive(Default)]
struct Counting(AtomicUsize);

impl LlmClient for Counting {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(Message::assistant("Sure.")) })
    }
}

#[test]
fn stops_before_sending_a_prompt_with_too_many_tokens() {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "sparsam",
            "model": { "provider": "mock", "id": "m" },
            "limits": { "max_prompt_tokens": 50 },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let client = Arc::new(Counting::default());
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", client.clone())
            .build(),
    ))
    .load(&spec)
    .unwrap();

    let short = block_on(agent.run("Short question?"));
    let long = block_on(agent.run(&"Very long question. ".repeat(40)));

    assert_eq!(short.unwrap(), "Sure.");
    let error = long.unwrap_err().to_string();
    assert!(error.contains("at most 50 tokens"), "{error}");
    assert_eq!(client.0.load(Ordering::SeqCst), 1);
}
