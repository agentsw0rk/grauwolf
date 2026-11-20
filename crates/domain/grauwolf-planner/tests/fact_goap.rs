//! GOAP over named facts from data (ADR-0070).

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

fn params() -> Value {
    json!({
        "state": { "has_flour": false, "dough_ready": false, "bread_baked": false },
        "actions": [
            { "name": "bake", "subgraph": "bake", "pre": { "dough_ready": true },
              "effects": { "bread_baked": true } },
            { "name": "buy", "subgraph": "buy", "effects": { "has_flour": true }, "cost": 2 },
            { "name": "knead", "subgraph": "knead", "pre": { "has_flour": true },
              "effects": { "dough_ready": true }, "description": "Knead for ten minutes." }
        ],
        "goals": [ { "name": "bread", "facts": { "bread_baked": true } } ]
    })
}

#[test]
fn plans_the_actions_from_data_and_returns_the_output_of_the_last_one() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let steps = HashMap::from([
        ("buy".to_owned(), step(&log, json!("Flour bought."))),
        ("knead".to_owned(), step(&log, json!("Dough kneaded."))),
        ("bake".to_owned(), step(&log, json!("Bread baked."))),
    ]);
    let planner = fact_planner(&params(), &steps).unwrap();

    let output = block_on(run_planner(
        &planner,
        &mut context(),
        "Bake a loaf of bread.".into(),
        10,
    ))
    .unwrap();

    assert_eq!(output, "Bread baked.");
    assert_eq!(
        *log.lock().unwrap(),
        [
            "Bake a loaf of bread.\n\nAction: buy",
            "Bake a loaf of bread.\n\nAction: knead\nKnead for ten minutes.",
            "Bake a loaf of bread.\n\nAction: bake",
        ]
    );
}

/// Step graph that yields `first` the first time, then `later`.
fn flaky_step(
    log: &Arc<Mutex<Vec<String>>>,
    first: Value,
    later: Value,
) -> Arc<Graph<AgentContext>> {
    let log = Arc::clone(log);
    let calls = Arc::new(Mutex::new(0));
    Arc::new(
        TypedGraph::<AgentContext, String, Value>::build(|g| {
            let run = g.node("run", move |_ctx, input: String| {
                log.lock().unwrap().push(input);
                let mut calls = calls.lock().unwrap();
                *calls += 1;
                let output = if *calls == 1 {
                    first.clone()
                } else {
                    later.clone()
                };
                Box::pin(async move { Ok(output) })
            });
            g.edge(&g.start(), &run);
            g.edge(&run, &g.finish());
        })
        .unwrap()
        .into_graph(),
    )
}

/// If a step reports a different state than expected, GOAP replans (ADR-0070).
#[test]
fn replans_when_a_step_reports_a_different_state() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let steps = HashMap::from([
        ("buy".to_owned(), step(&log, json!("Flour bought."))),
        (
            "knead".to_owned(),
            flaky_step(
                &log,
                json!({ "dough_ready": false }),
                json!({ "dough_ready": true }),
            ),
        ),
        ("bake".to_owned(), step(&log, json!("Bread baked."))),
    ]);
    let planner = fact_planner(&params(), &steps).unwrap();

    let output = block_on(run_planner(&planner, &mut context(), "Bread".into(), 10)).unwrap();

    let actions: Vec<String> = log
        .lock()
        .unwrap()
        .iter()
        .map(|input| input.lines().nth(2).unwrap().to_owned())
        .collect();
    assert_eq!(output, "Bread baked.");
    assert_eq!(
        actions,
        [
            "Action: buy",
            "Action: knead",
            "Action: knead",
            "Action: bake"
        ]
    );
}

/// As a node (ADR-0070): every planner with text input and output becomes a node via
/// `planner_node`.
#[test]
fn runs_as_a_node_of_a_graph() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let steps = HashMap::from([
        ("buy".to_owned(), step(&log, json!("Flour bought."))),
        ("knead".to_owned(), step(&log, json!("Dough kneaded."))),
        ("bake".to_owned(), step(&log, json!("Bread baked."))),
    ]);
    let planner = Arc::new(fact_planner(&params(), &steps).unwrap());
    let graph = TypedGraph::<AgentContext, String, String>::build(|g| {
        let baker = g.node(
            "baker",
            grauwolf_planner::planner_node(planner, 10, "baker"),
        );
        g.edge(&g.start(), &baker);
        g.edge(&baker, &g.finish());
    })
    .unwrap();

    let output = block_on(graph.run(&mut context(), "Bake a loaf of bread.".into())).unwrap();

    assert_eq!(output, "Bread baked.");
    assert_eq!(log.lock().unwrap().len(), 3);
}

