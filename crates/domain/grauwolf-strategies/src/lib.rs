//! Strategy templates (F05.12, ADR-0041, ADR-0108 stage 6): `strategy { kind, params }` becomes an ordinary
//! graph when loaded; everything else sees only the graph.

use grauwolf_graph::params::check_params;
use serde_json::{Value, json};

use grauwolf_spec_model::{
    ConditionSpec, EdgeSpec, GraphSpec, NodeSpec, STRING_TYPE, StrategySpec,
};

/// Result of a strategy: the graph and the types it needs.
pub struct Expansion {
    pub graph: GraphSpec,
    pub types: Vec<(String, Value)>,
}

/// Name of the verdict type of the `verified` strategy.
pub const VERIFICATION_TYPE: &str = "Verification";

/// Name of the moderation verdict type (`single_run` with `moderate`, ADR-0066).
pub const MODERATION_TYPE: &str = "Moderation";

/// Graph and types of the strategy, or an error message.
pub fn expand(strategy: &StrategySpec) -> Result<Expansion, String> {
    let params = strategy.params.clone().unwrap_or(Value::Null);
    let schema = match strategy.kind.as_str() {
        "single_run" => json!({
            "type": "object",
            "properties": {
                "parallel_tools": { "type": "boolean" },
                "moderate": { "type": "boolean" }
            },
            "additionalProperties": false
        }),
        "react" => json!({
            "type": "object",
            "properties": { "reasoning_prompt": { "type": "string" } },
            "additionalProperties": false
        }),
        "verified" => json!({
            "type": "object",
            "properties": { "verify_prompt": { "type": "string" } },
            "additionalProperties": false
        }),
        "chat" => json!({
            "type": "object",
            "properties": {
                "max_messages": { "type": "integer", "minimum": 1 },
                "keep": { "type": "integer", "minimum": 0 },
                "parallel_tools": { "type": "boolean" }
            },
            "additionalProperties": false
        }),
        other => {
            return Err(format!(
                "unknown strategy '{other}' (known: single_run, react, verified, chat)"
            ));
        }
    };
    check_params(&schema, &params)
        .map_err(|reason| format!("strategy '{}': {reason}", strategy.kind))?;
    let graph = |graph| Expansion {
        graph,
        types: Vec::new(),
    };
    Ok(match strategy.kind.as_str() {
        "chat" => {
            let max_messages = params["max_messages"].as_u64().unwrap_or(CHAT_MAX_MESSAGES);
            let keep = params["keep"].as_u64().unwrap_or(CHAT_KEEP);
            if keep >= max_messages {
                return Err(format!(
                    "strategy 'chat': \"keep\" ({keep}) must be less than \"max_messages\" ({max_messages})"
                ));
            }
            graph(chat(
                max_messages,
                keep,
                params["parallel_tools"].as_bool() != Some(false),
            ))
        }
        "react" => graph(react(
            params.get("reasoning_prompt").and_then(Value::as_str),
        )),
        "verified" => Expansion {
            graph: verified(params.get("verify_prompt").and_then(Value::as_str)),
            types: vec![(
                VERIFICATION_TYPE.into(),
                json!({
                    "type": "object",
                    "properties": {
                        "correct": { "type": "boolean" },
                        "feedback": { "type": "string" },
                        "answer": { "type": "string" }
                    },
                    "required": ["correct", "feedback"]
                }),
            )],
        },
        _ => {
            let task = single_run(params["parallel_tools"].as_bool() == Some(true));
            if params["moderate"].as_bool() == Some(true) {
                Expansion {
                    graph: moderated(task),
                    types: vec![(MODERATION_TYPE.into(), moderation_schema())],
                }
            } else {
                graph(task)
            }
        }
    })
}

