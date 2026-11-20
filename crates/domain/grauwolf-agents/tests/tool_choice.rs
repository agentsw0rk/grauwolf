//! Nodes with tool choice (F04.7, F05.9, ADR-0042).

use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::{AgentContext, implementations, nodes};
use grauwolf_graph::TypedGraph;
use grauwolf_llm::{LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolChoice, ToolDescriptor};
use serde_json::json;

/// Records the tool choice of every request.
#[derive(Default)]
struct Recorder(Mutex<Vec<ToolChoice>>);

impl LlmClient for Recorder {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        self.0
            .lock()
            .unwrap()
            .push(prompt.params.tool_choice.clone());
        Box::pin(async { Ok(Message::assistant("ok")) })
    }
}

fn context(recorder: Arc<Recorder>) -> AgentContext {
    let executor = PromptExecutor::builder().client("mock", recorder).build();
    AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").build(),
    )
}

#[test]
fn requests_with_a_tool_choice_only_for_that_request() {
    let recorder = Arc::new(Recorder::default());
    let graph = TypedGraph::<AgentContext, String, Message>::build(|g| {
        let only_tools = g.node("only_tools", nodes::llm_request_only_tools);
        let force = g.node("force", nodes::llm_request_force_tool("add"));
        let plain = g.node("plain", nodes::llm_request);
        let to_text = g.node("to_text", |_, _: Message| {
            Box::pin(async { Ok(String::from("continue")) })
        });
        let to_text_again = g.node("to_text_again", |_, _: Message| {
            Box::pin(async { Ok(String::from("continue")) })
        });
        g.edge(&g.start(), &only_tools);
        g.edge(&only_tools, &to_text);
        g.edge(&to_text, &force);
        g.edge(&force, &to_text_again);
        g.edge(&to_text_again, &plain);
        g.edge(&plain, &g.finish());
    })
    .unwrap();
    let mut ctx = context(recorder.clone());

    block_on(graph.run(&mut ctx, "Calculate".into())).unwrap();
    let interpreted = implementations::node(
        "llm_request_force_tool",
        &json!({ "tool": "multiply" }),
        &Default::default(),
    )
    .unwrap();
    block_on(interpreted(&mut ctx, json!("Calculate"))).unwrap();

    assert_eq!(
        *recorder.0.lock().unwrap(),
        [
            ToolChoice::Required,
            ToolChoice::Tool("add".into()),
            ToolChoice::Auto,
            ToolChoice::Tool("multiply".into())
        ]
    );
    assert_eq!(ctx.prompt().params.tool_choice, ToolChoice::Auto);
}

/// Cannot force tool calls (ADR-0063); records the tool choice and last message
/// of every request and calls `add`.
#[derive(Default)]
struct Unforcing(Mutex<Vec<(ToolChoice, Message)>>);

impl LlmClient for Unforcing {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        self.0.lock().unwrap().push((
            prompt.params.tool_choice.clone(),
            prompt.messages.last().unwrap().clone(),
        ));
        Box::pin(async { Ok(add_call()) })
    }

    fn forces_tool_choice(&self, _model: &LlModel) -> bool {
        false
    }
}

fn add_call() -> Message {
    Message::Assistant(grauwolf_prompt::AssistantMessage {
        parts: vec![grauwolf_prompt::AssistantPart::ToolCall(
            grauwolf_prompt::ToolCall {
                id: "c1".into(),
                tool: "add".into(),
                args: json!({ "a": 1, "b": 2 }),
            },
        )],
    })
}

#[test]
fn steers_with_an_instruction_kept_in_the_history_when_the_model_cannot_be_forced() {
    let client = Arc::new(Unforcing::default());
    let executor = PromptExecutor::builder()
        .client("mock", client.clone())
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("Calculate 1 + 2").build(),
    );

    let answer = block_on(ctx.request_llm_with(ToolChoice::Tool("add".into()))).unwrap();

    let requests = client.0.lock().unwrap().clone();
    let Message::System { content } = &requests[0].1 else {
        panic!("no instruction: {requests:?}");
    };
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, ToolChoice::Auto);
    assert!(content.contains("`add`"), "{content}");
    assert_eq!(answer, add_call());
    assert_eq!(
        ctx.prompt().messages,
        [
            Message::user("Calculate 1 + 2"),
            requests[0].1.clone(),
            add_call()
        ]
    );
}

/// Cannot force; answers the first time without a tool call (ADR-0063).
#[derive(Default)]
struct Hesitant(Mutex<usize>);

impl LlmClient for Hesitant {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        let mut calls = self.0.lock().unwrap();
        *calls += 1;
        let answer = if *calls == 1 {
            Message::assistant("I'll do it in a moment.")
        } else {
            add_call()
        };
        Box::pin(async move { Ok(answer) })
    }

    fn forces_tool_choice(&self, _model: &LlModel) -> bool {
        false
    }
}

#[test]
fn asks_once_more_without_keeping_an_answer_that_lacks_the_required_call() {
    let client = Arc::new(Hesitant::default());
    let executor = PromptExecutor::builder()
        .client("mock", client.clone())
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("Calculate 1 + 2").build(),
    );

    let answer = block_on(ctx.request_llm_with(ToolChoice::Required)).unwrap();

    assert_eq!(*client.0.lock().unwrap(), 2);
    assert_eq!(answer, add_call());
    assert_eq!(
        ctx.prompt().messages.len(),
        3,
        "{:?}",
        ctx.prompt().messages
    );
    assert_eq!(ctx.prompt().messages[2], add_call());
}
