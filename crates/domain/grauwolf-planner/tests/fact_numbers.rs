//! GOAP facts with numbers, free texts and operators (addendum to ADR-0070).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use grauwolf_agents::AgentContext;
use grauwolf_graph::{Graph, TypedGraph};
use grauwolf_llm::PromptExecutor;
use grauwolf_planner::goap::fact_planner;
use grauwolf_planner::run_planner;
use grauwolf_prompt::{LlModel, Prompt};
use grauwolf_test::MockLlmClient;
use serde_json::{Value, json};

fn context() -> AgentContext {
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(MockLlmClient::new("-")))
        .build();
    AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").build(),
    )
}

/// Step graph that records its input in `log` and yields `output`.
fn step(log: &Arc<Mutex<Vec<String>>>, output: Value) -> Arc<Graph<AgentContext>> {
    let log = Arc::clone(log);
    Arc::new(
        TypedGraph::<AgentContext, String, Value>::build(|g| {
            let run = g.node("run", move |_ctx, input: String| {
                log.lock().unwrap().push(input);
                let output = output.clone();
                Box::pin(async move { Ok(output) })
            });
            g.edge(&g.start(), &run);
            g.edge(&run, &g.finish());
        })
        .unwrap()
        .into_graph(),
    )
}

/// Runs `params` with one step graph per action name that yields `"ok"`; returns the names of
/// the executed actions in order.
fn executed(params: &Value) -> Vec<String> {
    let log = Arc::new(Mutex::new(Vec::new()));
    let steps: HashMap<_, _> = params["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|action| {
            let name = action["name"].as_str().unwrap().to_owned();
            (name, step(&log, json!("ok")))
        })
        .collect();
    let planner = fact_planner(params, &steps).unwrap();
    block_on(run_planner(&planner, &mut context(), "Task".into(), 20)).unwrap();
    let actions = log.lock().unwrap();
    actions
        .iter()
        .map(|input| {
            input
                .lines()
                .nth(2)
                .unwrap()
                .trim_start_matches("Action: ")
                .to_owned()
        })
        .collect()
}

fn plan_error(params: &Value) -> String {
    let steps: HashMap<_, _> = params["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|action| {
            let name = action["name"].as_str().unwrap().to_owned();
            (name, step(&Arc::default(), json!("ok")))
        })
        .collect();
    match fact_planner(params, &steps) {
        Ok(_) => panic!("accepted"),
        Err(error) => error,
    }
}

fn counter() -> Value {
    json!({
        "state": { "attempts": 0 },
        "actions": [
            { "name": "try", "subgraph": "try", "pre": { "attempts": { "lt": 3 } },
              "effects": { "attempts": { "add": 1 } } }
        ],
        "goals": [ { "name": "enough", "facts": { "attempts": { "ge": 3 } } } ]
    })
}

#[test]
fn adds_to_integers_and_compares_them_with_operators() {
    assert_eq!(executed(&counter()), ["try", "try", "try"]);
}

#[test]
fn plans_over_numbers_with_fractions() {
    let params = json!({
        "state": { "temperature": 20.0 },
        "actions": [
            { "name": "heat", "subgraph": "heat", "pre": { "temperature": { "lt": 25 } },
              "effects": { "temperature": { "add": 0.5 } } }
        ],
        "goals": [ { "name": "warm", "facts": { "temperature": { "gt": 20.9 } } } ]
    });

    assert_eq!(executed(&params), ["heat", "heat"]);
}

#[test]
fn compares_with_ne_for_booleans_enumerations_and_free_texts() {
    let params = json!({
        "values": { "phase": ["empty", "draft"] },
        "state": { "phase": "empty", "note": "", "sent": false },
        "actions": [
            { "name": "write", "subgraph": "write", "pre": { "phase": { "ne": "draft" } },
              "effects": { "phase": "draft" } },
            { "name": "annotate", "subgraph": "annotate", "pre": { "phase": { "eq": "draft" } },
              "effects": { "note": "checked" } },
            { "name": "send", "subgraph": "send", "pre": { "note": { "ne": "" }, "sent": { "ne": true } },
              "effects": { "sent": true } }
        ],
        "goals": [ { "name": "sent", "facts": { "sent": true } } ]
    });

    assert_eq!(executed(&params), ["write", "annotate", "send"]);
}

#[test]
fn observed_numbers_overwrite_facts_of_the_same_name() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let steps = HashMap::from([("try".to_owned(), step(&log, json!({ "attempts": 5 })))]);
    let planner = fact_planner(&counter(), &steps).unwrap();

    block_on(run_planner(&planner, &mut context(), "Task".into(), 20)).unwrap();

    assert_eq!(log.lock().unwrap().len(), 1);
}

#[test]
fn refuses_operators_and_values_that_do_not_fit_the_fact() {
    let with = |fact: Value, pre: Value, effects: Value| {
        json!({
            "values": { "phase": ["a", "b"] },
            "state": { "flag": false, "phase": "a", "count": 0, "ratio": 0.5, "note": "" },
            "actions": [ { "name": "go", "subgraph": "go", "pre": { fact.as_str().unwrap(): pre },
                           "effects": effects } ],
            "goals": [ { "name": "g", "facts": { "count": 1 } } ]
        })
    };
    let fails = |fact: &str, pre: Value, effects: Value, needle: &str| {
        let error = plan_error(&with(json!(fact), pre, effects));
        assert!(error.contains(needle), "{error}");
    };

    fails("flag", json!({ "gt": 1 }), json!({}), "flag");
    fails("note", json!({ "lt": "a" }), json!({}), "note");
    fails("phase", json!({ "eq": "c" }), json!({}), "phase");
    fails("count", json!({ "lt": "3" }), json!({}), "count");
    fails("count", json!({ "lt": 1.5 }), json!({}), "count");
    fails("count", json!({ "lt": 1, "gt": 0 }), json!({}), "count");
    fails("count", json!({ "between": 1 }), json!({}), "between");
    fails(
        "flag",
        json!(false),
        json!({ "flag": { "add": 1 } }),
        "flag",
    );
    fails(
        "count",
        json!(0),
        json!({ "count": { "add": 0.5 } }),
        "count",
    );
}
