//! Model switches during a run and models per subgraph (F04.9, F05.6, ADR-0043).

use std::sync::Arc;

use grauwolf_agents::AgentContext;
use grauwolf_agents::scopes::SubgraphScope;
use grauwolf_graph::Scope;
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::{LlModel, Message, Prompt};

fn context() -> AgentContext {
    AgentContext::new(
        Arc::new(PromptExecutor::builder().build()),
        LlModel::new("ollama", "small"),
        Prompt::builder("p").user("Hello").build(),
    )
}

#[test]
fn switches_the_model_and_keeps_the_history() {
    let mut ctx = context();

    let previous = ctx.set_model(LlModel::new("anthropic", "large"));

    assert_eq!(previous, LlModel::new("ollama", "small"));
    assert_eq!(ctx.model(), &LlModel::new("anthropic", "large"));
    assert_eq!(ctx.prompt().messages, [Message::user("Hello")]);
}

#[test]
fn subgraph_scopes_set_their_model_and_restore_the_outer_one() {
    let outer = SubgraphScope::new().with_model(LlModel::new("openai", "medium"));
    let inner = SubgraphScope::new()
        .with_model(LlModel::new("anthropic", "large"))
        .with_tools(["add"]);
    let mut ctx = context();

    outer.enter(&mut ctx);
    inner.enter(&mut ctx);
    let innermost = ctx.model().clone();
    // A checkpoint here records the model outside all subgraphs; the scopes set
    // their models again when resuming.
    let snapshot = ctx.snapshot();
    inner.exit(&mut ctx);
    let middle = ctx.model().clone();
    outer.exit(&mut ctx);

    assert_eq!(innermost, LlModel::new("anthropic", "large"));
    assert_eq!(middle, LlModel::new("openai", "medium"));
    assert_eq!(ctx.model(), &LlModel::new("ollama", "small"));
    assert_eq!(snapshot.model, LlModel::new("ollama", "small"));
}

#[test]
fn set_model_node_switches_for_the_rest_of_the_run_and_passes_the_value_through() {
    use futures_executor::block_on;
    use grauwolf_agents::{implementations, nodes};
    use grauwolf_graph::TypedGraph;
    use serde_json::json;

    let graph = TypedGraph::<AgentContext, String, String>::build(|g| {
        let switch = g.node("switch", nodes::set_model::<String>("anthropic", "large"));
        g.edge(&g.start(), &switch);
        g.edge(&switch, &g.finish());
    })
    .unwrap();
    let interpreted = implementations::node(
        "set_model",
        &json!({ "provider": "openai", "id": "medium", "value": "string" }),
        &Default::default(),
    )
    .unwrap();
    let mut ctx = context();

    let output = block_on(graph.run(&mut ctx, "continue".into())).unwrap();
    let typed_model = ctx.model().clone();
    let untyped = block_on(interpreted(&mut ctx, json!("continue"))).unwrap();

    assert_eq!(output, "continue");
    assert_eq!(typed_model, LlModel::new("anthropic", "large"));
    assert_eq!(untyped, json!("continue"));
    assert_eq!(ctx.model(), &LlModel::new("openai", "medium"));
}
