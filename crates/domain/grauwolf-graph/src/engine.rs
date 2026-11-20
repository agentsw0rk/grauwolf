//! Graph engine: runs a graph on `serde_json::Value` (ADR-0017).

use std::collections::HashMap;
use std::marker::PhantomData;

use futures_util::future::BoxFuture;
use serde_json::Value;

/// Reserved ID of the implicit start node.
pub const START: &str = "start";
/// Reserved ID of the implicit finish node.
pub const FINISH: &str = "finish";

/// Default limit for node executions per run.
pub const DEFAULT_MAX_STEPS: usize = 50;

/// Error reported by a node.
pub type NodeError = Box<dyn std::error::Error + Send + Sync>;

/// Executable node function: processes a value in the context `Ctx`.
pub type NodeFn<Ctx> = Box<
    dyn for<'a> Fn(&'a mut Ctx, Value) -> BoxFuture<'a, Result<Value, NodeError>> + Send + Sync,
>;

/// Edge condition as a filter map: `Some(value)` admits the edge (with the transformed value).
pub type ConditionFn<Ctx> = Box<
    dyn for<'a> Fn(&'a Ctx, Value) -> BoxFuture<'a, Result<Option<Value>, NodeError>> + Send + Sync,
>;

/// Transformation on an edge (ADR-0102): maps the value that passed the edge's condition to the
/// input of the target node.
pub type TransformFn<Ctx> =
    Box<dyn for<'a> Fn(&'a Ctx, Value) -> BoxFuture<'a, Result<Value, NodeError>> + Send + Sync>;

/// Error while building a graph.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GraphError {
    /// An edge points to a node that is not registered.
    #[error("edge points to unknown node '{node}'")]
    UnknownNode { node: String },
}

/// Error while running a graph.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// No matching edge leads on from node `node`.
    #[error("no matching edge from node '{node}'")]
    NoMatchingEdge { node: String },
    /// An edge points to a node that is not registered.
    #[error("unknown node '{node}'")]
    UnknownNode { node: String },
    /// A value could not be (de)serialized into the expected type.
    #[error("invalid data: {0}")]
    InvalidData(#[source] serde_json::Error),
    /// The run executed more than `limit` nodes (protection against endless loops).
    #[error("more than {limit} node executions – probably an endless loop")]
    MaxStepsExceeded { limit: usize },
    /// The condition of an edge from `node` failed.
    #[error("condition of an edge from '{node}' failed: {source}")]
    Condition {
        node: String,
        #[source]
        source: NodeError,
    },
    /// The path of a checkpoint does not fit the graph.
    #[error("checkpoint does not fit the graph: path {path:?} does not exist")]
    InvalidCheckpoint { path: Vec<String> },
    /// The transformation of an edge from `node` failed (ADR-0102).
    #[error("transformation of an edge from '{node}' failed: {source}")]
    Transform {
        node: String,
        #[source]
        source: NodeError,
    },
    /// The checkpoint observer failed.
    #[error("checkpoint failed: {0}")]
    Checkpoint(#[source] NodeError),
    /// A node failed.
    #[error("node '{node}' failed: {source}")]
    Node {
        node: String,
        #[source]
        source: NodeError,
    },
}

/// Called on entering and leaving a subgraph, e.g. to activate the subgraph's subset of
/// tools (ADR-0034).
pub trait Scope<Ctx>: Send + Sync {
    /// The run enters the subgraph.
    fn enter(&self, ctx: &mut Ctx);
    /// The run leaves the subgraph (also after an error).
    fn exit(&self, ctx: &mut Ctx);
}

/// A node: a function or a mounted subgraph.
enum Node<Ctx> {
    Fn(NodeFn<Ctx>),
    Subgraph {
        graph: Graph<Ctx>,
        scope: Option<Box<dyn Scope<Ctx>>>,
    },
}

/// Directed edge with an optional condition and an optional transformation.
struct Edge<Ctx> {
    from: String,
    to: String,
    condition: Option<ConditionFn<Ctx>>,
    transform: Option<TransformFn<Ctx>>,
}

/// Executable graph over the context `Ctx`.
pub struct Graph<Ctx> {
    nodes: HashMap<String, Node<Ctx>>,
    edges: Vec<Edge<Ctx>>,
    max_steps: usize,
    context: PhantomData<fn(&mut Ctx)>,
}

/// Position of a run: path of node IDs through the subgraphs to the next node, and
/// its input (ADR-0020, ADR-0034). This allows a run to be resumed.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Checkpoint {
    /// Node IDs from the main graph to the next node, e.g. `["solve", "call_llm"]`.
    pub path: Vec<String>,
    /// Input of the next node.
    pub input: Value,
}