/// Enumerations (addendum to ADR-0070): facts with text from a fixed list.
#[test]
fn plans_over_enumerated_facts_and_observes_allowed_values_only() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let steps = HashMap::from([
        ("write".to_owned(), step(&log, json!("Draft written."))),
        // The review first reports `needs_revision` instead of the expected effect, then `done`.
        (
            "review".to_owned(),
            flaky_step(
                &log,
                json!({ "phase": "needs_revision" }),
                json!({ "phase": "done" }),
            ),
        ),
        ("revise".to_owned(), step(&log, json!("Revised."))),
    ]);
    let params = json!({
        "values": { "phase": ["empty", "draft", "needs_revision", "done"] },
        "state": { "phase": "empty" },
        "actions": [
            { "name": "write", "subgraph": "write", "pre": { "phase": "empty" },
              "effects": { "phase": "draft" } },
            { "name": "review", "subgraph": "review", "pre": { "phase": "draft" },
              "effects": { "phase": "done" } },
            { "name": "revise", "subgraph": "revise", "pre": { "phase": "needs_revision" },
              "effects": { "phase": "draft" } }
        ],
        "goals": [ { "name": "done", "facts": { "phase": "done" } } ]
    });
    let planner = fact_planner(&params, &steps).unwrap();

    block_on(run_planner(&planner, &mut context(), "Brief".into(), 10)).unwrap();

    let actions: Vec<String> = log
        .lock()
        .unwrap()
        .iter()
        .map(|input| input.lines().nth(2).unwrap().to_owned())
        .collect();
    assert_eq!(
        actions,
        [
            "Action: write",
            "Action: review",
            "Action: revise",
            "Action: review"
        ]
    );
}

/// `06-goap.json` of the user guide: the node `edit`.
fn newsroom() -> Value {
    json!({
        "state": { "draft": "none", "facts_checked": false },
        "values": { "draft": ["none", "rough", "final"] },
        "actions": [
            { "name": "write_rough_draft", "subgraph": "write",
              "pre": { "draft": "none" }, "effects": { "draft": "rough" } },
            { "name": "revise", "subgraph": "write",
              "pre": { "draft": "rough" }, "effects": { "draft": "final" } },
            { "name": "check_facts", "subgraph": "check", "cost": 3,
              "pre": { "draft": "final" }, "effects": { "facts_checked": true } }
        ],
        "goals": [ { "name": "publish", "facts": { "draft": "final", "facts_checked": true } } ],
        "max_iterations": 6
    })
}

#[test]
fn the_expected_plan_is_the_plan_for_the_start_state_without_graphs() {
    assert_eq!(
        grauwolf_planner::goap::expected_plan(&newsroom()),
        Ok(vec![
            "write_rough_draft".to_owned(),
            "revise".to_owned(),
            "check_facts".to_owned()
        ])
    );
    assert_eq!(
        grauwolf_planner::goap::expected_plan(&params()),
        Ok(vec![
            "buy".to_owned(),
            "knead".to_owned(),
            "bake".to_owned()
        ])
    );
}

#[test]
fn without_a_reachable_goal_there_is_no_expected_plan() {
    let mut params = newsroom();
    params["actions"].as_array_mut().unwrap().remove(1);

    let reason = grauwolf_planner::goap::expected_plan(&params).unwrap_err();

    assert!(reason.contains("no plan found"), "{reason}");
}

#[test]
fn invalid_parameters_give_no_expected_plan_but_the_reason() {
    let mut params = newsroom();
    params["actions"][0]["pre"] = json!({ "unknown_fact": true });

    let reason = grauwolf_planner::goap::expected_plan(&params).unwrap_err();

    assert!(reason.contains("unknown_fact"), "{reason}");
}
