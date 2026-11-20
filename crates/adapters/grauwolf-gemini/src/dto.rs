//! Requests and responses of `generateContent` (ADR-0083).

use grauwolf_llm::LlmError;
use grauwolf_prompt::{
    AssistantMessage, AssistantPart, Attachment, AttachmentData, Message, Prompt, Reasoning,
    ToolCall, ToolChoice, ToolDescriptor, ToolResult,
};
use std::sync::{Arc, Mutex};

use futures_util::stream::{self, BoxStream, StreamExt};
use grauwolf_llm::StreamFrame;
use grauwolf_llm_http::{HttpError, SseEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Prefix of self-assigned IDs for function calls without `id`; they are not sent back to Gemini.
const GENERATED_ID_PREFIX: &str = "gemini-call-";

/// Provider name on our own reasoning.
const PROVIDER: &str = "gemini";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenerateRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<Content>,
    pub contents: Vec<Content>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tools>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_config: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<Value>,
}

impl GenerateRequest {
    /// The request with `generationConfig.temperature` set, if given (ADR-0097).
    pub fn with_temperature(mut self, temperature: Option<f64>) -> Self {
        if let Some(temperature) = temperature {
            let config = self.generation_config.get_or_insert_with(|| json!({}));
            config["temperature"] = json!(temperature);
        }
        self
    }
}