/// Receives a [`Checkpoint`] before every node, e.g. to save it, and after every
/// node its output, e.g. for events.
pub trait CheckpointObserver<Ctx>: Send + Sync {
    /// Called before the node at `checkpoint.path` is executed.
    fn checkpoint<'a>(
        &'a self,
        ctx: &'a Ctx,
        checkpoint: &'a Checkpoint,
    ) -> BoxFuture<'a, Result<(), NodeError>>;

    /// Called after the node at `path` has finished with `output` (also subgraphs).
    fn node_completed<'a>(
        &'a self,
        _ctx: &'a Ctx,
        _path: &'a [String],
        _output: &'a Value,
    ) -> BoxFuture<'a, Result<(), NodeError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Observer that does nothing.
struct NoObserver;

impl<Ctx> CheckpointObserver<Ctx> for NoObserver {
    fn checkpoint<'a>(
        &'a self,
        _ctx: &'a Ctx,
        _checkpoint: &'a Checkpoint,
    ) -> BoxFuture<'a, Result<(), NodeError>> {
        Box::pin(async { Ok(()) })
    }
}

impl<Ctx: Send + Sync> Graph<Ctx> {
    /// Runs the graph with `input` and returns the value that reaches `finish`.
    pub async fn run(&self, ctx: &mut Ctx, input: Value) -> Result<Value, EngineError> {
        self.run_observed(ctx, input, &NoObserver).await
    }

    /// Like [`Graph::run`], but reports a checkpoint to `observer` before every node.
    pub async fn run_observed(
        &self,
        ctx: &mut Ctx,
        input: Value,
        observer: &dyn CheckpointObserver<Ctx>,
    ) -> Result<Value, EngineError> {
        self.run_from(ctx, START, input, &[], observer).await
    }

    /// Resumes a run at `checkpoint`: the node at the end of the path runs with the
    /// saved input, scopes on the path are entered again (ADR-0034).
    pub async fn resume(
        &self,
        ctx: &mut Ctx,
        checkpoint: Checkpoint,
        observer: &dyn CheckpointObserver<Ctx>,
    ) -> Result<Value, EngineError> {
        let Checkpoint { path, input } = checkpoint;
        self.resume_at(ctx, &path, input, &[], observer, &path)
            .await
    }

