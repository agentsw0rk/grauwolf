//! Data-driven GOAP over named facts (ADR-0070 and its addenda): the facts, conditions and
//! effects of a `goap` node, and the planner that `fact_planner` builds from its parameters.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use grauwolf_agents::AgentContext;
use grauwolf_graph::{Graph, NodeError};
use serde_json::Value;

use crate::goap::{Action, Goal, GoapPlanner};
use crate::{Planner, PlannerError};

/// A floating-point fact value; compared and hashed by its bits, so states can be map keys.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Number(pub f64);

impl PartialEq for Number {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Number {}

impl Hash for Number {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

/// State of the data GOAP (ADR-0070): task, facts and output of the last action.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct FactState {
    /// Task from the node's input.
    pub task: String,
    /// Named facts.
    pub facts: BTreeMap<String, FactValue>,
    /// Output of the most recently executed action.
    pub output: String,
}

/// Value of a fact: true/false, an integer, a number, or a text (an enumeration value from the
/// list under `values`, or a free text) (ADR-0070).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum FactValue {
    /// True/false fact.
    Bool(bool),
    /// Integer fact.
    Int(i64),
    /// Number fact (JSON float).
    Number(Number),
    /// Enumeration or free-text fact.
    Text(String),
}

impl FactValue {
    /// Order of two integers or two numbers; `None` for everything else.
    fn order(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (Self::Int(a), Self::Int(b)) => Some(a.cmp(b)),
            (Self::Number(a), Self::Number(b)) => a.0.partial_cmp(&b.0),
            _ => None,
        }
    }
}

/// The type of a fact, fixed by its initial value in `state` and the list under `values`.
#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Bool,
    /// Text from a fixed list.
    Enum(Vec<String>),
    Int,
    Number,
    /// Text without a list.
    Text,
}

impl Kind {
    fn is_numeric(&self) -> bool {
        matches!(self, Self::Int | Self::Number)
    }

    /// The fact value for the JSON value `value`, if it fits the kind.
    fn value_of(&self, value: &Value) -> Option<FactValue> {
        match self {
            Self::Bool => value.as_bool().map(FactValue::Bool),
            Self::Enum(allowed) => value
                .as_str()
                .filter(|text| allowed.iter().any(|allowed| allowed == text))
                .map(|text| FactValue::Text(text.to_owned())),
            Self::Int => value.as_i64().map(FactValue::Int),
            Self::Number => value
                .as_f64()
                .map(|number| FactValue::Number(Number(number))),
            Self::Text => value.as_str().map(|text| FactValue::Text(text.to_owned())),
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Bool => "true or false".into(),
            Self::Enum(allowed) => format!("one of {allowed:?}"),
            Self::Int => "an integer".into(),
            Self::Number => "a number".into(),
            Self::Text => "a text".into(),
        }
    }
}

type Kinds = BTreeMap<String, Kind>;

/// Initial facts and the kind of each fact from `state` and `values`.
fn initial_facts(params: &Value) -> Result<(BTreeMap<String, FactValue>, Kinds), String> {
    let state = match &params["state"] {
        Value::Object(state) => state,
        Value::Null => &serde_json::Map::new(),
        _ => return Err("state: expected an object with facts".into()),
    };
    let mut facts = BTreeMap::new();
    let mut kinds = Kinds::new();
    for (name, value) in state {
        let (kind, fact) = match value {
            Value::Bool(fact) => (Kind::Bool, FactValue::Bool(*fact)),
            Value::Number(number) if number.is_i64() => (
                Kind::Int,
                FactValue::Int(number.as_i64().unwrap_or_default()),
            ),
            Value::Number(number) => (
                Kind::Number,
                FactValue::Number(Number(number.as_f64().unwrap_or_default())),
            ),
            Value::String(text) => {
                let kind = match params["values"].get(name).and_then(Value::as_array) {
                    Some(allowed) => Kind::Enum(
                        allowed
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect(),
                    ),
                    None => Kind::Text,
                };
                (kind, FactValue::Text(text.clone()))
            }
            _ => {
                return Err(format!(
                    "state.{name}: expected true, false, a number or a text"
                ));
            }
        };
        facts.insert(name.clone(), fact);
        kinds.insert(name.clone(), kind);
    }
    Ok((facts, kinds))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
}

