//! LLM planner (F10.2, ADR-0039): plan as a structured answer, each step runs through a graph
//! (text → text), e.g. with a tool loop.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_agents::AgentContext;
use grauwolf_graph::{Graph, Scope};
use grauwolf_llm::StructuredOutput;
use grauwolf_prompt::Prompt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Planner, PlannerError};

/// Plan of the LLM planner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    /// Goal of the plan.
    pub goal: String,
    /// Steps in order.
    pub steps: Vec<PlanStep>,
}

/// A step of the plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanStep {
    /// What to do.
    pub description: String,
    /// Is the step done?
    pub completed: bool,
}

/// State: the task and the results of the completed steps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanState {
    /// Task from the input.
    pub task: String,
    /// Results of the steps in order.
    pub results: Vec<String>,
}

/// LLM planner: plans via a structured answer and executes each step with `step`.
pub struct LlmPlanner {
    step: Arc<Graph<AgentContext>>,
    step_scope: Option<Box<dyn Scope<AgentContext>>>,
    critic: bool,
}

impl LlmPlanner {
    /// Planner whose steps `step` executes (input: step order, output: result).
    pub fn new(step: Arc<Graph<AgentContext>>) -> Self {
        Self {
            step,
            step_scope: None,
            critic: false,
        }
    }

    /// Executes each step within `scope`, e.g. with the tool subset of the subgraph
    /// (ADR-0034); planning and critic run outside of it.
    pub fn with_step_scope(mut self, scope: Box<dyn Scope<AgentContext>>) -> Self {
        self.step_scope = Some(scope);
        self
    }

    /// Has a critic check before each further step whether the plan still holds; otherwise
    /// it replans with the critic's reason (F10.3).
    pub fn with_critic(mut self) -> Self {
        self.critic = true;
        self
    }

    /// Asks the critic on a separate history; `Some(reason)` if it should replan.
    async fn criticize(
        &self,
        ctx: &mut AgentContext,
        state: &PlanState,
        plan: &Plan,
    ) -> Result<Option<String>, PlannerError> {
        let request = format!(
            "Evaluate the plan for the task:\n{}\n\n{}{}",
            state.task,
            describe(plan),
            results(state)
        );
        let critic = Prompt::builder("critic")
            .system(CRITIC_PROMPT)
            .user(request)
            .build();
        let original = ctx.replace_prompt(critic);
        let answer = ctx.request_llm_structured(&evaluation_output(), 2).await;
        ctx.replace_prompt(original);
        let answer = answer.map_err(PlannerError::Step)?;
        Ok(match answer["should_continue"].as_bool() {
            Some(false) => Some(answer["reason"].as_str().unwrap_or_default().to_owned()),
            _ => None,
        })
    }

    /// Requests a new plan; the request runs on a separate history.
    /// `rejected` is the discarded plan with the critic's reason.
    async fn request_plan(
        &self,
        ctx: &mut AgentContext,
        state: &PlanState,
        rejected: Option<(&Plan, &str)>,
    ) -> Result<Plan, PlannerError> {
        let mut request = format!("Create a plan for the task:\n{}", state.task);
        request.push_str(&results(state));
        if let Some((plan, reason)) = rejected {
            request.push_str(&format!(
                "\n\nThe previous plan was discarded.\n{}\nReason: {reason}",
                describe(plan)
            ));
        }
        let planning = Prompt::builder("planner")
            .system(
                "You are a planning agent. Break the task down into a few clearly executable \
                 steps in order. The last step formulates the answer to the \
                 user.",
            )
            .user(request)
            .build();
        let original = ctx.replace_prompt(planning);
        let answer = ctx.request_llm_structured(&plan_output(), 2).await;
        ctx.replace_prompt(original);
        let answer = answer.map_err(PlannerError::Step)?;
        Ok(Plan {
            goal: answer["goal"].as_str().unwrap_or_default().to_owned(),
            steps: answer["steps"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|description| PlanStep {
                    description: description.to_owned(),
                    completed: false,
                })
                .collect(),
        })
    }
}

