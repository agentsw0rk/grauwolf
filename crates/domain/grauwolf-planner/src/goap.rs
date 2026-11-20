//! Goal-Oriented Action Planning (F10.4–F10.6, ADR-0039): actions with a precondition,
//! expected effect (`belief`) and cost; A* searches the cheapest path per goal. Before each
//! step, planning restarts from the actual state – if an action deviates from its expected
//! effect, the plan adapts.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::hash::Hash;

use futures_util::future::BoxFuture;
use grauwolf_agents::AgentContext;
use grauwolf_graph::NodeError;

use crate::{Planner, PlannerError};

pub use crate::facts::{FactState, FactValue, expected_plan, fact_planner};

/// Execution of an action: yields the actual successor state.
pub type Execute<S> = Box<
    dyn for<'a> Fn(&'a mut AgentContext, S) -> BoxFuture<'a, Result<S, NodeError>> + Send + Sync,
>;

/// An action: applicable if `precondition` holds; `belief` is the expected effect.
pub struct Action<S> {
    name: String,
    precondition: Box<dyn Fn(&S) -> bool + Send + Sync>,
    belief: Box<dyn Fn(&S) -> S + Send + Sync>,
    cost: Box<dyn Fn(&S) -> f64 + Send + Sync>,
    execute: Execute<S>,
}

impl<S> Action<S> {
    /// Action `name` with precondition, expected effect and execution; cost 1.
    pub fn new<E>(
        name: impl Into<String>,
        precondition: impl Fn(&S) -> bool + Send + Sync + 'static,
        belief: impl Fn(&S) -> S + Send + Sync + 'static,
        execute: E,
    ) -> Self
    where
        E: for<'a> Fn(&'a mut AgentContext, S) -> BoxFuture<'a, Result<S, NodeError>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            name: name.into(),
            precondition: Box::new(precondition),
            belief: Box::new(belief),
            cost: Box::new(|_| 1.0),
            execute: Box::new(execute),
        }
    }

    /// Custom cost function (F10.5).
    pub fn cost(mut self, cost: impl Fn(&S) -> f64 + Send + Sync + 'static) -> Self {
        self.cost = Box::new(cost);
        self
    }
}

/// A goal: reached when `condition` holds.
pub struct Goal<S> {
    name: String,
    condition: Box<dyn Fn(&S) -> bool + Send + Sync>,
    heuristic: Box<dyn Fn(&S) -> f64 + Send + Sync>,
    priority: u32,
}

impl<S> Goal<S> {
    /// Goal `name`; without an estimate, A* searches like Dijkstra.
    pub fn new(
        name: impl Into<String>,
        condition: impl Fn(&S) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            condition: Box::new(condition),
            heuristic: Box::new(|_| 0.0),
            priority: 0,
        }
    }

    /// Priority of the goal (default 0): among the goals with a plan, the one with the smallest
    /// `cost / (priority + 1)` is pursued.
    pub fn priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }

    /// Name of the goal.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Estimate of the remaining cost for A*; must not overestimate the actual cost.
    pub fn heuristic(mut self, heuristic: impl Fn(&S) -> f64 + Send + Sync + 'static) -> Self {
        self.heuristic = Box::new(heuristic);
        self
    }
}

/// GOAP planner over the state `S`; the plan is the sequence of action names.
pub struct GoapPlanner<I, O, S> {
    initial: Box<dyn Fn(I) -> S + Send + Sync>,
    output: Box<dyn Fn(S) -> O + Send + Sync>,
    actions: Vec<Action<S>>,
    goals: Vec<Goal<S>>,
    max_search_nodes: usize,
    max_plan_depth: usize,
}

/// Builds a [`GoapPlanner`].
pub struct GoapPlannerBuilder<I, O, S> {
    planner: GoapPlanner<I, O, S>,
}

impl<I, O, S> GoapPlanner<I, O, S> {
    /// Planner that forms the state from the input with `initial` and the result from the
    /// final state with `output`.
    pub fn builder(
        initial: impl Fn(I) -> S + Send + Sync + 'static,
        output: impl Fn(S) -> O + Send + Sync + 'static,
    ) -> GoapPlannerBuilder<I, O, S> {
        GoapPlannerBuilder {
            planner: Self {
                initial: Box::new(initial),
                output: Box::new(output),
                actions: Vec::new(),
                goals: Vec::new(),
                max_search_nodes: DEFAULT_MAX_SEARCH_NODES,
                max_plan_depth: DEFAULT_MAX_PLAN_DEPTH,
            },
        }
    }
}

impl<I, O, S> GoapPlannerBuilder<I, O, S> {
    /// Adds an action.
    pub fn action(mut self, action: Action<S>) -> Self {
        self.planner.actions.push(action);
        self
    }

    /// Adds a goal.
    pub fn goal(mut self, goal: Goal<S>) -> Self {
        self.planner.goals.push(goal);
        self
    }

    /// Maximum number of states examined per search (default 10,000); numbers make the state
    /// space infinite. A goal that is not reached within the limit has no plan.
    pub fn max_search_nodes(mut self, limit: usize) -> Self {
        self.planner.max_search_nodes = limit;
        self
    }

    /// Maximum number of actions in a plan (default 50); longer paths are not followed.
    pub fn max_plan_depth(mut self, limit: usize) -> Self {
        self.planner.max_plan_depth = limit;
        self
    }

