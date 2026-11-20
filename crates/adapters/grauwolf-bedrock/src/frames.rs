//! Translates the events of `converse-stream` into frames (ADR-0033).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use futures_util::stream::{self, BoxStream, StreamExt};
use grauwolf_llm::{LlmError, StreamFrame};
use grauwolf_llm_http::HttpError;
use grauwolf_prompt::{Reasoning, ToolCall, Usage};
use serde_json::{Map, Value, json};

use crate::dto::{self, PROVIDER};
use crate::eventstream::{Decoder, EventMessage};

/// A content block whose deltas are still being collected. Text needs no collecting, it is
/// emitted at once.
#[derive(Debug)]
enum PartialBlock {
    ToolUse {
        id: String,
        name: String,
        json: String,
    },
    /// Reasoning text and signature, or redacted content.
    Reasoning {
        text: String,
        signature: String,
        redacted: Option<String>,
    },
}

#[derive(Default)]
struct StreamState {
    decoder: Decoder,
    blocks: BTreeMap<u64, PartialBlock>,
    stop_reason: Option<String>,
    usage: Option<Usage>,
    /// After an error or the end: nothing more is emitted.
    finished: bool,
}

type Frames = Vec<Result<StreamFrame, LlmError>>;

impl StreamState {
    fn chunk(&mut self, chunk: &[u8]) -> Frames {
        if self.finished {
            return Vec::new();
        }
        self.decoder.feed(chunk);
        let mut frames = Vec::new();
        // Messages before a damaged one are still delivered.
        loop {
            match self.decoder.next_message() {
                Ok(Some(message)) => frames.extend(self.message(&message)),
                Ok(None) => break,
                Err(error) => {
                    frames.extend(self.fail(LlmError::Client(Box::new(error))));
                }
            }
            if self.finished {
                break;
            }
        }
        frames
    }

    fn fail(&mut self, error: LlmError) -> Frames {
        self.finished = true;
        vec![Err(error)]
    }

    /// The transport ended: finish the response, or report that it was cut off.
    fn end(&mut self) -> Frames {
        if self.finished {
            return Vec::new();
        }
        if self.stop_reason.is_none() || self.decoder.has_partial_message() {
            return self.fail(LlmError::Client(
                "the response stream of Bedrock ended before the message was complete".into(),
            ));
        }
        self.finished = true;
        if let Err(error) = dto::check_stop_reason(self.stop_reason.as_deref()) {
            return vec![Err(error)];
        }
        self.usage
            .take()
            .map(|usage| Ok(StreamFrame::Usage(usage)))
            .into_iter()
            .chain([Ok(StreamFrame::End {
                finish_reason: self.stop_reason.take(),
            })])
            .collect()
    }

