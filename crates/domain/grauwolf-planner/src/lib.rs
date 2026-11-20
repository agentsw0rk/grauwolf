//! Planner agents (E10, ADR-0039): plan, execute a step, check – until the plan is fulfilled.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_agents::AgentContext;
use grauwolf_agents::events::AgentEvent;
use grauwolf_graph::NodeError;

mod facts;
pub mod goap;
mod llm;

pub use llm::{LlmPlanner, Plan, PlanState, PlanStep};

/// Errors of a planner run.
#[derive(Debug, thiserror::Error)]
pub enum PlannerError {
    /// The plan was not fulfilled after `limit` steps.
    #[error("plan not fulfilled after {limit} steps")]
    MaxIterations { limit: usize },
    /// No goal is reachable from the current state.
    #[error("no plan found: no goal is reachable from the current state within the search limits")]
    NoPlan,
    /// Planning, executing or checking failed.
    #[error(transparent)]
    Step(NodeError),
}

/// A planner (F10.1): builds a plan from a state and executes it step by step.
/// Before each step it may revise the plan (`build_plan` receives the previous one).
pub trait Planner: Send + Sync {
    /// Input of the run.
    type Input;
    /// Result of the run.
    type Output;
    /// State that the steps update.
    type State: Send + Sync;
    /// Plan.
    type Plan: Send + Sync + serde::Serialize;

    /// Initial state from the input.
    fn initial_state(&self, input: Self::Input) -> Self::State;

    /// New or revised plan; `plan` is the previous one (`None` the first time).
    fn build_plan<'a>(
        &'a self,
        ctx: &'a mut AgentContext,
        state: &'a Self::State,
        plan: Option<Self::Plan>,
    ) -> BoxFuture<'a, Result<Self::Plan, PlannerError>>;

    /// Executes the next step and returns the new state; marks the step in the plan.
    fn execute_step<'a>(
        &'a self,
        ctx: &'a mut AgentContext,
        state: Self::State,
        plan: &'a mut Self::Plan,
    ) -> BoxFuture<'a, Result<Self::State, PlannerError>>;

    /// Is the plan fulfilled?
    fn is_completed<'a>(
        &'a self,
        ctx: &'a mut AgentContext,
        state: &'a Self::State,
        plan: &'a Self::Plan,
    ) -> BoxFuture<'a, Result<bool, PlannerError>>;

    /// Result from the final state.
    fn output(&self, state: Self::State) -> Self::Output;
}

/// Node function of the kind `plan` (ADR-0039): runs `planner` with the input as the task, at
/// most `max_iterations` steps, and returns the result. The progress is kept under
/// `grauwolf.plan.<node_id>` in the agent storage, so a resumed run continues with the next
/// step (ADR-0052).
pub fn plan_node(
    planner: Arc<LlmPlanner>,
    max_iterations: usize,
    node_id: &str,
) -> impl for<'a> Fn(&'a mut AgentContext, String) -> BoxFuture<'a, Result<String, NodeError>>
+ Send
+ Sync
+ 'static {
    planner_node(planner, max_iterations, node_id)
}

/// Turns `planner` with text input and output into a node: at most `max_iterations` steps,
/// progress under `grauwolf.plan.<node_id>` (ADR-0052, ADR-0070).
pub fn planner_node<P>(
    planner: Arc<P>,
    max_iterations: usize,
    node_id: &str,
) -> impl for<'a> Fn(&'a mut AgentContext, String) -> BoxFuture<'a, Result<String, NodeError>>
+ Send
+ Sync
+ 'static
where
    P: Planner<Input = String, Output = String> + 'static,
    P::State: serde::Serialize + serde::de::DeserializeOwned,
    P::Plan: serde::de::DeserializeOwned,
{
    let key = format!("grauwolf.plan.{node_id}");
    move |ctx, task| {
        let (planner, key) = (Arc::clone(&planner), key.clone());
        Box::pin(async move {
            run_planner_resumable(planner.as_ref(), ctx, task, max_iterations, &key)
                .await
                .map_err(Into::into)
        })
    }
}

/// Saved progress of a planner run (ADR-0052).
#[derive(serde::Serialize, serde::Deserialize)]
struct Progress<S, P> {
    iteration: usize,
    state: S,
    plan: P,
}

/// Like [`run_planner`], but saves state, plan and iteration under `key` in the agent storage
/// after each step and resumes from there if progress already exists (ADR-0052).
pub async fn run_planner_resumable<P>(
    planner: &P,
    ctx: &mut AgentContext,
    input: P::Input,
    max_iterations: usize,
    key: &str,
) -> Result<P::Output, PlannerError>
where
    P: Planner,
    P::State: serde::Serialize + serde::de::DeserializeOwned,
    P::Plan: serde::de::DeserializeOwned,
{
    let stored: Option<Progress<P::State, P::Plan>> = ctx
        .load(key)
        .map_err(|error| PlannerError::Step(error.into()))?;
    let (first, mut state, mut plan) = match stored {
        Some(progress) => (progress.iteration + 1, progress.state, Some(progress.plan)),
        None => (1, planner.initial_state(input), None),
    };
    for iteration in first..=max_iterations {
        let mut current = planner.build_plan(ctx, &state, plan.take()).await?;
        ctx.emit(AgentEvent::PlanBuilt {
            iteration,
            plan: event_value(&current),
        });
        state = planner.execute_step(ctx, state, &mut current).await?;
        ctx.emit(AgentEvent::PlanStepExecuted {
            iteration,
            plan: event_value(&current),
        });
        if planner.is_completed(ctx, &state, &current).await? {
            ctx.remove(key);
            return Ok(planner.output(state));
        }
        let progress = Progress {
            iteration,
            state,
            plan: current,
        };
        ctx.store(key, &progress)
            .map_err(|error| PlannerError::Step(error.into()))?;
        ctx.save_progress().await.map_err(PlannerError::Step)?;
        (state, plan) = (progress.state, Some(progress.plan));
    }
    Err(PlannerError::MaxIterations {
        limit: max_iterations,
    })
}

/// Plan as JSON for events; plans that cannot be represented appear as `null`.
fn event_value(plan: &impl serde::Serialize) -> serde_json::Value {
    serde_json::to_value(plan).unwrap_or_default()
}

/// Runs `planner` with `input`: plan, execute a step, check – until the plan is fulfilled, at
/// most `max_iterations` steps.
pub async fn run_planner<P: Planner>(
    planner: &P,
    ctx: &mut AgentContext,
    input: P::Input,
    max_iterations: usize,
) -> Result<P::Output, PlannerError> {
    let mut state = planner.initial_state(input);
    let mut plan = None;
    for iteration in 1..=max_iterations {
        let mut current = planner.build_plan(ctx, &state, plan.take()).await?;
        ctx.emit(AgentEvent::PlanBuilt {
            iteration,
            plan: event_value(&current),
        });
        state = planner.execute_step(ctx, state, &mut current).await?;
        ctx.emit(AgentEvent::PlanStepExecuted {
            iteration,
            plan: event_value(&current),
        });
        if planner.is_completed(ctx, &state, &current).await? {
            return Ok(planner.output(state));
        }
        plan = Some(current);
    }
    Err(PlannerError::MaxIterations {
        limit: max_iterations,
    })
}
