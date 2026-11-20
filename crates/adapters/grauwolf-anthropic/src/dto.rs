//! DTOs of the Messages API; only the fields grauwolf uses.

use futures_util::stream::{self, BoxStream, StreamExt};
use grauwolf_llm::{LlmError, StreamFrame};
use grauwolf_llm_http::{HttpError, SseEvent};
use grauwolf_prompt::{
    AssistantMessage, AssistantPart, Attachment, AttachmentData, Message, Prompt, Reasoning,
    ToolCall, ToolChoice, ToolDescriptor,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Serialize)]
pub(crate) struct MessagesRequest<'a> {
    pub model: &'a str,
    pub max_tokens: u32,
    /// Sampling temperature (ADR-0097); the provider default if not given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<[SystemBlock; 1]>,
    pub messages: Vec<InputMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool<'a>>,
    /// Tool choice (ADR-0042); only with tools and not for `auto`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// Automatic prompt cache: breakpoint on the last block (ADR-0062).
    pub cache_control: CacheControl,
}

/// System prompt as a text block; its breakpoint caches tools and system prompt together,
/// independent of the conversation (ADR-0062).
#[derive(Debug, Serialize)]
pub(crate) struct SystemBlock {
    #[serde(rename = "type")]
    kind: &'static str,
    text: String,
    cache_control: CacheControl,
}

/// Cache breakpoint with a 5-minute lifetime (ADR-0062).
#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct CacheControl {
    #[serde(rename = "type")]
    kind: &'static str,
}

impl CacheControl {
    pub const EPHEMERAL: Self = Self { kind: "ephemeral" };
}

/// Structured response according to a JSON schema (native strategy, ADR-0031).
#[derive(Debug, Serialize)]
pub(crate) struct OutputConfig {
    pub format: OutputFormat,
}

#[derive(Debug, Serialize)]
pub(crate) struct OutputFormat {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub schema: Value,
}

impl OutputConfig {
    /// The API requires `additionalProperties: false` on every object; this does not change the
    /// meaning for grauwolf, because unknown fields are dropped anyway (ADR-0031).
    pub fn json_schema(schema: &Value) -> Self {
        Self {
            format: OutputFormat {
                kind: "json_schema",
                schema: close_objects(schema),
            },
        }
    }
}

/// Copy of `schema` in which every object schema carries `additionalProperties: false`.
fn close_objects(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            let mut closed: serde_json::Map<String, Value> = map
                .iter()
                .map(|(key, value)| (key.clone(), close_objects(value)))
                .collect();
            if map.get("type").and_then(Value::as_str) == Some("object") {
                closed.insert("additionalProperties".into(), Value::Bool(false));
            }
            Value::Object(closed)
        }
        Value::Array(items) => Value::Array(items.iter().map(close_objects).collect()),
        other => other.clone(),
    }
}

