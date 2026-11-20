//! Streaming port (F02.14, ADR-0033).

use futures_executor::block_on;
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use grauwolf_llm::{LlmClient, LlmError, StreamFrame, aggregate};
use grauwolf_prompt::{
    AssistantMessage, AssistantPart, LlModel, Message, Prompt, ToolCall, ToolDescriptor,
};
use serde_json::json;

/// Client without its own streaming that always gives the same answer.
struct FixedClient(Message);

impl LlmClient for FixedClient {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        let answer = self.0.clone();
        Box::pin(async move { Ok(answer) })
    }
}

#[test]
fn every_client_streams_its_answer_as_frames_that_aggregate_back() {
    let answer = Message::Assistant(AssistantMessage {
        parts: vec![
            AssistantPart::Text {
                text: "Calculating.".into(),
            },
            AssistantPart::ToolCall(ToolCall {
                id: "c1".into(),
                tool: "add".into(),
                args: json!({ "a": 2 }),
            }),
        ],
    });
    let client = FixedClient(answer.clone());
    let prompt = Prompt::builder("p").user("Hello").build();
    let model = LlModel::new("p", "m");

    let frames: Vec<StreamFrame> = block_on(
        client
            .execute_streaming(&prompt, &model, &[])
            .map(Result::unwrap)
            .collect(),
    );

    assert_eq!(
        frames.last(),
        Some(&StreamFrame::End {
            finish_reason: None
        })
    );
    assert_eq!(aggregate(frames), answer);
}

#[test]
fn aggregate_joins_text_deltas() {
    let frames = vec![
        StreamFrame::TextDelta { text: "Hel".into() },
        StreamFrame::TextDelta { text: "lo".into() },
        StreamFrame::End {
            finish_reason: Some("stop".into()),
        },
    ];

    assert_eq!(aggregate(frames), Message::assistant("Hello"));
}

#[test]
fn model_frames_do_not_change_the_aggregated_answer() {
    let frames = vec![
        StreamFrame::Model("m-2026-11-01".into()),
        StreamFrame::TextDelta { text: "Hi".into() },
        StreamFrame::End {
            finish_reason: None,
        },
    ];

    assert_eq!(aggregate(frames), Message::assistant("Hi"));
}
