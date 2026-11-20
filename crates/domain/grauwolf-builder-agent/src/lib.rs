//! Builder agent (F13.12, ADR-0035): creates a valid AgentSpec from a requirement in text form
//! and repairs it based on the errors with JSON pointers.
//!
//! The builder is itself a GraphAgent on the grauwolf engine:
//! `start → draft → check → (valid) finish`, `check → (issues, attempts left) repair → check`,
//! `check → (no attempts left) give_up`.

use grauwolf_spec::AgentSpecExt;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_agents::AgentContext;
use grauwolf_graph::{EngineError, GraphError, NodeError, TypedGraph};
use grauwolf_llm::{PromptExecutor, StructuredOutput};
use grauwolf_prompt::{LlModel, Message, Prompt};
use grauwolf_spec::AgentSpec;
use serde::{Deserialize, Serialize};

mod prompt;

/// An issue of a spec with a JSON pointer to the affected location (empty = whole spec).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecIssue {
    /// JSON-Pointer (RFC 6901).
    pub path: String,
    /// Description for humans and LLMs.
    pub message: String,
}

/// Port for checks the domain cannot run itself, e.g. compiling tool code (F13.9). Runs only
/// for specs that are already valid.
pub trait SpecChecker: Send + Sync {
    /// Issues of `spec`; empty if everything is fine.
    fn check<'a>(&'a self, spec: &'a AgentSpec) -> BoxFuture<'a, Vec<SpecIssue>>;
}

/// Checker without additional checks.
struct NoChecks;

impl SpecChecker for NoChecks {
    fn check<'a>(&'a self, _spec: &'a AgentSpec) -> BoxFuture<'a, Vec<SpecIssue>> {
        Box::pin(async { Vec::new() })
    }
}

/// Result of the builder.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltSpec {
    /// The valid spec.
    pub spec: AgentSpec,
    /// The same spec as formatted JSON, e.g. for saving.
    pub json: String,
    /// Number of repairs needed.
    pub repairs: u32,
}

/// Errors of the builder.
#[derive(Debug, thiserror::Error)]
pub enum BuilderError {
    /// The builder's graph could not be built.
    #[error(transparent)]
    Graph(#[from] GraphError),
    /// The spec is still not valid after all allowed repairs.
    #[error("no valid spec after {repairs} repairs:\n{}", render_issues(.issues))]
    GaveUp {
        repairs: u32,
        issues: Vec<SpecIssue>,
    },
    /// The run failed, e.g. because the LLM is not reachable.
    #[error(transparent)]
    Engine(#[from] EngineError),
}

/// Issues as lines `<path>: <message>`.
fn render_issues(issues: &[SpecIssue]) -> String {
    issues
        .iter()
        .map(|issue| format!("{}: {}", issue.path, issue.message))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Error of the node `give_up`; becomes [`BuilderError::GaveUp`].
#[derive(Debug, thiserror::Error)]
#[error("no valid spec")]
struct GaveUp(Checked);

/// Answer of the LLM to a requirement or a repair.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Attempt {
    answer: String,
    repairs: u32,
}

/// Result of checking an answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Checked {
    /// Formatted JSON of the spec, if it could be loaded.
    json: Option<String>,
    issues: Vec<SpecIssue>,
    repairs: u32,
}

/// Creates specs with an LLM.
pub struct Builder {
    executor: Arc<PromptExecutor>,
    model: LlModel,
    max_repairs: u32,
    checker: Arc<dyn SpecChecker>,
    agent_model: Option<LlModel>,
}

/// Default number of allowed repairs.
pub const DEFAULT_MAX_REPAIRS: u32 = 3;

impl Builder {
    /// Builder that queries `model` via `executor`.
    pub fn new(executor: Arc<PromptExecutor>, model: LlModel) -> Self {
        Self {
            executor,
            model,
            max_repairs: DEFAULT_MAX_REPAIRS,
            checker: Arc::new(NoChecks),
            agent_model: None,
        }
    }

    /// Model the generated agent should run with; written into the spec as binding.
    pub fn with_agent_model(mut self, model: LlModel) -> Self {
        self.agent_model = Some(model);
        self
    }

