//! Tools that require approval (ADR-0061).

use std::sync::Arc;

use crate::events::{Interceptor, ToolDecision};
use futures_util::future::BoxFuture;
use grauwolf_prompt::ToolCall;

/// Decides asynchronously on an approval-required tool call, e.g. by asking a person
/// (ADR-0061).
pub type ToolApprover = Arc<dyn Fn(&ToolCall) -> BoxFuture<'static, ToolDecision> + Send + Sync>;

/// Gate in front of approval-required tools: asks the run's approver; without one, it
/// rejects the call.
pub struct ApprovalGate {
    pub tools: Vec<String>,
    pub approver: Option<ToolApprover>,
}

impl Interceptor for ApprovalGate {
    fn approve_tool_call<'a>(&'a self, call: &'a ToolCall) -> BoxFuture<'a, ToolDecision> {
        if !self.tools.contains(&call.tool) {
            return Box::pin(std::future::ready(ToolDecision::Proceed));
        }
        match &self.approver {
            Some(approver) => approver(call),
            None => Box::pin(std::future::ready(ToolDecision::Deny(format!(
                "tool '{}' needs an approval that nobody can give here",
                call.tool
            )))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(tool: &str) -> ToolCall {
        ToolCall {
            id: "1".into(),
            tool: tool.into(),
            args: serde_json::json!({}),
        }
    }

    fn decide(gate: &ApprovalGate, tool: &str) -> ToolDecision {
        futures_executor::block_on(gate.approve_tool_call(&call(tool)))
    }

    #[test]
    fn other_tools_pass_and_listed_ones_are_denied_without_an_approver() {
        let gate = ApprovalGate {
            tools: vec!["run_command".into()],
            approver: None,
        };
        assert!(matches!(decide(&gate, "other"), ToolDecision::Proceed));
        match decide(&gate, "run_command") {
            ToolDecision::Deny(text) => assert_eq!(
                text,
                "tool 'run_command' needs an approval that nobody can give here"
            ),
            ToolDecision::Proceed => panic!("must be denied"),
        }
    }
}