/// Checks the input before `task` (ADR-0066): harmful input ends with the refusal from
/// `message`; otherwise the text goes to the first node of `task`.
fn moderated(mut task: GraphSpec) -> GraphSpec {
    let first = task
        .edges
        .iter()
        .position(|edge| edge.from == "start")
        .expect("every strategy begins at start");
    let entry = std::mem::replace(&mut task.edges[first].to, "moderate".into());
    let field = |field: &str| Some(json!({ "type": MODERATION_TYPE, "field": field }));
    task.nodes.extend([
        node(
            "moderate",
            "llm_moderate",
            Some(json!({ "type": MODERATION_TYPE })),
        ),
        node("refuse", "get_field", field("message")),
        node("pass", "get_field", field("text")),
    ]);
    task.edges.extend([
        EdgeSpec {
            from: "moderate".into(),
            transform: None,
            to: "refuse".into(),
            when: Some(ConditionSpec {
                kind: "on_field".into(),
                params: Some(json!({ "field": "harmful", "equals": true })),
            }),
        },
        edge("moderate", "pass", None),
        edge("refuse", "finish", None),
        edge("pass", &entry, None),
    ]);
    task
}

/// Schema of the verdict of `llm_moderate` (ADR-0066).
fn moderation_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "harmful": { "type": "boolean" },
            "categories": { "type": "array", "items": { "type": "string" } },
            "text": { "type": "string" },
            "message": { "type": "string" }
        },
        "required": ["harmful", "categories", "text", "message"]
    })
}

/// Task with verification (F05.13): tool loop until the answer, then check it; on
/// rejection the feedback goes back as a new task, otherwise the answer goes to `finish`.
fn verified(prompt: Option<&str>) -> GraphSpec {
    let mut task = single_run(false);
    for edge in &mut task.edges {
        if edge.to == "finish" {
            edge.to = "verify".into();
        }
    }
    let mut verify = json!({ "type": VERIFICATION_TYPE });
    if let Some(prompt) = prompt {
        verify["prompt"] = json!(prompt);
    }
    let field = |field: &str| Some(json!({ "type": VERIFICATION_TYPE, "field": field }));
    task.nodes.extend([
        node("verify", "llm_verify", Some(verify)),
        node("accept", "get_field", field("answer")),
        node("revise", "get_field", field("feedback")),
    ]);
    task.edges.extend([
        EdgeSpec {
            from: "verify".into(),
            transform: None,
            to: "accept".into(),
            when: Some(ConditionSpec {
                kind: "on_field".into(),
                params: Some(json!({ "field": "correct", "equals": true })),
            }),
        },
        edge("verify", "revise", None),
        edge("accept", "finish", None),
        edge("revise", "call_llm", None),
    ]);
    task
}

/// ReAct: think before the first action and after each tool result without tools, then
/// continue with tools (`llm_reason` → `llm_continue`).
fn react(prompt: Option<&str>) -> GraphSpec {
    let reason = |value: &str| {
        let mut params = json!({ "value": value });
        if let Some(prompt) = prompt {
            params["prompt"] = json!(prompt);
        }
        Some(params)
    };
    let nodes = vec![
        node("think", "llm_reason", reason("string")),
        node("call_llm", "llm_continue", None),
        node("execute_tool", "execute_tool", None),
        node("reflect", "llm_reason", reason("tool_result")),
        in_order_node(),
        node("reflect_all", "llm_reason", reason("tool_results")),
    ];
    let mut edges = vec![
        edge("start", "think", None),
        edge("think", "call_llm", None),
    ];
    edges.extend(route_answer(
        "call_llm",
        "execute_tool",
        "execute_tools_in_order",
    ));
    edges.extend([
        edge("execute_tool", "reflect", None),
        edge("reflect", "call_llm", None),
        edge("execute_tools_in_order", "reflect_all", None),
        edge("reflect_all", "call_llm", None),
    ]);
    GraphSpec { nodes, edges }
}

/// Default of the `chat` strategy: from this many messages on, the history is compressed (ADR-0076).
const CHAT_MAX_MESSAGES: u64 = 30;

/// Default of the `chat` strategy: this many messages stay verbatim when compressing.
const CHAT_KEEP: u64 = 10;

