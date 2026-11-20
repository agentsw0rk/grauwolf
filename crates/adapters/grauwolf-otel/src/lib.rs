//! OpenTelemetry for grauwolf (F06.9, F06.10, ADR-0049): `OtelFeature` turns agent events into
//! spans following the GenAI Semantic Conventions.

use std::collections::HashMap;
use std::sync::Mutex;

use grauwolf_agents::events::{AgentEvent, Feature, TraceParent};
use opentelemetry::global::BoxedTracer;
use opentelemetry::propagation::TextMapPropagator;
use opentelemetry::trace::{Span, Status, TraceContextExt, Tracer};
use opentelemetry::{Context, KeyValue};
use opentelemetry_sdk::propagation::TraceContextPropagator;

/// Feature that reports every run as a tree of spans: agent → node → LLM/tool call.
pub struct OtelFeature {
    tracer: BoxedTracer,
    agent_name: String,
    content: bool,
    state: Mutex<State>,
}

/// Open spans of a run; each context carries its span.
#[derive(Default)]
struct State {
    agent: Option<Context>,
    nodes: Vec<(Vec<String>, Context)>,
    llm_calls: Vec<Context>,
    tool_calls: HashMap<String, Context>,
    /// Open moderations; like LLM calls without an ID, the most recent one wins (ADR-0066).
    moderations: Vec<Context>,
    /// Open decisions (ADR-0100); their model request is nested in them.
    decisions: Vec<Context>,
}

impl State {
    /// Context for new spans: innermost open decision or node, otherwise the agent.
    fn parent(&self) -> Context {
        self.decisions
            .last()
            .cloned()
            .or_else(|| self.nodes.last().map(|(_, cx)| cx.clone()))
            .or_else(|| self.agent.clone())
            .unwrap_or_default()
    }
}

impl OtelFeature {
    /// Feature for the agent `agent_name` with `tracer`; contents are not recorded.
    pub fn new(tracer: BoxedTracer, agent_name: impl Into<String>) -> Self {
        Self {
            tracer,
            agent_name: agent_name.into(),
            content: false,
            state: Mutex::new(State::default()),
        }
    }

    /// Also records contents (responses, tool arguments and results); these can
    /// contain personal data.
    pub fn with_content(mut self, content: bool) -> Self {
        self.content = content;
        self
    }

    fn start(&self, name: String, parent: &Context, attributes: Vec<KeyValue>) -> Context {
        let mut span = self.tracer.start_with_context(name, parent);
        span.set_attributes(attributes);
        parent.with_span(span)
    }
}

/// The remote span context of a caller's `traceparent` (and `tracestate`); `None` when the
/// header is malformed, so that the agent starts a new trace.
fn extract_parent(trace_parent: &TraceParent) -> Option<Context> {
    let mut headers = HashMap::from([("traceparent".to_owned(), trace_parent.traceparent.clone())]);
    if let Some(state) = &trace_parent.tracestate {
        headers.insert("tracestate".to_owned(), state.clone());
    }
    let cx = TraceContextPropagator::new().extract(&headers);
    cx.span().span_context().is_valid().then_some(cx)
}

/// Ends the span in `cx`; with `error`, sets an error status.
fn end(cx: Context, error: Option<&str>) {
    let span = cx.span();
    if let Some(error) = error {
        span.set_status(Status::error(error.to_owned()));
    }
    span.end();
}

impl Feature for OtelFeature {
    /// W3C Trace Context of the span that makes the call (ADR-0123): the tool call `call`, or
    /// the innermost open node of the run.
    fn trace_headers(&self, call: Option<&str>) -> Vec<(String, String)> {
        let state = self.state.lock().expect("OpenTelemetry state");
        let cx = call
            .and_then(|id| state.tool_calls.get(id))
            .cloned()
            .unwrap_or_else(|| state.parent());
        let mut headers = HashMap::new();
        TraceContextPropagator::new().inject_context(&cx, &mut headers);
        let mut headers: Vec<_> = headers.into_iter().collect();
        headers.sort();
        headers
    }

