//! GOAP (F10.4–F10.6): actions with a precondition, expected effect and cost; A* searches
//! for the cheapest path to the goal, and after each step planning restarts from the actual state.

use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use grauwolf_agents::AgentContext;
use grauwolf_llm::PromptExecutor;
use grauwolf_planner::goap::{Action, Goal, GoapPlanner};
use grauwolf_planner::run_planner;
use grauwolf_prompt::{LlModel, Prompt};
use grauwolf_test::MockLlmClient;

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

/// Movement on a number line; `log` records the executed actions.
fn movement(
    log: &Arc<Mutex<Vec<&'static str>>>,
    jump_lands_at: i32,
) -> GoapPlanner<i32, String, i32> {
    let (step_log, jump_log) = (Arc::clone(log), Arc::clone(log));
    GoapPlanner::builder(|start: i32| start, |position: i32| format!("at {position}"))
        .action(
            Action::new(
                "step",
                |_| true,
                |position| position + 1,
                move |_ctx, position| {
                    step_log.lock().unwrap().push("step");
                    Box::pin(async move { Ok(position + 1) })
                },
            )
            .cost(|_| 1.0),
        )
        .action(
            Action::new(
                "jump",
                |_| true,
                |position| position + 3,
                move |_ctx, position| {
                    jump_log.lock().unwrap().push("jump");
                    Box::pin(async move { Ok(position + jump_lands_at) })
                },
            )
            .cost(|_| 2.5),
        )
        .goal(
            Goal::new("at_six", |position| *position == 6)
                .heuristic(|position: &i32| (6 - position).abs() as f64 / 3.0),
        )
        .build()
}

#[test]
fn executes_the_cheapest_action_sequence_to_the_goal() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let planner = movement(&log, 3);

    let output = block_on(run_planner(&planner, &mut context(), 0, 10)).unwrap();

    assert_eq!(output, "at 6");
    assert_eq!(*log.lock().unwrap(), ["jump", "jump"]);
}

#[test]
fn replans_from_the_actual_state_when_an_action_falls_short_of_its_belief() {
    let log = Arc::new(Mutex::new(Vec::new()));
    // Jumps land only 2 ahead instead of the expected 3.
    let planner = movement(&log, 2);

    let output = block_on(run_planner(&planner, &mut context(), 0, 10)).unwrap();

    // The plan was "jump, jump"; after the short jump, two more actions are needed.
    assert_eq!(output, "at 6");
    let log = log.lock().unwrap();
    assert_eq!(log.len(), 4, "{log:?}");
    assert_eq!(log[0], "jump");
}

#[test]
fn reports_when_no_goal_is_reachable() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let planner = movement(&log, 3);

    let error = block_on(run_planner(&planner, &mut context(), 7, 10)).unwrap_err();

    assert!(
        matches!(error, grauwolf_planner::PlannerError::NoPlan),
        "{error}"
    );
}
