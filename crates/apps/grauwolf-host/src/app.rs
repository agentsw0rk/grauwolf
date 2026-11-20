//! Options for the entry points of generated crates (ADR-0084, ADR-0114).

use std::sync::Arc;

use grauwolf_agents::approval::ToolApprover;
use grauwolf_agents::checkpoints::CheckpointStore;
use grauwolf_agents::events::{Feature, OnBehalfOf, TraceParent};
use grauwolf_agents::memory::ChatHistoryStore;
use grauwolf_agents::run_handles::RunHandles;
use grauwolf_agents::{Budget, FrameSink};
use grauwolf_prompt::{Attachment, Prices};
use grauwolf_spec::AgentSpec;

/// Features per agent, e.g. a tracing feature with the name of the spec.
pub type AgentFeatures = Arc<dyn Fn(&AgentSpec) -> Vec<Arc<dyn Feature>> + Send + Sync>;

/// What differs from run to run of a generated crate with feature `app`. What belongs to the
/// agent as a whole (environment, decider, observers, features of sub-agents) goes into
/// [`crate::provisioning::ProvisionOptions`].
#[derive(Clone, Default)]
pub struct RunOptions {
    /// User whose area of the long-term memory and the key-value store applies; without a user
    /// the shared area – like `spec run --user`.
    pub user: Option<String>,
    /// Features that see all events of the run.
    pub features: Vec<Arc<dyn Feature>>,
    /// Limits of this run; each field set here wins over the spec's `limits` (ADR-0072).
    pub budget: Option<Budget>,
    /// The conversation to continue: store and ID (ADR-0060).
    pub conversation: Option<(Arc<dyn ChatHistoryStore>, String)>,
    /// Attachments to the input (ADR-0081).
    pub attachments: Vec<Attachment>,
    /// Decides on tools that need an approval; without one they are refused (ADR-0061).
    pub approver: Option<ToolApprover>,
    /// Receives the frames of streaming nodes, i.e. the text deltas (ADR-0023).
    pub frame_sink: Option<FrameSink>,
    /// Checkpoints of the run: store and run ID (ADR-0020).
    pub checkpoints: Option<(Arc<dyn CheckpointStore>, String)>,
    /// Continue the run in `checkpoints` instead of starting one (ADR-0034); see
    /// [`RunOptions::resuming`].
    pub resume: bool,
    /// The SPIFFE ID of the calling workload, reported as `caller` in `agent_starting`
    /// (ADR-0109).
    pub caller: Option<String>,
    /// The trace context the caller sent, reported as `trace_parent` in `agent_starting`
    /// (ADR-0123): a tracing feature continues the caller's trace.
    pub trace_parent: Option<TraceParent>,
    /// The person the run acts for, reported as `on_behalf_of` in `agent_starting` (ADR-0111).
    pub on_behalf_of: Option<OnBehalfOf>,
}

impl RunOptions {
    /// Options that continue run `run_id` from its last checkpoint in `store` (the entry point
    /// `resume_provisioned` of a generated crate sets this itself).
    pub fn resuming(mut self, store: Arc<dyn CheckpointStore>, run_id: &str) -> Self {
        self.checkpoints = Some((store, run_id.to_owned()));
        self.resume = true;
        self
    }

    /// The handles of the run lifecycle for these options, with the price table of the
    /// provisioned agent.
    pub fn handles(&self, prices: Option<Arc<Prices>>) -> RunHandles {
        RunHandles {
            prices,
            budget: self.budget,
            frame_sink: self.frame_sink.clone(),
            approver: self.approver.clone(),
            conversation: self.conversation.clone(),
            attachments: self.attachments.clone(),
            checkpoints: self.checkpoints.clone(),
            resume: self.resume,
            caller: self.caller.clone(),
            trace_parent: self.trace_parent.clone(),
            on_behalf_of: self.on_behalf_of.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use grauwolf_agents::checkpoints::MemoryCheckpointStore;

    use super::*;

    #[test]
    fn resuming_options_name_the_checkpoints_and_ask_for_a_resume() {
        let options =
            RunOptions::default().resuming(Arc::new(MemoryCheckpointStore::default()), "r1");
        let handles = options.handles(None);
        assert!(handles.resume);
        assert_eq!(handles.checkpoints.map(|(_, id)| id).as_deref(), Some("r1"));
    }

    #[test]
    fn the_handles_carry_the_trace_context_of_the_caller() {
        let parent = TraceParent {
            traceparent: "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".into(),
            tracestate: Some("vendor=1".into()),
        };
        let options = RunOptions {
            trace_parent: Some(parent.clone()),
            ..RunOptions::default()
        };

        assert_eq!(options.handles(None).trace_parent, Some(parent));
    }

    #[test]
    fn the_handles_carry_the_budget_and_the_prices_of_the_agent() {
        let options = RunOptions {
            budget: Some(Budget {
                max_cost: Some(1.0),
                ..Budget::default()
            }),
            ..RunOptions::default()
        };
        let prices = Arc::new(Prices {
            currency: "USD".into(),
            models: Default::default(),
        });
        let handles = options.handles(Some(Arc::clone(&prices)));
        assert_eq!(handles.budget.and_then(|budget| budget.max_cost), Some(1.0));
        assert!(Arc::ptr_eq(handles.prices.as_ref().unwrap(), &prices));
        assert!(!handles.resume);
    }
}
