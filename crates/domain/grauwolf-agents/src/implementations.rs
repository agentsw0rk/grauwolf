//! Value implementations of the catalog kinds for the interpreter (ADR-0017, ADR-0018).
//!
//! Every kind in the built-in catalog must have an implementation here
//! (conformance test `tests/catalog_conformance.rs`).

use std::collections::BTreeMap;

use grauwolf_graph::{ConditionFn, NodeFn, erase_condition, erase_node};
use grauwolf_llm::StructuredOutput;
use grauwolf_prompt::{Message, ToolResult};
use serde_json::Value;

use crate::{AgentContext, HistoryCompression, conditions, nodes};

/// Implementation of the node kind `kind` with `params`, if known; `types` are the named
/// types of the spec (name → JSON schema).
pub fn node(
    kind: &str,
    params: &Value,
    types: &BTreeMap<String, Value>,
) -> Option<NodeFn<AgentContext>> {
    match kind {
        "llm_request_structured" => {
            let name = params.get("type")?.as_str()?;
            let retries = params.get("retries").map_or(Some(0), Value::as_u64)?;
            Some(llm_request_structured(
                StructuredOutput {
                    name: name.to_owned(),
                    schema: types.get(name)?.clone(),
                },
                usize::try_from(retries).ok()?,
            ))
        }
        "llm_request" => Some(erase_node(nodes::llm_request)),
        "llm_continue" => Some(erase_node(nodes::llm_continue)),
        "store" => {
            let key = params.get("key")?.as_str()?.to_owned();
            Some(Box::new(move |ctx: &mut AgentContext, value: Value| {
                let stored = ctx.store(key.clone(), &value);
                Box::pin(async move {
                    stored?;
                    Ok(value)
                })
            }))
        }
        "load" => {
            let key = params.get("key")?.as_str()?.to_owned();
            Some(Box::new(move |ctx: &mut AgentContext, _input: Value| {
                let value = nodes::load_value::<Value>(ctx, &key);
                Box::pin(async move { value })
            }))
        }
        "decide" => decide(DEFAULT_DECISION_NODE, params),
        "agent" => {
            let agent = params.get("agent")?.as_str()?.to_owned();
            Some(Box::new(move |ctx: &mut AgentContext, value: Value| {
                let agent = agent.clone();
                Box::pin(async move { ctx.run_agent_step(&agent, value).await })
            }))
        }
        "set_model" => {
            let model = model_from_params(params)?;
            Some(Box::new(move |ctx: &mut AgentContext, value: Value| {
                ctx.set_model(model.clone());
                Box::pin(async move { Ok(value) })
            }))
        }
        "llm_request_best_of" => Some(erase_node(nodes::llm_request_best_of(
            usize::try_from(params.get("choices")?.as_u64()?).ok()?,
            params
                .get("criteria")
                .and_then(Value::as_str)
                .unwrap_or(nodes::DEFAULT_BEST_OF_CRITERIA),
        ))),
        "llm_request_only_tools" => Some(erase_node(nodes::llm_request_only_tools)),
        "llm_request_force_tool" => Some(erase_node(nodes::llm_request_force_tool(
            params.get("tool")?.as_str()?,
        ))),
        "llm_verify" => {
            let name = params.get("type")?.as_str()?;
            let output = StructuredOutput {
                name: name.to_owned(),
                schema: types.get(name)?.clone(),
            };
            let prompt = params
                .get("prompt")
                .and_then(Value::as_str)
                .unwrap_or(nodes::DEFAULT_VERIFY_PROMPT)
                .to_owned();
            Some(Box::new(move |ctx: &mut AgentContext, input: Value| {
                let (output, prompt) = (output.clone(), prompt.clone());
                Box::pin(async move {
                    let answer: String = serde_json::from_value(input)?;
                    nodes::verify(ctx, &output, &prompt, answer).await
                })
            }))
        }
        "llm_moderate" => {
            let model = match (params.get("provider"), params.get("id")) {
                (Some(_), Some(_)) => Some(model_from_params(params)?),
                _ => None,
            };
            Some(Box::new(move |ctx: &mut AgentContext, input: Value| {
                let model = model.clone();
                Box::pin(async move {
                    let text: String = serde_json::from_value(input)?;
                    nodes::moderate(ctx, model.as_ref(), text).await
                })
            }))
        }
        "get_field" => {
            let field = params["field"].as_str()?.to_owned();
            Some(Box::new(move |_ctx: &mut AgentContext, value: Value| {
                let text = nodes::field_text(&value, &field);
                Box::pin(async move { Ok(Value::String(text)) })
            }))
        }
        "llm_reason" => {
            let prompt = params
                .get("prompt")
                .and_then(Value::as_str)
                .unwrap_or(nodes::DEFAULT_REASONING_PROMPT);
            match params["value"].as_str()? {
                "string" => Some(erase_node(nodes::llm_reason::<String>(prompt))),
                "tool_result" => Some(erase_node(nodes::llm_reason::<ToolResult>(prompt))),
                "tool_results" => Some(erase_node(nodes::llm_reason::<Vec<ToolResult>>(prompt))),
                _ => None,
            }
        }
        "execute_tool" => Some(erase_node(nodes::execute_tool)),
        "llm_send_tool_result" => Some(erase_node(nodes::llm_send_tool_result)),
        "llm_request_streaming" => Some(erase_node(nodes::llm_request_streaming)),
        "compress_history" => {
            let strategy = match params["strategy"].as_str()? {
                "summarize" => HistoryCompression::Summarize,
                "keep_last" => {
                    HistoryCompression::KeepLast(usize::try_from(params["keep"].as_u64()?).ok()?)
                }
                "facts" => {
                    HistoryCompression::Facts(usize::try_from(params["keep"].as_u64()?).ok()?)
                }
                _ => return None,
            };
            Some(compress_history(strategy))
        }
        "execute_multiple_tools" => Some(if params["sequential"].as_bool() == Some(true) {
            erase_node(nodes::execute_tools_in_order)
        } else {
            erase_node(nodes::execute_multiple_tools)
        }),
        "llm_send_multiple_tool_results" => Some(erase_node(nodes::llm_send_multiple_tool_results)),
        _ => None,
    }
}

