//! Branching on fields of structured values (F05.4, ADR-0041).

use std::sync::Arc;

use futures_executor::block_on;
use grauwolf_agents::{AgentContext, conditions, implementations};
use grauwolf_graph::{Catalogs, DataType, TypedGraph};
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::{LlModel, Prompt};
use grauwolf_test::MockLlmClient;
use serde::{Deserialize, Serialize};
use serde_json::json;

fn context() -> AgentContext {
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(MockLlmClient::new("?")))
        .build();
    AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").build(),
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Verdict {
    correct: bool,
    feedback: String,
}

#[test]
fn on_field_passes_values_whose_field_equals_the_expected_value() {
    let condition =
        implementations::condition("on_field", &json!({ "field": "correct", "equals": true }))
            .unwrap();
    let ctx = context();

    let accepted = block_on(condition(&ctx, json!({ "correct": true, "feedback": "" }))).unwrap();
    let rejected = block_on(condition(
        &ctx,
        json!({ "correct": false, "feedback": "Too short" }),
    ))
    .unwrap();

    assert_eq!(accepted, Some(json!({ "correct": true, "feedback": "" })));
    assert_eq!(rejected, None);
}

#[test]
fn on_field_works_on_typed_values_and_only_on_structured_types() {
    let graph = TypedGraph::<AgentContext, Verdict, Verdict>::build(|g| {
        let accepted = g.node("accepted", |_, verdict: Verdict| {
            Box::pin(async move { Ok(verdict) })
        });
        let rejected = g.node("rejected", |_, verdict: Verdict| {
            Box::pin(async move {
                Ok(Verdict {
                    feedback: format!("rejected: {}", verdict.feedback),
                    ..verdict
                })
            })
        });
        g.edge_when(&g.start(), &accepted, |_, verdict: Verdict| {
            conditions::field_equals(&verdict, "correct", &json!(true)).then_some(verdict)
        });
        g.edge(&g.start(), &rejected);
        g.edge(&accepted, &g.finish());
        g.edge(&rejected, &g.finish());
    })
    .unwrap();
    let descriptor = Catalogs::builtin()
        .conditions
        .get("on_field")
        .cloned()
        .unwrap();
    let params = json!({ "field": "correct", "equals": true });

    let output = block_on(graph.run(
        &mut context(),
        Verdict {
            correct: false,
            feedback: "Too short".into(),
        },
    ))
    .unwrap();

    assert_eq!(output.feedback, "rejected: Too short");
    assert_eq!(
        descriptor
            .signature(&params, &DataType::Named("Verdict".into()))
            .unwrap(),
        DataType::Named("Verdict".into())
    );
    assert!(descriptor.signature(&params, &DataType::String).is_err());
}

#[test]
fn get_field_returns_a_field_as_text() {
    let interpreted = implementations::node(
        "get_field",
        &json!({ "type": "Verdict", "field": "feedback" }),
        &Default::default(),
    )
    .unwrap();
    let graph = TypedGraph::<AgentContext, Verdict, String>::build(|g| {
        let feedback = g.node(
            "feedback",
            grauwolf_agents::nodes::get_field::<Verdict>("feedback"),
        );
        g.edge(&g.start(), &feedback);
        g.edge(&feedback, &g.finish());
    })
    .unwrap();
    let mut ctx = context();
    let verdict = Verdict {
        correct: false,
        feedback: "Too short".into(),
    };

    let typed = block_on(graph.run(&mut ctx, verdict.clone())).unwrap();
    let untyped = block_on(interpreted(
        &mut ctx,
        json!({ "correct": false, "feedback": "Too short" }),
    ))
    .unwrap();
    let not_text = block_on(interpreted(&mut ctx, json!({ "feedback": { "lines": 3 } }))).unwrap();

    assert_eq!(typed, "Too short");
    assert_eq!(untyped, json!("Too short"));
    assert_eq!(not_text, json!(r#"{"lines":3}"#));
    let descriptor = Catalogs::builtin().nodes.get("get_field").cloned().unwrap();
    assert_eq!(
        descriptor
            .signature(&json!({ "type": "Verdict", "field": "feedback" }))
            .unwrap(),
        (DataType::Named("Verdict".into()), DataType::String)
    );
}
