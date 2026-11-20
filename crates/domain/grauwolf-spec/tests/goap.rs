//! Nodes of kind `goap` (ADR-0070).

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, ValidationErrorKind};
use serde_json::{Value, json};

fn spec(params: Value) -> AgentSpec {
    let step = |name: &str| {
        json!({ "graph": {
            "nodes": [ { "id": name, "kind": "llm_request" } ],
            "edges": [ { "from": "start", "to": name },
                       { "from": name, "to": "finish", "when": { "kind": "on_assistant_message" } } ]
        } })
    };
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "baker",
            "model": { "provider": "ollama", "id": "m" },
            "subgraphs": { "buy": step("buy"), "knead": step("knead"), "bake": step("bake") },
            "graph": {
                "nodes": [ { "id": "baker", "kind": "goap", "params": params } ],
                "edges": [ { "from": "start", "to": "baker" }, { "from": "baker", "to": "finish" } ]
            }
        })
        .to_string(),
    )
    .unwrap()
}

fn valid() -> Value {
    json!({
        "state": { "has_flour": false, "dough_ready": false, "bread_baked": false },
        "actions": [
            { "name": "buy", "subgraph": "buy", "effects": { "has_flour": true }, "cost": 2 },
            { "name": "knead", "subgraph": "knead", "pre": { "has_flour": true },
              "effects": { "dough_ready": true } },
            { "name": "bake", "subgraph": "bake", "pre": { "dough_ready": true },
              "effects": { "bread_baked": true } }
        ],
        "goals": [ { "name": "bread", "facts": { "bread_baked": true } } ],
        "max_iterations": 8
    })
}