    /// Resumes at the path `path` (relative to this graph) and then runs until `finish`.
    fn resume_at<'a>(
        &'a self,
        ctx: &'a mut Ctx,
        path: &'a [String],
        input: Value,
        prefix: &'a [String],
        observer: &'a dyn CheckpointObserver<Ctx>,
        full_path: &'a [String],
    ) -> BoxFuture<'a, Result<Value, EngineError>> {
        Box::pin(async move {
            let invalid = || EngineError::InvalidCheckpoint {
                path: full_path.to_vec(),
            };
            let (id, rest) = path.split_first().ok_or_else(invalid)?;
            let node = self.nodes.get(id).ok_or_else(invalid)?;
            let here: Vec<String> = prefix.iter().cloned().chain([id.clone()]).collect();
            let output = if rest.is_empty() {
                let checkpoint = Checkpoint { path: here, input };
                observer
                    .checkpoint(ctx, &checkpoint)
                    .await
                    .map_err(EngineError::Checkpoint)?;
                self.execute(ctx, id, checkpoint.input, prefix, observer)
                    .await?
            } else {
                let Node::Subgraph { graph, scope } = node else {
                    return Err(invalid());
                };
                if let Some(scope) = scope {
                    scope.enter(ctx);
                }
                let result = graph
                    .resume_at(ctx, rest, input, &here, observer, full_path)
                    .await;
                if let Some(scope) = scope {
                    scope.exit(ctx);
                }
                let output = result?;
                observer
                    .node_completed(ctx, &here, &output)
                    .await
                    .map_err(EngineError::Checkpoint)?;
                output
            };
            self.run_from(ctx, id, output, prefix, observer).await
        })
    }

    /// Runs from node `current`, whose output is `value`, until `finish`. `prefix` is the
    /// path of the enclosing subgraphs.
    fn run_from<'a>(
        &'a self,
        ctx: &'a mut Ctx,
        current: &'a str,
        value: Value,
        prefix: &'a [String],
        observer: &'a dyn CheckpointObserver<Ctx>,
    ) -> BoxFuture<'a, Result<Value, EngineError>> {
        Box::pin(async move {
            let mut current = current;
            let mut value = value;
            let mut steps = 0;
            loop {
                let (to, next) = self.follow_edge(ctx, current, value).await?;
                if to == FINISH {
                    return Ok(next);
                }
                if steps == self.max_steps {
                    return Err(EngineError::MaxStepsExceeded {
                        limit: self.max_steps,
                    });
                }
                steps += 1;
                current = to;
                let checkpoint = Checkpoint {
                    path: prefix.iter().cloned().chain([current.to_owned()]).collect(),
                    input: next,
                };
                observer
                    .checkpoint(ctx, &checkpoint)
                    .await
                    .map_err(EngineError::Checkpoint)?;
                value = self
                    .execute(ctx, current, checkpoint.input, prefix, observer)
                    .await?;
            }
        })
    }

    /// Runs node `id` with `input`; a subgraph runs inside its scope.
    async fn execute(
        &self,
        ctx: &mut Ctx,
        id: &str,
        input: Value,
        prefix: &[String],
        observer: &dyn CheckpointObserver<Ctx>,
    ) -> Result<Value, EngineError> {
        let path: Vec<String> = prefix.iter().cloned().chain([id.to_owned()]).collect();
        let output = match self.nodes.get(id) {
            Some(Node::Fn(run)) => run(ctx, input).await.map_err(|source| EngineError::Node {
                node: id.to_owned(),
                source,
            })?,
            Some(Node::Subgraph { graph, scope }) => {
                if let Some(scope) = scope {
                    scope.enter(ctx);
                }
                let result = graph.run_from(ctx, START, input, &path, observer).await;
                if let Some(scope) = scope {
                    scope.exit(ctx);
                }
                result?
            }
            None => {
                return Err(EngineError::UnknownNode {
                    node: id.to_owned(),
                });
            }
        };
        observer
            .node_completed(ctx, &path, &output)
            .await
            .map_err(EngineError::Checkpoint)?;
        Ok(output)
    }
}

impl<Ctx> Graph<Ctx> {
    /// Starts building a graph.
    pub fn builder() -> GraphBuilder<Ctx> {
        GraphBuilder {
            graph: Graph {
                nodes: HashMap::new(),
                edges: Vec::new(),
                max_steps: DEFAULT_MAX_STEPS,
                context: PhantomData,
            },
        }
    }

    /// Picks the first edge from `from` whose condition matches and returns target and edge value;
    /// the transformation of the edge, if any, has been applied to the value (ADR-0102), so the
    /// value is the input of the target and what a checkpoint stores.
    async fn follow_edge(
        &self,
        ctx: &Ctx,
        from: &str,
        value: Value,
    ) -> Result<(&str, Value), EngineError> {
        for edge in self.edges.iter().filter(|edge| edge.from == from) {
            let passed = match &edge.condition {
                None => Some(value.clone()),
                Some(condition) => condition(ctx, value.clone()).await.map_err(|source| {
                    EngineError::Condition {
                        node: from.to_owned(),
                        source,
                    }
                })?,
            };
            if let Some(passed) = passed {
                let passed =
                    match &edge.transform {
                        None => passed,
                        Some(transform) => transform(ctx, passed).await.map_err(|source| {
                            EngineError::Transform {
                                node: from.to_owned(),
                                source,
                            }
                        })?,
                    };
                return Ok((&edge.to, passed));
            }
        }
        Err(EngineError::NoMatchingEdge {
            node: from.to_owned(),
        })
    }
}

/// Builds a [`Graph`].
pub struct GraphBuilder<Ctx> {
    graph: Graph<Ctx>,
}

