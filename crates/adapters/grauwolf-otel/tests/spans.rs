//! Spans from agent events (F06.10, ADR-0049).

use grauwolf_agents::events::{AgentEvent, Feature, OnBehalfOf, TraceParent};
use grauwolf_otel::OtelFeature;
use grauwolf_prompt::{LlModel, Message, ToolCall, ToolResult};
use opentelemetry::global::BoxedTracer;
use opentelemetry::trace::{Status, TracerProvider};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
use serde_json::json;

fn run(feature: &OtelFeature) {
    let events = [
        AgentEvent::AgentStarting {
            input: json!("What is 2+3?"),
            caller: None,
            trace_parent: None,
            on_behalf_of: None,
        },
        AgentEvent::NodeStarting {
            path: vec!["call_llm".into()],
            input: json!("What is 2+3?"),
        },
        AgentEvent::LlmCallStarting {
            model: LlModel::new("ollama", "qwen3.5:9b"),
            tools: vec!["add".into()],
        },
        AgentEvent::LlmCallCompleted {
            model: None,
            answer: Message::assistant("I am calculating."),
            usage: None,
            cost: None,
        },
        AgentEvent::NodeCompleted {
            path: vec!["call_llm".into()],
            output: json!({}),
        },
        AgentEvent::NodeStarting {
            path: vec!["execute_tool".into()],
            input: json!({}),
        },
        AgentEvent::ToolCallStarting {
            call: ToolCall {
                id: "add_1".into(),
                tool: "add".into(),
                args: json!({ "a": 2, "b": 3 }),
            },
        },
        AgentEvent::ToolCallCompleted {
            result: ToolResult {
                id: "add_1".into(),
                tool: "add".into(),
                content: "5".into(),
            },
        },
        AgentEvent::NodeCompleted {
            path: vec!["execute_tool".into()],
            output: json!({}),
        },
        AgentEvent::NodeStarting {
            path: vec!["send_result".into()],
            input: json!({}),
        },
        AgentEvent::LlmCallStarting {
            model: LlModel::new("ollama", "qwen3.5:9b"),
            tools: vec![],
        },
        AgentEvent::LlmCallFailed {
            error: "Timeout".into(),
        },
        AgentEvent::AgentFailed {
            error: "Node 'send_result' failed".into(),
            usage: None,
            cost: None,
        },
    ];
    for event in &events {
        feature.on_event(event);
    }
}

fn setup() -> (InMemorySpanExporter, SdkTracerProvider) {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    (exporter, provider)
}

fn span<'a>(spans: &'a [SpanData], name: &str) -> &'a SpanData {
    spans
        .iter()
        .find(|span| span.name == name)
        .unwrap_or_else(|| panic!("Span {name} missing"))
}

fn attribute(span: &SpanData, key: &str) -> Option<String> {
    span.attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == key)
        .map(|attribute| attribute.value.to_string())
}

#[test]
fn turns_a_run_into_nested_spans_following_the_genai_conventions() {
    let (exporter, provider) = setup();
    let feature = OtelFeature::new(
        BoxedTracer::new(Box::new(provider.tracer("test"))),
        "calculator",
    );

    run(&feature);

    let spans = exporter.get_finished_spans().unwrap();
    let agent = span(&spans, "invoke_agent calculator");
    let node = span(&spans, "node call_llm");
    let chat = spans
        .iter()
        .find(|span| span.name == "chat qwen3.5:9b")
        .unwrap();
    let tool = span(&spans, "execute_tool add");
    let tool_node = span(&spans, "node execute_tool");
    let failed_chat = spans
        .iter()
        .rfind(|span| span.name == "chat qwen3.5:9b")
        .unwrap();

    assert_eq!(node.parent_span_id, agent.span_context.span_id());
    assert_eq!(chat.parent_span_id, node.span_context.span_id());
    assert_eq!(tool.parent_span_id, tool_node.span_context.span_id());
    assert_eq!(
        attribute(agent, "gen_ai.operation.name").as_deref(),
        Some("invoke_agent")
    );
    assert_eq!(
        attribute(agent, "gen_ai.agent.name").as_deref(),
        Some("calculator")
    );
    assert_eq!(
        attribute(chat, "gen_ai.provider.name").as_deref(),
        Some("ollama")
    );
    assert_eq!(
        attribute(chat, "gen_ai.request.model").as_deref(),
        Some("qwen3.5:9b")
    );
    assert_eq!(
        attribute(tool, "gen_ai.tool.call.id").as_deref(),
        Some("add_1")
    );
    assert!(matches!(failed_chat.status, Status::Error { .. }));
    assert!(matches!(agent.status, Status::Error { .. }));
    // Without `with_content`, no contents.
    assert_eq!(attribute(tool, "gen_ai.tool.call.result"), None);
    assert!(span(&spans, "node send_result").end_time >= failed_chat.end_time);
}

