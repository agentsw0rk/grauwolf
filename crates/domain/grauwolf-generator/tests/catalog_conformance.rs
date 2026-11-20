//! Every catalog kind has a generator template (ADR-0018, ADR-0021).

use grauwolf_generator::templates;
use grauwolf_graph::Catalogs;

#[test]
fn every_builtin_node_kind_has_a_template() {
    let catalogs = Catalogs::builtin();
    let missing: Vec<&str> = catalogs
        .nodes
        .kinds()
        .filter(|kind| {
            let params = catalogs.nodes.get(kind).unwrap().example_params();
            templates::node(kind, &params).is_none()
        })
        .collect();

    assert!(
        missing.is_empty(),
        "Node kinds without template: {missing:?}"
    );
}

#[test]
fn every_builtin_condition_has_a_template() {
    let catalogs = Catalogs::builtin();
    let missing: Vec<&str> = catalogs
        .conditions
        .kinds()
        .filter(|kind| {
            let params = catalogs.conditions.get(kind).unwrap().example_params();
            templates::condition(kind, &params).is_none()
        })
        .collect();

    assert!(
        missing.is_empty(),
        "Conditions without template: {missing:?}"
    );
}

#[test]
fn execute_multiple_tools_generates_the_sequential_node_only_when_asked() {
    use serde_json::json;

    assert_eq!(
        templates::node("execute_multiple_tools", &json!({ "sequential": true })).as_deref(),
        Some("grauwolf_agents::nodes::execute_tools_in_order")
    );
    assert_eq!(
        templates::node("execute_multiple_tools", &json!(null)).as_deref(),
        Some("grauwolf_agents::nodes::execute_multiple_tools")
    );
}
