//! Content moderation (F01.15, ADR-0066).

use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_llm::{LlmClient, LlmError, Moderation, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};

/// Remembers the prompt and answers with a verdict in a code block.
#[derive(Default)]
struct Classifier(Mutex<Vec<Prompt>>);

impl LlmClient for Classifier {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        self.0.lock().unwrap().push(prompt.clone());
        Box::pin(async {
            Ok(Message::assistant(
                "```json\n{\"harmful\": true, \"categories\": [\"violence\"]}\n```",
            ))
        })
    }
}

#[test]
fn classifies_text_with_a_separate_structured_request_by_default() {
    let client = Arc::new(Classifier::default());
    let executor = PromptExecutor::builder()
        .client("mock", client.clone())
        .build();

    let moderation =
        block_on(executor.moderate("I want to hurt someone.", &LlModel::new("mock", "m"))).unwrap();

    assert_eq!(
        moderation,
        Moderation {
            harmful: true,
            categories: vec!["violence".into()],
            usage: None,
        }
    );
    let prompts = client.0.lock().unwrap();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0]
            .messages
            .contains(&Message::user("I want to hurt someone."))
    );
}

/// Reports usage for the classification (ADR-0067).
struct MeteredClassifier;

impl LlmClient for MeteredClassifier {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async {
            Ok(Message::assistant(
                "{\"harmful\": false, \"categories\": []}",
            ))
        })
    }

    fn execute_structured_with_usage<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        _output: &'a grauwolf_llm::StructuredOutput,
    ) -> BoxFuture<'a, Result<grauwolf_llm::Completion, LlmError>> {
        Box::pin(async move {
            Ok(grauwolf_llm::Completion {
                model: None,
                message: self.execute(prompt, model, &[]).await?,
                usage: Some(grauwolf_prompt::Usage {
                    input_tokens: 40,
                    output_tokens: 8,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                }),
            })
        })
    }
}

#[test]
fn reports_the_usage_of_the_classification() {
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(MeteredClassifier))
        .build();

    let moderation = block_on(executor.moderate("Hello", &LlModel::new("mock", "m"))).unwrap();

    let usage = moderation.usage.unwrap();
    assert_eq!((usage.input_tokens, usage.output_tokens), (40, 8));
}