    fn message(&mut self, message: &EventMessage) -> Frames {
        match message.message_type.as_str() {
            "event" => {}
            "exception" | "error" => return self.fail(exception(message)),
            _ => return Vec::new(),
        }
        let data: Value = match serde_json::from_slice(&message.payload) {
            Ok(data) => data,
            Err(error) => {
                return self.fail(LlmError::Client(
                    format!("invalid stream event from Bedrock: {error}").into(),
                ));
            }
        };
        let index = data
            .get("contentBlockIndex")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        match message.kind.as_str() {
            "contentBlockStart" => {
                if let Some(call) = data.pointer("/start/toolUse") {
                    self.blocks.insert(
                        index,
                        PartialBlock::ToolUse {
                            id: string(call, "toolUseId"),
                            name: string(call, "name"),
                            json: String::new(),
                        },
                    );
                }
                Vec::new()
            }
            "contentBlockDelta" => self.delta(index, &data["delta"]),
            "contentBlockStop" => self.block_stop(index),
            "messageStop" => {
                self.stop_reason = data
                    .get("stopReason")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                Vec::new()
            }
            "metadata" => {
                self.usage = data.get("usage").map(dto::usage);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn delta(&mut self, index: u64, delta: &Value) -> Frames {
        if let Some(text) = delta.get("text").and_then(Value::as_str) {
            return vec![Ok(StreamFrame::TextDelta { text: text.into() })];
        }
        if let Some(input) = delta.pointer("/toolUse/input").and_then(Value::as_str) {
            if let Some(PartialBlock::ToolUse { json, .. }) = self.blocks.get_mut(&index) {
                json.push_str(input);
            }
            return Vec::new();
        }
        if let Some(reasoning) = delta.get("reasoningContent") {
            let block = self
                .blocks
                .entry(index)
                .or_insert_with(|| PartialBlock::Reasoning {
                    text: String::new(),
                    signature: String::new(),
                    redacted: None,
                });
            if let PartialBlock::Reasoning {
                text,
                signature,
                redacted,
            } = block
            {
                text.push_str(&string(reasoning, "text"));
                signature.push_str(&string(reasoning, "signature"));
                if let Some(data) = reasoning.get("redactedContent").and_then(Value::as_str) {
                    *redacted = Some(data.to_owned());
                }
            }
        }
        Vec::new()
    }

    fn block_stop(&mut self, index: u64) -> Frames {
        match self.blocks.remove(&index) {
            Some(PartialBlock::ToolUse { id, name, json }) => {
                let args = if json.trim().is_empty() {
                    Ok(Value::Object(Map::new()))
                } else {
                    serde_json::from_str(&json)
                };
                vec![
                    args.map(|args| {
                        StreamFrame::ToolCall(ToolCall {
                            id,
                            tool: name.clone(),
                            args,
                        })
                    })
                    .map_err(|error| {
                        LlmError::Client(
                            format!("arguments of the tool call '{name}' are not JSON: {error}")
                                .into(),
                        )
                    }),
                ]
            }
            Some(PartialBlock::Reasoning {
                text,
                signature,
                redacted,
            }) => {
                let content = match redacted {
                    Some(data) => json!({ "redactedContent": data }),
                    None => {
                        let mut reasoning = json!({ "text": text });
                        if !signature.is_empty() {
                            reasoning["signature"] = json!(signature);
                        }
                        json!({ "reasoningText": reasoning })
                    }
                };
                vec![Ok(StreamFrame::Reasoning(Reasoning {
                    provider: PROVIDER.into(),
                    data: json!({ "reasoningContent": content }),
                }))]
            }
            None => Vec::new(),
        }
    }
}

fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// An exception or error message of the stream; overload and server errors are temporary.
fn exception(message: &EventMessage) -> LlmError {
    let detail = serde_json::from_slice::<Value>(&message.payload)
        .ok()
        .and_then(|payload| {
            payload
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default();
    let text = format!("Bedrock reports in the stream {}: {detail}", message.kind);
    let transient = matches!(
        message.kind.as_str(),
        "throttlingException"
            | "serviceUnavailableException"
            | "internalServerException"
            | "modelTimeoutException"
    );
    if transient {
        LlmError::Transient {
            source: text.into(),
            retry_after: None,
        }
    } else {
        LlmError::Client(text.into())
    }
}

/// Frames from the chunks of a `converse-stream` response.
pub(crate) fn frames_of_chunks(
    chunks: BoxStream<'static, Result<Vec<u8>, HttpError>>,
) -> BoxStream<'static, Result<StreamFrame, LlmError>> {
    let state = Arc::new(Mutex::new(StreamState::default()));
    chunks
        .map(Some)
        .chain(stream::once(async { None }))
        .flat_map(move |chunk| {
            let mut state = state.lock().expect("stream state");
            let frames = match chunk {
                Some(Ok(chunk)) => state.chunk(&chunk),
                Some(Err(error)) => state.fail(dto::llm_error(error)),
                None => state.end(),
            };
            stream::iter(frames)
        })
        .boxed()
}
