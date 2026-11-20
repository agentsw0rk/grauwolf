//! Node `llm_request` (F05.9).

use std::sync::Arc;

use futures_executor::block_on;
use grauwolf_agents::{AgentContext, conditions, nodes};
use grauwolf_graph::TypedGraph;
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::{LlModel, Message, Prompt};
use grauwolf_test::MockLlmClient;

fn context() -> AgentContext {
    let executor = PromptExecutor::builder()
        .client(
            "mock",
            Arc::new(MockLlmClient::new("?").answer_when_contains("Hello", "Hi")),
        )
        .build();
    AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("echo").system("Be brief.").build(),
    )
}

#[test]
fn llm_request_sends_the_input_and_records_the_exchange() {
    let graph = TypedGraph::<AgentContext, String, Message>::build(|g| {
        let call_llm = g.node("call_llm", nodes::llm_request);
        g.edge(&g.start(), &call_llm);
        g.edge(&call_llm, &g.finish());
    })
    .unwrap();
    let mut ctx = context();

    let answer = block_on(graph.run(&mut ctx, "Hello".to_string())).unwrap();

    assert_eq!(answer, Message::assistant("Hi"));
    assert_eq!(
        ctx.prompt().messages,
        vec![
            Message::system("Be brief."),
            Message::user("Hello"),
            Message::assistant("Hi"),
        ]
    );
}

#[test]
fn on_assistant_message_passes_only_assistant_text() {
    let graph = TypedGraph::<AgentContext, String, String>::build(|g| {
        let call_llm = g.node("call_llm", nodes::llm_request);
        g.edge(&g.start(), &call_llm);
        g.edge_when(&call_llm, &g.finish(), conditions::on_assistant_message);
    })
    .unwrap();
    let mut ctx = context();

    assert_eq!(
        block_on(graph.run(&mut ctx, "Hello".to_string())).unwrap(),
        "Hi"
    );
    assert_eq!(
        conditions::on_assistant_message(&ctx, Message::user("x")),
        None
    );
}

#[test]
fn llm_continue_requests_the_llm_without_appending_its_input() {
    let graph = TypedGraph::<AgentContext, String, Message>::build(|g| {
        let call_llm = g.node("call_llm", nodes::llm_continue);
        g.edge(&g.start(), &call_llm);
        g.edge(&call_llm, &g.finish());
    })
    .unwrap();
    let mut ctx = context();
    ctx.append(Message::user("Hello"));

    let answer =
        block_on(graph.run(&mut ctx, "Thought is already in the history".to_string())).unwrap();

    assert_eq!(answer, Message::assistant("Hi"));
    assert_eq!(
        ctx.prompt().messages,
        [
            Message::system("Be brief."),
            Message::user("Hello"),
            Message::assistant("Hi")
        ]
    );
}

#[test]
fn llm_continue_asks_to_act_when_the_history_ends_with_an_answer() {
    let graph = TypedGraph::<AgentContext, String, Message>::build(|g| {
        let call_llm = g.node("call_llm", nodes::llm_continue);
        g.edge(&g.start(), &call_llm);
        g.edge(&call_llm, &g.finish());
    })
    .unwrap();
    let mut ctx = context();
    ctx.append(Message::user("Hello"));
    ctx.append(Message::assistant("I should greet."));

    block_on(graph.run(&mut ctx, String::new())).unwrap();

    // Providers would continue a trailing answer as a prefill instead of answering anew.
    assert_eq!(
        ctx.prompt().messages[3],
        Message::user(nodes::CONTINUE_PROMPT)
    );
}
