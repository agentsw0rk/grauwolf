//! Answer cache in the executor (F02.13, ADR-0086).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures_executor::block_on;
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use grauwolf_llm::{
    LlmClient, LlmError, MemoryPromptCache, PromptExecutor, StreamFrame, StructuredOutput,
    aggregate,
};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor, Usage};

/// Answers with the number of the call and reports a usage of 10 tokens.
#[derive(Default)]
struct Counting(AtomicUsize);

impl LlmClient for Counting {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        let call = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move { Ok(Message::assistant(format!("Answer {call}"))) })
    }
}

fn executor(client: Arc<Counting>) -> PromptExecutor {
    PromptExecutor::builder()
        .client("mock", client)
        .cache(Arc::new(MemoryPromptCache::default()))
        .build()
}

#[test]
fn answers_repeated_requests_from_the_cache_without_usage() {
    let client = Arc::new(Counting::default());
    let executor = executor(client.clone());
    let model = LlModel::new("mock", "m");
    let question = Prompt::builder("p").user("Hello").build();
    let other = Prompt::builder("p").user("Goodbye").build();

    let first = block_on(executor.execute(&question, &model, &[])).unwrap();
    let again = block_on(executor.execute_with_usage(&question, &model, &[])).unwrap();
    let different = block_on(executor.execute(&other, &model, &[])).unwrap();

    assert_eq!(first, Message::assistant("Answer 1"));
    assert_eq!(again.message, Message::assistant("Answer 1"));
    assert_eq!(
        again.usage,
        Some(Usage {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: None,
            cache_write_tokens: None,
        })
    );
    assert_eq!(different, Message::assistant("Answer 2"));
    assert_eq!(client.0.load(Ordering::SeqCst), 2);
}

#[test]
fn keys_requests_by_the_temperature_and_think_of_the_model() {
    let client = Arc::new(Counting::default());
    let executor = executor(client.clone());
    let question = Prompt::builder("p").user("Hello").build();
    let plain = LlModel::new("mock", "m");

    let first = block_on(executor.execute(&question, &plain, &[])).unwrap();
    let warm = block_on(executor.execute(&question, &plain.clone().with_temperature(1.0), &[]));
    let thinking = block_on(executor.execute(&question, &plain.clone().with_think(true), &[]));
    let warm_again = block_on(executor.execute(&question, &plain.with_temperature(1.0), &[]));

    assert_eq!(first, Message::assistant("Answer 1"));
    assert_eq!(warm.unwrap(), Message::assistant("Answer 2"));
    assert_eq!(thinking.unwrap(), Message::assistant("Answer 3"));
    assert_eq!(warm_again.unwrap(), Message::assistant("Answer 2"));
}

#[test]
fn keys_structured_requests_by_their_schema() {
    let client = Arc::new(Counting::default());
    let executor = executor(client.clone());
    let model = LlModel::new("mock", "m");
    let prompt = Prompt::builder("p").user("Printer broken").build();
    let ticket = StructuredOutput {
        name: "Ticket".into(),
        schema: serde_json::json!({ "type": "object", "properties": { "title": { "type": "string" } } }),
    };
    let other = StructuredOutput {
        name: "Ticket".into(),
        schema: serde_json::json!({ "type": "object", "properties": { "urgent": { "type": "boolean" } } }),
    };

    let first = block_on(executor.execute_structured(&prompt, &model, &ticket)).unwrap();
    let again = block_on(executor.execute_structured_with_usage(&prompt, &model, &ticket)).unwrap();
    let with_other_schema = block_on(executor.execute_structured(&prompt, &model, &other)).unwrap();
    let plain = block_on(executor.execute(&prompt, &model, &[])).unwrap();

    assert_eq!(again.message, first);
    assert_eq!(again.usage.map(|usage| usage.input_tokens), Some(0));
    assert_ne!(with_other_schema, first);
    assert_ne!(plain, first);
    assert_eq!(client.0.load(Ordering::SeqCst), 3);
}

#[test]
fn replays_cached_answers_as_frames_and_stores_streamed_ones() {
    let client = Arc::new(Counting::default());
    let executor = executor(client.clone());
    let model = LlModel::new("mock", "m");
    let prompt = Prompt::builder("p").user("Hello").build();
    let collect = || {
        block_on(
            executor
                .execute_streaming(&prompt, &model, &[])
                .map(Result::unwrap)
                .collect::<Vec<_>>(),
        )
    };

    let streamed = collect();
    let replayed = collect();
    let plain = block_on(executor.execute(&prompt, &model, &[])).unwrap();

    assert_eq!(aggregate(streamed), Message::assistant("Answer 1"));
    assert_eq!(
        replayed,
        vec![
            StreamFrame::TextDelta {
                text: "Answer 1".into()
            },
            StreamFrame::Usage(Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: None,
                cache_write_tokens: None,
            }),
            StreamFrame::End {
                finish_reason: None
            },
        ]
    );
    assert_eq!(plain, Message::assistant("Answer 1"));
    assert_eq!(client.0.load(Ordering::SeqCst), 1);
}