/// `generationConfig` for a response as JSON following `schema` (ADR-0031).
pub(crate) fn json_schema_config(schema: &Value) -> Value {
    json!({ "responseMimeType": "application/json", "responseJsonSchema": schema })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Tools {
    function_declarations: Vec<FunctionDeclaration>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FunctionDeclaration {
    name: String,
    description: String,
    parameters_json_schema: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Content {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default)]
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Part {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function_call: Option<FunctionCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function_response: Option<FunctionResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_data: Option<InlineData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_data: Option<FileData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InlineData {
    mime_type: String,
    data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileData {
    mime_type: String,
    file_uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FunctionCall {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    name: String,
    #[serde(default)]
    args: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FunctionResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    name: String,
    response: Value,
}

impl Part {
    fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ..Self::default()
        }
    }

    /// Attachment as `inlineData` (Base64) or `fileData` (URL); Gemini checks the type itself.
    fn attachment(attachment: &Attachment) -> Self {
        let mime_type = attachment.media_type.clone();
        match &attachment.data {
            AttachmentData::Base64(data) => Self {
                inline_data: Some(InlineData {
                    mime_type,
                    data: data.clone(),
                }),
                ..Self::default()
            },
            AttachmentData::Url(url) => Self {
                file_data: Some(FileData {
                    mime_type,
                    file_uri: url.clone(),
                }),
                ..Self::default()
            },
        }
    }

    fn function_call(call: &ToolCall) -> Self {
        Self {
            function_call: Some(FunctionCall {
                id: own_id(&call.id),
                name: call.tool.clone(),
                args: call.args.clone(),
            }),
            ..Self::default()
        }
    }

    fn function_response(result: &ToolResult) -> Self {
        Self {
            function_response: Some(FunctionResponse {
                id: own_id(&result.id),
                name: result.tool.clone(),
                response: json!({ "result": result.content }),
            }),
            ..Self::default()
        }
    }
}

/// The ID for Gemini – self-assigned ones are dropped.
fn own_id(id: &str) -> Option<String> {
    (!id.starts_with(GENERATED_ID_PREFIX)).then(|| id.to_owned())
}

impl Content {
    fn new(role: &str, parts: Vec<Part>) -> Self {
        Self {
            role: Some(role.to_owned()),
            parts,
        }
    }
}

impl GenerateRequest {
    pub fn new(prompt: &Prompt, tools: &[ToolDescriptor]) -> Self {
        let leading = prompt
            .messages
            .iter()
            .take_while(|message| matches!(message, Message::System { .. }))
            .count();
        let system_instruction = (leading > 0).then(|| Content {
            role: None,
            parts: prompt.messages[..leading]
                .iter()
                .filter_map(|message| match message {
                    Message::System { content } => Some(Part::text(content)),
                    _ => None,
                })
                .collect(),
        });
        let mut contents: Vec<Content> = Vec::new();
        for message in &prompt.messages[leading..] {
            let content = content_of(message);
            // Consecutive results (parallel calls) belong in one content.
            match (contents.last_mut(), message) {
                (Some(last), Message::ToolResult(_))
                    if last
                        .parts
                        .iter()
                        .all(|part| part.function_response.is_some()) =>
                {
                    last.parts.extend(content.parts)
                }
                _ => contents.push(content),
            }
        }
        let tools = if tools.is_empty() {
            Vec::new()
        } else {
            vec![Tools {
                function_declarations: tools
                    .iter()
                    .map(|tool| FunctionDeclaration {
                        name: tool.name.clone(),
                        description: tool.description.clone(),
                        parameters_json_schema: tool.parameters.clone(),
                    })
                    .collect(),
            }]
        };
        let tool_config = if tools.is_empty() {
            None
        } else {
            tool_config(&prompt.params.tool_choice)
        };
        Self {
            system_instruction,
            contents,
            tools,
            tool_config,
            generation_config: None,
        }
    }
}

/// `toolConfig` for tool choice (ADR-0042); `Auto` is the default and is not sent.
fn tool_config(choice: &ToolChoice) -> Option<Value> {
    let config = match choice {
        ToolChoice::Auto => return None,
        ToolChoice::Required => json!({ "mode": "ANY" }),
        ToolChoice::None => json!({ "mode": "NONE" }),
        ToolChoice::Tool(name) => json!({ "mode": "ANY", "allowedFunctionNames": [name] }),
    };
    Some(json!({ "functionCallingConfig": config }))
}

/// Content for a message of the history; later system messages become user text.
fn content_of(message: &Message) -> Content {
    match message {
        Message::System { content } => Content::new("user", vec![Part::text(content)]),
        Message::User {
            content,
            attachments,
        } => Content::new(
            "user",
            attachments
                .iter()
                .map(Part::attachment)
                .chain([Part::text(content)])
                .collect(),
        ),
        Message::Assistant(answer) => Content::new("model", model_parts(&answer.parts)),
        Message::ToolResult(result) => Content::new("user", vec![Part::function_response(result)]),
    }
}

/// Parts of our own answer: thoughts unchanged, signatures on the following part (ADR-0083);
/// reasoning from other providers is dropped.
fn model_parts(parts: &[AssistantPart]) -> Vec<Part> {
    let mut result = Vec::new();
    let mut signature = None;
    for part in parts {
        let mut part = match part {
            AssistantPart::Text { text } => Part::text(text),
            AssistantPart::ToolCall(call) => Part::function_call(call),
            AssistantPart::Reasoning(reasoning) if reasoning.provider == PROVIDER => {
                match signature_of(&reasoning.data) {
                    Some(own) => {
                        signature = Some(own);
                        continue;
                    }
                    None => match serde_json::from_value(reasoning.data.clone()) {
                        Ok(thought) => thought,
                        Err(_) => continue,
                    },
                }
            }
            AssistantPart::Reasoning(_) => continue,
        };
        if part.thought_signature.is_none() {
            part.thought_signature = signature.take();
        }
        result.push(part);
    }
    if let Some(signature) = signature {
        result.push(Part {
            thought_signature: Some(signature),
            ..Part::text("")
        });
    }
    result
}

/// The signature, if `data` carries only a signature.
fn signature_of(data: &Value) -> Option<String> {
    let object = data.as_object()?;
    if object.len() != 1 {
        return None;
    }
    Some(object.get("thoughtSignature")?.as_str()?.to_owned())
}

fn reasoning(data: Value) -> AssistantPart {
    AssistantPart::Reasoning(Reasoning {
        provider: PROVIDER.to_owned(),
        data,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenerateResponse {
    /// The model version that answered.
    #[serde(default)]
    pub model_version: Option<String>,
    #[serde(default)]
    candidates: Vec<Candidate>,
    #[serde(default)]
    pub usage_metadata: Option<UsageMetadata>,
    #[serde(default)]
    prompt_feedback: Option<PromptFeedback>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PromptFeedback {
    #[serde(default)]
    block_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageMetadata {
    #[serde(default)]
    prompt_token_count: u64,
    #[serde(default)]
    candidates_token_count: u64,
    #[serde(default)]
    thoughts_token_count: u64,
    #[serde(default)]
    cached_content_token_count: Option<u64>,
}

impl UsageMetadata {
    /// Usage with thoughts as output and the cache read (ADR-0067).
    pub fn usage(self) -> grauwolf_prompt::Usage {
        grauwolf_prompt::Usage {
            input_tokens: self.prompt_token_count,
            output_tokens: self.candidates_token_count + self.thoughts_token_count,
            cache_read_tokens: self.cached_content_token_count,
            cache_write_tokens: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    #[serde(default)]
    content: Option<Content>,
    #[serde(default)]
    finish_reason: Option<String>,
}

/// Reasons for which an empty answer ends regularly.
const REGULAR_FINISH: [&str; 2] = ["STOP", "MAX_TOKENS"];

fn blocked(what: &str, reason: &str) -> LlmError {
    LlmError::Client(format!("gemini: {what} blocked ({reason})").into())
}

/// Parts of a response chunk, finish reason and usage.
pub(crate) struct Chunk {
    pub parts: Vec<AssistantPart>,
    pub finish_reason: Option<String>,
    pub usage: Option<UsageMetadata>,
    /// `modelVersion` of the chunk (ADR-0094).
    pub model: Option<String>,
}

impl GenerateResponse {
    pub fn into_message(self) -> Result<Message, LlmError> {
        if self.candidates.is_empty() && self.prompt_feedback.is_none() {
            return Err(LlmError::Client("gemini: answer without candidates".into()));
        }
        let mut answer: Vec<AssistantPart> = Vec::new();
        for part in self.into_chunk(&mut 0)?.parts {
            match (answer.last_mut(), part) {
                (Some(AssistantPart::Text { text: current }), AssistantPart::Text { text }) => {
                    current.push_str(&text)
                }
                (_, part) => answer.push(part),
            }
        }
        Ok(Message::Assistant(AssistantMessage { parts: answer }))
    }

    /// Parts in order (text not merged); `calls` counts calls without `id`
    /// across all chunks of a response.
    pub fn into_chunk(self, calls: &mut usize) -> Result<Chunk, LlmError> {
        let model = self.model_version.clone();
        if let Some(reason) = self
            .prompt_feedback
            .and_then(|feedback| feedback.block_reason)
        {
            return Err(blocked("request", &reason));
        }
        let (parts, finish_reason) = match self.candidates.into_iter().next() {
            Some(candidate) => (
                candidate
                    .content
                    .map(|content| content.parts)
                    .unwrap_or_default(),
                candidate.finish_reason,
            ),
            None => (Vec::new(), None),
        };
        if parts.is_empty() {
            if let Some(reason) = finish_reason
                .as_deref()
                .filter(|reason| !REGULAR_FINISH.contains(reason))
            {
                return Err(blocked("answer", reason));
            }
        }
        let mut answer: Vec<AssistantPart> = Vec::new();
        for mut part in parts {
            if part.thought == Some(true) {
                answer.push(reasoning(serde_json::to_value(&part).unwrap_or_default()));
                continue;
            }
            if let Some(signature) = part.thought_signature.take() {
                answer.push(reasoning(json!({ "thoughtSignature": signature })));
            }
            if let Some(text) = part.text.filter(|text| !text.is_empty()) {
                answer.push(AssistantPart::Text { text });
            }
            if let Some(call) = part.function_call {
                answer.push(AssistantPart::ToolCall(ToolCall {
                    id: call
                        .id
                        .unwrap_or_else(|| format!("{GENERATED_ID_PREFIX}{calls}")),
                    tool: call.name,
                    args: call.args,
                }));
                *calls += 1;
            }
        }
        Ok(Chunk {
            parts: answer,
            finish_reason,
            usage: self.usage_metadata,
            model,
        })
    }
}

/// State of a stream: call counter, last usage and last finish reason.
#[derive(Default)]
struct StreamState {
    calls: usize,
    usage: Option<UsageMetadata>,
    finish_reason: Option<String>,
    failed: bool,
    model_sent: bool,
}

impl StreamState {
    fn frames(&mut self, event: Result<SseEvent, HttpError>) -> Vec<Result<StreamFrame, LlmError>> {
        if self.failed {
            return Vec::new();
        }
        let chunk = event
            .map_err(LlmError::from)
            .and_then(|event| {
                serde_json::from_str::<GenerateResponse>(&event.data)
                    .map_err(|error| LlmError::Client(Box::new(error)))
            })
            .and_then(|response| response.into_chunk(&mut self.calls));
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                self.failed = true;
                return vec![Err(error)];
            }
        };
        self.usage = chunk.usage.or(self.usage);
        self.finish_reason = chunk.finish_reason.or(self.finish_reason.take());
        // The reported model goes out once, with the first chunk that names it.
        let model = chunk.model.filter(|_| !self.model_sent);
        self.model_sent |= model.is_some();
        model
            .map(|model| Ok(StreamFrame::Model(model)))
            .into_iter()
            .chain(chunk.parts.into_iter().map(|part| {
                Ok(match part {
                    AssistantPart::Text { text } => StreamFrame::TextDelta { text },
                    AssistantPart::ToolCall(call) => StreamFrame::ToolCall(call),
                    AssistantPart::Reasoning(reasoning) => StreamFrame::Reasoning(reasoning),
                })
            }))
            .collect()
    }

    /// Usage and end, if no error occurred.
    fn end(&mut self) -> Vec<Result<StreamFrame, LlmError>> {
        if self.failed {
            return Vec::new();
        }
        let usage = self
            .usage
            .map(|usage| Ok(StreamFrame::Usage(usage.usage())));
        usage
            .into_iter()
            .chain([Ok(StreamFrame::End {
                finish_reason: self.finish_reason.take(),
            })])
            .collect()
    }
}

/// Frames for the server-sent events of `streamGenerateContent` (ADR-0033).
pub(crate) fn frames_of_events(
    events: BoxStream<'static, Result<SseEvent, HttpError>>,
) -> BoxStream<'static, Result<StreamFrame, LlmError>> {
    let state = Arc::new(Mutex::new(StreamState::default()));
    let end = Arc::clone(&state);
    events
        .flat_map(move |event| stream::iter(state.lock().expect("stream state").frames(event)))
        .chain(
            stream::once(async move { stream::iter(end.lock().expect("stream state").end()) })
                .flatten(),
        )
        .boxed()
}

#[derive(Debug, Serialize)]
pub(crate) struct EmbedRequest {
    requests: Vec<Value>,
}

impl EmbedRequest {
    pub fn new(model: &str, texts: &[String]) -> Self {
        Self {
            requests: texts
                .iter()
                .map(|text| {
                    json!({ "model": format!("models/{model}"), "content": { "parts": [ { "text": text } ] } })
                })
                .collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct EmbedResponse {
    pub embeddings: Vec<EmbeddingValues>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct EmbeddingValues {
    pub values: Vec<f32>,
}
