//! What a caller brings to a run of generated code (ADR-0119): prices, a budget of its own,
//! the approver, the frame sink, a conversation, attachments and checkpoints. The interpreter
//! gets the same things through its `RunExtras`; generated code takes them as one value and lets
//! [`RunHandles`] apply them to the context and the run lifecycle, so both behave alike.

use std::sync::Arc;

use grauwolf_graph::{EngineError, NodeData, TypedGraph};
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::{Attachment, LlModel, Prices, Prompt};
use serde::Serialize;

use crate::approval::{ApprovalGate, ToolApprover};
use crate::checkpoints::{CheckpointStore, ExecutionState};
use crate::events::{OnBehalfOf, TraceParent};
use crate::memory::ChatHistoryStore;
use crate::run_lifecycle::{AgentRun, RunError};
use crate::{AgentContext, Budget, FrameSink};

/// Why a run could not start.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// There is no stored checkpoint for the run to resume.
    #[error("no checkpoint for run '{run_id}'")]
    NoCheckpoint {
        /// The run ID that was asked for.
        run_id: String,
    },
    /// The checkpoint store failed.
    #[error("checkpoint store failed: {0}")]
    Checkpoint(String),
    /// A resume without checkpoints to resume from.
    #[error("a resume needs a checkpoint store and a run ID")]
    ResumeWithoutCheckpoints,
    /// A checkpoint run together with a conversation (ADR-0077).
    #[error("checkpoint runs cannot be combined with a conversation")]
    CheckpointConversation,
    /// A fresh run without an input.
    #[error("a run that is not a resume needs an input")]
    MissingInput,
    /// `limits.max_cost` without a price table: costs would be unknown, the limit ineffective.
    #[error("`limits.max_cost` needs a price table (`GRAUWOLF_PRICES`, CLI `--prices`)")]
    CostLimitWithoutPrices,
    /// The input cannot be converted to JSON.
    #[error("input cannot be converted: {0}")]
    Input(#[source] serde_json::Error),
}

/// The handles of one run of generated code; the default is a plain run.
#[derive(Clone, Default)]
pub struct RunHandles {
    /// Price table: events carry the cost, `max_cost` can be checked (ADR-0071).
    pub prices: Option<Arc<Prices>>,
    /// The caller's limits; each field set here wins over the spec's `limits` (ADR-0072).
    pub budget: Option<Budget>,
    /// Receives the frames of streaming nodes (ADR-0023).
    pub frame_sink: Option<FrameSink>,
    /// Decides on approval-required tools; without one they are refused (ADR-0061).
    pub approver: Option<ToolApprover>,
    /// The conversation to continue: store and ID (ADR-0060).
    pub conversation: Option<(Arc<dyn ChatHistoryStore>, String)>,
    /// Attachments to the input (ADR-0081).
    pub attachments: Vec<Attachment>,
    /// Checkpoints of the run: store and run ID (ADR-0020).
    pub checkpoints: Option<(Arc<dyn CheckpointStore>, String)>,
    /// Continue the run of `checkpoints` instead of starting a new one (ADR-0034).
    pub resume: bool,
    /// The SPIFFE ID of the calling workload, reported as `caller` in `agent_starting`
    /// (ADR-0109).
    pub caller: Option<String>,
    /// The trace context the caller sent, reported as `trace_parent` in `agent_starting`
    /// (ADR-0123).
    pub trace_parent: Option<TraceParent>,
    /// The person the run acts for, reported as `on_behalf_of` in `agent_starting` (ADR-0111).
    pub on_behalf_of: Option<OnBehalfOf>,
}

/// The caller's budget over the limits of the spec, field by field (what the interpreter does
/// for `with_budget` and `limits`); `None` when nothing is limited.
pub fn merged_budget(caller: Option<Budget>, spec: Budget) -> Option<Budget> {
    let caller = caller.unwrap_or_default();
    let budget = Budget {
        max_tokens: caller.max_tokens.or(spec.max_tokens),
        max_cost: caller.max_cost.or(spec.max_cost),
        max_tool_calls: caller.max_tool_calls.or(spec.max_tool_calls),
        max_prompt_tokens: caller.max_prompt_tokens.or(spec.max_prompt_tokens),
    };
    (budget != Budget::default()).then_some(budget)
}