    /// Finishes building.
    pub fn build(self) -> GoapPlanner<I, O, S> {
        self.planner
    }
}

/// Default for the number of states examined per search; guards against infinite state spaces.
pub const DEFAULT_MAX_SEARCH_NODES: usize = 10_000;

/// Default for the length of a plan.
pub const DEFAULT_MAX_PLAN_DEPTH: usize = 50;

/// Entry of the open list, ordered by estimated total cost, ascending.
struct Open<S> {
    estimate: f64,
    state: S,
}

impl<S> PartialEq for Open<S> {
    fn eq(&self, other: &Self) -> bool {
        self.estimate.total_cmp(&other.estimate) == Ordering::Equal
    }
}

impl<S> Eq for Open<S> {}

impl<S> PartialOrd for Open<S> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<S> Ord for Open<S> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reversed: BinaryHeap yields the maximum, but the minimum is wanted.
        other.estimate.total_cmp(&self.estimate)
    }
}

impl<I, O, S: Clone + Eq + Hash> GoapPlanner<I, O, S> {
    /// The plan from `state` (action names in order), as a run builds it before each step: per
    /// reachable goal the cheapest path, and the goal with the smallest `cost / (priority + 1)`
    /// wins (ties: declaration order). `None` when no goal is reachable within the limits.
    pub fn plan(&self, state: &S) -> Option<Vec<String>> {
        self.goals
            .iter()
            .filter_map(|goal| {
                let (path, cost) = self.search(state, goal)?;
                Some((path, cost / (f64::from(goal.priority) + 1.0)))
            })
            .min_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(path, _)| {
                path.into_iter()
                    .map(|index| self.actions[index].name.clone())
                    .collect()
            })
    }

    /// Cheapest action sequence (indices) from `start` to `goal` with its cost (A*).
    fn search(&self, start: &S, goal: &Goal<S>) -> Option<(Vec<usize>, f64)> {
        let mut costs: HashMap<S, f64> = HashMap::from([(start.clone(), 0.0)]);
        let mut came_from: HashMap<S, (S, usize)> = HashMap::new();
        let mut depths: HashMap<S, usize> = HashMap::from([(start.clone(), 0)]);
        let mut open = BinaryHeap::from([Open {
            estimate: (goal.heuristic)(start),
            state: start.clone(),
        }]);
        let mut expansions = 0;
        while let Some(Open { state, .. }) = open.pop() {
            if (goal.condition)(&state) {
                let cost = costs[&state];
                let mut path = Vec::new();
                let mut current = state;
                while let Some((previous, action)) = came_from.get(&current) {
                    path.push(*action);
                    current = previous.clone();
                }
                path.reverse();
                return Some((path, cost));
            }
            expansions += 1;
            if expansions > self.max_search_nodes {
                return None;
            }
            let depth = depths[&state];
            if depth >= self.max_plan_depth {
                continue;
            }
            let cost = costs[&state];
            for (index, action) in self.actions.iter().enumerate() {
                if !(action.precondition)(&state) {
                    continue;
                }
                let next = (action.belief)(&state);
                let next_cost = cost + (action.cost)(&state);
                if costs.get(&next).is_none_or(|known| next_cost < *known) {
                    costs.insert(next.clone(), next_cost);
                    came_from.insert(next.clone(), (state.clone(), index));
                    depths.insert(next.clone(), depth + 1);
                    open.push(Open {
                        estimate: next_cost + (goal.heuristic)(&next),
                        state: next,
                    });
                }
            }
        }
        None
    }
}

impl<I, O, S> Planner for GoapPlanner<I, O, S>
where
    S: Clone + Eq + Hash + Send + Sync,
{
    type Input = I;
    type Output = O;
    type State = S;
    type Plan = Vec<String>;

    fn initial_state(&self, input: I) -> S {
        (self.initial)(input)
    }

    /// Always plans anew from the actual state ([`GoapPlanner::plan`]).
    fn build_plan<'a>(
        &'a self,
        _ctx: &'a mut AgentContext,
        state: &'a S,
        _plan: Option<Vec<String>>,
    ) -> BoxFuture<'a, Result<Vec<String>, PlannerError>> {
        Box::pin(async move { self.plan(state).ok_or(PlannerError::NoPlan) })
    }

    fn execute_step<'a>(
        &'a self,
        ctx: &'a mut AgentContext,
        state: S,
        plan: &'a mut Vec<String>,
    ) -> BoxFuture<'a, Result<S, PlannerError>> {
        Box::pin(async move {
            let Some(name) = plan.first() else {
                return Ok(state);
            };
            let action = self
                .actions
                .iter()
                .find(|action| &action.name == name)
                .ok_or(PlannerError::NoPlan)?;
            let next = (action.execute)(ctx, state)
                .await
                .map_err(PlannerError::Step)?;
            plan.remove(0);
            Ok(next)
        })
    }

    fn is_completed<'a>(
        &'a self,
        _ctx: &'a mut AgentContext,
        state: &'a S,
        _plan: &'a Vec<String>,
    ) -> BoxFuture<'a, Result<bool, PlannerError>> {
        Box::pin(async move { Ok(self.goals.iter().any(|goal| (goal.condition)(state))) })
    }

    fn output(&self, state: S) -> O {
        (self.output)(state)
    }
}
