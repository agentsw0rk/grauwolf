//! Messages of a conversation.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A message in the history of a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    /// Instruction for the LLM.
    System { content: String },
    /// User input, optionally with attachments such as images or PDFs (ADR-0081).
    User {
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<Attachment>,
    },
    /// Answer of the LLM: text, tool calls and reasoning as parts (ADR-0032).
    Assistant(AssistantMessage),
    /// Result of a tool call.
    ToolResult(ToolResult),
}

/// Attachment of a user message (ADR-0081); the kind follows from `media_type`
/// (`image/*`, `audio/*`, `application/pdf`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// Media type, e.g. `image/png`.
    pub media_type: String,
    /// Content or reference.
    pub data: AttachmentData,
}

/// Content of an attachment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentData {
    /// Content as Base64.
    Base64(String),
    /// Address from which the provider loads the content itself.
    Url(String),
}

impl Attachment {
    /// Is it an image (`image/*`)?
    pub fn is_image(&self) -> bool {
        self.media_type.starts_with("image/")
    }

    /// Is it audio (`audio/*`)?
    pub fn is_audio(&self) -> bool {
        self.media_type.starts_with("audio/")
    }

    /// Is it a PDF?
    pub fn is_pdf(&self) -> bool {
        self.media_type == "application/pdf"
    }

    /// Content as data URL (`data:<type>;base64,…`) or the URL itself.
    pub fn to_url(&self) -> String {
        match &self.data {
            AttachmentData::Base64(data) => format!("data:{};base64,{data}", self.media_type),
            AttachmentData::Url(url) => url.clone(),
        }
    }
}

/// An answer of the LLM, consisting of parts in the order of the provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantMessage {
    /// Parts of the answer.
    pub parts: Vec<AssistantPart>,
}

/// Part of an answer of the LLM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantPart {
    /// Text for the user.
    Text { text: String },
    /// Call of a tool.
    ToolCall(ToolCall),
    /// Provider-specific reasoning that must be sent back unchanged.
    Reasoning(Reasoning),
}

/// Reasoning of a provider; opaque for the core (ADR-0032).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reasoning {
    /// Provider whose adapter understands `data`, e.g. `anthropic`.
    pub provider: String,
    /// Data in the format of the provider.
    pub data: Value,
}

impl AssistantMessage {
    /// Joined text of all text parts.
    pub fn text(&self) -> String {
        self.parts
            .iter()
            .filter_map(|part| match part {
                AssistantPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// All tool calls in order.
    pub fn tool_calls(&self) -> impl Iterator<Item = &ToolCall> {
        self.parts.iter().filter_map(|part| match part {
            AssistantPart::ToolCall(call) => Some(call),
            _ => None,
        })
    }
}

/// Call of a tool by the LLM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Identifier that assigns the result to the call.
    pub id: String,
    /// Name of the tool.
    pub tool: String,
    /// Arguments as JSON.
    pub args: Value,
}

/// Result of a tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    /// Identifier of the associated call.
    pub id: String,
    /// Name of the tool.
    pub tool: String,
    /// Result as text for the LLM.
    pub content: String,
}

impl Message {
    /// System message with `content`.
    pub fn system(content: impl Into<String>) -> Self {
        Message::System {
            content: content.into(),
        }
    }

    /// Answer of the LLM that consists only of the text `content`.
    pub fn assistant(content: impl Into<String>) -> Self {
        Message::Assistant(AssistantMessage {
            parts: vec![AssistantPart::Text {
                text: content.into(),
            }],
        })
    }

    /// Answer of the LLM that consists only of the tool call `id` of the tool `tool` with `args`.
    pub fn tool_call(id: impl Into<String>, tool: impl Into<String>, args: Value) -> Self {
        Message::Assistant(AssistantMessage {
            parts: vec![AssistantPart::ToolCall(ToolCall {
                id: id.into(),
                tool: tool.into(),
                args,
            })],
        })
    }

    /// Result `content` of the tool call `id`.
    pub fn tool_result(
        id: impl Into<String>,
        tool: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Message::ToolResult(ToolResult {
            id: id.into(),
            tool: tool.into(),
            content: content.into(),
        })
    }

    /// User message with `content`.
    pub fn user(content: impl Into<String>) -> Self {
        Self::user_with(content, Vec::new())
    }

    /// User message with attachments (ADR-0081).
    pub fn user_with(content: impl Into<String>, attachments: Vec<Attachment>) -> Self {
        Message::User {
            content: content.into(),
            attachments,
        }
    }
}