    /// Additionally checks valid specs with `checker`, e.g. by compiling the tools.
    pub fn with_checker(mut self, checker: Arc<dyn SpecChecker>) -> Self {
        self.checker = checker;
        self
    }

    /// Allows at most `max_repairs` repairs.
    pub fn with_max_repairs(mut self, max_repairs: u32) -> Self {
        self.max_repairs = max_repairs;
        self
    }

    /// Creates a valid spec for `requirement`.
    pub async fn build(&self, requirement: &str) -> Result<BuiltSpec, BuilderError> {
        let mut ctx = AgentContext::new(
            Arc::clone(&self.executor),
            self.model.clone(),
            Prompt::builder("grauwolf_builder")
                .system(prompt::system_prompt(self.agent_model.as_ref()))
                .build(),
        );
        let checked = match graph(self.max_repairs, Arc::clone(&self.checker))?
            .run(&mut ctx, requirement.to_owned())
            .await
        {
            Ok(checked) => checked,
            Err(EngineError::Node { source, .. }) if source.is::<GaveUp>() => {
                let GaveUp(checked) = *source.downcast::<GaveUp>().expect("checked");
                return Err(BuilderError::GaveUp {
                    repairs: checked.repairs,
                    issues: checked.issues,
                });
            }
            Err(error) => return Err(error.into()),
        };
        let mut value: serde_json::Value =
            serde_json::from_str(&checked.json.unwrap_or_default()).expect("checked JSON");
        if let Some(model) = &self.agent_model {
            value["model"] = serde_json::json!({ "provider": model.provider, "id": model.id });
        }
        let json = serde_json::to_string_pretty(&value).expect("JSON value");
        let spec = AgentSpec::from_json(&json).expect("a checked spec can be loaded");
        Ok(BuiltSpec {
            spec,
            json,
            repairs: checked.repairs,
        })
    }
}

/// Graph of the builder with at most `max_repairs` repairs.
fn graph(
    max_repairs: u32,
    checker: Arc<dyn SpecChecker>,
) -> Result<TypedGraph<AgentContext, String, Checked>, GraphError> {
    TypedGraph::build(|g| {
        let draft = g.node("draft", draft);
        let check = g.node("check", move |_ctx: &mut AgentContext, attempt: Attempt| {
            let checker = Arc::clone(&checker);
            Box::pin(async move { check(checker.as_ref(), attempt).await })
        });
        let repair = g.node("repair", repair);
        let give_up = g.node("give_up", give_up);
        g.edge(&g.start(), &draft);
        g.edge(&draft, &check);
        g.edge_when(&check, &g.finish(), |_, checked: Checked| {
            checked.issues.is_empty().then_some(checked)
        });
        g.edge_when(&check, &repair, move |_, checked: Checked| {
            (checked.repairs < max_repairs).then_some(checked)
        });
        g.edge(&check, &give_up);
        g.edge(&repair, &check);
        g.edge(&give_up, &g.finish());
    })
}

/// `repair`: reports the issues with path to the LLM and asks for the corrected spec.
fn repair(ctx: &mut AgentContext, checked: Checked) -> BoxFuture<'_, Result<Attempt, NodeError>> {
    Box::pin(async move {
        ctx.append(Message::user(format!(
            "The spec has errors (format: <JSON pointer>: <error>):\n{}\n\
             Answer with nothing but the complete, corrected spec as JSON.",
            render_issues(&checked.issues)
        )));
        let answer = ctx.request_llm_json(&spec_output()).await?;
        Ok(Attempt {
            answer: text_of(&answer),
            repairs: checked.repairs + 1,
        })
    })
}

/// `give_up`: no more repairs allowed.
fn give_up(_ctx: &mut AgentContext, checked: Checked) -> BoxFuture<'_, Result<Checked, NodeError>> {
    Box::pin(async move { Err(Box::new(GaveUp(checked)) as NodeError) })
}

