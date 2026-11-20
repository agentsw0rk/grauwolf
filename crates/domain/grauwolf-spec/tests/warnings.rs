//! Warnings: findings that never fail the validation.

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, SpecWarning};
use serde_json::{Value, json};

fn spec(graph: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "graph": graph
        })
        .to_string(),
    )
    .unwrap()
}

fn strategy(kind: &str, params: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": kind, "params": params }
        })
        .to_string(),
    )
    .unwrap()
}

/// The tool loop of the first guides: one tool call per answer, nothing for several.
fn single_call_loop() -> Value {
    json!({
        "nodes": [
            { "id": "call_llm", "kind": "llm_request" },
            { "id": "execute_tool", "kind": "execute_tool" },
            { "id": "send_result", "kind": "llm_send_tool_result" }
        ],
        "edges": [
            { "from": "start", "to": "call_llm" },
            { "from": "call_llm", "to": "execute_tool", "when": { "kind": "on_tool_call" } },
            { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } },
            { "from": "execute_tool", "to": "send_result" },
            { "from": "send_result", "to": "execute_tool", "when": { "kind": "on_tool_call" } },
            { "from": "send_result", "to": "finish", "when": { "kind": "on_assistant_message" } }
        ]
    })
}

#[test]
fn warns_at_every_node_with_a_single_call_edge_but_no_multi_call_edge() {
    let spec = spec(single_call_loop());

    assert_eq!(spec.validate(), Ok(()));
    let pointers: Vec<String> = spec.warnings().into_iter().map(|w| w.pointer).collect();
    assert_eq!(pointers, ["/graph/nodes/0", "/graph/nodes/2"]);
    let message = &spec.warnings()[0].message;
    assert!(message.contains("`on_multiple_tool_calls`"), "{message}");
    assert!(message.contains("no matching edge"), "{message}");
}

#[test]
fn the_strategies_have_no_warnings() {
    let strategies = [
        ("single_run", json!({})),
        ("single_run", json!({ "parallel_tools": true })),
        ("react", json!({ "reasoning_prompt": "Think." })),
        ("verified", json!({ "verify_prompt": "Check." })),
        ("chat", json!({})),
        ("chat", json!({ "parallel_tools": true })),
    ];
    for (kind, params) in strategies {
        let spec = strategy(kind, params.clone());
        assert_eq!(
            spec.warnings(),
            Vec::<SpecWarning>::new(),
            "{kind} {params}"
        );
    }
}

#[test]
fn a_multi_call_edge_silences_the_warning() {
    let mut graph = single_call_loop();
    graph["nodes"].as_array_mut().unwrap().push(
        json!({ "id": "all", "kind": "execute_multiple_tools", "params": { "sequential": true } }),
    );
    graph["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "send_all", "kind": "llm_send_multiple_tool_results" }));
    let edges = graph["edges"].as_array_mut().unwrap();
    edges.push(
        json!({ "from": "call_llm", "to": "all", "when": { "kind": "on_multiple_tool_calls" } }),
    );
    edges.push(
        json!({ "from": "send_result", "to": "all", "when": { "kind": "on_multiple_tool_calls" } }),
    );
    edges.push(json!({ "from": "all", "to": "send_all" }));
    edges.push(
        json!({ "from": "send_all", "to": "finish", "when": { "kind": "on_assistant_message" } }),
    );

    assert_eq!(spec(graph).warnings(), Vec::<SpecWarning>::new());
}