impl<'a> MessagesRequest<'a> {
    /// Leading system messages become the `system` field; later ones remain as
    /// system messages in the history.
    pub fn new(
        model: &'a str,
        max_tokens: u32,
        prompt: &Prompt,
        tools: &'a [ToolDescriptor],
    ) -> Self {
        let leading = prompt
            .messages
            .iter()
            .take_while(|message| matches!(message, Message::System { .. }))
            .count();
        let system: Vec<&str> = prompt.messages[..leading]
            .iter()
            .filter_map(|message| match message {
                Message::System { content } => Some(content.as_str()),
                _ => None,
            })
            .collect();
        Self {
            model,
            max_tokens,
            temperature: None,
            system: (!system.is_empty()).then(|| {
                [SystemBlock {
                    kind: "text",
                    text: system.join("\n\n"),
                    cache_control: CacheControl::EPHEMERAL,
                }]
            }),
            messages: group_tool_results(prompt.messages[leading..].iter().map(InputMessage::from)),
            tools: tools.iter().map(Tool::from).collect(),
            tool_choice: tool_choice(&prompt.params.tool_choice, !tools.is_empty()),
            output_config: None,
            stream: None,
            cache_control: CacheControl::EPHEMERAL,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct Tool<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub input_schema: &'a Value,
}

impl<'a> From<&'a ToolDescriptor> for Tool<'a> {
    fn from(tool: &'a ToolDescriptor) -> Self {
        Self {
            name: &tool.name,
            description: &tool.description,
            input_schema: &tool.parameters,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct InputMessage {
    pub role: &'static str,
    pub content: InputContent,
}

/// Text as a string, tool calls and results as content blocks.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum InputContent {
    Text(String),
    Blocks(Vec<InputBlock>),
}

/// Known blocks or reasoning blocks that are returned unchanged (ADR-0032).
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum InputBlock {
    Known(KnownBlock),
    Raw(Value),
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum KnownBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
    /// Image as base64 or URL (ADR-0081).
    Image {
        source: Value,
    },
    /// PDF as base64 or URL (ADR-0081).
    Document {
        source: Value,
    },
}

/// Block for an attachment (ADR-0081): images and PDFs, each as base64 or URL; otherwise the
/// reason why it is not possible.
fn attachment_block(attachment: &Attachment) -> Result<KnownBlock, String> {
    let source = match &attachment.data {
        AttachmentData::Base64(data) => {
            json!({ "type": "base64", "media_type": attachment.media_type, "data": data })
        }
        AttachmentData::Url(url) => json!({ "type": "url", "url": url }),
    };
    if attachment.is_image() {
        Ok(KnownBlock::Image { source })
    } else if attachment.is_pdf() {
        Ok(KnownBlock::Document { source })
    } else {
        Err(format!(
            "anthropic: attachments of type {} are not supported (possible: images, PDF)",
            attachment.media_type
        ))
    }
}

/// Rejects attachments that Anthropic does not understand, before every request (ADR-0081).
pub(crate) fn check_attachments(prompt: &Prompt) -> Result<(), LlmError> {
    for message in &prompt.messages {
        if let Message::User { attachments, .. } = message {
            for attachment in attachments {
                attachment_block(attachment).map_err(|reason| LlmError::Client(reason.into()))?;
            }
        }
    }
    Ok(())
}

impl From<&Message> for InputMessage {
    fn from(message: &Message) -> Self {
        let text = |role, content: &str| Self {
            role,
            content: InputContent::Text(content.into()),
        };
        match message {
            Message::System { content } => text("system", content),
            Message::User {
                content,
                attachments,
            } if !attachments.is_empty() => Self {
                role: "user",
                content: InputContent::Blocks(
                    attachments
                        .iter()
                        .filter_map(|attachment| attachment_block(attachment).ok())
                        .chain([KnownBlock::Text {
                            text: content.clone(),
                        }])
                        .map(InputBlock::Known)
                        .collect(),
                ),
            },
            Message::User { content, .. } => text("user", content),
            Message::Assistant(answer) => match answer.parts.as_slice() {
                [AssistantPart::Text { text: content }] => text("assistant", content),
                parts => Self {
                    role: "assistant",
                    content: InputContent::Blocks(
                        parts
                            .iter()
                            .filter_map(|part| match part {
                                AssistantPart::Text { text } => {
                                    Some(InputBlock::Known(KnownBlock::Text { text: text.clone() }))
                                }
                                AssistantPart::ToolCall(call) => {
                                    Some(InputBlock::Known(KnownBlock::ToolUse {
                                        id: call.id.clone(),
                                        name: call.tool.clone(),
                                        input: call.args.clone(),
                                    }))
                                }
                                // Return our own thinking blocks unchanged; omit foreign reasoning.
                                AssistantPart::Reasoning(reasoning) => (reasoning.provider
                                    == PROVIDER)
                                    .then(|| InputBlock::Raw(reasoning.data.clone())),
                            })
                            .collect(),
                    ),
                },
            },
            Message::ToolResult(result) => Self {
                role: "user",
                content: InputContent::Blocks(vec![InputBlock::Known(KnownBlock::ToolResult {
                    tool_use_id: result.id.clone(),
                    content: result.content.clone(),
                })]),
            },
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct MessagesResponse {
    #[serde(default)]
    pub model: Option<String>,
    /// Read raw, so that thinking blocks stay unchanged.
    pub content: Vec<Value>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    /// Only filled for `stop_reason: refusal`.
    #[serde(default)]
    pub stop_details: Option<StopDetails>,
    #[serde(default)]
    pub usage: Option<ResponseUsage>,
}

/// Token counts of the response; `input_tokens` is only the uncached remainder.
#[derive(Debug, Clone, Copy, Deserialize)]
pub(crate) struct ResponseUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
}

impl ResponseUsage {
    /// Usage with all input tokens in `input_tokens` (ADR-0067).
    pub fn usage(self) -> grauwolf_prompt::Usage {
        grauwolf_prompt::Usage {
            input_tokens: self.input_tokens
                + self.cache_creation_input_tokens.unwrap_or(0)
                + self.cache_read_input_tokens.unwrap_or(0),
            output_tokens: self.output_tokens,
            cache_read_tokens: self.cache_read_input_tokens,
            cache_write_tokens: self.cache_creation_input_tokens,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct StopDetails {
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub explanation: Option<String>,
}

impl MessagesResponse {
    /// Response as a grauwolf message with all blocks in order (ADR-0032).
    /// A refusal by the model is an error.
    pub fn into_message(self) -> Result<Message, LlmError> {
        if self.stop_reason.as_deref() == Some("refusal") {
            let details = self.stop_details.unwrap_or(StopDetails {
                category: None,
                explanation: None,
            });
            return Err(refusal(
                details.category.as_deref(),
                details.explanation.as_deref(),
            ));
        }
        let parts = self.content.into_iter().filter_map(output_part).collect();
        Ok(Message::Assistant(AssistantMessage { parts }))
    }
}

/// Provider tag for own reasoning.
pub(crate) const PROVIDER: &str = "anthropic";

/// Part of a response block; unknown block kinds are dropped.
fn output_part(block: Value) -> Option<AssistantPart> {
    match block.get("type").and_then(Value::as_str)? {
        "text" => Some(AssistantPart::Text {
            text: block.get("text")?.as_str()?.to_owned(),
        }),
        "tool_use" => Some(AssistantPart::ToolCall(ToolCall {
            id: block.get("id")?.as_str()?.to_owned(),
            tool: block.get("name")?.as_str()?.to_owned(),
            args: block.get("input").cloned().unwrap_or(Value::Null),
        })),
        "thinking" | "redacted_thinking" => Some(AssistantPart::Reasoning(Reasoning {
            provider: PROVIDER.into(),
            data: block,
        })),
        _ => None,
    }
}

/// Merges consecutive tool results into one user message: the API
/// expects all results of parallel tool calls in the same message.
fn group_tool_results(messages: impl Iterator<Item = InputMessage>) -> Vec<InputMessage> {
    let mut grouped: Vec<InputMessage> = Vec::new();
    for message in messages {
        let is_result = is_tool_result(&message);
        match grouped.last_mut() {
            Some(previous) if is_result && is_tool_result(previous) => {
                if let (InputContent::Blocks(blocks), InputContent::Blocks(more)) =
                    (&mut previous.content, message.content)
                {
                    blocks.extend(more);
                }
            }
            _ => grouped.push(message),
        }
    }
    grouped
}

fn is_tool_result(message: &InputMessage) -> bool {
    matches!(&message.content, InputContent::Blocks(blocks)
        if blocks.iter().all(|block| matches!(block, InputBlock::Known(KnownBlock::ToolResult { .. }))))
}

/// A content block whose deltas are still being collected.
#[derive(Debug)]
enum PartialBlock {
    /// Text is emitted immediately as a delta; nothing to collect here.
    Text,
    ToolUse {
        id: String,
        name: String,
        json: String,
    },
    /// Thinking block (or other reasoning block) in the response format, without streaming.
    Reasoning(Value),
    Other,
}

/// State while reading the stream: open blocks by `index`, reason for the end.
#[derive(Debug, Default)]
struct StreamState {
    blocks: std::collections::BTreeMap<u64, PartialBlock>,
    stop_reason: Option<String>,
    stop_details: Option<Value>,
    finished: bool,
    /// Input from `message_start`, output last from `message_delta` (ADR-0067).
    usage: Option<ResponseUsage>,
}

impl StreamState {
    fn frames(&mut self, event: &SseEvent) -> Vec<Result<StreamFrame, LlmError>> {
        let data: Value = match serde_json::from_str(&event.data) {
            Ok(data) => data,
            Err(error) => {
                return vec![Err(LlmError::Client(
                    format!("invalid stream event from Anthropic: {error}").into(),
                ))];
            }
        };
        let index = data
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        match data.get("type").and_then(Value::as_str).unwrap_or_default() {
            "content_block_start" => {
                let block = data.get("content_block").cloned().unwrap_or(Value::Null);
                let partial = match block.get("type").and_then(Value::as_str) {
                    Some("text") => PartialBlock::Text,
                    Some("tool_use") => PartialBlock::ToolUse {
                        id: string(&block, "id"),
                        name: string(&block, "name"),
                        json: String::new(),
                    },
                    Some("thinking" | "redacted_thinking") => PartialBlock::Reasoning(block),
                    _ => PartialBlock::Other,
                };
                self.blocks.insert(index, partial);
                Vec::new()
            }
            "content_block_delta" => {
                let delta = data.get("delta").cloned().unwrap_or(Value::Null);
                match (
                    delta.get("type").and_then(Value::as_str),
                    self.blocks.get_mut(&index),
                ) {
                    (Some("text_delta"), _) => vec![Ok(StreamFrame::TextDelta {
                        text: string(&delta, "text"),
                    })],
                    (Some("input_json_delta"), Some(PartialBlock::ToolUse { json, .. })) => {
                        json.push_str(&string(&delta, "partial_json"));
                        Vec::new()
                    }
                    (Some("thinking_delta"), Some(PartialBlock::Reasoning(block))) => {
                        append_field(block, "thinking", &string(&delta, "thinking"));
                        Vec::new()
                    }
                    (Some("signature_delta"), Some(PartialBlock::Reasoning(block))) => {
                        append_field(block, "signature", &string(&delta, "signature"));
                        Vec::new()
                    }
                    _ => Vec::new(),
                }
            }
            "content_block_stop" => match self.blocks.remove(&index) {
                Some(PartialBlock::ToolUse { id, name, json }) => {
                    let args = if json.trim().is_empty() {
                        Ok(Value::Object(Default::default()))
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
                                format!(
                                    "arguments of the tool call '{name}' are not JSON: {error}"
                                )
                                .into(),
                            )
                        }),
                    ]
                }
                Some(PartialBlock::Reasoning(block)) => {
                    vec![Ok(StreamFrame::Reasoning(Reasoning {
                        provider: PROVIDER.into(),
                        data: block,
                    }))]
                }
                _ => Vec::new(),
            },
            "message_start" => {
                self.usage = data
                    .pointer("/message/usage")
                    .and_then(|usage| serde_json::from_value(usage.clone()).ok());
                // The model the provider reports (ADR-0094).
                data.pointer("/message/model")
                    .and_then(Value::as_str)
                    .map(|model| Ok(StreamFrame::Model(model.to_owned())))
                    .into_iter()
                    .collect()
            }
            "message_delta" => {
                if let (Some(usage), Some(output)) = (
                    self.usage.as_mut(),
                    data.pointer("/usage/output_tokens").and_then(Value::as_u64),
                ) {
                    usage.output_tokens = output;
                }
                let delta = data.get("delta").cloned().unwrap_or(Value::Null);
                if let Some(reason) = delta.get("stop_reason").and_then(Value::as_str) {
                    self.stop_reason = Some(reason.to_owned());
                }
                if let Some(details) = delta.get("stop_details").filter(|value| !value.is_null()) {
                    self.stop_details = Some(details.clone());
                }
                Vec::new()
            }
            "message_stop" => self.finish(),
            "error" => {
                self.finished = true;
                let error = data.get("error").cloned().unwrap_or(Value::Null);
                let message = format!(
                    "Anthropic reports in the stream {}: {}",
                    string(&error, "type"),
                    string(&error, "message")
                );
                let transient = matches!(
                    error.get("type").and_then(Value::as_str),
                    Some("overloaded_error" | "rate_limit_error" | "api_error")
                );
                vec![Err(if transient {
                    LlmError::Transient {
                        source: message.into(),
                        retry_after: None,
                    }
                } else {
                    LlmError::Client(message.into())
                })]
            }
            _ => Vec::new(),
        }
    }

    /// Finishes the response: a refusal becomes an error, otherwise `End` (once).
    fn finish(&mut self) -> Vec<Result<StreamFrame, LlmError>> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        if self.stop_reason.as_deref() == Some("refusal") {
            let details = self.stop_details.take().unwrap_or(Value::Null);
            return vec![Err(refusal(
                details.get("category").and_then(Value::as_str),
                details.get("explanation").and_then(Value::as_str),
            ))];
        }
        self.usage
            .take()
            .map(|usage| Ok(StreamFrame::Usage(usage.usage())))
            .into_iter()
            .chain([Ok(StreamFrame::End {
                finish_reason: self.stop_reason.take(),
            })])
            .collect()
    }
}

fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn append_field(block: &mut Value, key: &str, addition: &str) {
    if let Value::Object(map) = block {
        let current = map.get(key).and_then(Value::as_str).unwrap_or_default();
        let joined = format!("{current}{addition}");
        map.insert(key.to_owned(), Value::String(joined));
    }
}

/// Translates the Messages API SSE events into frames (ADR-0033).
pub(crate) fn frames_of_events(
    events: BoxStream<'static, Result<SseEvent, HttpError>>,
) -> BoxStream<'static, Result<StreamFrame, LlmError>> {
    let state = std::sync::Arc::new(std::sync::Mutex::new(StreamState::default()));
    events
        .flat_map(move |event| {
            let frames = match event {
                Ok(event) => state.lock().expect("stream state").frames(&event),
                Err(error) => vec![Err(LlmError::from(error))],
            };
            stream::iter(frames)
        })
        .boxed()
}

/// Error for a refusal by the model (`stop_reason: refusal`).
fn refusal(category: Option<&str>, explanation: Option<&str>) -> LlmError {
    LlmError::Client(
        format!(
            "request refused by the model (category: {}): {}",
            category.unwrap_or("unknown"),
            explanation.unwrap_or("no reason given")
        )
        .into(),
    )
}

/// `tool_choice` of the Messages API; `None` without tools or for `Auto`.
fn tool_choice(choice: &ToolChoice, has_tools: bool) -> Option<Value> {
    if !has_tools {
        return None;
    }
    match choice {
        ToolChoice::Auto => None,
        ToolChoice::Required => Some(serde_json::json!({ "type": "any" })),
        ToolChoice::None => Some(serde_json::json!({ "type": "none" })),
        ToolChoice::Tool(name) => Some(serde_json::json!({ "type": "tool", "name": name })),
    }
}
