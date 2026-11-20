//! Typed builder on top of the value engine (ADR-0002, ADR-0017).
//!
//! Edges are type-checked at compile time: `edge(&a, &b)` only compiles if the
//! output type of `a` matches the input type of `b`. At the node boundaries, values
//! are (de)serialized to/from `serde_json::Value`.

use std::marker::PhantomData;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::engine::{
    Checkpoint, CheckpointObserver, ConditionFn, EngineError, FINISH, Graph, GraphBuilder,
    GraphError, NodeError, NodeFn, START, Scope, TransformFn,
};

/// Data that may flow between nodes: serializable (ADR-0010).
pub trait NodeData: Serialize + DeserializeOwned + Send + 'static {}

impl<T: Serialize + DeserializeOwned + Send + 'static> NodeData for T {}

/// Reference to a node with input type `I` and output type `O`.
pub struct NodeHandle<I, O> {
    id: String,
    types: PhantomData<fn(I) -> O>,
}

impl<I, O> NodeHandle<I, O> {
    fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            types: PhantomData,
        }
    }

    /// ID of the node.
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// Graph with typed input and output.
pub struct TypedGraph<Ctx, I, O> {
    graph: Graph<Ctx>,
    types: PhantomData<fn(I) -> O>,
}

impl<Ctx: Send + Sync + 'static, I: NodeData, O: NodeData> TypedGraph<Ctx, I, O> {
    /// Builds a graph; `define` registers nodes and edges.
    pub fn build(
        define: impl FnOnce(&mut TypedGraphBuilder<Ctx, I, O>),
    ) -> Result<Self, GraphError> {
        let mut builder = TypedGraphBuilder {
            inner: Graph::builder(),
            types: PhantomData,
        };
        define(&mut builder);
        Ok(Self {
            graph: builder.inner.build()?,
            types: PhantomData,
        })
    }

    /// The untyped graph, e.g. for the steps of a planner (ADR-0039).
    pub fn into_graph(self) -> Graph<Ctx> {
        self.graph
    }

    /// Runs the graph.
    pub async fn run(&self, ctx: &mut Ctx, input: I) -> Result<O, EngineError> {
        let input = serde_json::to_value(input).map_err(EngineError::InvalidData)?;
        let output = self.graph.run(ctx, input).await?;
        serde_json::from_value(output).map_err(EngineError::InvalidData)
    }

    /// Runs the graph and reports a checkpoint to `observer` before every node.
    pub async fn run_observed(
        &self,
        ctx: &mut Ctx,
        input: I,
        observer: &dyn CheckpointObserver<Ctx>,
    ) -> Result<O, EngineError> {
        let input = serde_json::to_value(input).map_err(EngineError::InvalidData)?;
        let output = self.graph.run_observed(ctx, input, observer).await?;
        serde_json::from_value(output).map_err(EngineError::InvalidData)
    }

    /// Resumes a run at `checkpoint` (ADR-0034).
    pub async fn resume(
        &self,
        ctx: &mut Ctx,
        checkpoint: Checkpoint,
        observer: &dyn CheckpointObserver<Ctx>,
    ) -> Result<O, EngineError> {
        let output = self.graph.resume(ctx, checkpoint, observer).await?;
        serde_json::from_value(output).map_err(EngineError::InvalidData)
    }
}

/// Builder for [`TypedGraph`].
pub struct TypedGraphBuilder<Ctx, I, O> {
    inner: GraphBuilder<Ctx>,
    types: PhantomData<fn(I) -> O>,
}

