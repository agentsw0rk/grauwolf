//! Node `llm_reason` (ReAct, ADR-0041): reasoning without tools.

use std::sync::Arc;

use futures_executor::block_on;
use grauwolf_agents::{AgentContext, nodes};
use grauwolf_graph::TypedGraph;
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::{LlModel, Message, Prompt, ToolResult};
use grauwolf_test::MockLlmClient;

fn context() -> AgentContext {
    let mock = MockLlmClient::new("?")
        .answer_when_contains("Think it through.", "Add first, then answer.");
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(mock))
        .build();
    AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("react").build(),
    )
}

#[test]
fn appends_the_input_and_the_prompt_and_returns_the_thought() {
    let graph = TypedGraph::<AgentContext, String, String>::build(|g| {
        let reason = g.node("reason", nodes::llm_reason::<String>("Think it through."));
        g.edge(&g.start(), &reason);
        g.edge(&reason, &g.finish());
    })
    .unwrap();
    let mut ctx = context();

    let thought = block_on(graph.run(&mut ctx, "What is 2+3?".to_string())).unwrap();

    assert_eq!(thought, "Add first, then answer.");
    assert_eq!(
        ctx.prompt().messages,
        [
            Message::user("What is 2+3?"),
            Message::user("Think it through."),
            Message::assistant("Add first, then answer.")
        ]
    );
}

#[test]
fn appends_tool_results_before_thinking_about_them() {
    let graph = TypedGraph::<AgentContext, ToolResult, String>::build(|g| {
        let reason = g.node(
            "reason",
            nodes::llm_reason::<ToolResult>("Think it through."),
        );
        g.edge(&g.start(), &reason);
        g.edge(&reason, &g.finish());
    })
    .unwrap();
    let mut ctx = context();
    let result = ToolResult {
        id: "add_1".into(),
        tool: "add".into(),
        content: "5".into(),
    };

    block_on(graph.run(&mut ctx, result.clone())).unwrap();

    assert_eq!(ctx.prompt().messages[0], Message::ToolResult(result));
}
