//! JSON of the Converse API; only the fields grauwolf uses. Built as `serde_json::Value`, because
//! the API's unions (`{ "text": … }`, `{ "toolUse": … }`) are keyed by their variant name.

use grauwolf_llm::{LlmError, StructuredOutput};
use grauwolf_llm_http::HttpError;
use grauwolf_prompt::{
    AssistantMessage, AssistantPart, Attachment, AttachmentData, Message, Prompt, Reasoning,
    ToolCall, ToolChoice, ToolDescriptor, Usage,
};
use serde_json::{Map, Value, json};

/// Provider tag for own reasoning.
pub(crate) const PROVIDER: &str = "bedrock";

/// Everything that goes into a Converse request besides the prompt.
pub(crate) struct RequestOptions<'a> {
    pub tools: &'a [ToolDescriptor],
    /// Overrides the tool choice of the prompt (structured output: the forced tool).
    pub tool_choice: Option<&'a ToolChoice>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
}

/// Body of a `converse` or `converse-stream` request.
pub(crate) fn request(prompt: &Prompt, options: &RequestOptions<'_>) -> Result<Value, LlmError> {
    let leading = prompt
        .messages
        .iter()
        .take_while(|message| matches!(message, Message::System { .. }))
        .count();
    let system: Vec<Value> = prompt.messages[..leading]
        .iter()
        .filter_map(|message| match message {
            Message::System { content } if !content.is_empty() => Some(json!({ "text": content })),
            _ => None,
        })
        .collect();
    let messages = messages(&prompt.messages[leading..])?;
    let history_has_tools = messages.iter().any(|message| {
        message["content"].as_array().is_some_and(|blocks| {
            blocks
                .iter()
                .any(|block| block.get("toolUse").is_some() || block.get("toolResult").is_some())
        })
    });
    let mut body = Map::new();
    if !system.is_empty() {
        body.insert("system".into(), Value::Array(system));
    }
    body.insert("messages".into(), Value::Array(messages));
    let choice = options.tool_choice.unwrap_or(&prompt.params.tool_choice);
    if let Some(config) = tool_config(options.tools, choice, history_has_tools) {
        body.insert("toolConfig".into(), config);
    }
    let mut inference = Map::new();
    if let Some(max_tokens) = options.max_tokens {
        inference.insert("maxTokens".into(), json!(max_tokens));
    }
    if let Some(temperature) = options.temperature {
        inference.insert("temperature".into(), json!(temperature));
    }
    if !inference.is_empty() {
        body.insert("inferenceConfig".into(), Value::Object(inference));
    }
    Ok(Value::Object(body))
}

/// `toolConfig`: the Converse API has no `none` choice. With `ToolChoice::None`, the tools are
/// left out – unless the history already contains tool calls, because the API then insists on
/// `toolConfig`; in that case the choice stays `auto`.
fn tool_config(
    tools: &[ToolDescriptor],
    choice: &ToolChoice,
    history_has_tools: bool,
) -> Option<Value> {
    if tools.is_empty() || (*choice == ToolChoice::None && !history_has_tools) {
        return None;
    }
    let specs: Vec<Value> = tools
        .iter()
        .map(|tool| {
            let mut spec = json!({
                "name": tool.name,
                "inputSchema": { "json": tool.parameters },
            });
            if !tool.description.is_empty() {
                spec["description"] = json!(tool.description);
            }
            json!({ "toolSpec": spec })
        })
        .collect();
    let mut config = json!({ "tools": specs });
    match choice {
        ToolChoice::Auto | ToolChoice::None => {}
        ToolChoice::Required => config["toolChoice"] = json!({ "any": {} }),
        ToolChoice::Tool(name) => config["toolChoice"] = json!({ "tool": { "name": name } }),
    }
    Some(config)
}

