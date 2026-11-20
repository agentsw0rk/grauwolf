//! Goal priorities and the bounded search of the data GOAP (addendum to ADR-0070).

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

/// Step graph that records the action it is asked for in `log` and answers "ok".
fn step(log: &Arc<Mutex<Vec<String>>>) -> Arc<Graph<AgentContext>> {
    let log = Arc::clone(log);
    Arc::new(
        TypedGraph::<AgentContext, String, String>::build(|g| {
            let run = g.node("run", move |_ctx, input: String| {
                log.lock()
                    .unwrap()
                    .push(input.lines().nth(2).unwrap().to_owned());
                Box::pin(async { Ok("ok".to_owned()) })
            });
            g.edge(&g.start(), &run);
            g.edge(&run, &g.finish());
        })
        .unwrap()
        .into_graph(),
    )
}

/// Runs `params`; returns the executed actions, or the error of the planner.
fn run(params: &Value) -> Result<Vec<String>, String> {
    let log = Arc::new(Mutex::new(Vec::new()));
    let steps: HashMap<_, _> = params["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|action| (action["subgraph"].as_str().unwrap().to_owned(), step(&log)))
        .collect();
    let planner = fact_planner(params, &steps)?;
    block_on(run_planner(&planner, &mut context(), "Task".into(), 100))
        .map_err(|error| error.to_string())?;
    let log = log.lock().unwrap();
    Ok(log.clone())
}

/// Two goals, each reached by one action: `a` costs `cost_a` (goal priority `priority_a`), `b`
/// costs `cost_b` (priority 0).
fn two_goals(cost_a: f64, priority_a: u64, cost_b: f64) -> Value {
    json!({
        "state": { "a": false, "b": false },
        "actions": [
            { "name": "do_a", "subgraph": "a", "effects": { "a": true }, "cost": cost_a },
            { "name": "do_b", "subgraph": "b", "effects": { "b": true }, "cost": cost_b }
        ],
        "goals": [
            { "name": "goal_a", "facts": { "a": true }, "priority": priority_a },
            { "name": "goal_b", "facts": { "b": true } }
        ]
    })
}

#[test]
fn pursues_the_goal_with_the_smallest_cost_divided_by_priority_plus_one() {
    // a: 3 / (3 + 1) = 0.75 beats b: 1 / 1, although b is cheaper in absolute terms.
    assert_eq!(run(&two_goals(3.0, 3, 1.0)).unwrap(), ["Action: do_a"]);
    // a: 3 / 2 = 1.5 loses against b: 1.
    assert_eq!(run(&two_goals(3.0, 1, 1.0)).unwrap(), ["Action: do_b"]);
}

#[test]
fn equal_ratios_go_to_the_goal_declared_first() {
    // a: 2 / 2 = 1 and b: 1 / 1 = 1.
    assert_eq!(run(&two_goals(2.0, 1, 1.0)).unwrap(), ["Action: do_a"]);
}

fn counter(goal: u64) -> Value {
    json!({
        "state": { "n": 0 },
        "actions": [ { "name": "grow", "subgraph": "grow", "effects": { "n": { "add": 1 } } } ],
        "goals": [ { "name": "big", "facts": { "n": { "ge": goal } } } ]
    })
}

#[test]
fn the_plan_depth_bounds_the_search_over_numbers() {
    // Default depth 50: 60 steps are out of reach, 40 are fine.
    let error = run(&counter(60)).unwrap_err();
    assert!(
        error.contains("no plan") && error.contains("limit"),
        "{error}"
    );
    assert_eq!(run(&counter(40)).unwrap().len(), 40);

    let mut shallow = counter(5);
    shallow["max_plan_depth"] = json!(4);
    assert!(run(&shallow).is_err());
    shallow["max_plan_depth"] = json!(5);
    assert_eq!(run(&shallow).unwrap().len(), 5);
}

#[test]
fn the_number_of_search_nodes_is_limited() {
    let mut limited = counter(10);
    limited["max_search_nodes"] = json!(3);
    assert!(run(&limited).is_err());
    limited["max_search_nodes"] = json!(100);
    assert_eq!(run(&limited).unwrap().len(), 10);
}

#[test]
fn refuses_negative_priorities_and_limits_below_one() {
    let mut priority = two_goals(1.0, 0, 1.0);
    priority["goals"][0]["priority"] = json!(-1);
    let mut nodes = counter(1);
    nodes["max_search_nodes"] = json!(0);
    let mut depth = counter(1);
    depth["max_plan_depth"] = json!(0);

    assert!(run(&priority).unwrap_err().contains("priority"));
    assert!(run(&nodes).unwrap_err().contains("max_search_nodes"));
    assert!(run(&depth).unwrap_err().contains("max_plan_depth"));
}
