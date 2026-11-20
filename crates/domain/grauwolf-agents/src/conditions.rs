//! Implementations of the catalog conditions (ADR-0021).

use grauwolf_prompt::{Message, ToolCall};

use crate::AgentContext;

/// `true` if the field `field` of `value` equals `expected`; helper for `on_field`
/// (ADR-0041). Values that cannot be serialized and missing fields count as unequal.
pub fn field_equals<T: serde::Serialize>(
    value: &T,
    field: &str,
    expected: &serde_json::Value,
) -> bool {
    serde_json::to_value(value).is_ok_and(|value| value.get(field) == Some(expected))
}

/// `on_assistant_message`: passes answers without tool calls and returns their text.
pub fn on_assistant_message(_ctx: &AgentContext, message: Message) -> Option<String> {
    match message {
        Message::Assistant(answer) if answer.tool_calls().next().is_none() => Some(answer.text()),
        _ => None,
    }
}

/// `on_tool_call`: passes answers with exactly one tool call and returns it.
pub fn on_tool_call(_ctx: &AgentContext, message: Message) -> Option<ToolCall> {
    let Message::Assistant(answer) = message else {
        return None;
    };
    let mut calls = answer.tool_calls();
    match (calls.next(), calls.next()) {
        (Some(call), None) => Some(call.clone()),
        _ => None,
    }
}

/// `on_multiple_tool_calls`: passes answers with at least one tool call and returns all
/// calls (for `execute_multiple_tools`).
pub fn on_multiple_tool_calls(_ctx: &AgentContext, message: Message) -> Option<Vec<ToolCall>> {
    let Message::Assistant(answer) = message else {
        return None;
    };
    let calls: Vec<ToolCall> = answer.tool_calls().cloned().collect();
    (!calls.is_empty()).then_some(calls)
}

/// `on_history_longer`: passes the value through if the history has more than `MESSAGES`
/// messages (ADR-0037).
pub fn on_history_longer<T, const MESSAGES: usize>(ctx: &AgentContext, value: T) -> Option<T> {
    (ctx.prompt().messages.len() > MESSAGES).then_some(value)
}

/// `on_history_tokens_above`: passes the value through if the history has more than `TOKENS`
/// tokens, counted with the context's tokenizer (ADR-0087).
pub fn on_history_tokens_above<T, const TOKENS: u64>(ctx: &AgentContext, value: T) -> Option<T> {
    (ctx.prompt_tokens() > TOKENS).then_some(value)
}

/// `on_decision`: passes the value through if the answer of the latest decision is one of
/// `answers` (ADR-0100).
pub fn on_decision<T>(ctx: &AgentContext, answers: &[&str], value: T) -> Option<T> {
    let answer = crate::decision::latest_answer(ctx)?;
    answers.contains(&answer.as_str()).then_some(value)
}