/// Messages after the system prompt. The API wants the roles to alternate, so consecutive
/// messages of the same role (tool results, later system messages as user text) are merged.
fn messages(messages: &[Message]) -> Result<Vec<Value>, LlmError> {
    let mut merged: Vec<(&'static str, Vec<Value>)> = Vec::new();
    for message in messages {
        let (role, blocks) = match message {
            Message::System { content } => ("user", text_block(content).into_iter().collect()),
            Message::User {
                content,
                attachments,
            } => ("user", user_blocks(attachments, content)?),
            Message::Assistant(answer) => ("assistant", assistant_blocks(answer)),
            Message::ToolResult(result) => ("user", vec![tool_result(&result.id, &result.content)]),
        };
        if blocks.is_empty() {
            continue;
        }
        match merged.last_mut() {
            Some((last, existing)) if *last == role => existing.extend(blocks),
            _ => merged.push((role, blocks)),
        }
    }
    Ok(merged
        .into_iter()
        .map(|(role, content)| json!({ "role": role, "content": content }))
        .collect())
}

/// Blank text is rejected by the API.
fn text_block(text: &str) -> Option<Value> {
    (!text.trim().is_empty()).then(|| json!({ "text": text }))
}

/// Attachments (ADR-0081) before the text. An attachment that Bedrock cannot take is an error.
fn user_blocks(attachments: &[Attachment], content: &str) -> Result<Vec<Value>, LlmError> {
    let mut blocks = Vec::with_capacity(attachments.len() + 1);
    for (index, attachment) in attachments.iter().enumerate() {
        blocks.push(
            attachment_block(attachment, index + 1)
                .map_err(|reason| LlmError::Client(reason.into()))?,
        );
    }
    blocks.extend(text_block(content));
    Ok(blocks)
}

/// Block for an attachment: images and documents as bytes (Base64) or from S3 (`s3://` URL);
/// otherwise the reason why it is not possible. `number` names the document.
fn attachment_block(attachment: &Attachment, number: usize) -> Result<Value, String> {
    let source = match &attachment.data {
        AttachmentData::Base64(data) => json!({ "bytes": data }),
        AttachmentData::Url(url) if url.starts_with("s3://") => {
            json!({ "s3Location": { "uri": url } })
        }
        AttachmentData::Url(_) => {
            return Err(
                "bedrock: attachments by URL are not supported (possible: Base64 content \
                        or an s3:// URL)"
                    .into(),
            );
        }
    };
    let media_type = attachment.media_type.as_str();
    if let Some(format) = image_format(media_type) {
        return Ok(json!({ "image": { "format": format, "source": source } }));
    }
    if let Some(format) = document_format(media_type) {
        return Ok(json!({ "document": {
            "format": format, "name": format!("document-{number}"), "source": source
        } }));
    }
    Err(format!(
        "bedrock: attachments of type {media_type} are not supported (possible: png, jpeg, gif, \
         webp, pdf, csv, doc, docx, xls, xlsx, html, txt, md)"
    ))
}

fn image_format(media_type: &str) -> Option<&'static str> {
    match media_type {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpeg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

fn document_format(media_type: &str) -> Option<&'static str> {
    match media_type {
        "application/pdf" => Some("pdf"),
        "text/csv" => Some("csv"),
        "application/msword" => Some("doc"),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => Some("docx"),
        "application/vnd.ms-excel" => Some("xls"),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => Some("xlsx"),
        "text/html" => Some("html"),
        "text/plain" => Some("txt"),
        "text/markdown" => Some("md"),
        _ => None,
    }
}

fn assistant_blocks(answer: &AssistantMessage) -> Vec<Value> {
    answer
        .parts
        .iter()
        .filter_map(|part| match part {
            AssistantPart::Text { text } => text_block(text),
            AssistantPart::ToolCall(call) => Some(json!({
                "toolUse": { "toolUseId": call.id, "name": call.tool, "input": call.args }
            })),
            // Own reasoning comes back unchanged; reasoning of other providers is dropped.
            AssistantPart::Reasoning(reasoning) => {
                (reasoning.provider == PROVIDER).then(|| reasoning.data.clone())
            }
        })
        .collect()
}

/// A tool result: a JSON object as `json`, anything else as `text`.
fn tool_result(id: &str, content: &str) -> Value {
    let body = match serde_json::from_str::<Value>(content) {
        Ok(value @ Value::Object(_)) => json!({ "json": value }),
        _ if content.trim().is_empty() => json!({ "text": "(no output)" }),
        _ => json!({ "text": content }),
    };
    json!({ "toolResult": { "toolUseId": id, "content": [body] } })
}

/// The reply of `converse` as message, usage and finish reason.
pub(crate) struct Reply {
    pub message: Message,
    pub usage: Option<Usage>,
}

/// Reads a `converse` response. Answers withheld by guardrails or content filters are errors.
pub(crate) fn reply(response: &Value) -> Result<Reply, LlmError> {
    let stop_reason = response.get("stopReason").and_then(Value::as_str);
    check_stop_reason(stop_reason)?;
    let parts = response
        .pointer("/output/message/content")
        .and_then(Value::as_array)
        .map(|blocks| blocks.iter().filter_map(output_part).collect())
        .unwrap_or_default();
    Ok(Reply {
        message: Message::Assistant(AssistantMessage { parts }),
        usage: response.get("usage").map(usage),
    })
}

pub(crate) fn check_stop_reason(stop_reason: Option<&str>) -> Result<(), LlmError> {
    match stop_reason {
        Some(reason @ ("guardrail_intervened" | "content_filtered")) => Err(LlmError::Client(
            format!("Bedrock withheld the answer (stop reason {reason})").into(),
        )),
        _ => Ok(()),
    }
}

/// Part of a response block; unknown block kinds are dropped.
fn output_part(block: &Value) -> Option<AssistantPart> {
    if let Some(text) = block.get("text").and_then(Value::as_str) {
        return Some(AssistantPart::Text {
            text: text.to_owned(),
        });
    }
    if let Some(call) = block.get("toolUse") {
        return Some(AssistantPart::ToolCall(ToolCall {
            id: call.get("toolUseId")?.as_str()?.to_owned(),
            tool: call.get("name")?.as_str()?.to_owned(),
            args: call.get("input").cloned().unwrap_or(Value::Null),
        }));
    }
    block.get("reasoningContent").map(|_| {
        AssistantPart::Reasoning(Reasoning {
            provider: PROVIDER.into(),
            data: block.clone(),
        })
    })
}

/// Token counts; `inputTokens` is only the uncached remainder (ADR-0067).
pub(crate) fn usage(usage: &Value) -> Usage {
    let count = |name: &str| usage.get(name).and_then(Value::as_u64);
    let read = count("cacheReadInputTokens");
    let write = count("cacheWriteInputTokens");
    Usage {
        input_tokens: count("inputTokens").unwrap_or(0) + read.unwrap_or(0) + write.unwrap_or(0),
        output_tokens: count("outputTokens").unwrap_or(0),
        cache_read_tokens: read,
        cache_write_tokens: write,
    }
}

/// Error of an HTTP request with a hint for the cases that have a known cause. The
/// classification (transient or not) stays that of `LlmError::from`.
pub(crate) fn llm_error(error: HttpError) -> LlmError {
    let hint = match &error {
        HttpError::Status {
            status: 400, body, ..
        } => Some(format!(
            "Bedrock rejected the request (ValidationException): {body}"
        )),
        HttpError::Status {
            status: 403, body, ..
        } => Some(format!(
            "Bedrock denied access (AccessDeniedException): check the credentials and that access \
             to the model is granted for this account in this region: {body}"
        )),
        HttpError::Status {
            status: 404, body, ..
        } => Some(format!(
            "Bedrock does not know the model or the region has no such endpoint \
             (ResourceNotFoundException): check the model id: {body}"
        )),
        _ => None,
    };
    match (LlmError::from(error), hint) {
        (LlmError::Client(_), Some(hint)) => LlmError::Client(hint.into()),
        (other, _) => other,
    }
}

/// The tool that carries a structured answer: named after the type, its input is the answer.
/// The API wants an object as the root of a tool's schema; any other root is wrapped in an
/// object with the single property `value` (see [`structured_answer`]).
pub(crate) fn structured_tool(output: &StructuredOutput) -> ToolDescriptor {
    let name: String = output
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    let parameters = if output.schema.get("type").and_then(Value::as_str) == Some("object") {
        output.schema.clone()
    } else {
        json!({ "type": "object", "properties": { "value": output.schema }, "required": ["value"] })
    };
    ToolDescriptor {
        name: if name.is_empty() {
            "structured_output".into()
        } else {
            name
        },
        description: format!(
            "Returns the answer as structured data of type {}.",
            output.name
        ),
        parameters,
    }
}

/// The answer as JSON text from the call of the forced tool; plain text of the model stays as
/// it is (the schema may then not be met, which the caller validates), anything else is an error.
pub(crate) fn structured_answer(
    message: Message,
    tool: &str,
    schema: &Value,
) -> Result<Message, LlmError> {
    let Message::Assistant(answer) = &message else {
        return Ok(message);
    };
    let wrapped = schema.get("type").and_then(Value::as_str) != Some("object");
    let call = answer.parts.iter().find_map(|part| match part {
        AssistantPart::ToolCall(call) if call.tool == tool => Some(call),
        _ => None,
    });
    if let Some(call) = call {
        let value = if wrapped {
            call.args.get("value").cloned().unwrap_or(Value::Null)
        } else {
            call.args.clone()
        };
        return Ok(Message::assistant(value.to_string()));
    }
    if answer
        .parts
        .iter()
        .any(|part| matches!(part, AssistantPart::Text { .. }))
    {
        return Ok(message);
    }
    Err(LlmError::Client(
        format!("Bedrock did not call the tool '{tool}' for the structured answer").into(),
    ))
}