/// Conversation (ADR-0076): if the history is longer than `max_messages`, all messages except
/// the last `keep` are condensed into facts before the answer; then the tool loop.
fn chat(max_messages: u64, keep: u64, parallel_tools: bool) -> GraphSpec {
    let mut task = single_run(parallel_tools);
    task.nodes.insert(
        0,
        node(
            "compress",
            "compress_history",
            Some(json!({ "value": STRING_TYPE, "strategy": "facts", "keep": keep })),
        ),
    );
    task.edges.retain(|edge| edge.from != "start");
    task.edges.splice(
        0..0,
        [
            EdgeSpec {
                from: "start".into(),
                transform: None,
                to: "compress".into(),
                when: Some(ConditionSpec {
                    kind: "on_history_longer".into(),
                    params: Some(json!({ "messages": max_messages })),
                }),
            },
            edge("start", "call_llm", None),
            edge("compress", "call_llm", None),
        ],
    );
    task
}

/// Tool loop: ask the LLM, run the tools, return the results until a text answer comes.
///
/// Sequential mode (without `parallel_tools`) has two paths: an answer with exactly one tool
/// call takes `execute_tool`/`send_result`; an answer with several calls takes
/// `execute_tools_in_order`/`send_results`, which runs the calls one after another. The
/// single-call edge comes first and the first matching edge wins (ADR-0041 addendum), so
/// `on_multiple_tool_calls` (at least one call) only gets the answers with two or more calls.
fn single_run(parallel_tools: bool) -> GraphSpec {
    if parallel_tools {
        return GraphSpec {
            nodes: vec![
                node("call_llm", "llm_request", None),
                node("execute_tools", "execute_multiple_tools", None),
                node("send_results", "llm_send_multiple_tool_results", None),
            ],
            edges: vec![
                edge("start", "call_llm", None),
                edge("call_llm", "execute_tools", Some("on_multiple_tool_calls")),
                edge("call_llm", "finish", Some("on_assistant_message")),
                edge("execute_tools", "send_results", None),
                edge(
                    "send_results",
                    "execute_tools",
                    Some("on_multiple_tool_calls"),
                ),
                edge("send_results", "finish", Some("on_assistant_message")),
            ],
        };
    }
    let several = "execute_tools_in_order";
    let mut edges = vec![edge("start", "call_llm", None)];
    edges.extend(route_answer("call_llm", "execute_tool", several));
    edges.push(edge("execute_tool", "send_result", None));
    edges.extend(route_answer("send_result", "execute_tool", several));
    edges.push(edge(several, "send_results", None));
    edges.extend(route_answer("send_results", "execute_tool", several));
    GraphSpec {
        nodes: vec![
            node("call_llm", "llm_request", None),
            node("execute_tool", "execute_tool", None),
            node("send_result", "llm_send_tool_result", None),
            in_order_node(),
            node("send_results", "llm_send_multiple_tool_results", None),
        ],
        edges,
    }
}

/// The node of the multi-call path of the sequential strategies: runs all calls of an answer
/// one after another.
fn in_order_node() -> NodeSpec {
    node(
        "execute_tools_in_order",
        "execute_multiple_tools",
        Some(json!({ "sequential": true })),
    )
}

/// Edges that route the answer of `from`: exactly one tool call to `single`, otherwise several
/// calls to `several`, a text answer to `finish`. The order matters: the first matching edge
/// wins, and `on_multiple_tool_calls` also matches a single call.
fn route_answer(from: &str, single: &str, several: &str) -> [EdgeSpec; 3] {
    [
        edge(from, single, Some("on_tool_call")),
        edge(from, several, Some("on_multiple_tool_calls")),
        edge(from, "finish", Some("on_assistant_message")),
    ]
}

fn node(id: &str, kind: &str, params: Option<Value>) -> NodeSpec {
    NodeSpec {
        id: id.into(),
        kind: kind.into(),
        params,
    }
}

fn edge(from: &str, to: &str, when: Option<&str>) -> EdgeSpec {
    EdgeSpec {
        from: from.into(),
        transform: None,
        to: to.into(),
        when: when.map(|kind| ConditionSpec {
            kind: kind.into(),
            params: None,
        }),
    }
}
