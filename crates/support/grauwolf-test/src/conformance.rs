//! LlmClient conformance suite (ADR-0005): every provider adapter must pass it against a real
//! model (opt-in, live test of the adapter crate).
//!
//! The checks are deliberately lenient, because real models do not answer deterministically;
//! they check the mapping of the protocol, not the quality of the answer.

use futures_util::StreamExt;
use grauwolf_llm::{LlmClient, LlmError, StreamFrame, StructuredOutput, aggregate};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};
use serde_json::{Value, json};

/// A failed check.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    /// Name of the check: `text`, `tool_call`, `tool_result`, `structured` or `streaming`.
    pub check: &'static str,
    /// What was expected and what came back.
    pub reason: String,
}

/// Runs all checks against `client` and `model` and returns the failures.
pub async fn check_llm_client(client: &dyn LlmClient, model: &LlModel) -> Vec<Failure> {
    let checks = [
        ("text", text(client, model).await),
        ("tool_call", tool_call(client, model).await),
        ("tool_result", tool_result(client, model).await),
        ("structured", structured(client, model).await),
        ("streaming", streaming(client, model).await),
    ];
    checks
        .into_iter()
        .filter_map(|(check, result)| result.err().map(|reason| Failure { check, reason }))
        .collect()
}

/// Tool that the suite offers.
pub fn add_tool() -> ToolDescriptor {
    ToolDescriptor {
        name: "add".into(),
        description: "Adds two numbers a and b.".into(),
        parameters: json!({
            "type": "object",
            "properties": { "a": { "type": "number" }, "b": { "type": "number" } },
            "required": ["a", "b"]
        }),
    }
}

async fn text(client: &dyn LlmClient, model: &LlModel) -> Result<(), String> {
    let prompt = Prompt::builder("conformance")
        .user("Reply with exactly one word: Hello")
        .build();
    let answer = client.execute(&prompt, model, &[]).await;
    match answer.as_ref().ok().and_then(text_of) {
        Some(text) if text.to_lowercase().contains("hello") => Ok(()),
        _ => Err(format!(
            "expected a text answer with 'Hello', got: {answer:?}"
        )),
    }
}

async fn tool_call(client: &dyn LlmClient, model: &LlModel) -> Result<(), String> {
    let prompt = Prompt::builder("conformance")
        .user("Compute 2+3 using the add tool.")
        .build();
    let answer = client.execute(&prompt, model, &[add_tool()]).await;
    let called = matches!(&answer, Ok(Message::Assistant(message)) if message.tool_calls().any(|call| {
        call.tool == "add" && number(&call.args["a"]) + number(&call.args["b"]) == 5.0
    }));
    if called {
        Ok(())
    } else {
        Err(format!(
            "expected a call of 'add' with 2 and 3, got: {answer:?}"
        ))
    }
}

async fn tool_result(client: &dyn LlmClient, model: &LlModel) -> Result<(), String> {
    let mut prompt = Prompt::builder("conformance")
        .user("Compute 2+3 using the add tool.")
        .build();
    prompt.messages.push(Message::tool_call(
        "call_1",
        "add",
        json!({ "a": 2, "b": 3 }),
    ));
    prompt
        .messages
        .push(Message::tool_result("call_1", "add", "5"));
    let answer = client.execute(&prompt, model, &[add_tool()]).await;
    match answer.as_ref().ok().and_then(text_of) {
        Some(text) if text.contains('5') => Ok(()),
        _ => Err(format!(
            "expected a text answer with the result 5, got: {answer:?}"
        )),
    }
}

async fn structured(client: &dyn LlmClient, model: &LlModel) -> Result<(), String> {
    let prompt = Prompt::builder("conformance")
        .user("Create a ticket for this bug report: The Login button does not react. Priority 1.")
        .build();
    let output = StructuredOutput {
        name: "Ticket".into(),
        schema: json!({
            "type": "object",
            "properties": { "title": { "type": "string" }, "priority": { "type": "integer" } },
            "required": ["title", "priority"]
        }),
    };
    let answer = client.execute_structured(&prompt, model, &output).await;
    let Some(content) = answer.as_ref().ok().and_then(text_of) else {
        return Err(format!("expected a text answer, got: {answer:?}"));
    };
    let value: Value = serde_json::from_str(strip_code_fence(&content))
        .map_err(|error| format!("expected JSON ({error}), got: {content}"))?;
    if value["title"].is_string() && value["priority"].is_i64() {
        Ok(())
    } else {
        Err(format!(
            "expected a ticket with title and priority, got: {value}"
        ))
    }
}

async fn streaming(client: &dyn LlmClient, model: &LlModel) -> Result<(), String> {
    let prompt = Prompt::builder("conformance")
        .user("Reply with exactly one word: Hello")
        .build();
    let frames: Vec<Result<StreamFrame, LlmError>> = client
        .execute_streaming(&prompt, model, &[])
        .collect()
        .await;
    let frames = frames
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("expected a stream without errors: {error}"))?;
    if !matches!(frames.last(), Some(StreamFrame::End { .. })) {
        return Err(format!("stream should end with End, got: {frames:?}"));
    }
    match text_of(&aggregate(frames)) {
        Some(text) if text.to_lowercase().contains("hello") => Ok(()),
        other => Err(format!(
            "expected a streamed text answer with 'Hello', got: {other:?}"
        )),
    }
}

/// Text of an answer without tool calls.
fn text_of(message: &Message) -> Option<String> {
    match message {
        Message::Assistant(answer) if answer.tool_calls().next().is_none() => Some(answer.text()),
        _ => None,
    }
}

/// Number from a JSON value; models sometimes return numbers as text.
fn number(value: &Value) -> f64 {
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse().ok())
        .unwrap_or(f64::NAN)
}

fn strip_code_fence(text: &str) -> &str {
    let text = text.trim();
    match text.strip_prefix("```") {
        Some(rest) => {
            let body = rest.split_once('\n').map_or("", |(_, body)| body);
            body.trim_end().strip_suffix("```").unwrap_or(body).trim()
        }
        None => text,
    }
}
