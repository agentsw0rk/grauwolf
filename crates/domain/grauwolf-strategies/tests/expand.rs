//! Expansion of the strategy templates (F05.12, ADR-0041) without loading or validating a spec.

use grauwolf_spec_model::{GraphSpec, StrategySpec};
use grauwolf_strategies::{Expansion, expand};
use serde_json::{Value, json};

/// Expansion in the shape the assertions read: `.graph` and `.types` like an expanded spec.
struct Expanded {
    graph: GraphSpec,
    types: serde_json::Map<String, Value>,
}

fn try_expand(strategy: Value) -> Result<Expansion, String> {
    expand(&serde_json::from_value::<StrategySpec>(strategy).unwrap())
}

fn expand_json(strategy: Value) -> Expanded {
    let expansion = try_expand(strategy).unwrap();
    Expanded {
        graph: expansion.graph,
        types: expansion.types.into_iter().collect(),
    }
}

#[test]
fn react_thinks_before_each_action_and_after_each_tool_result() {
    let spec = expand_json(
        json!({ "kind": "react", "params": { "reasoning_prompt": "Think it through." } }),
    );

    let nodes: Vec<(&str, &str, Option<&Value>)> = spec
        .graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node.kind.as_str(), node.params.as_ref()))
        .collect();
    let edges: Vec<(&str, &str, Option<&str>)> = spec
        .graph
        .edges
        .iter()
        .map(|edge| {
            (
                edge.from.as_str(),
                edge.to.as_str(),
                edge.when.as_ref().map(|when| when.kind.as_str()),
            )
        })
        .collect();

    assert_eq!(
        nodes,
        [
            (
                "think",
                "llm_reason",
                Some(&json!({ "value": "string", "prompt": "Think it through." }))
            ),
            ("call_llm", "llm_continue", None),
            ("execute_tool", "execute_tool", None),
            (
                "reflect",
                "llm_reason",
                Some(&json!({ "value": "tool_result", "prompt": "Think it through." }))
            ),
            (
                "execute_tools_in_order",
                "execute_multiple_tools",
                Some(&json!({ "sequential": true }))
            ),
            (
                "reflect_all",
                "llm_reason",
                Some(&json!({ "value": "tool_results", "prompt": "Think it through." }))
            ),
        ]
    );
    assert_eq!(
        edges,
        [
            ("start", "think", None),
            ("think", "call_llm", None),
            ("call_llm", "execute_tool", Some("on_tool_call")),
            (
                "call_llm",
                "execute_tools_in_order",
                Some("on_multiple_tool_calls")
            ),
            ("call_llm", "finish", Some("on_assistant_message")),
            ("execute_tool", "reflect", None),
            ("reflect", "call_llm", None),
            ("execute_tools_in_order", "reflect_all", None),
            ("reflect_all", "call_llm", None),
        ]
    );
}

#[test]
fn verified_checks_the_answer_and_revises_it_with_the_feedback() {
    let spec = expand_json(
        json!({ "kind": "verified", "params": { "verify_prompt": "Check carefully." } }),
    );

    let edges: Vec<(&str, &str, Option<&str>)> = spec
        .graph
        .edges
        .iter()
        .map(|edge| {
            (
                edge.from.as_str(),
                edge.to.as_str(),
                edge.when.as_ref().map(|when| when.kind.as_str()),
            )
        })
        .collect();

    assert_eq!(
        edges,
        [
            ("start", "call_llm", None),
            ("call_llm", "execute_tool", Some("on_tool_call")),
            (
                "call_llm",
                "execute_tools_in_order",
                Some("on_multiple_tool_calls")
            ),
            ("call_llm", "verify", Some("on_assistant_message")),
            ("execute_tool", "send_result", None),
            ("send_result", "execute_tool", Some("on_tool_call")),
            (
                "send_result",
                "execute_tools_in_order",
                Some("on_multiple_tool_calls")
            ),
            ("send_result", "verify", Some("on_assistant_message")),
            ("execute_tools_in_order", "send_results", None),
            ("send_results", "execute_tool", Some("on_tool_call")),
            (
                "send_results",
                "execute_tools_in_order",
                Some("on_multiple_tool_calls")
            ),
            ("send_results", "verify", Some("on_assistant_message")),
            ("verify", "accept", Some("on_field")),
            ("verify", "revise", None),
            ("accept", "finish", None),
            ("revise", "call_llm", None),
        ]
    );
    assert_eq!(
        spec.graph
            .nodes
            .iter()
            .find(|node| node.id == "verify")
            .unwrap()
            .params,
        Some(json!({ "type": "Verification", "prompt": "Check carefully." }))
    );
    assert_eq!(
        spec.types["Verification"]["required"],
        json!(["correct", "feedback"])
    );
}

#[test]
fn rejects_unknown_strategies_and_bad_params() {
    let unknown = try_expand(json!({ "kind": "magic" })).err().unwrap();
    let bad_param = try_expand(json!({ "kind": "single_run", "params": { "speed": 3 } }))
        .err()
        .unwrap();

    assert!(unknown.contains("unknown strategy 'magic'"), "{unknown}");
    assert!(
        bad_param.contains("/speed: unknown parameter"),
        "{bad_param}"
    );
}

#[test]
fn moderation_adds_its_verdict_type() {
    let moderated = expand_json(json!({ "kind": "single_run", "params": { "moderate": true } }));

    assert!(moderated.types.contains_key("Moderation"));
    assert!(
        moderated
            .graph
            .nodes
            .iter()
            .any(|node| node.id == "moderate")
    );
}