/// `draft`: sends the requirement to the LLM.
fn draft(ctx: &mut AgentContext, requirement: String) -> BoxFuture<'_, Result<Attempt, NodeError>> {
    Box::pin(async move {
        ctx.append(Message::user(requirement));
        let answer = ctx.request_llm_json(&spec_output()).await?;
        Ok(Attempt {
            answer: text_of(&answer),
            repairs: 0,
        })
    })
}

/// `check`: loads and validates the answer, then `checker` checks it (e.g. compiling).
async fn check(checker: &dyn SpecChecker, attempt: Attempt) -> Result<Checked, NodeError> {
    let json = undouble_tool_line_breaks(strip_code_fence(&attempt.answer));
    let issues = match AgentSpec::from_json(&json) {
        Ok(spec) => match spec.validate() {
            Ok(()) => checker.check(&spec).await,
            Err(errors) => errors
                .into_iter()
                .map(|error| SpecIssue {
                    path: error.path,
                    message: error.kind.to_string(),
                })
                .collect(),
        },
        Err(error) => vec![SpecIssue {
            path: String::new(),
            message: with_excerpt(&error, &json),
        }],
    };
    let json = serde_json::from_str::<serde_json::Value>(&json)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok());
    Ok(Checked {
        json,
        issues,
        repairs: attempt.repairs,
    })
}

/// Structured answer following the JSON schema of the DSL (F13.2): providers with native
/// support produce syntactically valid JSON with it. With enforced output, models omit
/// optional fields; that is why `nodes` and `edges` are required in a `graph` here. `graph`
/// itself stays optional, because a spec with a `strategy` (e.g. `single_run`) has none.
fn spec_output() -> StructuredOutput {
    let mut schema = AgentSpec::json_schema();
    schema["$defs"]["GraphSpec"]["required"] = serde_json::json!(["nodes", "edges"]);
    StructuredOutput {
        name: "AgentSpec".into(),
        schema,
    }
}

/// Some models escape line breaks in tool code twice (`\\n` in the JSON): the code then
/// contains a single backslash, and feedback does not change that. If tool code has no real
/// line break but `\n`, these become line breaks. In Rust strings this does not change the
/// meaning; only an escaped backslash before `n` (`\\n`) would become a line continuation.
/// Answers that are not JSON stay unchanged.
fn undouble_tool_line_breaks(answer: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(answer) else {
        return answer.to_owned();
    };
    let mut changed = false;
    if let Some(tools) = value
        .get_mut("tools")
        .and_then(serde_json::Value::as_array_mut)
    {
        for code in tools.iter_mut().filter_map(|tool| tool.get_mut("code")) {
            if let Some(text) = code.as_str()
                && !text.contains('\n')
                && text.contains("\\n")
            {
                *code = serde_json::Value::String(text.replace("\\n", "\n"));
                changed = true;
            }
        }
    }
    if changed {
        value.to_string()
    } else {
        answer.to_owned()
    }
}

/// Error message when loading; for JSON errors with the lines around the error location, so
/// the LLM can find it in its answer.
fn with_excerpt(error: &grauwolf_spec::SpecError, json: &str) -> String {
    let grauwolf_spec::SpecError::Json(json_error) = error else {
        return error.to_string();
    };
    let line = json_error.line();
    if line == 0 {
        return error.to_string();
    }
    let excerpt: Vec<String> = json
        .lines()
        .enumerate()
        .map(|(index, text)| (index + 1, text))
        .filter(|(number, _)| number + 2 >= line && *number <= line + 1)
        .map(|(number, text)| format!("{number} | {text}"))
        .collect();
    format!("{error}\n{}", excerpt.join("\n"))
}

fn text_of(message: &Message) -> String {
    match message {
        Message::Assistant(answer) => answer.text(),
        _ => String::new(),
    }
}

/// Removes a Markdown code block (```json … ```) around `text`, if present.
fn strip_code_fence(text: &str) -> &str {
    let text = text.trim();
    match text.strip_prefix("```") {
        Some(rest) => {
            let body = rest.split_once('\n').map_or("", |(_, body)| body);
            body.trim_end().strip_suffix("```").unwrap_or(body).trim()
        }
        None => text,
    }
}
