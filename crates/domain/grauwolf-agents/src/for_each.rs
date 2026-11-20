//! The `for_each` node (ADR-0101, F14.13): runs a body once per element of a list, each on its
//! own fork of the context (shared counters, own history), several at a time within one task.

use std::collections::BTreeMap;

use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use futures_util::stream;
use grauwolf_graph::{Graph, NodeData, NodeError, Scope};
use serde_json::Value;

use crate::AgentContext;
use crate::events::AgentEvent;

/// What runs for each element.
pub enum Body {
    /// The sub-agent `name` from `agent_tools`, as the `agent` node runs it.
    Agent(String),
    /// A graph `T → U` (a subgraph of the spec) with an optional scope (tools, model).
    Graph {
        /// Name of the subgraph, for messages.
        name: String,
        /// The graph run for each element.
        graph: Graph<AgentContext>,
        /// Entered before and left after each element.
        scope: Option<Box<dyn Scope<AgentContext>>>,
    },
}

/// What a failing element does to the node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnError {
    /// The node fails with the first error.
    #[default]
    Fail,
    /// The element is left out of the output; its `for_each_item_failed` event reports it.
    Skip,
}

/// Configuration of a `for_each` node.
pub struct ForEach {
    body: Body,
    max_parallel: usize,
    on_error: OnError,
    text_output: bool,
}

impl ForEach {
    /// Runs `body` for each element, one at a time.
    pub fn new(body: Body) -> Self {
        Self {
            body,
            max_parallel: 1,
            on_error: OnError::Fail,
            text_output: false,
        }
    }

    /// Returns the list as JSON text instead of an array, for a node whose output is `string`.
    pub fn with_text_output(mut self) -> Self {
        self.text_output = true;
        self
    }

    /// What a failing element does to the node (default: [`OnError::Fail`]).
    pub fn with_on_error(mut self, on_error: OnError) -> Self {
        self.on_error = on_error;
        self
    }

    /// At most `max_parallel` elements at the same time (at least 1).
    pub fn with_max_parallel(mut self, max_parallel: usize) -> Self {
        self.max_parallel = max_parallel.max(1);
        self
    }
}

/// The elements of the input: a list, or text that holds a JSON list (a code fence around it
/// is ignored).
fn elements(input: Value) -> Result<Vec<Value>, NodeError> {
    let value = match input {
        Value::String(text) => serde_json::from_str(strip_fence(&text)).map_err(|error| {
            format!("for_each needs a JSON array as input, the text is not JSON: {error}")
        })?,
        other => other,
    };
    match value {
        Value::Array(items) => Ok(items),
        other => Err(format!(
            "for_each needs a JSON array as input, got {}",
            kind_of(&other)
        )
        .into()),
    }
}

/// `text` without a surrounding Markdown code fence, which models often add around JSON.
fn strip_fence(text: &str) -> &str {
    let text = text.trim();
    match text
        .strip_prefix("```")
        .and_then(|rest| rest.strip_suffix("```"))
    {
        Some(inner) => inner
            .split_once('\n')
            .map_or(inner, |(_, body)| body)
            .trim(),
        None => text,
    }
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Runs the body of `config` for one element on `fork`.
async fn run_item(
    config: &ForEach,
    fork: &mut AgentContext,
    item: Value,
) -> Result<Value, NodeError> {
    match &config.body {
        Body::Agent(name) => fork.run_agent_step(name, item).await,
        Body::Graph { name, graph, scope } => {
            if let Some(scope) = scope {
                scope.enter(fork);
            }
            let result = graph.run(fork, item).await;
            if let Some(scope) = scope {
                scope.exit(fork);
            }
            result.map_err(|error| NodeError::from(format!("subgraph '{name}': {error}")))
        }
    }
}

/// Key of the progress of a `for_each` node in the agent storage (ADR-0052).
pub fn progress_key(node_id: &str) -> String {
    format!("grauwolf.for_each.{node_id}")
}

/// An element that has been dealt with, in the progress (ADR-0052).
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Finished {
    /// The body returned `output`.
    Done { output: Value },
    /// The body failed and the element was skipped; it is not run again.
    Skipped,
}

/// Runs the body for each element of `input` and returns the results in input order. The
/// node `node_id` reports `for_each_item_*` events. A failing element fails the node, or is
/// left out with [`OnError::Skip`].
pub async fn run_for_each(
    ctx: &mut AgentContext,
    config: &ForEach,
    node_id: &str,
    input: Value,
) -> Result<Value, NodeError> {
    let items = elements(input)?;
    let total = items.len();
    // Elements that finished before an interruption keep their result (ADR-0052).
    let key = progress_key(node_id);
    let mut finished: BTreeMap<usize, Finished> = ctx.load(&key)?.unwrap_or_default();
    finished.retain(|index, _| *index < total);
    let work: Vec<(usize, AgentContext, Value)> = items
        .into_iter()
        .enumerate()
        .filter(|(index, _)| !finished.contains_key(index))
        .map(|(index, item)| {
            let mut fork = ctx.fork();
            fork.remove(&key);
            (index, fork, item)
        })
        .collect();
    let mut running = stream::iter(work.into_iter().map(|(index, mut fork, item)| async move {
        fork.emit(AgentEvent::ForEachItemStarting {
            node: node_id.to_owned(),
            index,
        });
        let result = run_item(config, &mut fork, item).await;
        fork.emit(match &result {
            Ok(_) => AgentEvent::ForEachItemCompleted {
                node: node_id.to_owned(),
                index,
            },
            Err(error) => AgentEvent::ForEachItemFailed {
                node: node_id.to_owned(),
                index,
                error: error.to_string(),
            },
        });
        (index, result)
    }))
    .buffer_unordered(config.max_parallel);
    while let Some((index, result)) = running.next().await {
        match result {
            Ok(output) => {
                finished.insert(index, Finished::Done { output });
            }
            Err(error) if config.on_error == OnError::Fail => {
                return Err(format!("for_each '{node_id}', element {index}: {error}").into());
            }
            // Skipped: the event has reported it, the output leaves the element out.
            Err(_) => {
                finished.insert(index, Finished::Skipped);
            }
        }
        ctx.store(&key, &finished)?;
        ctx.save_progress().await?;
    }
    drop(running);
    ctx.remove(&key);
    let list = Value::Array(
        finished
            .into_values()
            .filter_map(|element| match element {
                Finished::Done { output } => Some(output),
                Finished::Skipped => None,
            })
            .collect(),
    );
    Ok(if config.text_output {
        Value::String(list.to_string())
    } else {
        list
    })
}

/// Node function for typed code: like [`run_for_each`] with input `I` and output `O`.
pub fn for_each<I: NodeData, O: NodeData>(
    config: ForEach,
    node_id: &'static str,
) -> impl for<'a> Fn(&'a mut AgentContext, I) -> BoxFuture<'a, Result<O, NodeError>>
+ Send
+ Sync
+ 'static {
    let config = std::sync::Arc::new(config);
    move |ctx, input| {
        let config = std::sync::Arc::clone(&config);
        Box::pin(async move {
            let input = serde_json::to_value(input)?;
            let output = run_for_each(ctx, &config, node_id, input).await?;
            Ok(serde_json::from_value(output)?)
        })
    }
}