    fn on_event(&self, event: &AgentEvent) {
        let mut state = self.state.lock().expect("OpenTelemetry state");
        match event {
            AgentEvent::AgentStarting {
                caller,
                trace_parent,
                on_behalf_of,
                ..
            } => {
                *state = State::default();
                let mut attributes = vec![
                    KeyValue::new("gen_ai.operation.name", "invoke_agent"),
                    KeyValue::new("gen_ai.agent.name", self.agent_name.clone()),
                ];
                if let Some(caller) = caller {
                    attributes.push(KeyValue::new("grauwolf.caller", caller.clone()));
                }
                // The person the run acts for (ADR-0111); never the token.
                if let Some(person) = on_behalf_of {
                    attributes.push(KeyValue::new(
                        "grauwolf.on_behalf_of.oid",
                        person.oid.clone(),
                    ));
                    if let Some(name) = &person.name {
                        attributes.push(KeyValue::new("grauwolf.on_behalf_of.name", name.clone()));
                    }
                    if let Some(upn) = &person.upn {
                        attributes.push(KeyValue::new("grauwolf.on_behalf_of.upn", upn.clone()));
                    }
                    attributes.push(KeyValue::new("grauwolf.txn", person.txn.clone()));
                    if let Some(grant) = &person.grant {
                        attributes.push(KeyValue::new("grauwolf.grant", grant.clone()));
                    }
                }
                // The span continues the trace of a caller that sent one (ADR-0123).
                let parent = trace_parent
                    .as_ref()
                    .and_then(extract_parent)
                    .unwrap_or_default();
                let agent = self.start(
                    format!("invoke_agent {}", self.agent_name),
                    &parent,
                    attributes,
                );
                state.agent = Some(agent);
            }
            AgentEvent::AgentCompleted { usage, cost, .. }
            | AgentEvent::AgentFailed { usage, cost, .. } => {
                let error = match event {
                    AgentEvent::AgentFailed { error, .. } => Some(error.as_str()),
                    _ => None,
                };
                let open = std::mem::take(&mut *state);
                for cx in open.decisions.into_iter().rev() {
                    end(cx, error);
                }
                for cx in open.moderations.into_iter().rev() {
                    end(cx, error);
                }
                for cx in open.llm_calls.into_iter().rev() {
                    end(cx, error);
                }
                for (_, cx) in open.tool_calls {
                    end(cx, error);
                }
                for (_, cx) in open.nodes.into_iter().rev() {
                    end(cx, error);
                }
                if let Some(agent) = open.agent {
                    if let Some(usage) = usage {
                        record_usage(&agent, usage);
                    }
                    if let Some(cost) = cost {
                        agent
                            .span()
                            .set_attribute(KeyValue::new("grauwolf.cost", *cost));
                    }
                    end(agent, error);
                }
            }
            AgentEvent::NodeStarting { path, .. } => {
                let parent = state.parent();
                let node = self.start(
                    format!("node {}", path.join("/")),
                    &parent,
                    vec![KeyValue::new("grauwolf.node.path", path.join("/"))],
                );
                state.nodes.push((path.clone(), node));
            }
            AgentEvent::NodeCompleted { path, .. } => {
                // Subgraphs only report their end; without a matching start, nothing to end.
                if let Some(index) = state.nodes.iter().rposition(|(open, _)| open == path) {
                    let (_, node) = state.nodes.remove(index);
                    end(node, None);
                }
            }
            AgentEvent::LlmCallStarting { model, tools } => {
                let parent = state.parent();
                let call = self.start(
                    format!("chat {}", model.id),
                    &parent,
                    vec![
                        KeyValue::new("gen_ai.operation.name", "chat"),
                        KeyValue::new("gen_ai.provider.name", model.provider.clone()),
                        KeyValue::new("gen_ai.request.model", model.id.clone()),
                        KeyValue::new("grauwolf.tools", tools.join(",")),
                    ],
                );
                state.llm_calls.push(call);
            }
            AgentEvent::LlmCallCompleted {
                answer,
                usage,
                cost,
                model,
            } => {
                if let Some(call) = state.llm_calls.pop() {
                    if let Some(model) = model {
                        call.span()
                            .set_attribute(KeyValue::new("gen_ai.response.model", model.clone()));
                    }
                    if let Some(usage) = usage {
                        record_usage(&call, usage);
                    }
                    // Cost in the currency of the price table (ADR-0071).
                    if let Some(cost) = cost {
                        call.span()
                            .set_attribute(KeyValue::new("grauwolf.cost", *cost));
                    }
                    if self.content {
                        call.span().set_attribute(KeyValue::new(
                            "gen_ai.output.messages",
                            serde_json::to_string(answer).unwrap_or_default(),
                        ));
                    }
                    end(call, None);
                }
            }
            AgentEvent::LlmCallFailed { error } => {
                if let Some(call) = state.llm_calls.pop() {
                    end(call, Some(error));
                }
            }
            AgentEvent::ToolCallStarting { call } => {
                let parent = state.parent();
                let mut attributes = vec![
                    KeyValue::new("gen_ai.operation.name", "execute_tool"),
                    KeyValue::new("gen_ai.tool.name", call.tool.clone()),
                    KeyValue::new("gen_ai.tool.call.id", call.id.clone()),
                ];
                if self.content {
                    attributes.push(KeyValue::new(
                        "gen_ai.tool.call.arguments",
                        call.args.to_string(),
                    ));
                }
                let span = self.start(format!("execute_tool {}", call.tool), &parent, attributes);
                state.tool_calls.insert(call.id.clone(), span);
            }
            AgentEvent::ToolCallCompleted { result } => {
                if let Some(call) = state.tool_calls.remove(&result.id) {
                    if self.content {
                        call.span().set_attribute(KeyValue::new(
                            "gen_ai.tool.call.result",
                            result.content.clone(),
                        ));
                    }
                    // Tool errors reach the LLM as text (ADR-0027) and start with "Error:".
                    let error = result
                        .content
                        .starts_with("Error:")
                        .then_some(result.content.as_str());
                    end(call, error);
                }
            }
            // Moderation (ADR-0066): model and verdict, never the checked text.
            AgentEvent::ModerationStarting { model } => {
                let parent = state.parent();
                let moderation = self.start(
                    format!("moderate {}", model.id),
                    &parent,
                    vec![
                        KeyValue::new("gen_ai.operation.name", "moderation"),
                        KeyValue::new("gen_ai.provider.name", model.provider.clone()),
                        KeyValue::new("gen_ai.request.model", model.id.clone()),
                    ],
                );
                state.moderations.push(moderation);
            }
            AgentEvent::ModerationCompleted {
                harmful,
                categories,
                usage,
                cost,
            } => {
                if let Some(moderation) = state.moderations.pop() {
                    if let Some(usage) = usage {
                        record_usage(&moderation, usage);
                    }
                    if let Some(cost) = cost {
                        moderation
                            .span()
                            .set_attribute(KeyValue::new("grauwolf.cost", *cost));
                    }
                    moderation
                        .span()
                        .set_attribute(KeyValue::new("grauwolf.moderation.harmful", *harmful));
                    moderation.span().set_attribute(KeyValue::new(
                        "grauwolf.moderation.categories",
                        categories.join(","),
                    ));
                    end(moderation, None);
                }
            }
            AgentEvent::ModerationFailed { error } => {
                if let Some(moderation) = state.moderations.pop() {
                    end(moderation, Some(error));
                }
            }
            // Decision (ADR-0100): the verdict, never the state.
            AgentEvent::DecisionStarting { node } => {
                let parent = state.parent();
                let decision = self.start(
                    format!("decide {node}"),
                    &parent,
                    vec![KeyValue::new("grauwolf.decision.node", node.clone())],
                );
                state.decisions.push(decision);
            }
            AgentEvent::DecisionCompleted {
                answer,
                probability,
                confidence,
                model,
                ..
            } => {
                if let Some(decision) = state.decisions.pop() {
                    let span = decision.span();
                    span.set_attribute(KeyValue::new("grauwolf.decision.answer", answer.clone()));
                    if let Some(probability) = probability {
                        span.set_attribute(KeyValue::new(
                            "grauwolf.decision.probability",
                            *probability,
                        ));
                    }
                    if let Some(confidence) = confidence {
                        span.set_attribute(KeyValue::new(
                            "grauwolf.decision.confidence",
                            *confidence,
                        ));
                    }
                    if let Some(model) = model {
                        span.set_attribute(KeyValue::new("grauwolf.decision.model", model.clone()));
                    }
                    end(decision, None);
                }
            }
            AgentEvent::DecisionFailed { error, .. } => {
                if let Some(decision) = state.decisions.pop() {
                    end(decision, Some(error));
                }
            }
            // Automatic long-term memory (ADR-0069) as span events with the number of facts.
            AgentEvent::MemoryRecalled { facts } | AgentEvent::MemoryRemembered { facts } => {
                let name = match event {
                    AgentEvent::MemoryRecalled { .. } => "memory_recalled",
                    _ => "memory_remembered",
                };
                let count = KeyValue::new("grauwolf.memory.facts", facts.len() as i64);
                state.parent().span().add_event(name, vec![count]);
            }
            // Knowledge as context (ADR-0113): a span event with the number of sections.
            AgentEvent::KnowledgeInjected { count, .. } => {
                let count = KeyValue::new("grauwolf.knowledge.sections", *count as i64);
                state
                    .parent()
                    .span()
                    .add_event("knowledge_injected", vec![count]);
            }
            // Switch to a fallback model (ADR-0088) on the open LLM call.
            AgentEvent::LlmFallback { from, to, error } => {
                let attributes = vec![
                    KeyValue::new(
                        "grauwolf.fallback.from",
                        format!("{}:{}", from.provider, from.id),
                    ),
                    KeyValue::new("grauwolf.fallback.to", format!("{}:{}", to.provider, to.id)),
                    KeyValue::new("error.message", error.clone()),
                ];
                match state.llm_calls.last() {
                    Some(call) => call.span().add_event("llm_fallback", attributes),
                    None => state.parent().span().add_event("llm_fallback", attributes),
                }
            }
            AgentEvent::MemoryFailed { error } => {
                state.parent().span().add_event(
                    "memory_failed",
                    vec![KeyValue::new("error.message", error.clone())],
                );
            }
            AgentEvent::KnowledgeFailed { error } => {
                state.parent().span().add_event(
                    "knowledge_failed",
                    vec![KeyValue::new("error.message", error.clone())],
                );
            }
            // Elements of a `for_each` node (ADR-0101) as span events of the open span.
            AgentEvent::ForEachItemStarting { node, index }
            | AgentEvent::ForEachItemCompleted { node, index }
            | AgentEvent::ForEachItemFailed { node, index, .. } => {
                let name = match event {
                    AgentEvent::ForEachItemStarting { .. } => "for_each_item_starting",
                    AgentEvent::ForEachItemCompleted { .. } => "for_each_item_completed",
                    _ => "for_each_item_failed",
                };
                let mut attributes = vec![
                    KeyValue::new("grauwolf.for_each.node", node.clone()),
                    KeyValue::new("grauwolf.for_each.index", *index as i64),
                ];
                if let AgentEvent::ForEachItemFailed { error, .. } = event {
                    attributes.push(KeyValue::new("error.message", error.clone()));
                }
                state.parent().span().add_event(name, attributes);
            }
            AgentEvent::SkillLoaded { name } => {
                state.parent().span().add_event(
                    "skill_loaded",
                    vec![KeyValue::new("grauwolf.skill.name", name.clone())],
                );
            }
            AgentEvent::PlanBuilt { iteration, plan }
            | AgentEvent::PlanStepExecuted { iteration, plan } => {
                let name = match event {
                    AgentEvent::PlanBuilt { .. } => "plan_built",
                    _ => "plan_step_executed",
                };
                let mut attributes =
                    vec![KeyValue::new("grauwolf.plan.iteration", *iteration as i64)];
                if self.content {
                    attributes.push(KeyValue::new("grauwolf.plan", plan.to_string()));
                }
                state.parent().span().add_event(name, attributes);
            }
        }
    }
}

/// Token usage following the GenAI convention on the span of `cx` (ADR-0067).
fn record_usage(cx: &Context, usage: &grauwolf_prompt::Usage) {
    let span = cx.span();
    let count = |key: &'static str, value: u64| {
        KeyValue::new(key, i64::try_from(value).unwrap_or(i64::MAX))
    };
    span.set_attribute(count("gen_ai.usage.input_tokens", usage.input_tokens));
    span.set_attribute(count("gen_ai.usage.output_tokens", usage.output_tokens));
    if let Some(read) = usage.cache_read_tokens {
        span.set_attribute(count("gen_ai.usage.cache_read.input_tokens", read));
    }
    if let Some(write) = usage.cache_write_tokens {
        span.set_attribute(count("gen_ai.usage.cache_creation.input_tokens", write));
    }
}
