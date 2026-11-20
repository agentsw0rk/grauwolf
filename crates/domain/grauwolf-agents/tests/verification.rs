//! Node `llm_verify` (F05.13, ADR-0041): have an answer checked.

use std::collections::BTreeMap;
use std::sync::Arc;

use futures_executor::block_on;
use grauwolf_agents::structured::Structured;
use grauwolf_agents::{AgentContext, implementations, nodes};
use grauwolf_graph::TypedGraph;
use grauwolf_llm::{PromptExecutor, StructuredOutput};
use grauwolf_prompt::{LlModel, Message, Prompt};
use grauwolf_test::MockLlmClient;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "correct": { "type": "boolean" },
            "feedback": { "type": "string" },
            "answer": { "type": "string" }
        },
        "required": ["correct", "feedback"]
    })
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Verification {
    correct: bool,
    feedback: String,
    answer: Option<String>,
}

impl Structured for Verification {
    fn structured_output() -> StructuredOutput {
        StructuredOutput {
            name: "Verification".into(),
            schema: schema(),
        }
    }
}

fn context() -> AgentContext {
    let mock = MockLlmClient::new("?").answer_when_contains(
        "Check carefully.\n\nAnswer:\n4",
        r#"{ "correct": false, "feedback": "2+3 is 5, not 4." }"#,
    );
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(mock))
        .build();
    AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("What is 2+3?").build(),
    )
}

#[test]
fn asks_the_llm_to_verify_the_answer_and_keeps_the_answer_in_the_verdict() {
    let graph = TypedGraph::<AgentContext, String, Verification>::build(|g| {
        let verify = g.node(
            "verify",
            nodes::llm_verify::<Verification>("Check carefully."),
        );
        g.edge(&g.start(), &verify);
        g.edge(&verify, &g.finish());
    })
    .unwrap();
    let interpreted = implementations::node(
        "llm_verify",
        &json!({ "type": "Verification", "prompt": "Check carefully." }),
        &BTreeMap::from([("Verification".to_string(), schema())]),
    )
    .unwrap();
    let mut ctx = context();

    let typed = block_on(graph.run(&mut ctx, "4".into())).unwrap();
    let untyped = block_on(interpreted(&mut context(), json!("4"))).unwrap();

    assert_eq!(
        typed,
        Verification {
            correct: false,
            feedback: "2+3 is 5, not 4.".into(),
            answer: Some("4".into())
        }
    );
    assert_eq!(untyped, serde_json::to_value(&typed).unwrap());
    assert_eq!(
        ctx.prompt().messages[1],
        Message::user("Check carefully.\n\nAnswer:\n4")
    );
}
