//! Scopes for subgraphs (ADR-0034).

use grauwolf_graph::Scope;
use grauwolf_prompt::LlModel;

use crate::AgentContext;

/// Restricts the tools within a subgraph to a subset (F05.6, ADR-0027).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSubset(Vec<String>);

impl ToolSubset {
    /// Subset of the tool names `tools`.
    pub fn new(tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self(tools.into_iter().map(Into::into).collect())
    }
}

impl Scope<AgentContext> for ToolSubset {
    fn enter(&self, ctx: &mut AgentContext) {
        ctx.enter_tool_scope(self.0.clone());
    }

    fn exit(&self, ctx: &mut AgentContext) {
        ctx.leave_tool_scope();
    }
}

/// Scope of a subgraph with an optional tool subset and an optional model of its own
/// (F05.6, ADR-0034, ADR-0043); on exit the outer settings apply again.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubgraphScope {
    tools: Option<Vec<String>>,
    self_tools: Option<bool>,
    model: Option<LlModel>,
}

impl SubgraphScope {
    /// Scope without restriction and without a model of its own.
    pub fn new() -> Self {
        Self::default()
    }

    /// Restricts the tools to `tools`.
    pub fn with_tools(mut self, tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tools = Some(tools.into_iter().map(Into::into).collect());
        self
    }

    /// Allows or forbids self-built tools in the subgraph (ADR-0075).
    pub fn with_self_tools(mut self, allowed: bool) -> Self {
        self.self_tools = Some(allowed);
        self
    }

    /// Uses `model` in the subgraph.
    pub fn with_model(mut self, model: LlModel) -> Self {
        self.model = Some(model);
        self
    }
}

impl Scope<AgentContext> for SubgraphScope {
    fn enter(&self, ctx: &mut AgentContext) {
        if let Some(tools) = &self.tools {
            ctx.enter_tool_scope(tools.clone());
        }
        if let Some(allowed) = self.self_tools {
            ctx.enter_building_scope(allowed);
        }
        if let Some(model) = &self.model {
            ctx.enter_model_scope(model.clone());
        }
    }

    fn exit(&self, ctx: &mut AgentContext) {
        if self.model.is_some() {
            ctx.leave_model_scope();
        }
        if self.self_tools.is_some() {
            ctx.leave_tool_scope();
        }
        if self.tools.is_some() {
            ctx.leave_tool_scope();
        }
    }
}
