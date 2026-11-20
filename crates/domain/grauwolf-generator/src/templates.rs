//! Generator templates of the catalog kinds (ADR-0018, ADR-0021).
//!
//! Every kind in the built-in catalog must have a template here
//! (conformance test `tests/catalog_conformance.rs`).

use serde_json::Value;

/// ID that the events of `decide` carry when the template does not know the node's ID.
const DEFAULT_DECISION_NODE: &str = "decide";

/// Like [`node`] for the node `id`; `decide` reports its events under this ID (ADR-0100).
pub fn node_for(id: &str, kind: &str, params: &Value) -> Option<String> {
    match kind {
        "decide" => Some(format!(
            "grauwolf_agents::nodes::decide::<{}>({id:?}, {:?})",
            rust_type(params["value"].as_str()?),
            params.to_string()
        )),
        "agent" => Some(format!(
            "grauwolf_agents::nodes::agent_step::<{}, {}>({:?})",
            rust_type(
                params
                    .get("input")
                    .and_then(Value::as_str)
                    .unwrap_or("string")
            ),
            rust_type(
                params
                    .get("output")
                    .and_then(Value::as_str)
                    .unwrap_or("string")
            ),
            params["agent"].as_str()?
        )),
        _ => node(kind, params),
    }
}

/// Rust expression of the node function for the node kind `kind` with `params`, if known.
pub fn node(kind: &str, params: &Value) -> Option<String> {
    let path = match kind {
        "decide" => return node_for(DEFAULT_DECISION_NODE, kind, params),
        "agent" => return node_for("agent", kind, params),
        "llm_request" => "grauwolf_agents::nodes::llm_request",
        "llm_continue" => "grauwolf_agents::nodes::llm_continue",
        "store" => {
            return Some(format!(
                "grauwolf_agents::nodes::store::<{}>({:?})",
                rust_type(params["value"].as_str()?),
                params.get("key")?.as_str()?
            ));
        }
        "load" => {
            return Some(format!(
                "grauwolf_agents::nodes::load::<{}, {}>({:?})",
                rust_type(params["value"].as_str()?),
                rust_type(params["type"].as_str()?),
                params.get("key")?.as_str()?
            ));
        }
        "set_model"
            if ["fallbacks", "temperature", "think"]
                .iter()
                .any(|key| params.get(key).is_some()) =>
        {
            return Some(format!(
                "grauwolf_agents::nodes::switch_model::<{}>({})",
                rust_type(params["value"].as_str()?),
                model_from_params(params)?
            ));
        }
        "set_model" => {
            return Some(format!(
                "grauwolf_agents::nodes::set_model::<{}>({:?}, {:?})",
                rust_type(params["value"].as_str()?),
                params.get("provider")?.as_str()?,
                params.get("id")?.as_str()?
            ));
        }
        "llm_request_best_of" => {
            let criteria = params.get("criteria").and_then(Value::as_str).map_or(
                "grauwolf_agents::nodes::DEFAULT_BEST_OF_CRITERIA".to_owned(),
                |criteria| format!("{criteria:?}"),
            );
            return Some(format!(
                "grauwolf_agents::nodes::llm_request_best_of({}, {criteria})",
                params.get("choices")?.as_u64()?
            ));
        }
        "llm_request_only_tools" => "grauwolf_agents::nodes::llm_request_only_tools",
        "llm_request_force_tool" => {
            return Some(format!(
                "grauwolf_agents::nodes::llm_request_force_tool({:?})",
                params.get("tool")?.as_str()?
            ));
        }
        "execute_tool" => "grauwolf_agents::nodes::execute_tool",
        "llm_send_tool_result" => "grauwolf_agents::nodes::llm_send_tool_result",
        "llm_request_streaming" => "grauwolf_agents::nodes::llm_request_streaming",
        "compress_history" => {
            let value = rust_type(params["value"].as_str()?);
            return match params["strategy"].as_str()? {
                "summarize" => Some(format!(
                    "grauwolf_agents::nodes::compress_history_summarize::<{value}>"
                )),
                "keep_last" => Some(format!(
                    "grauwolf_agents::nodes::compress_history_keep_last::<{value}, {}>",
                    params["keep"].as_u64()?
                )),
                "facts" => Some(format!(
                    "grauwolf_agents::nodes::compress_history_facts::<{value}, {}>",
                    params["keep"].as_u64()?
                )),
                _ => None,
            };
        }
        "llm_verify" => {
            let prompt = params.get("prompt").and_then(Value::as_str).map_or(
                "grauwolf_agents::nodes::DEFAULT_VERIFY_PROMPT".to_owned(),
                |prompt| format!("{prompt:?}"),
            );
            return Some(format!(
                "grauwolf_agents::nodes::llm_verify::<{}>({prompt})",
                params.get("type")?.as_str()?
            ));
        }
        "llm_moderate" => {
            let model = match (params.get("provider"), params.get("id")) {
                (Some(_), Some(_)) => format!("Some({})", model_from_params(params)?),
                _ => "None".to_owned(),
            };
            return Some(format!(
                "grauwolf_agents::nodes::llm_moderate::<{}>({model})",
                params.get("type")?.as_str()?
            ));
        }
        // The type becomes the generated struct of the same name (ADR-0022).
        "get_field" => {
            return Some(format!(
                "grauwolf_agents::nodes::get_field::<{}>({:?})",
                params.get("type")?.as_str()?,
                params["field"].as_str()?
            ));
        }
        "llm_reason" => {
            let value = rust_type(params["value"].as_str()?);
            let prompt = params.get("prompt").and_then(Value::as_str).map_or(
                "grauwolf_agents::nodes::DEFAULT_REASONING_PROMPT".to_owned(),
                |prompt| format!("{prompt:?}"),
            );
            return Some(format!(
                "grauwolf_agents::nodes::llm_reason::<{value}>({prompt})"
            ));
        }
        "execute_multiple_tools" if params["sequential"].as_bool() == Some(true) => {
            "grauwolf_agents::nodes::execute_tools_in_order"
        }
        "execute_multiple_tools" => "grauwolf_agents::nodes::execute_multiple_tools",
        "llm_send_multiple_tool_results" => {
            "grauwolf_agents::nodes::llm_send_multiple_tool_results"
        }
        // The type becomes the generated struct of the same name (ADR-0022, ADR-0031).
        "llm_request_structured" => {
            let name = params.get("type")?.as_str()?;
            return Some(match params.get("retries") {
                None => format!("grauwolf_agents::nodes::llm_request_structured::<{name}>"),
                Some(retries) => format!(
                    "grauwolf_agents::nodes::llm_request_structured_with_retries::<{name}, {}>",
                    retries.as_u64()?
                ),
            });
        }
        _ => return None,
    };
    Some(path.to_owned())
}