impl RunHandles {
    /// The state to resume from, when this is a resume; `None` for a new run.
    pub async fn resume_state(&self) -> Result<Option<ExecutionState>, StartError> {
        if self.conversation.is_some() && self.checkpoints.is_some() {
            return Err(StartError::CheckpointConversation);
        }
        if !self.resume {
            return Ok(None);
        }
        let Some((store, run_id)) = &self.checkpoints else {
            return Err(StartError::ResumeWithoutCheckpoints);
        };
        store
            .load(run_id)
            .await
            .map_err(|error| StartError::Checkpoint(error.to_string()))?
            .map(Some)
            .ok_or_else(|| StartError::NoCheckpoint {
                run_id: run_id.clone(),
            })
    }

    /// The context of the run: new, or restored from `state` when resuming. It has the prices,
    /// the budget (the caller's over `limits` of the spec), the frame sink and – for the tools
    /// in `approval_tools` – the approval gate with the run's approver. The caller adds tools,
    /// features and the decider. `limits.max_cost` without prices is an error, as when the
    /// interpreter loads such a spec.
    pub fn context(
        &self,
        executor: Arc<PromptExecutor>,
        model: LlModel,
        prompt: Prompt,
        state: Option<&ExecutionState>,
        limits: Budget,
        approval_tools: &[&str],
    ) -> Result<AgentContext, StartError> {
        let budget = merged_budget(self.budget, limits);
        if budget.is_some_and(|budget| budget.max_cost.is_some()) && self.prices.is_none() {
            return Err(StartError::CostLimitWithoutPrices);
        }
        let mut ctx = match state {
            Some(state) => AgentContext::restore(executor, state.context.clone()),
            None => AgentContext::new(executor, model, prompt),
        };
        // Sub-agents of a flow run with the approver of this run (ADR-0101).
        if let Some(approver) = &self.approver {
            ctx = ctx.with_run_handle(Arc::new(approver.clone()));
        }
        if let Some(prices) = &self.prices {
            ctx = ctx.with_prices(Arc::clone(prices));
        }
        if let Some(budget) = budget {
            ctx = ctx.with_budget(budget);
        }
        if let Some(sink) = &self.frame_sink {
            ctx = ctx.with_frame_sink(Arc::clone(sink));
        }
        if !approval_tools.is_empty() {
            ctx.intercept(Arc::new(ApprovalGate {
                tools: approval_tools
                    .iter()
                    .map(|tool| (*tool).to_owned())
                    .collect(),
                approver: self.approver.clone(),
            }));
        }
        Ok(ctx)
    }

    /// The lifecycle of the run with conversation, attachments and checkpoints. A resume takes
    /// the input of the checkpoint and skips what is already part of it.
    pub fn start<I: Serialize>(
        &self,
        input: Option<&I>,
        state: Option<&ExecutionState>,
    ) -> Result<AgentRun, StartError> {
        let value = match (state, input) {
            (Some(state), _) => state.checkpoint.input.clone(),
            (None, Some(input)) => serde_json::to_value(input).map_err(StartError::Input)?,
            (None, None) => return Err(StartError::MissingInput),
        };
        let mut run = AgentRun::new(value)
            .with_caller(self.caller.clone())
            .with_trace_parent(self.trace_parent.clone())
            .with_on_behalf_of(self.on_behalf_of.clone());
        if let Some((store, run_id)) = &self.checkpoints {
            run = run.with_checkpoints(Arc::clone(store), run_id);
        }
        if state.is_some() {
            return Ok(run.resumed());
        }
        if let Some((store, id)) = &self.conversation {
            run = run.with_conversation(Arc::clone(store), id);
        }
        Ok(run.with_attachments(self.attachments.clone()))
    }

    /// Runs `graph` in the lifecycle `run`: from `input`, or from the checkpoint in `state`.
    pub async fn run_graph<I, O>(
        run: AgentRun,
        ctx: &mut AgentContext,
        graph: TypedGraph<AgentContext, I, O>,
        input: Option<I>,
        state: Option<ExecutionState>,
    ) -> Result<O, RunError>
    where
        I: NodeData,
        O: NodeData,
    {
        run.run(ctx, move |ctx, observer| {
            Box::pin(async move {
                match (state, input) {
                    (Some(state), _) => graph.resume(ctx, state.checkpoint, observer).await,
                    (None, Some(input)) => graph.run_observed(ctx, input, observer).await,
                    (None, None) => Err(EngineError::InvalidData(serde::de::Error::custom(
                        "the run has no input",
                    ))),
                }
            })
        })
        .await
    }
}
