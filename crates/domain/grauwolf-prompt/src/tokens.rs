//! Count tokens before the request (F06.13, ADR-0087).

use crate::{AssistantPart, Message, Prompt};

/// Surcharge per message for role and separator.
const PER_MESSAGE: u64 = 4;

/// Flat rate per attachment (image, PDF, audio); the real number depends on size and provider.
const PER_ATTACHMENT: u64 = 1000;

/// Port for counting tokens; provider-exact tokenizers can be plugged in here.
pub trait Tokenizer: Send + Sync {
    /// Tokens of `text`.
    fn count(&self, text: &str) -> u64;
}

/// Estimate without dependencies: one token per started four characters.
#[derive(Debug, Clone, Copy, Default)]
pub struct EstimatingTokenizer;

impl Tokenizer for EstimatingTokenizer {
    fn count(&self, text: &str) -> u64 {
        (text.chars().count() as u64).div_ceil(4)
    }
}

/// Tokens of all messages of `prompt`: text, tool calls (name and arguments as JSON),
/// tool results, a surcharge per message and a flat rate per attachment. Reasoning is not
/// counted, because providers send it back in different ways.
pub fn count_prompt(tokenizer: &dyn Tokenizer, prompt: &Prompt) -> u64 {
    prompt
        .messages
        .iter()
        .map(|message| {
            PER_MESSAGE
                + match message {
                    Message::System { content } => tokenizer.count(content),
                    Message::User {
                        content,
                        attachments,
                    } => tokenizer.count(content) + PER_ATTACHMENT * attachments.len() as u64,
                    Message::Assistant(answer) => answer
                        .parts
                        .iter()
                        .map(|part| match part {
                            AssistantPart::Text { text } => tokenizer.count(text),
                            AssistantPart::ToolCall(call) => {
                                tokenizer.count(&call.tool)
                                    + tokenizer.count(&call.args.to_string())
                            }
                            AssistantPart::Reasoning(_) => 0,
                        })
                        .sum(),
                    Message::ToolResult(result) => tokenizer.count(&result.content),
                }
        })
        .sum()
}
