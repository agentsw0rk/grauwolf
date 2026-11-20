//! Streaming frames and their aggregation (F02.14, ADR-0033).

use grauwolf_prompt::{AssistantMessage, AssistantPart, Message, Reasoning, ToolCall};

/// A piece of a streamed answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamFrame {
    /// New text, to be appended to the previous text.
    TextDelta { text: String },
    /// A complete tool call.
    ToolCall(ToolCall),
    /// Complete reasoning of a provider.
    Reasoning(Reasoning),
    /// Token usage of the answer, before `End` (ADR-0067).
    Usage(grauwolf_prompt::Usage),
    /// The model the provider reports for the answer, anywhere before `End` (ADR-0094).
    /// Not part of the answer: `aggregate` ignores it, the prompt cache does not store it.
    Model(String),
    /// End of the answer.
    End { finish_reason: Option<String> },
}

/// Builds from `frames` the answer that a request without streaming would have returned.
pub fn aggregate(frames: impl IntoIterator<Item = StreamFrame>) -> Message {
    let mut parts: Vec<AssistantPart> = Vec::new();
    for frame in frames {
        match frame {
            StreamFrame::TextDelta { text } => match parts.last_mut() {
                Some(AssistantPart::Text { text: current }) => current.push_str(&text),
                _ => parts.push(AssistantPart::Text { text }),
            },
            StreamFrame::ToolCall(call) => parts.push(AssistantPart::ToolCall(call)),
            StreamFrame::Reasoning(reasoning) => parts.push(AssistantPart::Reasoning(reasoning)),
            StreamFrame::Usage(_) | StreamFrame::Model(_) | StreamFrame::End { .. } => {}
        }
    }
    Message::Assistant(AssistantMessage { parts })
}

/// Frames of a finished answer, terminated with `End`.
pub(crate) fn frames_of(
    message: Message,
    usage: Option<grauwolf_prompt::Usage>,
    model: Option<String>,
) -> Vec<StreamFrame> {
    let parts = match message {
        Message::Assistant(answer) => answer.parts,
        _ => Vec::new(),
    };
    model
        .map(StreamFrame::Model)
        .into_iter()
        .chain(parts.into_iter().map(|part| match part {
            AssistantPart::Text { text } => StreamFrame::TextDelta { text },
            AssistantPart::ToolCall(call) => StreamFrame::ToolCall(call),
            AssistantPart::Reasoning(reasoning) => StreamFrame::Reasoning(reasoning),
        }))
        .chain(usage.map(StreamFrame::Usage))
        .chain([StreamFrame::End {
            finish_reason: None,
        }])
        .collect()
}
