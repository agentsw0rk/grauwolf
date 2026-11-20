//! The plan loop (F10.1): plan, execute a step, check until the plan is fulfilled.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::AgentContext;
use grauwolf_agents::events::{AgentEvent, Feature};
use grauwolf_llm::PromptExecutor;
use grauwolf_planner::{Planner, PlannerError, run_planner};
use grauwolf_prompt::{LlModel, Prompt};
use grauwolf_test::MockLlmClient;
use serde_json::json;

/// Counts up to the target; the plan is the target number, each step counts one further.
struct CountTo {
    target: u32,
    plans: AtomicUsize,
}

impl Planner for CountTo {
    type Input = u32;
    type Output = String;
    type State = u32;
    type Plan = u32;

    fn initial_state(&self, input: u32) -> u32 {
        input
    }

    fn build_plan<'a>(
        &'a self,
        _ctx: &'a mut AgentContext,
        _state: &'a u32,
        plan: Option<u32>,
    ) -> BoxFuture<'a, Result<u32, PlannerError>> {
        self.plans.fetch_add(1, Ordering::Relaxed);
        Box::pin(async move { Ok(plan.unwrap_or(self.target)) })
    }

    fn execute_step<'a>(
        &'a self,
        _ctx: &'a mut AgentContext,
        state: u32,
        _plan: &'a mut u32,
    ) -> BoxFuture<'a, Result<u32, PlannerError>> {
        Box::pin(async move { Ok(state + 1) })
    }

    fn is_completed<'a>(
        &'a self,
        _ctx: &'a mut AgentContext,
        state: &'a u32,
        plan: &'a u32,
    ) -> BoxFuture<'a, Result<bool, PlannerError>> {
        Box::pin(async move { Ok(state >= plan) })
    }

    fn output(&self, state: u32) -> String {
        format!("at {state}")
    }
}

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

#[test]
fn plans_and_executes_steps_until_the_plan_is_completed() {
    let planner = CountTo {
        target: 3,
        plans: AtomicUsize::new(0),
    };

    let output = block_on(run_planner(&planner, &mut context(), 0, 10)).unwrap();

    assert_eq!(output, "at 3");
    assert_eq!(planner.plans.load(Ordering::Relaxed), 3);
}

/// Collects all events.
#[derive(Default)]
struct Recorder(std::sync::Mutex<Vec<AgentEvent>>);

impl Feature for Recorder {
    fn on_event(&self, event: &AgentEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

#[test]
fn reports_each_plan_and_executed_step_as_events() {
    let planner = CountTo {
        target: 2,
        plans: AtomicUsize::new(0),
    };
    let recorder = Arc::new(Recorder::default());
    let mut ctx = context().install(recorder.clone());

    block_on(run_planner(&planner, &mut ctx, 0, 10)).unwrap();

    assert_eq!(
        *recorder.0.lock().unwrap(),
        [
            AgentEvent::PlanBuilt {
                iteration: 1,
                plan: json!(2)
            },
            AgentEvent::PlanStepExecuted {
                iteration: 1,
                plan: json!(2)
            },
            AgentEvent::PlanBuilt {
                iteration: 2,
                plan: json!(2)
            },
            AgentEvent::PlanStepExecuted {
                iteration: 2,
                plan: json!(2)
            },
        ]
    );
}
