//! Sub-agents as steps of a flow (ADR-0101): the handles of the calling run that a sub-agent
//! adopts, so that budget, usage and costs are shared.

use std::any::Any;
use std::sync::Arc;

use grauwolf_graph::NodeError;
use serde_json::Value;

use crate::AgentContext;
use crate::accounting::Accounting;

/// The calling run as a sub-agent sees it: the shared counters and limits (usage, cost, tool
/// calls, budget) and the opaque handle that the runtime put into the context with
/// [`AgentContext::with_run_handle`], e.g. the approver of the run.
pub struct ParentRun {
    pub(crate) accounting: Accounting,
    pub(crate) handle: Option<Arc<dyn Any + Send + Sync>>,
}

impl ParentRun {
    /// The handle of the run, if it has one of type `T`.
    pub fn handle<T: Any>(&self) -> Option<&T> {
        self.handle.as_ref()?.downcast_ref()
    }
}

impl AgentContext {
    /// Runs the sub-agent `name` (a tool of the registry, e.g. from `agent_tools`) as a step
    /// with `input` (ADR-0101). The tool gets the calling run as [`ParentRun`] in its
    /// `StepHandles`; the tool scopes of the run do not apply, the node names the agent.
    pub async fn run_agent_step(&self, name: &str, input: Value) -> Result<Value, NodeError> {
        let tool = self
            .tools
            .registry()
            .find(name)
            .ok_or_else(|| format!("agent '{name}' is not available as a sub-agent"))?;
        let parent: grauwolf_tools::StepHandles = Arc::new(ParentRun {
            accounting: self.accounting.fork(),
            handle: self.run_handle.clone(),
        });
        // A remote agent continues the trace of the step that calls it (ADR-0123).
        let headers = self.trace_headers(None);
        grauwolf_tools::with_trace_headers(headers, tool.call_step(input, Some(parent)))
            .await
            .map_err(|error| NodeError::from(format!("agent '{name}': {error}")))
    }
}
