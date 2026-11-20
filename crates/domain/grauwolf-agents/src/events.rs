//! Typed agent events and installable features (F06.1, F06.3, ADR-0007).
//!
//! Events are serializable and thus equally usable for tests, logs and exports (JSONL, tracing,
//! OpenTelemetry); export targets live in adapters.

use futures_util::future::BoxFuture;
use grauwolf_prompt::{LlModel, Message, Prompt, ToolCall, ToolDescriptor, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The W3C Trace Context of a caller, as the raw header values (ADR-0123). The runtime does not
/// parse them: the tracing feature does, and ignores what is malformed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceParent {
    /// The `traceparent` header.
    pub traceparent: String,
    /// The `tracestate` header, if the caller sent one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracestate: Option<String>,
}

/// The person a run acts for (ADR-0111), from a verified Txn-Token: their Entra object ID, the
/// display name and UPN when the token carries them, and the ID of the delegation chain. Never
/// the token itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnBehalfOf {
    /// The person's Entra object ID (`sub` of the Txn-Token); the run's user.
    pub oid: String,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// User principal name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upn: Option<String>,
    /// The ID of the delegation chain (`txn`), the same at every hop.
    pub txn: String,
    /// The grant of an unattended run (ADR-0111 stage 3), from `azd` of the Txn-Token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<String>,
}