impl Op {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "eq" => Self::Eq,
            "ne" => Self::Ne,
            "gt" => Self::Gt,
            "lt" => Self::Lt,
            "ge" => Self::Ge,
            "le" => Self::Le,
            _ => return None,
        })
    }
}

/// A condition on one fact: `fact <op> value` (precondition or goal).
#[derive(Debug, Clone)]
struct Condition {
    fact: String,
    op: Op,
    value: FactValue,
}

impl Condition {
    fn holds(&self, facts: &BTreeMap<String, FactValue>) -> bool {
        let Some(actual) = facts.get(&self.fact) else {
            return false;
        };
        match self.op {
            Op::Eq => *actual == self.value,
            Op::Ne => *actual != self.value,
            ordering => actual
                .order(&self.value)
                .is_some_and(|order| match ordering {
                    Op::Gt => order.is_gt(),
                    Op::Lt => order.is_lt(),
                    Op::Ge => order.is_ge(),
                    _ => order.is_le(),
                }),
        }
    }
}

/// Conditions from `{ fact: value | { op: value } }`; `what` names the location for errors.
fn conditions_of(value: &Value, what: &str, kinds: &Kinds) -> Result<Vec<Condition>, String> {
    let Some(object) = value.as_object() else {
        return if value.is_null() {
            Ok(Vec::new())
        } else {
            Err(format!("{what}: expected an object with facts"))
        };
    };
    object
        .iter()
        .map(|(fact, wanted)| {
            let kind = kinds
                .get(fact)
                .ok_or_else(|| format!("{what}: fact '{fact}' is not in `state`"))?;
            let (op, wanted) = match wanted {
                Value::Object(operator) => {
                    let mut entries = operator.iter();
                    let (Some((name, operand)), None) = (entries.next(), entries.next()) else {
                        return Err(format!(
                            "{what}.{fact}: expected exactly one operator (eq, ne, gt, lt, ge, le)"
                        ));
                    };
                    let op = Op::parse(name).ok_or_else(|| {
                        format!("{what}.{fact}: unknown operator '{name}' (eq, ne, gt, lt, ge, le)")
                    })?;
                    (op, operand)
                }
                plain => (Op::Eq, plain),
            };
            if !matches!(op, Op::Eq | Op::Ne) && !kind.is_numeric() {
                return Err(format!(
                    "{what}.{fact}: gt, lt, ge and le need an integer or number fact"
                ));
            }
            let value = kind
                .value_of(wanted)
                .ok_or_else(|| format!("{what}.{fact}: expected {}", kind.describe()))?;
            Ok(Condition {
                fact: fact.clone(),
                op,
                value,
            })
        })
        .collect()
}

/// What an effect does to a fact.
#[derive(Debug, Clone)]
enum Change {
    Set(FactValue),
    /// Adds to an integer or number fact.
    Add(FactValue),
}

/// Effects from `{ fact: value | { "add": n } }`.
fn effects_of(value: &Value, what: &str, kinds: &Kinds) -> Result<Vec<(String, Change)>, String> {
    let Some(object) = value.as_object() else {
        return if value.is_null() {
            Ok(Vec::new())
        } else {
            Err(format!("{what}: expected an object with facts"))
        };
    };
    object
        .iter()
        .map(|(fact, wanted)| {
            let kind = kinds
                .get(fact)
                .ok_or_else(|| format!("{what}: fact '{fact}' is not in `state`"))?;
            let change = match wanted {
                Value::Object(operator) if operator.len() == 1 && operator.contains_key("add") => {
                    if !kind.is_numeric() {
                        return Err(format!(
                            "{what}.{fact}: add needs an integer or number fact"
                        ));
                    }
                    Change::Add(
                        kind.value_of(&operator["add"]).ok_or_else(|| {
                            format!("{what}.{fact}: add expects {}", kind.describe())
                        })?,
                    )
                }
                Value::Object(_) => {
                    return Err(format!(
                        "{what}.{fact}: expected a value or exactly `{{ \"add\": n }}`"
                    ));
                }
                plain => Change::Set(
                    kind.value_of(plain)
                        .ok_or_else(|| format!("{what}.{fact}: expected {}", kind.describe()))?,
                ),
            };
            Ok((fact.clone(), change))
        })
        .collect()
}

