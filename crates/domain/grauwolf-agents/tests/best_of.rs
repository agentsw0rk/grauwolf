//! Node `llm_request_best_of` (F04.7, ADR-0042): several answers, the best one wins.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::{AgentContext, implementations, nodes};
use grauwolf_graph::TypedGraph;
use grauwolf_llm::{LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};
use serde_json::json;

/// Numbers answers; the evaluation (prompt `best_of`) picks number 2.
#[derive(Default)]
struct Candidates(AtomicUsize);

impl LlmClient for Candidates {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        let answer = if prompt.id == "best_of" {
            assert!(
                format!("{:?}", prompt.messages).contains("Answer 3"),
                "all candidates missing"
            );
            r#"{ "best": 2 }"#.to_owned()
        } else {
            format!("Answer {}", self.0.fetch_add(1, Ordering::Relaxed) + 1)
        };
        Box::pin(async move { Ok(Message::assistant(answer)) })
    }
}

fn context() -> AgentContext {
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(Candidates::default()))
        .build();
    AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("agent").build(),
    )
}

#[test]
fn keeps_only_the_answer_the_judge_picked() {
    let graph = TypedGraph::<AgentContext, String, Message>::build(|g| {
        let best = g.node("best", nodes::llm_request_best_of(3, "Shortest"));
        g.edge(&g.start(), &best);
        g.edge(&best, &g.finish());
    })
    .unwrap();
    let mut ctx = context();

    let answer = block_on(graph.run(&mut ctx, "Write a slogan".into())).unwrap();
    let interpreted = implementations::node(
        "llm_request_best_of",
        &json!({ "choices": 3 }),
        &Default::default(),
    )
    .unwrap();
    let mut other = context();
    let untyped = block_on(interpreted(&mut other, json!("Write a slogan"))).unwrap();

    assert_eq!(answer, Message::assistant("Answer 2"));
    assert_eq!(
        ctx.prompt().messages,
        [
            Message::user("Write a slogan"),
            Message::assistant("Answer 2")
        ]
    );
    assert_eq!(
        untyped,
        serde_json::to_value(Message::assistant("Answer 2")).unwrap()
    );
}
