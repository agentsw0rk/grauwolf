//! DTOs of the Ollama API `/api/chat`; only the fields grauwolf uses.

use grauwolf_llm::{LlmError, StreamFrame};
use grauwolf_prompt::{AssistantMessage, AssistantPart, AttachmentData, Message, ToolDescriptor};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize)]
pub(crate) struct ChatRequest<'a> {
    pub model: &'a str,
    pub stream: bool,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool<'a>>,
    /// JSON schema for structured responses (native strategy, ADR-0031).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<&'a Value>,
    /// Thinking of reasoning models on/off; if not given, the server decides.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub think: Option<bool>,
    /// Sampling options; only `temperature` is used (ADR-0097).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Options>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Options {
    pub temperature: f64,
}

impl Options {
    /// The options of a model, if it sets any.
    pub fn of(temperature: Option<f64>) -> Option<Self> {
        temperature.map(|temperature| Self { temperature })
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct Tool<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: ToolFunction<'a>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ToolFunction<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub parameters: &'a Value,
}

impl<'a> From<&'a ToolDescriptor> for Tool<'a> {
    fn from(tool: &'a ToolDescriptor) -> Self {
        Self {
            kind: "function",
            function: ToolFunction {
                name: &tool.name,
                description: &tool.description,
                parameters: &tool.parameters,
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// For `role: tool`: name of the tool whose result the message contains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Images as base64 on a user message (ADR-0081).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}

/// Rejects attachments that Ollama does not understand, before every request
/// (ADR-0081): only images, and only as base64.
pub(crate) fn check_attachments(prompt: &grauwolf_prompt::Prompt) -> Result<(), LlmError> {
    for message in &prompt.messages {
        let Message::User { attachments, .. } = message else {
            continue;
        };
        for attachment in attachments {
            let reason = match &attachment.data {
                _ if !attachment.is_image() => format!(
                    "ollama: attachments of type {} are not supported (possible: images)",
                    attachment.media_type
                ),
                AttachmentData::Url(_) => {
                    "ollama: images only work as a file (Base64), not as a URL".to_owned()
                }
                AttachmentData::Base64(_) => continue,
            };
            return Err(LlmError::Client(reason.into()));
        }
    }
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ToolCall {
    /// Newer Ollama versions provide an ID, older ones do not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub function: ToolCallFunction,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ToolCallFunction {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

impl From<&Message> for ChatMessage {
    /// Ollama matches tool results via `tool_name`; the ID is not sent.
    fn from(message: &Message) -> Self {
        let text = |role: &str, content: &str| Self {
            role: role.into(),
            content: content.into(),
            tool_calls: Vec::new(),
            tool_name: None,
            images: Vec::new(),
        };
        match message {
            Message::System { content } => text("system", content),
            Message::User {
                content,
                attachments,
            } => Self {
                images: attachments
                    .iter()
                    .filter_map(|attachment| match &attachment.data {
                        AttachmentData::Base64(data) if attachment.is_image() => Some(data.clone()),
                        _ => None,
                    })
                    .collect(),
                ..text("user", content)
            },
            // Reasoning of other providers is not sent (ADR-0032).
            Message::Assistant(answer) => Self {
                tool_calls: answer
                    .tool_calls()
                    .map(|call| ToolCall {
                        id: None,
                        function: ToolCallFunction {
                            name: call.tool.clone(),
                            arguments: call.args.clone(),
                        },
                    })
                    .collect(),
                ..text("assistant", &answer.text())
            },
            Message::ToolResult(result) => Self {
                tool_name: Some(result.tool.clone()),
                ..text("tool", &result.content)
            },
        }
    }
}

impl ChatMessage {
    /// Model response as a grauwolf message: text (omitted if empty and there are
    /// tool calls) and all tool calls; without an ID, the tool name serves as the ID.
    pub fn into_message(self) -> Message {
        let calls = self.tool_calls.into_iter().map(|call| {
            AssistantPart::ToolCall(grauwolf_prompt::ToolCall {
                id: call.id.unwrap_or_else(|| call.function.name.clone()),
                tool: call.function.name,
                args: call.function.arguments,
            })
        });
        Message::Assistant(AssistantMessage {
            parts: text_and_calls(self.content, calls.collect()),
        })
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ChatResponse {
    #[serde(default)]
    pub model: Option<String>,
    pub message: ChatMessage,
    /// Input tokens (ADR-0067); missing e.g. when Ollama takes the prompt from the cache.
    #[serde(default)]
    pub prompt_eval_count: Option<u64>,
    /// Erzeugte Tokens.
    #[serde(default)]
    pub eval_count: Option<u64>,
}

impl ChatResponse {
    /// Usage, if Ollama reports at least one of the numbers; without cache details.
    pub fn usage(&self) -> Option<grauwolf_prompt::Usage> {
        usage_of(self.prompt_eval_count, self.eval_count)
    }
}

/// Usage from Ollama's counters, if at least one is reported (ADR-0067).
fn usage_of(prompt: Option<u64>, eval: Option<u64>) -> Option<grauwolf_prompt::Usage> {
    (prompt.is_some() || eval.is_some()).then(|| grauwolf_prompt::Usage {
        input_tokens: prompt.unwrap_or(0),
        output_tokens: eval.unwrap_or(0),
        cache_read_tokens: None,
        cache_write_tokens: None,
    })
}

/// Parts of a response: the text, unless it is empty or there are tool calls, then the calls.
fn text_and_calls(text: String, calls: Vec<AssistantPart>) -> Vec<AssistantPart> {
    let text = (!text.is_empty() || calls.is_empty()).then_some(AssistantPart::Text { text });
    text.into_iter().chain(calls).collect()
}

/// A line of the streamed response.
#[derive(Debug, Deserialize)]
pub(crate) struct ChatChunk {
    /// The model that answered, reported on every line (ADR-0094).
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub message: Option<ChatMessage>,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub done_reason: Option<String>,
    /// Counters of the last line (ADR-0067).
    #[serde(default)]
    pub prompt_eval_count: Option<u64>,
    #[serde(default)]
    pub eval_count: Option<u64>,
}

/// Frames for an NDJSON line: text delta, tool calls and, at the end, `End`.
pub(crate) fn frames_of_chunk(line: &str) -> Vec<Result<StreamFrame, LlmError>> {
    let chunk: ChatChunk = match serde_json::from_str(line) {
        Ok(chunk) => chunk,
        Err(error) => {
            return vec![Err(LlmError::Client(
                format!("invalid stream line from Ollama: {error}").into(),
            ))];
        }
    };
    let mut frames = Vec::new();
    if let Some(message) = chunk.message {
        if !message.content.is_empty() {
            frames.push(Ok(StreamFrame::TextDelta {
                text: message.content,
            }));
        }
        frames.extend(message.tool_calls.into_iter().map(|call| {
            Ok(StreamFrame::ToolCall(grauwolf_prompt::ToolCall {
                id: call.id.unwrap_or_else(|| call.function.name.clone()),
                tool: call.function.name,
                args: call.function.arguments,
            }))
        }));
    }
    if chunk.done {
        frames.extend(chunk.model.map(|model| Ok(StreamFrame::Model(model))));
        if let Some(usage) = usage_of(chunk.prompt_eval_count, chunk.eval_count) {
            frames.push(Ok(StreamFrame::Usage(usage)));
        }
        frames.push(Ok(StreamFrame::End {
            finish_reason: chunk.done_reason,
        }));
    }
    frames
}
