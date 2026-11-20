//! Token usage per call (ADR-0067).

use std::sync::Arc;

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_llm::{Completion, LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor, Usage};

/// Does not know its usage.
struct Plain;

impl LlmClient for Plain {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async { Ok(Message::assistant("without numbers")) })
    }
}

/// Reports its usage.
struct Counting;

impl LlmClient for Counting {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async move { Ok(self.execute_with_usage(prompt, model, tools).await?.message) })
    }

    fn execute_with_usage<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async {
            Ok(Completion {
                model: None,
                message: Message::assistant("with numbers"),
                usage: Some(Usage {
                    input_tokens: 120,
                    output_tokens: 7,
                    cache_read_tokens: Some(100),
                    cache_write_tokens: None,
                }),
            })
        })
    }
}

#[test]
fn reports_usage_when_the_client_knows_it() {
    let executor = PromptExecutor::builder()
        .client("plain", Arc::new(Plain))
        .client("counting", Arc::new(Counting))
        .build();
    let prompt = Prompt::builder("p").user("Hello").build();

    let plain =
        block_on(executor.execute_with_usage(&prompt, &LlModel::new("plain", "m"), &[])).unwrap();
    let counted =
        block_on(executor.execute_with_usage(&prompt, &LlModel::new("counting", "m"), &[]))
            .unwrap();

    assert_eq!(plain.message, Message::assistant("without numbers"));
    assert_eq!(plain.usage, None);
    assert_eq!(counted.usage.unwrap().cache_read_tokens, Some(100));
}