/// ID that the events of `decide` carry when the implementation does not know the node's ID.
const DEFAULT_DECISION_NODE: &str = "decide";

/// Implementation of the node `decide` with `params` that reports its events as `node_id`
/// (ADR-0100); the interpreter passes the ID of the node.
pub fn decide(node_id: &str, params: &Value) -> Option<NodeFn<AgentContext>> {
    let node = std::sync::Arc::new(crate::decision::DecisionNode::from_params(params).ok()?);
    let node_id = node_id.to_owned();
    Some(Box::new(move |ctx: &mut AgentContext, value: Value| {
        let (node, node_id) = (std::sync::Arc::clone(&node), node_id.clone());
        Box::pin(async move {
            crate::decision::decide(ctx, &node_id, &node, &value).await?;
            Ok(value)
        })
    }))
}

/// Implementation of the condition `kind` with `params`, if known.
pub fn condition(kind: &str, params: &Value) -> Option<ConditionFn<AgentContext>> {
    match kind {
        "on_history_longer" => {
            let limit = usize::try_from(params["messages"].as_u64()?).ok()?;
            Some(Box::new(move |ctx: &AgentContext, value: Value| {
                let passed = (ctx.prompt().messages.len() > limit).then_some(value);
                Box::pin(async move { Ok(passed) })
            }))
        }
        "on_history_tokens_above" => {
            let limit = params["tokens"].as_u64()?;
            Some(Box::new(move |ctx: &AgentContext, value: Value| {
                let passed = (ctx.prompt_tokens() > limit).then_some(value);
                Box::pin(async move { Ok(passed) })
            }))
        }
        "on_field" => {
            let field = params["field"].as_str()?.to_owned();
            let expected = params.get("equals")?.clone();
            Some(Box::new(move |_ctx: &AgentContext, value: Value| {
                let passed = conditions::field_equals(&value, &field, &expected).then_some(value);
                Box::pin(async move { Ok(passed) })
            }))
        }
        "on_decision" => {
            let answers: Vec<String> = params["answers"]
                .as_array()?
                .iter()
                .map(|answer| answer.as_str().map(str::to_owned))
                .collect::<Option<_>>()?;
            Some(Box::new(move |ctx: &AgentContext, value: Value| {
                let answer = crate::decision::latest_answer(ctx);
                let passed = answer
                    .filter(|answer| answers.contains(answer))
                    .map(|_| value);
                Box::pin(async move { Ok(passed) })
            }))
        }
        "on_assistant_message" => Some(erase_condition(conditions::on_assistant_message)),
        "on_tool_call" => Some(erase_condition(conditions::on_tool_call)),
        "on_multiple_tool_calls" => Some(erase_condition(conditions::on_multiple_tool_calls)),
        _ => None,
    }
}

/// `llm_request_structured` on `Value`: schema from the spec instead of a Rust type (ADR-0031).
fn llm_request_structured(output: StructuredOutput, retries: usize) -> NodeFn<AgentContext> {
    Box::new(move |ctx, input| {
        let output = output.clone();
        Box::pin(async move {
            let input: String = serde_json::from_value(input)?;
            ctx.append(Message::user(input));
            ctx.request_llm_structured(&output, retries).await
        })
    })
}

/// `compress_history` on `Value`: the value is not read, only passed through.
fn compress_history(strategy: HistoryCompression) -> NodeFn<AgentContext> {
    Box::new(move |ctx, value| {
        Box::pin(async move {
            ctx.compress_history(strategy).await?;
            Ok(value)
        })
    })
}

/// Model from `provider`/`id` of the node parameters, including `temperature`, `think` and
/// `fallbacks` (ADR-0088, ADR-0097).
fn model_from_params(params: &Value) -> Option<grauwolf_prompt::LlModel> {
    let model = |value: &Value| {
        let mut model = grauwolf_prompt::LlModel::new(
            value.get("provider")?.as_str()?,
            value.get("id")?.as_str()?,
        );
        if let Some(temperature) = value.get("temperature").and_then(Value::as_f64) {
            model = model.with_temperature(temperature);
        }
        if let Some(think) = value.get("think").and_then(Value::as_bool) {
            model = model.with_think(think);
        }
        Some(model)
    };
    let fallbacks = params
        .get("fallbacks")
        .and_then(Value::as_array)
        .map(|list| list.iter().map(model).collect::<Option<Vec<_>>>())
        .unwrap_or(Some(Vec::new()))?;
    Some(model(params)?.with_fallbacks(fallbacks))
}