/// An event in an agent's run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AgentEvent {
    /// The agent starts with `input`.
    AgentStarting {
        input: Value,
        /// The SPIFFE ID of the calling workload, when the call was authenticated by one
        /// (ADR-0109); never a token.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<String>,
        /// The trace context the caller sent (`traceparent` and `tracestate`, ADR-0123); a
        /// tracing feature starts its span for this agent as a child of it. It links spans and
        /// grants nothing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trace_parent: Option<TraceParent>,
        /// The person the run acts for, when the request carried a verified delegation
        /// (ADR-0111); never the token.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_behalf_of: Option<OnBehalfOf>,
    },
    /// The agent finished with `output`.
    AgentCompleted {
        output: Value,
        /// Total token usage of the run, if known (ADR-0067).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<grauwolf_prompt::Usage>,
        /// Total cost of the run, if a price table is known (ADR-0071).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost: Option<f64>,
    },
    /// The agent failed.
    AgentFailed {
        error: String,
        /// Total token usage up to the error, if known (ADR-0067).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<grauwolf_prompt::Usage>,
        /// Total cost up to the error, if known (ADR-0071).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost: Option<f64>,
    },
    /// The node at `path` starts with `input`.
    NodeStarting { path: Vec<String>, input: Value },
    /// The node at `path` finished with `output`.
    NodeCompleted { path: Vec<String>, output: Value },
    /// An LLM request to `model` starts; `tools` are the offered tools.
    LlmCallStarting { model: LlModel, tools: Vec<String> },
    /// The LLM request returned `answer`; `usage` are the consumed tokens, if the provider
    /// reports them (ADR-0067).
    LlmCallCompleted {
        answer: Message,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<grauwolf_prompt::Usage>,
        /// Cost of the call, if a price table knows the model (ADR-0071).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost: Option<f64>,
        /// The model the provider reports to have answered, if it says (ADR-0094).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// The LLM request failed.
    LlmCallFailed { error: String },
    /// `from` failed; the request goes to the fallback model `to` (ADR-0088).
    LlmFallback {
        from: LlModel,
        to: LlModel,
        error: String,
    },
    /// A tool call starts.
    ToolCallStarting { call: ToolCall },
    /// A tool call finished; errors are in the result as text.
    ToolCallCompleted { result: ToolResult },
    /// A moderation with `model` starts (ADR-0066); the checked text is not in the event.
    ModerationStarting { model: LlModel },
    /// The moderation has reached a verdict.
    ModerationCompleted {
        harmful: bool,
        categories: Vec<String>,
        /// Tokens consumed by the check, if reported (ADR-0067 addendum).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<grauwolf_prompt::Usage>,
        /// Cost at the moderation model's price (ADR-0071).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost: Option<f64>,
    },
    /// The moderation failed.
    ModerationFailed { error: String },
    /// The decision node `node` asks its decider; the state is not in the event (ADR-0100).
    DecisionStarting { node: String },
    /// The decision node `node` reached the verdict `answer`.
    DecisionCompleted {
        node: String,
        answer: String,
        /// Probability of yes (`yes_no`), if known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        probability: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        confidence: Option<f64>,
        /// The checkpoint or model that answered, if known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// The decision node `node` failed.
    DecisionFailed { node: String, error: String },
    /// The `for_each` node `node` starts the body for the element `index` (from 0, ADR-0101).
    ForEachItemStarting { node: String, index: usize },
    /// The body finished for the element `index` of the `for_each` node `node`.
    ForEachItemCompleted { node: String, index: usize },
    /// The body failed for the element `index` of the `for_each` node `node`.
    ForEachItemFailed {
        node: String,
        index: usize,
        error: String,
    },
    /// The long-term memory returned `facts` before the run (ADR-0069).
    MemoryRecalled { facts: Vec<String> },
    /// After the run, `facts` were stored in the long-term memory.
    MemoryRemembered { facts: Vec<String> },
    /// Recalling or storing failed; the run continues.
    MemoryFailed { error: String },
    /// The knowledge base added `count` sections from `sources` to the prompt (ADR-0113); the
    /// text is not part of the event.
    KnowledgeInjected { sources: Vec<String>, count: usize },
    /// Searching the knowledge base for the context failed; the run continues (ADR-0113).
    KnowledgeFailed { error: String },
    /// The agent loaded the skill `name` with `load_skill` (ADR-0103).
    SkillLoaded { name: String },
    /// A planner created or confirmed a plan before step `iteration` (from 1) (F10.7).
    PlanBuilt { iteration: usize, plan: Value },
    /// A planner executed step `iteration`; `plan` is the plan afterwards.
    PlanStepExecuted { iteration: usize, plan: Value },
}

/// Intercepts LLM and tool calls (F06.4, ADR-0047); all hooks do nothing by default.
pub trait Interceptor: Send + Sync {
    /// Before an LLM request; changes apply only to this request, not to the history.
    fn before_llm_call(&self, _prompt: &mut Prompt, _tools: &mut Vec<ToolDescriptor>) {}

    /// After an LLM answer, before it is appended to the history.
    fn after_llm_call(&self, _answer: &mut Message) {}

    /// Before a tool call; may change the arguments or refuse the call.
    fn before_tool_call(&self, _call: &mut ToolCall) -> ToolDecision {
        ToolDecision::Proceed
    }

    /// Approves a tool call after all `before_tool_call` hooks have agreed; may wait, e.g.
    /// for a human (ADR-0061).
    fn approve_tool_call<'a>(&'a self, _call: &'a ToolCall) -> BoxFuture<'a, ToolDecision> {
        Box::pin(std::future::ready(ToolDecision::Proceed))
    }

    /// After a tool call (including a refused one), before the result is passed on.
    fn after_tool_call(&self, _result: &mut ToolResult) {}
}

/// An interceptor's decision about a tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolDecision {
    /// Execute the call.
    Proceed,
    /// Do not execute; the text goes to the LLM as the tool result.
    Deny(String),
}

/// Extension that is installed in the [`crate::AgentContext`] and receives all events.
pub trait Feature: Send + Sync {
    /// Handles `event`; should return quickly and not block.
    fn on_event(&self, event: &AgentEvent);

    /// The HTTP headers (`traceparent`, and `tracestate` if there is one) with which a call out
    /// of this run continues the feature's trace (ADR-0123). `call` is the ID of the tool call
    /// that makes the request; without it, the innermost open step of the run (e.g. an `agent`
    /// node). Empty when the feature does not trace, which is the default.
    fn trace_headers(&self, call: Option<&str>) -> Vec<(String, String)> {
        let _ = call;
        Vec::new()
    }
}
