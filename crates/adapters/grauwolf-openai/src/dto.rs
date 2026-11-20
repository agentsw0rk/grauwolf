//! DTOs of the Chat Completions API; only the fields grauwolf uses.

use futures_util::stream::{self, BoxStream, StreamExt};
use grauwolf_llm::{LlmError, StreamFrame};
use grauwolf_llm_http::{HttpError, SseEvent};
use grauwolf_prompt::{
    AssistantMessage, AssistantPart, Attachment, AttachmentData, Message, ToolChoice,
    ToolDescriptor,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Serialize)]
pub(crate) struct ChatRequest<'a> {
    pub model: &'a str,
    /// Sampling temperature (ADR-0097); the provider default if not given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool<'a>>,
    /// Tool choice (ADR-0042); only with tools and not for `auto`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    /// Number of response variants (ADR-0042); one if not given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<usize>,
    /// Structured response according to a JSON schema (native strategy, ADR-0031).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// When streaming, `{ include_usage: true }`: usage in a last chunk (ADR-0067).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<Value>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ResponseFormat<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub json_schema: JsonSchemaFormat<'a>,
}

/// Without `strict`: strict mode requires `additionalProperties: false` and only
/// required fields, which schemas from specs do not guarantee.
#[derive(Debug, Serialize)]
pub(crate) struct JsonSchemaFormat<'a> {
    pub name: &'a str,
    pub schema: &'a Value,
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
    /// `null` for the assistant's tool calls; with attachments, a list of parts (ADR-0081).
    pub content: Option<Content>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// For `role: tool`: ID of the call whose result the message contains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ToolCallFunction {
    pub name: String,
    /// Arguments as JSON text (not as an object).
    pub arguments: String,
}

/// Content of a message: text or parts (text, image, audio, file).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum Content {
    Text(String),
    Parts(Vec<Value>),
}

impl Content {
    /// Text of the content; for parts, the text parts one after another.
    pub(crate) fn into_text(self) -> String {
        match self {
            Content::Text(text) => text,
            Content::Parts(parts) => parts
                .iter()
                .filter_map(|part| part["text"].as_str())
                .collect(),
        }
    }
}

/// Part for an attachment (ADR-0081): images as URL or data URL, audio `wav`/`mp3` and PDF
/// as base64; otherwise the reason why it is not possible.
fn attachment_part(attachment: &Attachment) -> Result<Value, String> {
    let base64 = || match &attachment.data {
        AttachmentData::Base64(data) => Ok(data.clone()),
        AttachmentData::Url(_) => Err(format!(
            "openai: attachments of type {} only work as a file (Base64), not as a URL",
            attachment.media_type
        )),
    };
    if attachment.is_image() {
        return Ok(json!({ "type": "image_url", "image_url": { "url": attachment.to_url() } }));
    }
    if attachment.is_pdf() {
        base64()?;
        return Ok(
            json!({ "type": "file", "file": { "filename": "anhang.pdf", "file_data": attachment.to_url() } }),
        );
    }
    let format = match attachment.media_type.as_str() {
        "audio/wav" | "audio/x-wav" | "audio/wave" => Some("wav"),
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        _ => None,
    };
    match format {
        Some(format) => Ok(
            json!({ "type": "input_audio", "input_audio": { "data": base64()?, "format": format } }),
        ),
        None => Err(format!(
            "openai: attachments of type {} are not supported (possible: images, audio wav/mp3, PDF)",
            attachment.media_type
        )),
    }
}

/// Messages of a request; attachments that OpenAI does not understand are rejected beforehand.
pub(crate) fn messages(prompt: &grauwolf_prompt::Prompt) -> Result<Vec<ChatMessage>, LlmError> {
    for message in &prompt.messages {
        if let Message::User { attachments, .. } = message {
            for attachment in attachments {
                attachment_part(attachment).map_err(|reason| LlmError::Client(reason.into()))?;
            }
        }
    }
    Ok(prompt.messages.iter().map(ChatMessage::from).collect())
}