/// Applies `effects` to `facts`.
fn apply(effects: &[(String, Change)], facts: &mut BTreeMap<String, FactValue>) {
    for (fact, change) in effects {
        match change {
            Change::Set(value) => {
                facts.insert(fact.clone(), value.clone());
            }
            Change::Add(delta) => {
                let sum = match (facts.get(fact), delta) {
                    (Some(FactValue::Int(a)), FactValue::Int(b)) => {
                        Some(FactValue::Int(a.saturating_add(*b)))
                    }
                    (Some(FactValue::Number(a)), FactValue::Number(b)) => {
                        Some(FactValue::Number(Number(a.0 + b.0)))
                    }
                    _ => None,
                };
                if let Some(sum) = sum {
                    facts.insert(fact.clone(), sum);
                }
            }
        }
    }
}

/// Who runs an action (ADR-0070 addendum): a subgraph or a sub-agent.
enum Executor {
    Graph(Arc<Graph<AgentContext>>),
    /// Name of an entry of `agent_tools`, run as a step (ADR-0101).
    Agent(String),
    /// Nothing: the planner only plans ([`expected_plan`]) and never runs an action.
    Planning,
}

impl Executor {
    async fn run(&self, ctx: &mut AgentContext, input: String) -> Result<Value, NodeError> {
        match self {
            Self::Graph(graph) => Ok(graph.run(ctx, Value::String(input)).await?),
            Self::Agent(name) => ctx.run_agent_step(name, Value::String(input)).await,
            Self::Planning => Err(NodeError::from(
                "the planner was built for planning only and runs no action",
            )),
        }
    }
}

/// Part of an input template.
enum Piece {
    Text(String),
    /// `{{task}}`: the input of the planner.
    Task,
    /// `{{fact}}`: the value of a fact as text.
    Fact(String),
}

/// Input template with `{{task}}` and `{{fact}}` placeholders.
fn template_of(template: &str, what: &str, kinds: &Kinds) -> Result<Vec<Piece>, String> {
    let mut pieces = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let (text, after) = rest.split_at(start);
        pieces.push(Piece::Text(text.to_owned()));
        let end = after
            .find("}}")
            .ok_or_else(|| format!("{what}: placeholder `{{{{` is not closed by `}}}}`"))?;
        let name = after[2..end].trim();
        pieces.push(if name == "task" {
            Piece::Task
        } else if kinds.contains_key(name) {
            Piece::Fact(name.to_owned())
        } else {
            return Err(format!(
                "{what}: unknown placeholder '{{{{{name}}}}}' (expected `task` or a fact of `state`)"
            ));
        });
        rest = &after[end + 2..];
    }
    pieces.push(Piece::Text(rest.to_owned()));
    Ok(pieces)
}

impl FactValue {
    /// The value as the text that a template puts in.
    fn text(&self) -> String {
        match self {
            Self::Bool(fact) => fact.to_string(),
            Self::Int(fact) => fact.to_string(),
            Self::Number(fact) => fact.0.to_string(),
            Self::Text(fact) => fact.clone(),
        }
    }
}

/// An action as it runs: executor, input, success check and the effects after the run.
struct ActionRun {
    name: String,
    executor: Executor,
    /// Input template; without one, the task and the action.
    input: Option<Vec<Piece>>,
    /// Order appended to the task when there is no template.
    order: String,
    output_fact: Option<String>,
    success_contains: Option<String>,
    on_success: Vec<(String, Change)>,
    /// `None`: a failure ends the run.
    on_failure: Option<Vec<(String, Change)>>,
    kinds: Arc<Kinds>,
}

impl ActionRun {
    fn input(&self, state: &FactState) -> String {
        let Some(pieces) = &self.input else {
            return format!("{}{}", state.task, self.order);
        };
        pieces
            .iter()
            .map(|piece| match piece {
                Piece::Text(text) => text.clone(),
                Piece::Task => state.task.clone(),
                Piece::Fact(fact) => state
                    .facts
                    .get(fact)
                    .map(FactValue::text)
                    .unwrap_or_default(),
            })
            .collect()
    }

