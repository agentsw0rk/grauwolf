//! Strategy templates instead of written-out graphs (F05.12, ADR-0041): loading and validating
//! expanded specs; the templates themselves are tested in `grauwolf-strategies`.

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;

use grauwolf_spec::AgentSpec;
use serde_json::{Value, json};

fn example(name: &str) -> AgentSpec {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/specs")
        .join(name);
    AgentSpec::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn with_strategy(strategy: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": strategy
        })
        .to_string(),
    )
    .unwrap()
}

#[test]
fn single_run_expands_to_the_tool_loop() {
    let sequential = with_strategy(json!({ "kind": "single_run" }));
    let parallel =
        with_strategy(json!({ "kind": "single_run", "params": { "parallel_tools": true } }));

    assert_eq!(parallel.graph, example("parallel_calculator.json").graph);
    assert_eq!(sequential.graph, example("calculator.json").graph);
    assert_eq!(sequential.validate(), Ok(()));
}

#[test]
fn react_and_verified_expand_to_valid_specs() {
    let react = with_strategy(json!({ "kind": "react" }));
    let verified = with_strategy(json!({ "kind": "verified" }));

    assert_eq!(react.validate(), Ok(()));
    assert_eq!(verified.validate(), Ok(()));
}

#[test]
fn rejects_unknown_strategies_bad_params_and_strategy_next_to_graph() {
    let load = |extra: Value| {
        let mut spec = json!({ "spec_version": 1, "name": "agent", "model": { "provider": "ollama", "id": "m" } });
        spec.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        AgentSpec::from_json(&spec.to_string())
            .unwrap_err()
            .to_string()
    };

    let unknown = load(json!({ "strategy": { "kind": "magic" } }));
    let bad_param = load(json!({ "strategy": { "kind": "single_run", "params": { "speed": 3 } } }));
    let both = load(json!({
        "strategy": { "kind": "single_run" },
        "graph": { "edges": [ { "from": "start", "to": "finish" } ] }
    }));

    assert!(
        unknown.contains("/strategy: unknown strategy 'magic'"),
        "{unknown}"
    );
    assert!(
        bad_param.contains("/speed: unknown parameter"),
        "{bad_param}"
    );
    assert!(
        both.contains("`strategy` and `graph` are mutually exclusive"),
        "{both}"
    );
}

#[test]
fn chat_compresses_long_histories_into_facts_before_the_tool_loop() {
    let defaults = with_strategy(json!({ "kind": "chat" }));
    let tuned = with_strategy(
        json!({ "kind": "chat", "params": { "max_messages": 8, "keep": 2, "parallel_tools": false } }),
    );
    let bad = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "chat", "params": { "max_messages": 4, "keep": 4 } }
        })
        .to_string(),
    );

    assert_eq!(defaults.graph, example("chat_assistant.json").graph);
    assert_eq!(defaults.validate(), Ok(()));
    let compress = &tuned.graph.nodes[0];
    assert_eq!(compress.params.as_ref().unwrap()["keep"], 2);
    assert_eq!(
        tuned.graph.edges[0]
            .when
            .as_ref()
            .unwrap()
            .params
            .as_ref()
            .unwrap()["messages"],
        8
    );
    assert!(
        tuned
            .graph
            .nodes
            .iter()
            .any(|node| node.kind == "execute_tool")
    );
    let error = bad.unwrap_err().to_string();
    assert!(
        error.contains("keep") && error.contains("max_messages"),
        "{error}"
    );
}