impl From<&Message> for ChatMessage {
    fn from(message: &Message) -> Self {
        let text = |role: &str, content: &str| Self {
            role: role.into(),
            content: Some(Content::Text(content.into())),
            tool_calls: Vec::new(),
            tool_call_id: None,
        };
        match message {
            Message::System { content } => text("system", content),
            Message::User {
                content,
                attachments,
            } if !attachments.is_empty() => Self {
                content: Some(Content::Parts(
                    std::iter::once(json!({ "type": "text", "text": content }))
                        .chain(
                            attachments
                                .iter()
                                .filter_map(|attachment| attachment_part(attachment).ok()),
                        )
                        .collect(),
                )),
                ..text("user", "")
            },
            Message::User { content, .. } => text("user", content),
            // Reasoning of other providers is not sent (ADR-0032).
            Message::Assistant(answer) => {
                let tool_calls: Vec<ToolCall> = answer
                    .tool_calls()
                    .map(|call| ToolCall {
                        id: call.id.clone(),
                        kind: "function".into(),
                        function: ToolCallFunction {
                            name: call.tool.clone(),
                            arguments: call.args.to_string(),
                        },
                    })
                    .collect();
                let content = answer.text();
                Self {
                    // For pure tool calls the API expects `null` instead of empty text.
                    content: (!(content.is_empty() && !tool_calls.is_empty()))
                        .then_some(Content::Text(content)),
                    tool_calls,
                    ..text("assistant", "")
                }
            }
            Message::ToolResult(result) => Self {
                tool_call_id: Some(result.id.clone()),
                ..text("tool", &result.content)
            },
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ChatResponse {
    #[serde(default)]
    pub model: Option<String>,
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub usage: Option<ResponseUsage>,
}

/// Token counts of the response; `prompt_tokens` already includes cached tokens.
#[derive(Debug, Clone, Copy, Deserialize)]
pub(crate) struct ResponseUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    prompt_tokens_details: Option<PromptTokensDetails>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct PromptTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u64>,
}

impl ResponseUsage {
    /// Usage per ADR-0067; OpenAI does not report cache writes.
    pub fn usage(self) -> grauwolf_prompt::Usage {
        grauwolf_prompt::Usage {
            input_tokens: self.prompt_tokens,
            output_tokens: self.completion_tokens,
            cache_read_tokens: self
                .prompt_tokens_details
                .and_then(|details| details.cached_tokens),
            cache_write_tokens: None,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct Choice {
    pub message: ChatMessage,
}

impl ChatResponse {
    /// First response variant as a grauwolf message.
    pub fn into_message(self) -> Result<Message, LlmError> {
        let choice = self
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| LlmError::Client("answer without choices".into()))?;
        assistant_message(choice.message)
    }

    /// All response variants (`n`, ADR-0042).
    pub fn into_messages(self) -> Result<Vec<Message>, LlmError> {
        self.choices
            .into_iter()
            .map(|choice| assistant_message(choice.message))
            .collect()
    }
}

/// Response as a grauwolf message: text (omitted if empty and there are tool calls) and all
/// Tool calls.
fn assistant_message(message: ChatMessage) -> Result<Message, LlmError> {
    let calls = message
        .tool_calls
        .into_iter()
        .map(|call| {
            let args = serde_json::from_str(&call.function.arguments).map_err(|error| {
                LlmError::Client(
                    format!(
                        "arguments of the tool call '{}' are not JSON: {error}",
                        call.function.name
                    )
                    .into(),
                )
            })?;
            Ok(AssistantPart::ToolCall(grauwolf_prompt::ToolCall {
                id: call.id,
                tool: call.function.name,
                args,
            }))
        })
        .collect::<Result<Vec<_>, LlmError>>()?;
    let text = message.content.map(Content::into_text).unwrap_or_default();
    let text = (!text.is_empty() || calls.is_empty()).then_some(AssistantPart::Text { text });
    Ok(Message::Assistant(AssistantMessage {
        parts: text.into_iter().chain(calls).collect(),
    }))
}

/// A chunk of the streamed response.
#[derive(Debug, Deserialize)]
struct StreamChunk {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    choices: Vec<StreamChoice>,
    #[serde(default)]
    usage: Option<ResponseUsage>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCallFragment>,
}

#[derive(Debug, Deserialize)]
struct ToolCallFragment {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<FunctionFragment>,
}

#[derive(Debug, Deserialize)]
struct FunctionFragment {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

/// A tool call whose fragments are still being collected.
#[derive(Debug, Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
}

/// State while reading the stream: tool calls by `index`, reason for the end.
#[derive(Debug, Default)]
struct StreamState {
    calls: std::collections::BTreeMap<usize, PartialCall>,
    finish_reason: Option<String>,
    finished: bool,
    usage: Option<ResponseUsage>,
    /// The reported model (ADR-0094) is emitted once, with the first chunk that names it.
    model_sent: bool,
}

impl StreamState {
    /// Frames for one event; on `[DONE]`, the collected tool calls and `End`.
    fn frames(&mut self, data: &str) -> Vec<Result<StreamFrame, LlmError>> {
        if data == "[DONE]" {
            return self.finish();
        }
        let chunk: StreamChunk = match serde_json::from_str(data) {
            Ok(chunk) => chunk,
            Err(error) => {
                return vec![Err(LlmError::Client(
                    format!("invalid stream event from OpenAI: {error}").into(),
                ))];
            }
        };
        if chunk.usage.is_some() {
            self.usage = chunk.usage;
        }
        let mut frames = Vec::new();
        if let Some(model) = chunk.model.filter(|_| !self.model_sent) {
            self.model_sent = true;
            frames.push(Ok(StreamFrame::Model(model)));
        }
        for choice in chunk.choices {
            if let Some(text) = choice.delta.content.filter(|text| !text.is_empty()) {
                frames.push(Ok(StreamFrame::TextDelta { text }));
            }
            for fragment in choice.delta.tool_calls {
                let call = self.calls.entry(fragment.index).or_default();
                if let Some(id) = fragment.id {
                    call.id = id;
                }
                if let Some(function) = fragment.function {
                    call.name.push_str(&function.name.unwrap_or_default());
                    call.arguments
                        .push_str(&function.arguments.unwrap_or_default());
                }
            }
            if choice.finish_reason.is_some() {
                self.finish_reason = choice.finish_reason;
            }
        }
        frames
    }

    /// Emits the collected tool calls and `End` (once).
    fn finish(&mut self) -> Vec<Result<StreamFrame, LlmError>> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        let mut frames: Vec<Result<StreamFrame, LlmError>> = std::mem::take(&mut self.calls)
            .into_values()
            .map(|call| {
                let args = if call.arguments.is_empty() {
                    Ok(Value::Object(Default::default()))
                } else {
                    serde_json::from_str(&call.arguments)
                };
                args.map(|args| {
                    StreamFrame::ToolCall(grauwolf_prompt::ToolCall {
                        id: call.id,
                        tool: call.name.clone(),
                        args,
                    })
                })
                .map_err(|error| {
                    LlmError::Client(
                        format!(
                            "arguments of the tool call '{}' are not JSON: {error}",
                            call.name
                        )
                        .into(),
                    )
                })
            })
            .collect();
        if let Some(usage) = self.usage.take() {
            frames.push(Ok(StreamFrame::Usage(usage.usage())));
        }
        frames.push(Ok(StreamFrame::End {
            finish_reason: self.finish_reason.take(),
        }));
        frames
    }
}

/// Translates the SSE events into frames; if the stream ends without `[DONE]`, it still
/// ends the stream.
pub(crate) fn frames_of_events(
    events: BoxStream<'static, Result<SseEvent, HttpError>>,
) -> BoxStream<'static, Result<StreamFrame, LlmError>> {
    let state = std::sync::Arc::new(std::sync::Mutex::new(StreamState::default()));
    let closing = state.clone();
    events
        .flat_map(move |event| {
            let frames = match event {
                Ok(event) => state.lock().expect("stream state").frames(&event.data),
                Err(error) => vec![Err(LlmError::from(error))],
            };
            stream::iter(frames)
        })
        .chain(
            stream::once(async move { closing.lock().expect("stream state").finish() })
                .flat_map(stream::iter),
        )
        .boxed()
}

/// `tool_choice` of the Chat Completions API; `None` without tools or for `Auto`.
pub(crate) fn tool_choice(choice: &ToolChoice, has_tools: bool) -> Option<Value> {
    if !has_tools {
        return None;
    }
    match choice {
        ToolChoice::Auto => None,
        ToolChoice::Required => Some(Value::from("required")),
        ToolChoice::None => Some(Value::from("none")),
        ToolChoice::Tool(name) => Some(serde_json::json!({
            "type": "function",
            "function": { "name": name }
        })),
    }
}
