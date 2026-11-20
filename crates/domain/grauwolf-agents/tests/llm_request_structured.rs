//! Node `llm_request_structured` (F05.9, ADR-0031).

use std::sync::Arc;

use futures_executor::block_on;
use grauwolf_agents::{AgentContext, Structured, nodes};
use grauwolf_graph::TypedGraph;
use grauwolf_llm::{PromptExecutor, StructuredOutput};
use grauwolf_prompt::{LlModel, Message, Prompt};
use grauwolf_test::MockLlmClient;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Ticket {
    title: String,
    priority: i64,
}

impl Structured for Ticket {
    fn structured_output() -> StructuredOutput {
        StructuredOutput {
            name: "Ticket".into(),
            schema: serde_json::json!({
                "type": "object",
                "properties": { "title": { "type": "string" }, "priority": { "type": "integer" } },
                "required": ["title", "priority"]
            }),
        }
    }
}

fn context(answer: &str) -> AgentContext {
    let executor = PromptExecutor::builder()
        .client(
            "mock",
            Arc::new(MockLlmClient::new("?").answer_when_contains("Login", answer)),
        )
        .build();
    AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("triage").build(),
    )
}

fn graph() -> TypedGraph<AgentContext, String, Ticket> {
    TypedGraph::build(|g| {
        let triage = g.node("triage", nodes::llm_request_structured::<Ticket>);
        g.edge(&g.start(), &triage);
        g.edge(&triage, &g.finish());
    })
    .unwrap()
}

#[test]
fn parses_the_answer_into_the_requested_type() {
    let answer = "```json\n{ \"title\": \"Login broken\", \"priority\": 1 }\n```";
    let mut ctx = context(answer);

    let ticket = block_on(graph().run(&mut ctx, "Bug in Login".to_string())).unwrap();

    assert_eq!(
        ticket,
        Ticket {
            title: "Login broken".into(),
            priority: 1
        }
    );
    assert_eq!(
        ctx.prompt().messages,
        vec![Message::user("Bug in Login"), Message::assistant(answer)]
    );
}

#[test]
fn fails_when_the_answer_does_not_match_the_schema() {
    let mut ctx = context(r#"{ "title": "Login broken" }"#);

    let error = block_on(graph().run(&mut ctx, "Bug in Login".to_string())).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("/priority: missing required field"),
        "{error}"
    );
}

#[test]
fn value_implementation_uses_the_schema_from_the_spec_types() {
    let types = std::collections::BTreeMap::from([(
        "Ticket".to_string(),
        Ticket::structured_output().schema,
    )]);
    let run = grauwolf_agents::implementations::node(
        "llm_request_structured",
        &serde_json::json!({ "type": "Ticket" }),
        &types,
    )
    .unwrap();
    let graph = grauwolf_graph::Graph::builder()
        .node("triage", run)
        .edge(grauwolf_graph::START, "triage")
        .edge("triage", grauwolf_graph::FINISH)
        .build()
        .unwrap();
    let mut ctx = context(r#"{ "title": "Login broken", "priority": 1, "extra": true }"#);

    let value = block_on(graph.run(&mut ctx, serde_json::json!("Bug in Login"))).unwrap();

    assert_eq!(
        value,
        serde_json::json!({ "title": "Login broken", "priority": 1 })
    );
}

#[test]
fn asks_again_with_the_error_when_retries_are_allowed() {
    let invalid = r#"{ "title": "Login broken" }"#;
    let valid = r#"{ "title": "Login broken", "priority": 1 }"#;
    let executor = PromptExecutor::builder()
        .client(
            "mock",
            Arc::new(
                MockLlmClient::new("?")
                    .answer_when_contains("missing required field", valid)
                    .answer_when_contains("Login", invalid),
            ),
        )
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("triage").build(),
    );
    let graph = TypedGraph::<AgentContext, String, Ticket>::build(|g| {
        let triage = g.node(
            "triage",
            nodes::llm_request_structured_with_retries::<Ticket, 1>,
        );
        g.edge(&g.start(), &triage);
        g.edge(&triage, &g.finish());
    })
    .unwrap();

    let ticket = block_on(graph.run(&mut ctx, "Bug in Login".to_string())).unwrap();

    assert_eq!(ticket.priority, 1);
    let messages = &ctx.prompt().messages;
    assert_eq!(messages.len(), 4, "{messages:?}");
    assert_eq!(messages[1], Message::assistant(invalid));
    assert!(
        matches!(&messages[2], Message::User { content, .. } if content.contains("/priority: missing required field")),
        "{messages:?}"
    );
    assert_eq!(messages[3], Message::assistant(valid));
}
