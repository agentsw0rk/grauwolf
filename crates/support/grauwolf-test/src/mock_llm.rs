//! Rule-based LLM client for tests (F12.1).

use futures_util::future::BoxFuture;
use grauwolf_llm::{LlmClient, LlmError};
use grauwolf_prompt::{
    AssistantMessage, AssistantPart, LlModel, Message, Prompt, ToolCall, ToolDescriptor,
};
use serde_json::Value;

/// Answers based on rules to the last user message or the last tool result;
/// if no rule matches, the default answer is returned.
#[derive(Debug, Clone)]
pub struct MockLlmClient {
    rules: Vec<(String, Message)>,
    default_answer: String,
}

impl MockLlmClient {
    /// Mock that answers with `default_answer` when no rule matches.
    pub fn new(default_answer: impl Into<String>) -> Self {
        Self {
            rules: Vec::new(),
            default_answer: default_answer.into(),
        }
    }

    /// Answers with `answer` if the last input contains `needle`.
    /// Rules are checked in the order they were registered.
    pub fn answer_when_contains(
        mut self,
        needle: impl Into<String>,
        answer: impl Into<String>,
    ) -> Self {
        self.rules.push((needle.into(), Message::assistant(answer)));
        self
    }

    /// Calls the tool `tool` with `args` if the last input contains `needle`.
    /// The call ID is the tool name.
    pub fn call_tool_when_contains(
        mut self,
        needle: impl Into<String>,
        tool: impl Into<String>,
        args: Value,
    ) -> Self {
        let tool = tool.into();
        self.rules
            .push((needle.into(), Message::tool_call(tool.clone(), tool, args)));
        self
    }

    /// Calls several tools in one answer (parallel tool calls) if the last input contains
    /// `needle`. The call IDs are `<tool>_<n>`, starting at 1.
    pub fn call_tools_when_contains(
        mut self,
        needle: impl Into<String>,
        calls: impl IntoIterator<Item = (impl Into<String>, Value)>,
    ) -> Self {
        let parts = calls
            .into_iter()
            .enumerate()
            .map(|(index, (tool, args))| {
                let tool = tool.into();
                AssistantPart::ToolCall(ToolCall {
                    id: format!("{tool}_{}", index + 1),
                    tool,
                    args,
                })
            })
            .collect();
        self.rules.push((
            needle.into(),
            Message::Assistant(AssistantMessage { parts }),
        ));
        self
    }

    fn answer_for(&self, prompt: &Prompt) -> Message {
        // Text of the last answer (e.g. a thought in ReAct) and everything after it, without
        // system messages, joined line by line.
        let start = prompt
            .messages
            .iter()
            .rposition(|message| matches!(message, Message::Assistant(_)))
            .unwrap_or(0);
        let texts: Vec<String> = prompt.messages[start..]
            .iter()
            .filter_map(|message| match message {
                Message::User { content, .. } => Some(content.clone()),
                Message::ToolResult(result) => Some(result.content.clone()),
                Message::Assistant(answer) => Some(answer.text()).filter(|text| !text.is_empty()),
                _ => None,
            })
            .collect();
        let last_input = (!texts.is_empty()).then(|| texts.join("\n"));
        last_input
            .and_then(|text| {
                self.rules
                    .iter()
                    .find(|(needle, _)| text.contains(needle.as_str()))
                    .map(|(_, answer)| answer.clone())
            })
            .unwrap_or_else(|| Message::assistant(self.default_answer.clone()))
    }
}

impl LlmClient for MockLlmClient {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        let answer = self.answer_for(prompt);
        Box::pin(async move { Ok(answer) })
    }
}
