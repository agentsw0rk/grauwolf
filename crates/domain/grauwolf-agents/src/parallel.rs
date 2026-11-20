//! Parallel branches with a merge strategy (F05.11, ADR-0045): each branch runs concurrently on
//! a copy of the context; afterwards the results are merged.

use std::collections::BTreeMap;

use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use futures_util::stream::FuturesUnordered;
use grauwolf_graph::{Graph, NodeData, NodeError, Scope};
use serde_json::Value;

use crate::AgentContext;
use crate::checkpoints::ContextSnapshot;

/// A branch: name, graph and optional scope (tool subset, model).
pub struct Branch {
    name: String,
    graph: Graph<AgentContext>,
    scope: Option<Box<dyn Scope<AgentContext>>>,
}

impl Branch {
    /// Branch `name` that runs `graph`.
    pub fn new(name: impl Into<String>, graph: Graph<AgentContext>) -> Self {
        Self {
            name: name.into(),
            graph,
            scope: None,
        }
    }

    /// Enters `scope` before the branch and leaves it afterwards.
    pub fn with_scope(mut self, scope: Box<dyn Scope<AgentContext>>) -> Self {
        self.scope = Some(scope);
        self
    }
}

/// How the results of the branches are merged.
#[derive(Debug, Clone, PartialEq)]
pub enum Merge {
    /// Text results as `## <branch>` sections; the caller's history stays unchanged.
    Concat,
    /// Result and context of the branch with this index.
    Index(usize),
    /// An evaluation without tools picks the best branch by `criteria`; its result and
    /// context are adopted.
    LlmSelect {
        /// What to select by.
        criteria: String,
    },
}

/// Runs all `branches` concurrently with `input` and merges them according to `merge`.
pub async fn run_parallel(
    ctx: &mut AgentContext,
    branches: &[Branch],
    input: Value,
    merge: &Merge,
) -> Result<Value, NodeError> {
    run_parallel_resumable(ctx, branches, input, merge, "grauwolf.parallel").await
}

/// Key of the progress of a `parallel` node in the agent storage (ADR-0052).
pub fn progress_key(node_id: &str) -> String {
    format!("grauwolf.parallel.{node_id}")
}

/// Result of a finished branch in the progress (ADR-0052).
#[derive(serde::Serialize, serde::Deserialize)]
struct Finished {
    output: Value,
    context: ContextSnapshot,
}

/// Like [`run_parallel`], but saves each finished branch under `key` in the agent storage
/// (index → result and context) and, when resuming, runs only the missing ones (ADR-0052).
pub async fn run_parallel_resumable(
    ctx: &mut AgentContext,
    branches: &[Branch],
    input: Value,
    merge: &Merge,
    key: &str,
) -> Result<Value, NodeError> {
    let mut finished: BTreeMap<usize, Finished> = ctx.load(key)?.unwrap_or_default();
    let mut pending = branches
        .iter()
        .enumerate()
        .filter(|(index, _)| !finished.contains_key(index))
        .map(|(index, branch)| {
            let mut fork = ctx.fork();
            fork.remove(key);
            let input = input.clone();
            async move {
                let result = run_branch(branch, &mut fork, input).await;
                (index, result.map(|output| (output, fork)))
            }
        })
        .collect::<FuturesUnordered<_>>();
    let mut failure = None;
    while let Some((index, result)) = pending.next().await {
        match result {
            Ok((output, fork)) => {
                finished.insert(
                    index,
                    Finished {
                        output,
                        context: fork.snapshot(),
                    },
                );
                ctx.store(key, &finished)?;
                ctx.save_progress().await?;
            }
            // The remaining branches run to completion so that their results are saved.
            Err(error) => failure = failure.or(Some(error)),
        }
    }
    drop(pending);
    if let Some(error) = failure {
        return Err(error);
    }
    ctx.remove(key);
    let results: Vec<(Value, AgentContext)> = finished
        .into_values()
        .map(|done| {
            let mut branch = ctx.fork();
            branch.apply(done.context);
            (done.output, branch)
        })
        .collect();
    merge_results(ctx, branches, &input, results, merge).await
}

/// Runs a branch on `fork`, with its scope.
async fn run_branch(
    branch: &Branch,
    fork: &mut AgentContext,
    input: Value,
) -> Result<Value, NodeError> {
    if let Some(scope) = &branch.scope {
        scope.enter(fork);
    }
    let result = branch.graph.run(fork, input).await;
    if let Some(scope) = &branch.scope {
        scope.exit(fork);
    }
    result.map_err(|error| NodeError::from(format!("branch '{}': {error}", branch.name)))
}

/// Merges the results (in branch order) according to `merge`.
async fn merge_results(
    ctx: &mut AgentContext,
    branches: &[Branch],
    input: &Value,
    results: Vec<(Value, AgentContext)>,
    merge: &Merge,
) -> Result<Value, NodeError> {
    match merge {
        Merge::Concat => {
            let sections = branches
                .iter()
                .zip(&results)
                .map(|(branch, (output, _))| match output {
                    Value::String(text) => Ok(format!("## {}\n{text}", branch.name)),
                    other => Err(NodeError::from(format!(
                        "concat needs text outputs, branch '{}' delivers {other}",
                        branch.name
                    ))),
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Value::String(sections.join("\n\n")))
        }
        Merge::Index(index) => adopt(ctx, results, *index),
        Merge::LlmSelect { criteria } => {
            let texts: Vec<String> = results.iter().map(|(output, _)| text(output)).collect();
            let best = crate::nodes::pick_best(ctx, &text(input), &texts, criteria).await?;
            // A number outside the list counts as the first, as in `llm_request_best_of`.
            let best = if best < results.len() { best } else { 0 };
            adopt(ctx, results, best)
        }
    }
}

/// Adopts the result and context of the branch `index`.
fn adopt(
    ctx: &mut AgentContext,
    mut results: Vec<(Value, AgentContext)>,
    index: usize,
) -> Result<Value, NodeError> {
    if index >= results.len() {
        return Err(format!("branch {index} does not exist ({} branches)", results.len()).into());
    }
    let (output, branch) = results.swap_remove(index);
    ctx.absorb(branch);
    Ok(output)
}

/// Value as text for the evaluation.
fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Node function for typed code: like [`run_parallel`] with input and output `I`/`O`.
pub fn parallel<I: NodeData, O: NodeData>(
    branches: Vec<Branch>,
    merge: Merge,
    node_id: &str,
) -> impl for<'a> Fn(&'a mut AgentContext, I) -> BoxFuture<'a, Result<O, NodeError>>
+ Send
+ Sync
+ 'static {
    let branches = std::sync::Arc::new(branches);
    let key = progress_key(node_id);
    move |ctx, input| {
        let (branches, merge, key) = (std::sync::Arc::clone(&branches), merge.clone(), key.clone());
        Box::pin(async move {
            let input = serde_json::to_value(input)?;
            let output = run_parallel_resumable(ctx, &branches, input, &merge, &key).await?;
            Ok(serde_json::from_value(output)?)
        })
    }
}