fn reason(params: Value) -> String {
    let errors = spec(params).validate().unwrap_err();
    let error = errors
        .iter()
        .find(|error| error.path == "/graph/nodes/0/params")
        .unwrap_or_else(|| panic!("{errors:?}"));
    match &error.kind {
        ValidationErrorKind::InvalidParams { kind, reason } if kind == "goap" => reason.clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn accepts_goap_over_known_facts_and_subgraphs() {
    assert_eq!(spec(valid()).validate(), Ok(()));
}

#[test]
fn reports_unknown_facts_subgraphs_and_names() {
    let mut unknown_fact = valid();
    unknown_fact["actions"][1]["pre"] = json!({ "has_butter": true });
    let mut unknown_subgraph = valid();
    unknown_subgraph["actions"][0]["subgraph"] = json!("shop");
    let mut duplicate = valid();
    duplicate["actions"][2]["name"] = json!("buy");
    let mut no_goal = valid();
    no_goal["goals"] = json!([]);
    let mut negative = valid();
    negative["actions"][0]["cost"] = json!(-1);
    let mut extra = valid();
    extra["planner"] = json!("llm");

    assert!(reason(unknown_fact).contains("has_butter"));
    assert!(reason(unknown_subgraph).contains("shop"));
    assert!(reason(duplicate).contains("buy"));
    assert!(reason(no_goal).contains("goals"));
    assert!(reason(negative).contains("cost"));
    assert!(reason(extra).contains("planner"));
}

/// `spec graph` shows the subgraphs of the actions as included (ADR-0070).
#[test]
fn draws_the_subgraphs_of_the_actions() {
    let mermaid = spec(valid()).to_mermaid();

    for subgraph in ["buy", "knead", "bake"] {
        assert!(
            mermaid.contains(&format!("baker -.-> sg_{subgraph}")),
            "{mermaid}"
        );
    }
}

fn with_phase() -> Value {
    let mut params = valid();
    params["values"] = json!({ "phase": ["shopping", "baking", "done"] });
    params["state"]["phase"] = json!("shopping");
    params["actions"][0]["effects"]["phase"] = json!("baking");
    params["goals"][0]["facts"]["phase"] = json!("done");
    params["actions"][2]["effects"]["phase"] = json!("done");
    params
}

/// Enumerations (addendum ADR-0070): texts only from the list of their fact.
#[test]
fn accepts_enumerated_facts_with_listed_values() {
    assert_eq!(spec(with_phase()).validate(), Ok(()));
}

#[test]
fn reports_unlisted_values_and_mixed_kinds() {
    let mut unlisted = with_phase();
    unlisted["actions"][1]["pre"] = json!({ "phase": "kneading" });
    let mut mixed = with_phase();
    mixed["goals"][0]["facts"]["phase"] = json!(true);
    let mut unknown_fact = with_phase();
    unknown_fact["values"]["oven"] = json!(["on", "off"]);

    assert!(reason(unlisted).contains("kneading"));
    assert!(reason(mixed).contains("phase"));
    assert!(reason(unknown_fact).contains("oven"));
}

/// ADR-0080: typos in actions and goals are reported instead of being silently ignored.
#[test]
fn reports_unknown_keys_in_actions_and_goals() {
    let mut typo = valid();
    typo["actions"][1]["efects"] = json!({ "dough_ready": true });
    let mut goal = valid();
    goal["goals"][0]["weight"] = json!(1);

    assert!(
        reason(typo).contains("actions[1].efects"),
        "unknown key is missing"
    );
    assert!(reason(goal).contains("goals[0].weight"));
}

fn with_numbers() -> Value {
    let mut params = valid();
    params["state"]["attempts"] = json!(0);
    params["state"]["ratio"] = json!(0.5);
    params["state"]["note"] = json!("");
    params["state"]["task_text"] = json!("");
    params["actions"][1]["pre"]["attempts"] = json!({ "lt": 3 });
    params["actions"][1]["pre"]["note"] = json!({ "ne": "x" });
    params["actions"][1]["on_failure"] = json!({ "attempts": { "add": 1 } });
    params["actions"][0]["effects"]["ratio"] = json!({ "add": 0.25 });
    params["goals"][0]["facts"]["attempts"] = json!({ "ge": 0 });
    params["goals"][0]["facts"]["ratio"] = json!({ "gt": 0 });
    params["goals"][0]["priority"] = json!(2);
    params["input_fact"] = json!("task_text");
    params["output_fact"] = json!("note");
    params["max_search_nodes"] = json!(500);
    params["max_plan_depth"] = json!(20);
    params
}

/// Addendum ADR-0070: numbers, free texts, operators, `add`, priorities and limits.
#[test]
fn accepts_numbers_free_texts_operators_priorities_and_limits() {
    assert_eq!(spec(with_numbers()).validate(), Ok(()));
}

#[test]
fn reports_operators_and_values_that_do_not_fit_the_fact_type() {
    let edit = |f: &dyn Fn(&mut Value)| {
        let mut params = with_numbers();
        f(&mut params);
        reason(params)
    };

    let ordering_on_bool = edit(&|p| p["actions"][2]["pre"]["dough_ready"] = json!({ "gt": 1 }));
    let ordering_on_text = edit(&|p| p["actions"][1]["pre"]["note"] = json!({ "lt": "x" }));
    let two_operators = edit(&|p| p["goals"][0]["facts"]["attempts"] = json!({ "ge": 0, "le": 5 }));
    let unknown_operator = edit(&|p| p["goals"][0]["facts"]["attempts"] = json!({ "between": 5 }));
    let text_for_integer = edit(&|p| p["goals"][0]["facts"]["attempts"] = json!({ "ge": "1" }));
    let fraction_for_integer = edit(&|p| p["goals"][0]["facts"]["attempts"] = json!({ "ge": 1.5 }));
    let add_to_bool = edit(&|p| p["actions"][0]["effects"]["has_flour"] = json!({ "add": 1 }));
    let add_to_text = edit(&|p| p["actions"][0]["effects"]["note"] = json!({ "add": 1 }));
    let fraction_added_to_integer =
        edit(&|p| p["actions"][1]["on_failure"]["attempts"] = json!({ "add": 0.5 }));
    let operator_in_effect = edit(&|p| p["actions"][0]["effects"]["attempts"] = json!({ "gt": 1 }));
    let unknown_fact_on_failure = edit(&|p| p["actions"][1]["on_failure"]["oven"] = json!(true));

    assert!(
        ordering_on_bool.contains("dough_ready") && ordering_on_bool.contains("integer or number")
    );
    assert!(ordering_on_text.contains("note"));
    assert!(two_operators.contains("exactly one operator"));
    assert!(unknown_operator.contains("between"));
    assert!(text_for_integer.contains("attempts") && text_for_integer.contains("integer"));
    assert!(fraction_for_integer.contains("attempts"));
    assert!(add_to_bool.contains("has_flour") && add_to_bool.contains("add"));
    assert!(add_to_text.contains("note"));
    assert!(fraction_added_to_integer.contains("attempts"));
    assert!(operator_in_effect.contains("gt"));
    assert!(
        unknown_fact_on_failure.contains("oven") && unknown_fact_on_failure.contains("on_failure")
    );
}

#[test]
fn reports_executor_mistakes() {
    let edit = |f: &dyn Fn(&mut Value)| {
        let mut params = with_numbers();
        f(&mut params);
        reason(params)
    };

    let both = edit(&|p| p["actions"][0]["agent"] = json!("writer"));
    let neither = edit(&|p| {
        p["actions"][0].as_object_mut().unwrap().remove("subgraph");
    });

    assert!(both.contains("actions[0]") && both.contains("exclude"));
    assert!(neither.contains("actions[0]") && neither.contains("subgraph"));
}

#[test]
fn reports_templates_and_text_facts_that_do_not_fit() {
    let edit = |f: &dyn Fn(&mut Value)| {
        let mut params = with_numbers();
        f(&mut params);
        reason(params)
    };

    let placeholder = edit(&|p| p["actions"][0]["input"] = json!("Buy {{butter}}"));
    let unclosed = edit(&|p| p["actions"][0]["input"] = json!("Buy {{task"));
    let output_on_bool = edit(&|p| p["actions"][0]["output_fact"] = json!("has_flour"));
    let unknown_output = edit(&|p| p["actions"][0]["output_fact"] = json!("report"));
    let input_on_integer = edit(&|p| p["input_fact"] = json!("attempts"));
    let output_on_enum = edit(&|p| p["output_fact"] = json!("phase"));
    let contains = edit(&|p| p["actions"][1]["success_contains"] = json!(3));

    assert!(placeholder.contains("butter"));
    assert!(unclosed.contains("{{"));
    assert!(output_on_bool.contains("has_flour") && output_on_bool.contains("free-text"));
    assert!(unknown_output.contains("report"));
    assert!(input_on_integer.contains("attempts"));
    assert!(output_on_enum.contains("phase") || output_on_enum.contains("free-text"));
    assert!(contains.contains("success_contains"));
}

#[test]
fn accepts_placeholders_of_task_and_facts() {
    let mut params = with_numbers();
    params["actions"][0]["input"] =
        json!("Buy for {{task}} (attempt {{ attempts }}), note: {{note}}");

    assert_eq!(spec(params).validate(), Ok(()));
}

#[test]
fn reports_priorities_and_limits_out_of_range() {
    let edit = |f: &dyn Fn(&mut Value)| {
        let mut params = with_numbers();
        f(&mut params);
        reason(params)
    };

    let negative = edit(&|p| p["goals"][0]["priority"] = json!(-1));
    let fraction = edit(&|p| p["goals"][0]["priority"] = json!(0.5));
    let nodes = edit(&|p| p["max_search_nodes"] = json!(0));
    let depth = edit(&|p| p["max_plan_depth"] = json!(0));

    assert!(negative.contains("goals[0].priority"));
    assert!(fraction.contains("goals[0].priority"));
    assert!(nodes.contains("max_search_nodes"));
    assert!(depth.contains("max_plan_depth"));
}

/// An agent as executor must be in `agent_tools` (`UnknownAgent`); then it needs no subgraph.
#[test]
fn an_agent_executor_must_be_an_agent_tool() {
    let with_agent = |declare: bool| {
        let mut params = valid();
        params["actions"][0] = json!({ "name": "buy", "agent": "shopper",
            "effects": { "has_flour": true } });
        let mut value = serde_json::to_value(spec(params)).unwrap();
        if declare {
            value["agent_tools"] =
                json!({ "shopper": { "spec": "shopper.json", "description": "Shops" } });
        }
        AgentSpec::from_json(&value.to_string()).unwrap()
    };

    let errors = with_agent(false).validate().unwrap_err();
    assert!(
        errors.iter().any(|error| error.path == "/graph/nodes/0/params"
            && matches!(&error.kind, ValidationErrorKind::UnknownAgent { name } if name == "shopper")),
        "{errors:?}"
    );
    assert_eq!(with_agent(true).validate(), Ok(()));
}