#[test]
fn records_contents_only_when_asked_to() {
    let (exporter, provider) = setup();
    let feature = OtelFeature::new(
        BoxedTracer::new(Box::new(provider.tracer("test"))),
        "calculator",
    )
    .with_content(true);

    run(&feature);

    let spans = exporter.get_finished_spans().unwrap();
    let tool = span(&spans, "execute_tool add");
    let chat = spans
        .iter()
        .find(|span| span.name == "chat qwen3.5:9b")
        .unwrap();
    assert_eq!(
        attribute(tool, "gen_ai.tool.call.arguments").as_deref(),
        Some(r#"{"a":2,"b":3}"#)
    );
    assert_eq!(
        attribute(tool, "gen_ai.tool.call.result").as_deref(),
        Some("5")
    );
    assert!(
        attribute(chat, "gen_ai.output.messages")
            .unwrap()
            .contains("I am calculating.")
    );
}

/// Moderation (ADR-0066): own span under the node with model and verdict, without text.
#[test]
fn turns_moderations_into_spans_with_their_verdict() {
    let (exporter, provider) = setup();
    let feature = OtelFeature::new(
        BoxedTracer::new(Box::new(provider.tracer("test"))),
        "guarded",
    )
    .with_content(true);
    let model = LlModel::new("openai", "omni-moderation-latest");
    let events = [
        AgentEvent::AgentStarting {
            input: json!("I want to hurt someone."),
            caller: None,
            trace_parent: None,
            on_behalf_of: None,
        },
        AgentEvent::NodeStarting {
            path: vec!["moderate".into()],
            input: json!("I want to hurt someone."),
        },
        AgentEvent::ModerationStarting {
            model: model.clone(),
        },
        AgentEvent::ModerationCompleted {
            harmful: true,
            categories: vec!["violence".into(), "harassment".into()],
            usage: None,
            cost: None,
        },
        AgentEvent::ModerationStarting { model },
        AgentEvent::ModerationFailed {
            error: "Timeout".into(),
        },
        AgentEvent::AgentFailed {
            error: "Node 'moderate' failed".into(),
            usage: None,
            cost: None,
        },
    ];
    for event in &events {
        feature.on_event(event);
    }

    let spans = exporter.get_finished_spans().unwrap();
    let node = span(&spans, "node moderate");
    let moderations: Vec<&SpanData> = spans
        .iter()
        .filter(|span| span.name == "moderate omni-moderation-latest")
        .collect();

    assert_eq!(moderations.len(), 2);
    let (ok, failed) = (moderations[0], moderations[1]);
    assert_eq!(ok.parent_span_id, node.span_context.span_id());
    assert_eq!(
        attribute(ok, "gen_ai.operation.name").as_deref(),
        Some("moderation")
    );
    assert_eq!(
        attribute(ok, "gen_ai.provider.name").as_deref(),
        Some("openai")
    );
    assert_eq!(
        attribute(ok, "grauwolf.moderation.harmful").as_deref(),
        Some("true")
    );
    assert_eq!(
        attribute(ok, "grauwolf.moderation.categories").as_deref(),
        Some("violence,harassment")
    );
    assert!(matches!(failed.status, Status::Error { .. }));
    // The checked text appears nowhere, not even with `with_content`.
    assert!(
        ok.attributes
            .iter()
            .all(|attribute| !attribute.value.to_string().contains("verletzen"))
    );
}

/// Decisions (ADR-0100): own span under the node with the verdict, without the state; the LLM
/// call of a model-based decider is nested in it.
#[test]
fn turns_decisions_into_spans_with_their_verdict() {
    let (exporter, provider) = setup();
    let feature = OtelFeature::new(
        BoxedTracer::new(Box::new(provider.tracer("test"))),
        "router",
    )
    .with_content(true);
    let events = [
        AgentEvent::AgentStarting {
            input: json!("My invoice is wrong"),
            caller: None,
            trace_parent: None,
            on_behalf_of: None,
        },
        AgentEvent::NodeStarting {
            path: vec!["route".into()],
            input: json!("My invoice is wrong"),
        },
        AgentEvent::DecisionStarting {
            node: "route".into(),
        },
        AgentEvent::LlmCallStarting {
            model: LlModel::new("ollama", "qwen3.5:9b"),
            tools: vec![],
        },
        AgentEvent::LlmCallCompleted {
            model: None,
            answer: Message::assistant("{}"),
            usage: None,
            cost: None,
        },
        AgentEvent::DecisionCompleted {
            node: "route".into(),
            answer: "billing".into(),
            probability: Some(0.75),
            confidence: Some(0.5),
            model: Some("qwen3.5:9b".into()),
        },
        AgentEvent::DecisionStarting {
            node: "route".into(),
        },
        AgentEvent::DecisionFailed {
            node: "route".into(),
            error: "System One returned HTTP 503".into(),
        },
        AgentEvent::AgentFailed {
            error: "Node 'route' failed".into(),
            usage: None,
            cost: None,
        },
    ];
    for event in &events {
        feature.on_event(event);
    }

    let spans = exporter.get_finished_spans().unwrap();
    let node = span(&spans, "node route");
    let decisions: Vec<&SpanData> = spans
        .iter()
        .filter(|span| span.name == "decide route")
        .collect();
    let chat = span(&spans, "chat qwen3.5:9b");

    assert_eq!(decisions.len(), 2);
    let (ok, failed) = (decisions[0], decisions[1]);
    assert_eq!(ok.parent_span_id, node.span_context.span_id());
    assert_eq!(chat.parent_span_id, ok.span_context.span_id());
    assert_eq!(
        attribute(ok, "grauwolf.decision.answer").as_deref(),
        Some("billing")
    );
    assert_eq!(
        attribute(ok, "grauwolf.decision.probability").as_deref(),
        Some("0.75")
    );
    assert_eq!(
        attribute(ok, "grauwolf.decision.confidence").as_deref(),
        Some("0.5")
    );
    assert_eq!(
        attribute(ok, "grauwolf.decision.model").as_deref(),
        Some("qwen3.5:9b")
    );
    assert!(matches!(failed.status, Status::Error { .. }));
    assert!(
        ok.attributes
            .iter()
            .all(|attribute| !attribute.value.to_string().contains("invoice"))
    );
}

/// Token usage (ADR-0067) as GenAI attributes on the chat span.
#[test]
fn records_token_usage_on_chat_spans() {
    let (exporter, provider) = setup();
    let feature = OtelFeature::new(
        BoxedTracer::new(Box::new(provider.tracer("test"))),
        "calculator",
    );
    for event in [
        AgentEvent::AgentStarting {
            input: json!("Hello"),
            caller: None,
            trace_parent: None,
            on_behalf_of: None,
        },
        AgentEvent::LlmCallStarting {
            model: LlModel::new("anthropic", "claude-haiku-5-5"),
            tools: vec![],
        },
        AgentEvent::LlmCallCompleted {
            model: None,
            answer: Message::assistant("Hi"),
            usage: Some(grauwolf_prompt::Usage {
                input_tokens: 5930,
                output_tokens: 12,
                cache_read_tokens: Some(5906),
                cache_write_tokens: Some(20),
            }),
            cost: None,
        },
        AgentEvent::AgentCompleted {
            output: json!("Hi"),
            usage: None,
            cost: None,
        },
    ] {
        feature.on_event(&event);
    }

    let spans = exporter.get_finished_spans().unwrap();
    let chat = span(&spans, "chat claude-haiku-5-5");

    assert_eq!(
        attribute(chat, "gen_ai.usage.input_tokens").as_deref(),
        Some("5930")
    );
    assert_eq!(
        attribute(chat, "gen_ai.usage.output_tokens").as_deref(),
        Some("12")
    );
    assert_eq!(
        attribute(chat, "gen_ai.usage.cache_read.input_tokens").as_deref(),
        Some("5906")
    );
    assert_eq!(
        attribute(chat, "gen_ai.usage.cache_creation.input_tokens").as_deref(),
        Some("20")
    );
}

/// The model the provider reported (ADR-0094) as `gen_ai.response.model`.
#[test]
fn records_the_reported_model_on_chat_spans() {
    let (exporter, provider) = setup();
    let feature = OtelFeature::new(
        BoxedTracer::new(Box::new(provider.tracer("test"))),
        "calculator",
    );
    for event in [
        AgentEvent::AgentStarting {
            input: json!("Hello"),
            caller: None,
            trace_parent: None,
            on_behalf_of: None,
        },
        AgentEvent::LlmCallStarting {
            model: LlModel::new("anthropic", "claude-haiku-5-5"),
            tools: vec![],
        },
        AgentEvent::LlmCallCompleted {
            answer: Message::assistant("Hi"),
            usage: None,
            cost: None,
            model: Some("claude-haiku-5-5-20261001".into()),
        },
        AgentEvent::AgentCompleted {
            output: json!("Hi"),
            usage: None,
            cost: None,
        },
    ] {
        feature.on_event(&event);
    }

    let spans = exporter.get_finished_spans().unwrap();
    let chat = span(&spans, "chat claude-haiku-5-5");

    assert_eq!(
        attribute(chat, "gen_ai.response.model").as_deref(),
        Some("claude-haiku-5-5-20261001")
    );
}

/// Sum per run (ADR-0067) on the agent span.
#[test]
fn records_the_total_usage_on_the_agent_span() {
    let (exporter, provider) = setup();
    let feature = OtelFeature::new(
        BoxedTracer::new(Box::new(provider.tracer("test"))),
        "calculator",
    );
    feature.on_event(&AgentEvent::AgentStarting {
        input: json!("Hello"),
        caller: None,
        trace_parent: None,
        on_behalf_of: None,
    });
    feature.on_event(&AgentEvent::AgentCompleted {
        output: json!("Hi"),
        usage: Some(grauwolf_prompt::Usage {
            input_tokens: 7038,
            output_tokens: 39,
            cache_read_tokens: Some(3495),
            cache_write_tokens: Some(3534),
        }),
        cost: None,
    });

    let spans = exporter.get_finished_spans().unwrap();
    let agent = span(&spans, "invoke_agent calculator");

    assert_eq!(
        attribute(agent, "gen_ai.usage.input_tokens").as_deref(),
        Some("7038")
    );
    assert_eq!(
        attribute(agent, "gen_ai.usage.output_tokens").as_deref(),
        Some("39")
    );
    assert_eq!(
        attribute(agent, "gen_ai.usage.cache_read.input_tokens").as_deref(),
        Some("3495")
    );
}

/// The SPIFFE ID of the caller (ADR-0109) as `grauwolf.caller` on the agent span, only if known.
#[test]
fn records_the_caller_on_the_agent_span_when_there_is_one() {
    let (exporter, provider) = setup();
    let feature = OtelFeature::new(
        BoxedTracer::new(Box::new(provider.tracer("test"))),
        "calculator",
    );
    for caller in [Some("spiffe://example.org/ns/a/sa/coordinator"), None] {
        feature.on_event(&AgentEvent::AgentStarting {
            input: json!("Hello"),
            caller: caller.map(str::to_owned),
            trace_parent: None,
            on_behalf_of: None,
        });
        feature.on_event(&AgentEvent::AgentCompleted {
            output: json!("Hi"),
            usage: None,
            cost: None,
        });
    }

    let spans = exporter.get_finished_spans().unwrap();
    let callers: Vec<_> = spans
        .iter()
        .map(|span| attribute(span, "grauwolf.caller"))
        .collect();
    assert_eq!(
        callers,
        [
            Some("spiffe://example.org/ns/a/sa/coordinator".to_owned()),
            None
        ]
    );
}

/// The person a run acts for (ADR-0111) on the agent span: oid, name, upn and the chain ID.
#[test]
fn records_the_person_on_the_agent_span_when_there_is_one() {
    let (exporter, provider) = setup();
    let feature = OtelFeature::new(
        BoxedTracer::new(Box::new(provider.tracer("test"))),
        "reviewer",
    );
    let alice = OnBehalfOf {
        oid: "oid-alice".into(),
        name: Some("Alice".into()),
        upn: Some("alice@example.org".into()),
        txn: "txn-1".into(),
        grant: Some("grt_1".into()),
    };
    for on_behalf_of in [Some(alice), None] {
        feature.on_event(&AgentEvent::AgentStarting {
            input: json!("Hello"),
            caller: None,
            trace_parent: None,
            on_behalf_of,
        });
        feature.on_event(&AgentEvent::AgentCompleted {
            output: json!("Hi"),
            usage: None,
            cost: None,
        });
    }

    let spans = exporter.get_finished_spans().unwrap();
    let of = |name: &str| -> Vec<_> { spans.iter().map(|span| attribute(span, name)).collect() };
    assert_eq!(
        of("grauwolf.on_behalf_of.oid"),
        [Some("oid-alice".to_owned()), None]
    );
    assert_eq!(of("grauwolf.grant"), [Some("grt_1".to_owned()), None]);
    assert_eq!(
        of("grauwolf.on_behalf_of.name"),
        [Some("Alice".to_owned()), None]
    );
    assert_eq!(
        of("grauwolf.on_behalf_of.upn"),
        [Some("alice@example.org".to_owned()), None]
    );
    assert_eq!(of("grauwolf.txn"), [Some("txn-1".to_owned()), None]);
}

/// Cost (ADR-0071) as `grauwolf.cost` on chat and agent spans.
#[test]
fn records_costs_on_chat_and_agent_spans() {
    let (exporter, provider) = setup();
    let feature = OtelFeature::new(
        BoxedTracer::new(Box::new(provider.tracer("test"))),
        "calculator",
    );
    for event in [
        AgentEvent::AgentStarting {
            input: json!("Hello"),
            caller: None,
            trace_parent: None,
            on_behalf_of: None,
        },
        AgentEvent::LlmCallStarting {
            model: LlModel::new("anthropic", "claude-haiku-5-5"),
            tools: vec![],
        },
        AgentEvent::LlmCallCompleted {
            model: None,
            answer: Message::assistant("Hi"),
            usage: None,
            cost: Some(0.000_25),
        },
        AgentEvent::AgentCompleted {
            output: json!("Hi"),
            usage: None,
            cost: Some(0.000_75),
        },
    ] {
        feature.on_event(&event);
    }

    let spans = exporter.get_finished_spans().unwrap();

    assert_eq!(
        attribute(span(&spans, "chat claude-haiku-5-5"), "grauwolf.cost").as_deref(),
        Some("0.00025")
    );
    assert_eq!(
        attribute(span(&spans, "invoke_agent calculator"), "grauwolf.cost").as_deref(),
        Some("0.00075")
    );
}

#[test]
fn records_for_each_elements_as_span_events() {
    let (exporter, provider) = setup();
    let feature = OtelFeature::new(BoxedTracer::new(Box::new(provider.tracer("test"))), "flow");
    feature.on_event(&AgentEvent::AgentStarting {
        input: json!("Hello"),
        caller: None,
        trace_parent: None,
        on_behalf_of: None,
    });
    feature.on_event(&AgentEvent::ForEachItemStarting {
        node: "each".into(),
        index: 1,
    });
    feature.on_event(&AgentEvent::ForEachItemFailed {
        node: "each".into(),
        index: 1,
        error: "boom".into(),
    });
    feature.on_event(&AgentEvent::AgentCompleted {
        output: json!("Hi"),
        usage: None,
        cost: None,
    });

    let spans = exporter.get_finished_spans().unwrap();
    let agent = span(&spans, "invoke_agent flow");
    let names: Vec<&str> = agent
        .events
        .events
        .iter()
        .map(|event| event.name.as_ref())
        .collect();

    assert_eq!(names, ["for_each_item_starting", "for_each_item_failed"]);
}

fn header(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

/// The ID of a W3C `traceparent` value: `00-<trace id>-<span id>-<flags>`.
fn ids(traceparent: &str) -> (String, String) {
    let parts: Vec<_> = traceparent.split('-').collect();
    assert_eq!(parts.len(), 4, "{traceparent}");
    (parts[1].to_owned(), parts[2].to_owned())
}

fn start_tool_call(feature: &OtelFeature, id: &str, tool: &str) {
    feature.on_event(&AgentEvent::ToolCallStarting {
        call: ToolCall {
            id: id.into(),
            tool: tool.into(),
            args: json!({}),
        },
    });
}

/// ADR-0123: the context of a call out of the run is the span that makes the call.
#[test]
fn gives_the_headers_of_the_tool_call_span_for_a_call_out_of_the_run() {
    let (exporter, provider) = setup();
    let feature = OtelFeature::new(
        BoxedTracer::new(Box::new(provider.tracer("test"))),
        "coordinator",
    );
    feature.on_event(&AgentEvent::AgentStarting {
        input: json!("go"),
        caller: None,
        trace_parent: None,
        on_behalf_of: None,
    });
    start_tool_call(&feature, "a", "reviewer");
    start_tool_call(&feature, "b", "other");

    let first = feature.trace_headers(Some("a"));
    let second = feature.trace_headers(Some("b"));
    for id in ["a", "b"] {
        feature.on_event(&AgentEvent::ToolCallCompleted {
            result: ToolResult {
                id: id.into(),
                tool: "x".into(),
                content: "ok".into(),
            },
        });
    }
    feature.on_event(&AgentEvent::AgentCompleted {
        output: json!("done"),
        usage: None,
        cost: None,
    });

    let spans = exporter.get_finished_spans().unwrap();
    let reviewer = span(&spans, "execute_tool reviewer");
    let other = span(&spans, "execute_tool other");
    let agent = span(&spans, "invoke_agent coordinator");
    let (trace, parent) = ids(&header(&first, "traceparent").expect("traceparent"));
    assert_eq!(trace, agent.span_context.trace_id().to_string());
    assert_eq!(parent, reviewer.span_context.span_id().to_string());
    let (_, parent) = ids(&header(&second, "traceparent").expect("traceparent"));
    assert_eq!(parent, other.span_context.span_id().to_string());
}

#[test]
fn gives_the_innermost_open_node_for_a_call_without_a_tool_call_id() {
    let (exporter, provider) = setup();
    let feature = OtelFeature::new(
        BoxedTracer::new(Box::new(provider.tracer("test"))),
        "coordinator",
    );
    feature.on_event(&AgentEvent::AgentStarting {
        input: json!("go"),
        caller: None,
        trace_parent: None,
        on_behalf_of: None,
    });
    feature.on_event(&AgentEvent::NodeStarting {
        path: vec!["review".into()],
        input: json!({}),
    });

    let headers = feature.trace_headers(None);
    feature.on_event(&AgentEvent::AgentCompleted {
        output: json!("done"),
        usage: None,
        cost: None,
    });

    let spans = exporter.get_finished_spans().unwrap();
    let node = span(&spans, "node review");
    let (_, parent) = ids(&header(&headers, "traceparent").expect("traceparent"));
    assert_eq!(parent, node.span_context.span_id().to_string());
}

#[test]
fn gives_no_headers_before_the_agent_has_started() {
    let (_, provider) = setup();
    let feature = OtelFeature::new(
        BoxedTracer::new(Box::new(provider.tracer("test"))),
        "coordinator",
    );

    assert!(feature.trace_headers(None).is_empty());
}

const CALLER_TRACE: &str = "0af7651916cd43dd8448eb211c80319c";
const CALLER_SPAN: &str = "b7ad6b7169203331";

/// A run of `calculator` that starts with `trace_parent`, as a list of spans.
fn spans_of_run_with(trace_parent: Option<TraceParent>) -> (Vec<SpanData>, Vec<(String, String)>) {
    let (exporter, provider) = setup();
    let feature = OtelFeature::new(
        BoxedTracer::new(Box::new(provider.tracer("test"))),
        "calculator",
    );
    feature.on_event(&AgentEvent::AgentStarting {
        input: json!("Hello"),
        caller: None,
        trace_parent,
        on_behalf_of: None,
    });
    let headers = feature.trace_headers(None);
    feature.on_event(&AgentEvent::AgentCompleted {
        output: json!("Hi"),
        usage: None,
        cost: None,
    });
    (exporter.get_finished_spans().unwrap(), headers)
}

fn parent(traceparent: &str, tracestate: Option<&str>) -> Option<TraceParent> {
    Some(TraceParent {
        traceparent: traceparent.to_owned(),
        tracestate: tracestate.map(str::to_owned),
    })
}

/// ADR-0123: the agent span is a child of the caller's span, in the caller's trace.
#[test]
fn continues_the_trace_of_the_caller_when_agent_starting_has_a_trace_parent() {
    let sent = format!("00-{CALLER_TRACE}-{CALLER_SPAN}-01");

    let (spans, headers) = spans_of_run_with(parent(&sent, Some("vendor=1")));

    let agent = span(&spans, "invoke_agent calculator");
    assert_eq!(agent.span_context.trace_id().to_string(), CALLER_TRACE);
    assert_eq!(agent.parent_span_id.to_string(), CALLER_SPAN);
    // What the agent sends on keeps the trace and the state of the caller.
    assert_eq!(
        header(&headers, "traceparent").map(|value| ids(&value).0),
        Some(CALLER_TRACE.to_owned())
    );
    assert_eq!(header(&headers, "tracestate").as_deref(), Some("vendor=1"));
}

#[test]
fn a_malformed_trace_parent_starts_a_new_trace() {
    for malformed in [
        "garbage",
        "00-xyz-b7ad6b7169203331-01",
        "00-00000000000000000000000000000000-b7ad6b7169203331-01",
        "",
    ] {
        let (spans, _) = spans_of_run_with(parent(malformed, None));

        let agent = span(&spans, "invoke_agent calculator");
        assert_ne!(agent.span_context.trace_id().to_string(), CALLER_TRACE);
        assert_eq!(
            agent.parent_span_id,
            opentelemetry::trace::SpanId::INVALID,
            "{malformed:?}"
        );
    }
}

#[test]
fn without_a_trace_parent_the_agent_span_is_a_root() {
    let (spans, _) = spans_of_run_with(None);

    let agent = span(&spans, "invoke_agent calculator");
    assert_eq!(agent.parent_span_id, opentelemetry::trace::SpanId::INVALID);
}
