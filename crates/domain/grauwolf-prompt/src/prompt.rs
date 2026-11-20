//! Prompt: history of messages that is sent to an LLM.

use serde::{Deserialize, Serialize};

use crate::Message;

/// A prompt with ID and message history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prompt {
    /// Identifier of the prompt, e.g. the agent name.
    pub id: String,
    /// Messages in chronological order.
    pub messages: Vec<Message>,
    /// Parameters of the request, e.g. the tool choice (ADR-0042).
    #[serde(default)]
    pub params: LlmParams,
}

/// Parameters of an LLM request (ADR-0042).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmParams {
    /// Whether and which tool the LLM must call.
    #[serde(default)]
    pub tool_choice: ToolChoice,
}

/// Tool choice of a request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    /// The LLM decides for itself.
    #[default]
    Auto,
    /// The LLM must call a tool.
    Required,
    /// The LLM must not call a tool.
    None,
    /// The LLM must call the tool with this name.
    Tool(String),
}

impl Prompt {
    /// Starts a prompt with the identifier `id`.
    pub fn builder(id: impl Into<String>) -> PromptBuilder {
        PromptBuilder {
            prompt: Prompt {
                id: id.into(),
                messages: Vec::new(),
                params: LlmParams::default(),
            },
        }
    }
}

impl Prompt {
    /// This prompt with `addition` appended to its first system message (after a blank line);
    /// without a system message, `addition` becomes the first message.
    pub fn extend_system(mut self, addition: &str) -> Self {
        match self.messages.first_mut() {
            Some(Message::System { content }) => {
                content.push_str("\n\n");
                content.push_str(addition);
            }
            _ => self.messages.insert(0, Message::system(addition)),
        }
        self
    }
}

/// Builds a [`Prompt`] message by message.
#[derive(Debug, Clone)]
pub struct PromptBuilder {
    prompt: Prompt,
}

impl PromptBuilder {
    /// Appends a system message.
    pub fn system(mut self, content: impl Into<String>) -> Self {
        self.prompt.messages.push(Message::system(content));
        self
    }

    /// Appends a user message.
    pub fn user(mut self, content: impl Into<String>) -> Self {
        self.prompt.messages.push(Message::user(content));
        self
    }

    /// Finishes the build.
    pub fn build(self) -> Prompt {
        self.prompt
    }
}

#[cfg(test)]
mod extend_system_tests {
    use super::*;

    #[test]
    fn appends_to_the_system_message_or_adds_one() {
        let with = Prompt::builder("a").system("Be brief.").user("hi").build();
        let with = with.extend_system("More.");
        assert_eq!(with.messages[0], Message::system("Be brief.\n\nMore."));
        assert_eq!(with.messages.len(), 2);
        let without = Prompt::builder("a").build().extend_system("More.");
        assert_eq!(without.messages, vec![Message::system("More.")]);
    }
}