/// Rust expression of the condition function for `kind` with `params`, if known.
pub fn condition(kind: &str, params: &Value) -> Option<String> {
    let path = match kind {
        "on_assistant_message" => "grauwolf_agents::conditions::on_assistant_message",
        "on_tool_call" => "grauwolf_agents::conditions::on_tool_call",
        "on_multiple_tool_calls" => "grauwolf_agents::conditions::on_multiple_tool_calls",
        // The value type follows from the edge (`_`), the limit is a const parameter.
        // Types follow from the edge; the field is compared via serde (ADR-0041).
        "on_field" => {
            return Some(format!(
                "|_ctx, value| grauwolf_agents::conditions::field_equals(&value, {:?}, &serde_json::json!({})).then_some(value)",
                params["field"].as_str()?,
                params.get("equals")?
            ));
        }
        "on_decision" => {
            let answers: Vec<String> = params["answers"]
                .as_array()?
                .iter()
                .map(|answer| answer.as_str().map(|answer| format!("{answer:?}")))
                .collect::<Option<_>>()?;
            return Some(format!(
                "|ctx, value| grauwolf_agents::conditions::on_decision(ctx, &[{}], value)",
                answers.join(", ")
            ));
        }
        "on_history_longer" => {
            return Some(format!(
                "grauwolf_agents::conditions::on_history_longer::<_, {}>",
                params["messages"].as_u64()?
            ));
        }
        "on_history_tokens_above" => {
            return Some(format!(
                "grauwolf_agents::conditions::on_history_tokens_above::<_, {}>",
                params["tokens"].as_u64()?
            ));
        }
        _ => return None,
    };
    Some(path.to_owned())
}

/// Rust type for a data type name of the catalogs (`string`, `tool_result`, …, named types).
fn rust_type(name: &str) -> String {
    match name {
        "string" => "String".into(),
        "message" => "grauwolf_prompt::Message".into(),
        "tool_call" => "grauwolf_prompt::ToolCall".into(),
        "tool_result" => "grauwolf_prompt::ToolResult".into(),
        "tool_calls" => "Vec<grauwolf_prompt::ToolCall>".into(),
        "tool_results" => "Vec<grauwolf_prompt::ToolResult>".into(),
        list if list.starts_with("array<") && list.ends_with('>') => {
            format!("Vec<{}>", rust_type(&list["array<".len()..list.len() - 1]))
        }
        named => named.into(),
    }
}

/// Expression for the model from `provider`/`id` of the node parameters with `fallbacks` (ADR-0088).
fn model_from_params(params: &Value) -> Option<String> {
    let model = |value: &Value| {
        let mut expr = format!(
            "grauwolf_prompt::LlModel::new({:?}, {:?})",
            value.get("provider")?.as_str()?,
            value.get("id")?.as_str()?
        );
        if let Some(temperature) = value.get("temperature").and_then(Value::as_f64) {
            expr.push_str(&format!(".with_temperature({temperature:?})"));
        }
        if let Some(think) = value.get("think").and_then(Value::as_bool) {
            expr.push_str(&format!(".with_think({think})"));
        }
        Some(expr)
    };
    let mut expr = model(params)?;
    if let Some(list) = params.get("fallbacks").and_then(Value::as_array) {
        let fallbacks = list.iter().map(model).collect::<Option<Vec<_>>>()?;
        expr.push_str(&format!(".with_fallbacks(vec![{}])", fallbacks.join(", ")));
    }
    Some(expr)
}
