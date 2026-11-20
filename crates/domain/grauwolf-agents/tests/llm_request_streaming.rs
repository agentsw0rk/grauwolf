//! Node `llm_request_streaming` (F05.9, ADR-0023, ADR-0033).

use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use grauwolf_agents::{AgentContext, nodes};
use grauwolf_graph::TypedGraph;
use grauwolf_llm::{PromptExecutor, StreamFrame};
use grauwolf_prompt::{LlModel, Message, Prompt};
use grauwolf_test::MockLlmClient;

#[test]
fn passes_frames_to_the_sink_and_the_aggregated_answer_to_the_edge() {
    let executor = PromptExecutor::builder()
        .client(
            "mock",
            Arc::new(MockLlmClient::new("?").answer_when_contains("Hello", "Hi")),
        )
        .build();
    let seen: Arc<Mutex<Vec<StreamFrame>>> = Arc::default();
    let sink = seen.clone();
    let mut ctx = AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").build(),
    )
    .with_frame_sink(Arc::new(move |frame: &StreamFrame| {
        sink.lock().unwrap().push(frame.clone());
    }));
    let graph = TypedGraph::<AgentContext, String, Message>::build(|g| {
        let call_llm = g.node("call_llm", nodes::llm_request_streaming);
        g.edge(&g.start(), &call_llm);
        g.edge(&call_llm, &g.finish());
    })
    .unwrap();

    let answer = block_on(graph.run(&mut ctx, "Hello".to_string())).unwrap();

    assert_eq!(answer, Message::assistant("Hi"));
    assert_eq!(
        *seen.lock().unwrap(),
        [
            StreamFrame::TextDelta { text: "Hi".into() },
            StreamFrame::End {
                finish_reason: None
            }
        ]
    );
    assert_eq!(
        ctx.prompt().messages,
        [Message::user("Hello"), Message::assistant("Hi")]
    );
}
