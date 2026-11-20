//! Agent storage (F04.10, F05.8, F07.18, ADR-0044).

use std::sync::Arc;

use grauwolf_agents::AgentContext;
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::{LlModel, Prompt};
use serde_json::json;

fn context() -> AgentContext {
    AgentContext::new(
        Arc::new(PromptExecutor::builder().build()),
        LlModel::new("mock", "m"),
        Prompt::builder("p").build(),
    )
}

#[test]
fn stores_values_by_key_and_keeps_them_in_checkpoints() {
    let mut ctx = context();

    ctx.store("task", &"What is 2+3?").unwrap();
    ctx.store("attempts", &2).unwrap();
    let restored =
        AgentContext::restore(Arc::new(PromptExecutor::builder().build()), ctx.snapshot());

    assert_eq!(
        ctx.load::<String>("task").unwrap(),
        Some("What is 2+3?".to_owned())
    );
    assert_eq!(restored.load::<u32>("attempts").unwrap(), Some(2));
    assert_eq!(restored.load::<u32>("missing").unwrap(), None);
    assert!(restored.load::<u32>("task").is_err());
    let stored: grauwolf_agents::checkpoints::ContextSnapshot =
        serde_json::from_value(json!({ "prompt": { "id": "p", "messages": [] }, "model": { "provider": "mock", "id": "m" } }))
            .unwrap();
    assert!(stored.storage.is_empty());
}

#[test]
fn store_and_load_nodes_pass_values_around_the_edges() {
    use futures_executor::block_on;
    use grauwolf_agents::{implementations, nodes};
    use grauwolf_graph::TypedGraph;

    let graph = TypedGraph::<AgentContext, String, String>::build(|g| {
        let keep = g.node("keep", nodes::store::<String>("task"));
        let other = g.node("other", |_, _: String| Box::pin(async { Ok(42_u32) }));
        let recall = g.node("recall", nodes::load::<u32, String>("task"));
        g.edge(&g.start(), &keep);
        g.edge(&keep, &other);
        g.edge(&other, &recall);
        g.edge(&recall, &g.finish());
    })
    .unwrap();
    let load = implementations::node(
        "load",
        &json!({ "key": "task", "value": "string", "type": "string" }),
        &Default::default(),
    )
    .unwrap();
    let missing = implementations::node(
        "load",
        &json!({ "key": "missing", "value": "string", "type": "string" }),
        &Default::default(),
    )
    .unwrap();
    let mut ctx = context();

    let output = block_on(graph.run(&mut ctx, "What is 2+3?".into())).unwrap();
    let untyped = block_on(load(&mut ctx, json!("any"))).unwrap();
    let error = block_on(missing(&mut ctx, json!("any"))).unwrap_err();

    assert_eq!(output, "What is 2+3?");
    assert_eq!(untyped, json!("What is 2+3?"));
    assert!(error.to_string().contains("'missing'"), "{error}");
}
