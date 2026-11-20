//! Code in graphs (ADR-0102, F14.14): a `code` node, an `on_code` condition and an edge
//! transformation call a built code module – a core WASM module with tool ABI v1, run in the
//! sandbox like a tool – with the value as JSON. The code is a pure function of the value: it
//! sees no history, storage, tools, model, clock or randomness.
//!
//! The interpreter and generated code both use these functions, so they behave the same.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_graph::{ConditionFn, NodeData, NodeError, NodeFn, TransformFn};
use grauwolf_tools::{DynTool, ToolArtifact, ToolDescriptor, ToolError, ToolRuntime};
use serde_json::Value;

use crate::AgentContext;

/// A built piece of code of a graph, ready to run.
#[derive(Clone)]
pub struct CodeModule {
    tool: Arc<dyn DynTool>,
}

impl CodeModule {
    /// Wraps a module that `runtime` loaded from `artifact`; `artifact.name` names it in errors.
    pub fn load(runtime: &dyn ToolRuntime, artifact: &ToolArtifact) -> Result<Self, ToolError> {
        let descriptor = ToolDescriptor {
            name: artifact.name.clone(),
            description: "code of a graph".into(),
            parameters: serde_json::json!({}),
        };
        Ok(Self::new(runtime.instantiate(artifact, descriptor)?))
    }

    /// Wraps a callable module.
    pub fn new(tool: Arc<dyn DynTool>) -> Self {
        Self { tool }
    }

    /// Runs the code with `input` and returns its result as JSON. `Err(message)` of the code
    /// becomes the error with that message; a panic, a trap or an exhausted fuel limit is an
    /// error as well.
    pub async fn call(&self, input: Value) -> Result<Value, NodeError> {
        let text = self
            .tool
            .call_json(input)
            .await
            .map_err(|error| match error {
                ToolError::Execution(message) => NodeError::from(message),
                other => NodeError::from(other.to_string()),
            })?;
        Ok(serde_json::from_str(&text)?)
    }

    /// Runs the check of an `on_code` condition.
    pub async fn check(&self, input: Value) -> Result<bool, NodeError> {
        match self.call(input).await? {
            Value::Bool(passed) => Ok(passed),
            other => Err(format!("the condition did not answer true or false: {other}").into()),
        }
    }
}

/// Node function of a `code` node on values (interpreter).
pub fn code_node(module: CodeModule) -> NodeFn<AgentContext> {
    Box::new(move |_ctx: &mut AgentContext, input: Value| {
        let module = module.clone();
        Box::pin(async move { module.call(input).await })
    })
}

/// Typed node function of a `code` node (generated code): same behavior as [`code_node`].
pub fn code_node_typed<I: NodeData, O: NodeData>(
    module: CodeModule,
) -> impl for<'a> Fn(&'a mut AgentContext, I) -> BoxFuture<'a, Result<O, NodeError>>
+ Send
+ Sync
+ 'static {
    move |_ctx, input| {
        let module = module.clone();
        Box::pin(async move {
            let output = module.call(serde_json::to_value(input)?).await?;
            Ok(serde_json::from_value(output)?)
        })
    }
}

/// Edge condition `on_code`: passes the value through if the code answers `true`.
pub fn code_condition(module: CodeModule) -> ConditionFn<AgentContext> {
    Box::new(move |_ctx: &AgentContext, value: Value| {
        let module = module.clone();
        Box::pin(async move { Ok(module.check(value.clone()).await?.then_some(value)) })
    })
}

/// Edge transformation: the target receives the result of the code.
pub fn code_transform(module: CodeModule) -> TransformFn<AgentContext> {
    Box::new(move |_ctx: &AgentContext, value: Value| {
        let module = module.clone();
        Box::pin(async move { module.call(value).await })
    })
}