    /// Runs the executor; success applies `on_success`, a failure `on_failure` (or fails the
    /// node without it). A failure is an executor error or an answer without `success_contains`.
    async fn run(&self, ctx: &mut AgentContext, state: FactState) -> Result<FactState, NodeError> {
        let input = self.input(&state);
        let mut next = state;
        let output = match self.executor.run(ctx, input).await {
            Ok(output) => output,
            // An exhausted run budget is never an ordinary failure: `on_failure` would hide it
            // and the run would only end at `max_iterations` (ADR-0072).
            Err(_) if ctx.budget_exhausted().is_some() => {
                let reason = ctx.budget_exhausted().unwrap_or_default();
                return Err(NodeError::from(format!("action '{}': {reason}", self.name)));
            }
            Err(error) => return self.failed(next, &error.to_string()),
        };
        let answer = match &output {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        next.output = answer.clone();
        if let Some(fact) = &self.output_fact {
            next.facts
                .insert(fact.clone(), FactValue::Text(answer.clone()));
        }
        if let Some(wanted) = self
            .success_contains
            .as_ref()
            .filter(|wanted| !answer.contains(wanted.as_str()))
        {
            return self.failed(next, &format!("the answer does not contain '{wanted}'"));
        }
        apply(&self.on_success, &mut next.facts);
        observe(&output, &self.kinds, &mut next.facts);
        Ok(next)
    }

    fn failed(&self, mut next: FactState, reason: &str) -> Result<FactState, NodeError> {
        match &self.on_failure {
            Some(effects) => {
                apply(effects, &mut next.facts);
                Ok(next)
            }
            None => Err(NodeError::from(format!(
                "action '{}' failed: {reason}",
                self.name
            ))),
        }
    }
}

/// A free-text fact named by `params[key]`, if the key is there.
fn text_fact(params: &Value, key: &str, kinds: &Kinds) -> Result<Option<String>, String> {
    let Some(name) = params.get(key).filter(|name| !name.is_null()) else {
        return Ok(None);
    };
    let name = name
        .as_str()
        .ok_or_else(|| format!("{key}: expected the name of a free-text fact"))?;
    match kinds.get(name) {
        Some(Kind::Text) => Ok(Some(name.to_owned())),
        Some(_) => Err(format!("{key}: fact '{name}' is not a free-text fact")),
        None => Err(format!("{key}: fact '{name}' is not in `state`")),
    }
}

/// A search limit (integer ≥ 1) from `params[key]`, if the key is there.
fn limit_of(params: &Value, key: &str) -> Result<Option<usize>, String> {
    match params.get(key).filter(|limit| !limit.is_null()) {
        None => Ok(None),
        Some(limit) => limit
            .as_u64()
            .filter(|limit| *limit >= 1)
            .and_then(|limit| usize::try_from(limit).ok())
            .map(Some)
            .ok_or_else(|| format!("{key}: expected an integer ≥ 1")),
    }
}

/// GOAP from the parameters of a `goap` node (ADR-0070); `steps` holds the executable graph
/// (with its scope) for each subgraph name. Actions with an `agent` need no graph; they run
/// the sub-agent as a step. The interpreter and generated code build the planner the same way.
pub fn fact_planner(
    params: &Value,
    steps: &HashMap<String, Arc<Graph<AgentContext>>>,
) -> Result<GoapPlanner<String, String, FactState>, String> {
    build(params, |what, subgraph| {
        steps
            .get(subgraph)
            .cloned()
            .map(Executor::Graph)
            .ok_or_else(|| format!("{what}: no graph for subgraph '{subgraph}'"))
    })
}

/// The plan the `goap` node with `params` makes for its start state (ADR-0126 decision 15):
/// the action names in order, found by the same search as in a run, from `state` with an empty
/// task (the node input is not known before a run, so an `input_fact` starts empty). No action
/// runs. `Err` says why there is none: invalid parameters, or no goal reachable within the
/// search limits.
pub fn expected_plan(params: &Value) -> Result<Vec<String>, String> {
    let planner = build(params, |_, _| Ok(Executor::Planning))?;
    let state = planner.initial_state(String::new());
    planner
        .plan(&state)
        .ok_or_else(|| PlannerError::NoPlan.to_string())
}

/// GOAP from the parameters of a `goap` node; `graph` gives the executor of an action with a
/// `subgraph` (its context for messages and the subgraph's name).
fn build(
    params: &Value,
    graph: impl Fn(&str, &str) -> Result<Executor, String>,
) -> Result<GoapPlanner<String, String, FactState>, String> {
    let (initial, kinds) = initial_facts(params)?;
    let input_fact = text_fact(params, "input_fact", &kinds)?;
    let output_fact = text_fact(params, "output_fact", &kinds)?;
    let kinds = Arc::new(kinds);
    let mut builder = GoapPlanner::builder(
        move |task: String| {
            let mut facts = initial.clone();
            if let Some(fact) = &input_fact {
                facts.insert(fact.clone(), FactValue::Text(task.clone()));
            }
            FactState {
                task,
                facts,
                output: String::new(),
            }
        },
        move |state: FactState| match &output_fact {
            Some(fact) => state
                .facts
                .get(fact)
                .map(FactValue::text)
                .unwrap_or_default(),
            None => state.output,
        },
    );
    for (index, action) in params["actions"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let what = format!("actions[{index}]");
        let name = action["name"]
            .as_str()
            .ok_or_else(|| format!("{what}.name is missing"))?
            .to_owned();
        let executor = match (action["subgraph"].as_str(), action["agent"].as_str()) {
            (Some(subgraph), None) => graph(&what, subgraph)?,
            (None, Some(agent)) => Executor::Agent(agent.to_owned()),
            (Some(_), Some(_)) => {
                return Err(format!("{what}: `subgraph` and `agent` exclude each other"));
            }
            (None, None) => return Err(format!("{what}: needs a `subgraph` or an `agent`")),
        };
        let pre = conditions_of(&action["pre"], &format!("{what}.pre"), &kinds)?;
        let effects = effects_of(&action["effects"], &format!("{what}.effects"), &kinds)?;
        let on_success = match action.get("on_success") {
            Some(on_success) => effects_of(on_success, &format!("{what}.on_success"), &kinds)?,
            None => effects.clone(),
        };
        let on_failure = action
            .get("on_failure")
            .map(|on_failure| effects_of(on_failure, &format!("{what}.on_failure"), &kinds))
            .transpose()?;
        let input = action["input"]
            .as_str()
            .map(|template| template_of(template, &format!("{what}.input"), &kinds))
            .transpose()?;
        let cost = action["cost"].as_f64().unwrap_or(1.0);
        let mut order = format!("\n\nAction: {name}");
        if let Some(description) = action["description"].as_str() {
            order.push_str(&format!("\n{description}"));
        }
        let run = Arc::new(ActionRun {
            name: name.clone(),
            executor,
            input,
            order,
            output_fact: text_fact(action, "output_fact", &kinds)
                .map_err(|error| format!("{what}.{error}"))?,
            success_contains: action["success_contains"].as_str().map(str::to_owned),
            on_success,
            on_failure,
            kinds: Arc::clone(&kinds),
        });
        builder = builder.action(
            Action::new(
                name,
                move |state: &FactState| pre.iter().all(|condition| condition.holds(&state.facts)),
                move |state: &FactState| {
                    let mut next = state.clone();
                    apply(&effects, &mut next.facts);
                    next
                },
                move |ctx, state: FactState| {
                    let run = Arc::clone(&run);
                    Box::pin(async move { run.run(ctx, state).await })
                },
            )
            .cost(move |_| cost),
        );
    }
    for (index, goal) in params["goals"].as_array().into_iter().flatten().enumerate() {
        let what = format!("goals[{index}]");
        let name = goal["name"]
            .as_str()
            .ok_or_else(|| format!("{what}.name is missing"))?
            .to_owned();
        let facts = conditions_of(&goal["facts"], &format!("{what}.facts"), &kinds)?;
        let priority = match goal.get("priority") {
            None => 0,
            Some(priority) => priority
                .as_u64()
                .and_then(|priority| u32::try_from(priority).ok())
                .ok_or_else(|| format!("{what}.priority: expected an integer ≥ 0"))?,
        };
        builder = builder.goal(
            Goal::new(name, move |state: &FactState| {
                facts.iter().all(|condition| condition.holds(&state.facts))
            })
            .priority(priority),
        );
    }
    if let Some(limit) = limit_of(params, "max_search_nodes")? {
        builder = builder.max_search_nodes(limit);
    }
    if let Some(limit) = limit_of(params, "max_plan_depth")? {
        builder = builder.max_plan_depth(limit);
    }
    Ok(builder.build())
}

/// Observed state: fields of an object output overwrite same-named facts of the same kind
/// (enumerations only with an allowed value, ADR-0070).
fn observe(output: &Value, kinds: &Kinds, facts: &mut BTreeMap<String, FactValue>) {
    let Some(object) = output.as_object() else {
        return;
    };
    for (fact, value) in object {
        if let Some(observed) = kinds.get(fact).and_then(|kind| kind.value_of(value)) {
            facts.insert(fact.clone(), observed);
        }
    }
}