/// System prompt of the critic. Built-in model text, deliberately not a literal translation of the
/// earlier German text (ADR-0091 addendum): `should_continue = false` makes the planner discard
/// the plan and build a new one, so the prompt says exactly that and keeps `true` the normal case
/// (a critic that answered `false` once the goal was reached made the planner loop).
const CRITIC_PROMPT: &str = "You review a plan that is being carried out. should_continue = true \
    means: carry on with the remaining steps of this plan. should_continue = false means: throw \
    this plan away and make a new one. Answer false only if the remaining steps cannot reach the \
    goal any more (wrong approach, impossible step, results that contradict the plan), and say in \
    reason what is wrong. Otherwise answer true, also if the plan is imperfect or the goal is \
    nearly reached already.";

/// Schema of the critic answer: continue or replan, with a reason.
fn evaluation_output() -> StructuredOutput {
    StructuredOutput {
        name: "PlanEvaluation".into(),
        schema: json!({
            "type": "object",
            "properties": {
                "should_continue": { "type": "boolean" },
                "reason": { "type": "string" }
            },
            "required": ["should_continue", "reason"]
        }),
    }
}

/// Goal and steps, completed ones marked.
fn describe(plan: &Plan) -> String {
    let mut text = format!("Plan – goal: {}", plan.goal);
    for (index, step) in plan.steps.iter().enumerate() {
        let done = if step.completed { " [done]" } else { "" };
        text.push_str(&format!("\n{}. {}{done}", index + 1, step.description));
    }
    text
}

/// Previous results as a section, empty without results.
fn results(state: &PlanState) -> String {
    let mut text = String::new();
    if !state.results.is_empty() {
        text.push_str("\n\nResults so far:");
        for (index, result) in state.results.iter().enumerate() {
            text.push_str(&format!("\n{}. {result}", index + 1));
        }
    }
    text
}

/// Schema of the planning answer: goal and steps as texts.
fn plan_output() -> StructuredOutput {
    StructuredOutput {
        name: "Plan".into(),
        schema: json!({
            "type": "object",
            "properties": {
                "goal": { "type": "string" },
                "steps": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["goal", "steps"]
        }),
    }
}

impl Planner for LlmPlanner {
    type Input = String;
    type Output = String;
    type State = PlanState;
    type Plan = Plan;

    fn initial_state(&self, input: String) -> PlanState {
        PlanState {
            task: input,
            results: Vec::new(),
        }
    }

    fn build_plan<'a>(
        &'a self,
        ctx: &'a mut AgentContext,
        state: &'a PlanState,
        plan: Option<Plan>,
    ) -> BoxFuture<'a, Result<Plan, PlannerError>> {
        Box::pin(async move {
            let Some(plan) = plan else {
                return self.request_plan(ctx, state, None).await;
            };
            if !self.critic {
                return Ok(plan);
            }
            match self.criticize(ctx, state, &plan).await? {
                Some(reason) => self.request_plan(ctx, state, Some((&plan, &reason))).await,
                None => Ok(plan),
            }
        })
    }

    fn execute_step<'a>(
        &'a self,
        ctx: &'a mut AgentContext,
        mut state: PlanState,
        plan: &'a mut Plan,
    ) -> BoxFuture<'a, Result<PlanState, PlannerError>> {
        Box::pin(async move {
            let total = plan.steps.len();
            let Some((index, step)) = plan
                .steps
                .iter_mut()
                .enumerate()
                .find(|(_, step)| !step.completed)
            else {
                return Ok(state);
            };
            let order = format!(
                "Goal: {}\nCarry out step {} of {total}: {} and answer with the result.",
                plan.goal,
                index + 1,
                step.description
            );
            if let Some(scope) = &self.step_scope {
                scope.enter(ctx);
            }
            let result = self.step.run(ctx, Value::String(order)).await;
            if let Some(scope) = &self.step_scope {
                scope.exit(ctx);
            }
            let result = result.map_err(|error| PlannerError::Step(error.into()))?;
            step.completed = true;
            state.results.push(match result {
                Value::String(text) => text,
                other => other.to_string(),
            });
            Ok(state)
        })
    }

    fn is_completed<'a>(
        &'a self,
        _ctx: &'a mut AgentContext,
        _state: &'a PlanState,
        plan: &'a Plan,
    ) -> BoxFuture<'a, Result<bool, PlannerError>> {
        Box::pin(async move { Ok(plan.steps.iter().all(|step| step.completed)) })
    }

    fn output(&self, state: PlanState) -> String {
        state.results.last().cloned().unwrap_or_default()
    }
}