impl<Ctx: Send + Sync + 'static, I: NodeData, O: NodeData> TypedGraphBuilder<Ctx, I, O> {
    /// Limits the node executions per run (default: [`crate::DEFAULT_MAX_STEPS`]).
    pub fn max_steps(&mut self, limit: usize) {
        let inner = std::mem::replace(&mut self.inner, Graph::builder());
        self.inner = inner.max_steps(limit);
    }

    /// The start node: provides the input of the graph.
    pub fn start(&self) -> NodeHandle<I, I> {
        NodeHandle::new(START)
    }

    /// The finish node: receives the output of the graph.
    pub fn finish(&self) -> NodeHandle<O, O> {
        NodeHandle::new(FINISH)
    }

    /// Registers a node with typed input and output.
    pub fn node<NI, NO, F>(&mut self, id: impl Into<String>, run: F) -> NodeHandle<NI, NO>
    where
        NI: NodeData,
        NO: NodeData,
        F: for<'a> Fn(&'a mut Ctx, NI) -> BoxFuture<'a, Result<NO, NodeError>>
            + Send
            + Sync
            + 'static,
    {
        let id = id.into();
        self.inner.add_node(id.clone(), erase_node(run));
        NodeHandle::new(id)
    }

    /// Mounts `graph` as node `id`; the handle carries the input and output type of the subgraph
    /// (ADR-0034). `scope` is called on entering and leaving.
    pub fn subgraph<NI: NodeData, NO: NodeData>(
        &mut self,
        id: impl Into<String>,
        graph: TypedGraph<Ctx, NI, NO>,
        scope: Option<Box<dyn Scope<Ctx>>>,
    ) -> NodeHandle<NI, NO> {
        let id = id.into();
        self.inner.add_subgraph(id.clone(), graph.graph, scope);
        NodeHandle::new(id)
    }

    /// Connects `from` to `to`; only compiles if the types match.
    ///
    /// ```
    /// use grauwolf_graph::TypedGraph;
    ///
    /// let graph = TypedGraph::<(), String, usize>::build(|g| {
    ///     let length = g.node("length", |_, text: String| Box::pin(async move { Ok(text.len()) }));
    ///     g.edge(&g.start(), &length);
    ///     g.edge(&length, &g.finish());
    /// });
    /// assert!(graph.is_ok());
    /// ```
    ///
    /// An edge from `usize` to `String` is a compile error:
    ///
    /// ```compile_fail
    /// use grauwolf_graph::TypedGraph;
    ///
    /// let graph = TypedGraph::<(), String, String>::build(|g| {
    ///     let length = g.node("length", |_, text: String| Box::pin(async move { Ok(text.len()) }));
    ///     g.edge(&g.start(), &length);
    ///     g.edge(&length, &g.finish());
    /// });
    /// ```
    pub fn edge<A, B, C>(&mut self, from: &NodeHandle<A, B>, to: &NodeHandle<B, C>) {
        self.inner
            .add_edge(from.id.clone(), to.id.clone(), None, None);
    }

    /// Connects `from` to `to` under a condition: `condition` maps the output value
    /// of `from` to `Some(input for to)` or rejects the edge with `None`.
    pub fn edge_when<A, B, C, D, F>(
        &mut self,
        from: &NodeHandle<A, B>,
        to: &NodeHandle<C, D>,
        condition: F,
    ) where
        B: NodeData,
        C: NodeData,
        F: Fn(&Ctx, B) -> Option<C> + Send + Sync + 'static,
        Ctx: Sync,
    {
        self.inner.add_edge(
            from.id.clone(),
            to.id.clone(),
            Some(erase_condition(condition)),
            None,
        );
    }

    /// Connects `from` to `to` with an optional prebuilt condition and an optional
    /// transformation (ADR-0102), e.g. code from a spec. The types of the transformed value are
    /// not checked at compile time, but when `to` reads its input.
    pub fn edge_with<A, B, C, D>(
        &mut self,
        from: &NodeHandle<A, B>,
        to: &NodeHandle<C, D>,
        condition: Option<ConditionFn<Ctx>>,
        transform: Option<TransformFn<Ctx>>,
    ) {
        self.inner
            .add_edge(from.id.clone(), to.id.clone(), condition, transform);
    }
}

/// Turns a typed node function into a node function on `Value`:
/// the input is deserialized, the output serialized.
pub fn erase_node<Ctx, I, O, F>(run: F) -> NodeFn<Ctx>
where
    Ctx: Send + 'static,
    I: NodeData,
    O: NodeData,
    F: for<'a> Fn(&'a mut Ctx, I) -> BoxFuture<'a, Result<O, NodeError>> + Send + Sync + 'static,
{
    let run = Arc::new(run);
    Box::new(move |ctx, value| {
        let run = Arc::clone(&run);
        Box::pin(async move {
            let input: I = serde_json::from_value(value)?;
            let output = run(ctx, input).await?;
            Ok(serde_json::to_value(output)?)
        })
    })
}

/// Turns a typed filter map `B -> Option<C>` into an edge condition on `Value`.
pub fn erase_condition<Ctx, B, C, F>(condition: F) -> ConditionFn<Ctx>
where
    Ctx: Sync + 'static,
    B: NodeData,
    C: NodeData,
    F: Fn(&Ctx, B) -> Option<C> + Send + Sync + 'static,
{
    Box::new(move |ctx, value| {
        let passed = serde_json::from_value::<B>(value)
            .map_err(NodeError::from)
            .and_then(|input| {
                condition(ctx, input)
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(NodeError::from)
            });
        Box::pin(async move { passed })
    })
}

/// Turns a typed function `B -> C` into an edge transformation on `Value` (ADR-0102).
pub fn erase_transform<Ctx, B, C, F>(transform: F) -> TransformFn<Ctx>
where
    Ctx: Sync + 'static,
    B: NodeData,
    C: NodeData,
    F: Fn(&Ctx, B) -> Result<C, NodeError> + Send + Sync + 'static,
{
    Box::new(move |ctx, value| {
        let mapped = serde_json::from_value::<B>(value)
            .map_err(NodeError::from)
            .and_then(|input| transform(ctx, input))
            .and_then(|output| serde_json::to_value(output).map_err(NodeError::from));
        Box::pin(async move { mapped })
    })
}
