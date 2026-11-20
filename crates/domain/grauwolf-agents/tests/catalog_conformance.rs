//! Every catalog kind has an implementation (ADR-0018, ADR-0021).

use grauwolf_agents::implementations;
use grauwolf_graph::Catalogs;
use std::collections::BTreeMap;

use serde_json::json;

#[test]
fn every_builtin_node_kind_has_an_implementation() {
    let catalogs = Catalogs::builtin();
    // The example parameters name their type `Example`.
    let types = BTreeMap::from([("Example".to_string(), json!({ "type": "object" }))]);
    let missing: Vec<&str> = catalogs
        .nodes
        .kinds()
        .filter(|kind| {
            let params = catalogs.nodes.get(kind).unwrap().example_params();
            implementations::node(kind, &params, &types).is_none()
        })
        .collect();

    assert!(
        missing.is_empty(),
        "Node kinds without implementation: {missing:?}"
    );
}

#[test]
fn every_builtin_condition_has_an_implementation() {
    let catalogs = Catalogs::builtin();
    let missing: Vec<&str> = catalogs
        .conditions
        .kinds()
        .filter(|kind| {
            let params = catalogs.conditions.get(kind).unwrap().example_params();
            implementations::condition(kind, &params).is_none()
        })
        .collect();

    assert!(
        missing.is_empty(),
        "Conditions without implementation: {missing:?}"
    );
}