impl<Ctx> GraphBuilder<Ctx> {
    /// Registers node `id` with the function `run`.
    pub fn node<F>(mut self, id: impl Into<String>, run: F) -> Self
    where
        F: for<'a> Fn(&'a mut Ctx, Value) -> BoxFuture<'a, Result<Value, NodeError>>
            + Send
            + Sync
            + 'static,
    {
        self.add_node(id.into(), Box::new(run));
        self
    }

    /// Mounts `graph` as node `id`; `scope` is called on entering and leaving.
    pub fn subgraph(
        mut self,
        id: impl Into<String>,
        graph: Graph<Ctx>,
        scope: Option<Box<dyn Scope<Ctx>>>,
    ) -> Self {
        self.add_subgraph(id.into(), graph, scope);
        self
    }

    /// Adds an unconditional edge from `from` to `to`.
    pub fn edge(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.add_edge(from.into(), to.into(), None, None);
        self
    }

    /// Adds an edge that only applies if `condition` returns a value;
    /// that value is passed on to `to`.
    pub fn edge_when<F>(
        mut self,
        from: impl Into<String>,
        to: impl Into<String>,
        condition: F,
    ) -> Self
    where
        F: for<'a> Fn(&'a Ctx, Value) -> BoxFuture<'a, Result<Option<Value>, NodeError>>
            + Send
            + Sync
            + 'static,
    {
        self.add_edge(from.into(), to.into(), Some(Box::new(condition)), None);
        self
    }

    /// Adds an edge that maps the value with `transform` before `to` receives it (ADR-0102).
    pub fn edge_transformed<T>(
        mut self,
        from: impl Into<String>,
        to: impl Into<String>,
        transform: T,
    ) -> Self
    where
        T: for<'a> Fn(&'a Ctx, Value) -> BoxFuture<'a, Result<Value, NodeError>>
            + Send
            + Sync
            + 'static,
    {
        self.add_edge(from.into(), to.into(), None, Some(Box::new(transform)));
        self
    }

    /// Adds an edge that only applies if `condition` returns a value; `transform` then maps that
    /// value to the input of `to` (ADR-0102).
    pub fn edge_when_transformed<F, T>(
        mut self,
        from: impl Into<String>,
        to: impl Into<String>,
        condition: F,
        transform: T,
    ) -> Self
    where
        F: for<'a> Fn(&'a Ctx, Value) -> BoxFuture<'a, Result<Option<Value>, NodeError>>
            + Send
            + Sync
            + 'static,
        T: for<'a> Fn(&'a Ctx, Value) -> BoxFuture<'a, Result<Value, NodeError>>
            + Send
            + Sync
            + 'static,
    {
        self.add_edge(
            from.into(),
            to.into(),
            Some(Box::new(condition)),
            Some(Box::new(transform)),
        );
        self
    }

    pub(crate) fn add_subgraph(
        &mut self,
        id: String,
        graph: Graph<Ctx>,
        scope: Option<Box<dyn Scope<Ctx>>>,
    ) {
        self.graph.nodes.insert(id, Node::Subgraph { graph, scope });
    }

    pub(crate) fn add_node(&mut self, id: String, run: NodeFn<Ctx>) {
        self.graph.nodes.insert(id, Node::Fn(run));
    }

    pub(crate) fn add_edge(
        &mut self,
        from: String,
        to: String,
        condition: Option<ConditionFn<Ctx>>,
        transform: Option<TransformFn<Ctx>>,
    ) {
        self.graph.edges.push(Edge {
            from,
            to,
            condition,
            transform,
        });
    }

    /// Limits the number of node executions per run (default: [`DEFAULT_MAX_STEPS`]).
    pub fn max_steps(mut self, limit: usize) -> Self {
        self.graph.max_steps = limit;
        self
    }

    /// Finishes building and checks that all edge ends are registered.
    pub fn build(self) -> Result<Graph<Ctx>, GraphError> {
        let graph = self.graph;
        let unknown = graph
            .edges
            .iter()
            .flat_map(|edge| [&edge.from, &edge.to])
            .find(|id| *id != START && *id != FINISH && !graph.nodes.contains_key(*id));
        match unknown {
            Some(node) => Err(GraphError::UnknownNode { node: node.clone() }),
            None => Ok(graph),
        }
    }
}
