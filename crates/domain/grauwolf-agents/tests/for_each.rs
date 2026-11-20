//! The `for_each` node: a body per element of a list (ADR-0101).

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use futures_executor::block_on;
use grauwolf_agents::AgentContext;
use grauwolf_agents::events::{AgentEvent, Feature};
use grauwolf_agents::for_each::{Body, ForEach, OnError, run_for_each};
use grauwolf_graph::TypedGraph;
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::{LlModel, Message, Prompt};
use serde_json::{Value, json};

#[derive(Default)]
struct Recorder(Mutex<Vec<AgentEvent>>);

impl Feature for Recorder {
    fn on_event(&self, event: &AgentEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

fn context() -> AgentContext {
    AgentContext::new(
        Arc::new(PromptExecutor::builder().build()),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("Parent history").build(),
    )
}

/// Pending for `turns` polls, so that elements finish in a different order than they start.
struct Turns(usize);

impl Future for Turns {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 == 0 {
            return Poll::Ready(());
        }
        self.0 -= 1;
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

type Log = Arc<Mutex<Vec<i64>>>;

/// Body `i64 → i64`: waits longer for smaller numbers, notes the number in `finished` when it
/// is done, writes into its own history and fails for 13; returns ten times the number.
fn body(finished: &Log) -> Body {
    let finished = Arc::clone(finished);
    let graph = TypedGraph::<AgentContext, i64, i64>::build(|g| {
        let work = g.node("work", move |ctx: &mut AgentContext, number: i64| {
            let finished = Arc::clone(&finished);
            ctx.append(Message::user(format!("element {number}")));
            Box::pin(async move {
                Turns(usize::try_from(5 - number).unwrap_or(0)).await;
                if number == 13 {
                    return Err("unlucky".into());
                }
                finished.lock().unwrap().push(number);
                Ok(number * 10)
            })
        });
        g.edge(&g.start(), &work);
        g.edge(&work, &g.finish());
    })
    .unwrap();
    Body::Graph {
        name: "times_ten".into(),
        graph: graph.into_graph(),
        scope: None,
    }
}

fn run(
    ctx: &mut AgentContext,
    config: &ForEach,
    input: Value,
) -> Result<Value, grauwolf_graph::NodeError> {
    block_on(run_for_each(ctx, config, "each", input))
}

#[test]
fn results_come_in_input_order_although_elements_finish_in_another_order() {
    let finished = Log::default();
    let config = ForEach::new(body(&finished)).with_max_parallel(3);

    let output = run(&mut context(), &config, json!([1, 2, 3])).unwrap();

    assert_eq!(output, json!([10, 20, 30]));
    assert_eq!(*finished.lock().unwrap(), [3, 2, 1]);
}

#[test]
fn one_element_at_a_time_by_default() {
    let finished = Log::default();
    let config = ForEach::new(body(&finished));

    let output = run(&mut context(), &config, json!([1, 2, 3])).unwrap();

    assert_eq!(output, json!([10, 20, 30]));
    assert_eq!(*finished.lock().unwrap(), [1, 2, 3]);
}

#[test]
fn every_element_has_its_own_history_and_the_callers_history_stays() {
    let mut ctx = context();

    run(
        &mut ctx,
        &ForEach::new(body(&Log::default())),
        json!([1, 2]),
    )
    .unwrap();

    assert_eq!(ctx.prompt().messages, [Message::user("Parent history")]);
}

#[test]
fn text_with_a_json_array_is_parsed_also_inside_a_code_fence() {
    let config = ForEach::new(body(&Log::default()));

    for text in ["[1, 2]", "```json\n[1, 2]\n```"] {
        let output = run(&mut context(), &config, json!(text)).unwrap();
        assert_eq!(output, json!([10, 20]), "{text}");
    }
}

#[test]
fn input_that_is_no_array_fails_with_a_clear_error() {
    let config = ForEach::new(body(&Log::default()));

    let object = run(&mut context(), &config, json!({ "a": 1 })).unwrap_err();
    let prose = run(&mut context(), &config, json!("three topics")).unwrap_err();

    assert!(
        object
            .to_string()
            .contains("needs a JSON array as input, got an object"),
        "{object}"
    );
    assert!(
        prose
            .to_string()
            .contains("needs a JSON array as input, the text is not JSON"),
        "{prose}"
    );
}

#[test]
fn a_failing_element_fails_the_node_and_stops_the_elements_that_have_not_started() {
    let finished = Log::default();
    let config = ForEach::new(body(&finished));

    let error = run(&mut context(), &config, json!([1, 13, 2])).unwrap_err();

    assert_eq!(
        error.to_string(),
        "for_each 'each', element 1: subgraph 'times_ten': node 'work' failed: unlucky"
    );
    assert_eq!(*finished.lock().unwrap(), [1]);
}

#[test]
fn each_element_reports_its_start_and_its_result() {
    let recorder = Arc::new(Recorder::default());
    let mut ctx = context().install(recorder.clone());
    let config = ForEach::new(body(&Log::default()));

    let _ = run(&mut ctx, &config, json!([1, 13]));

    let events = recorder.0.lock().unwrap();
    let item = |event: &AgentEvent| serde_json::to_value(event).unwrap();
    assert_eq!(
        events.iter().map(item).collect::<Vec<_>>(),
        [
            json!({ "event": "for_each_item_starting", "node": "each", "index": 0 }),
            json!({ "event": "for_each_item_completed", "node": "each", "index": 0 }),
            json!({ "event": "for_each_item_starting", "node": "each", "index": 1 }),
            json!({ "event": "for_each_item_failed", "node": "each", "index": 1,
                    "error": "subgraph 'times_ten': node 'work' failed: unlucky" }),
        ]
    );
}

#[test]
fn skipping_leaves_failed_elements_out_of_the_output_and_reports_them() {
    let recorder = Arc::new(Recorder::default());
    let mut ctx = context().install(recorder.clone());
    let config = ForEach::new(body(&Log::default()))
        .with_max_parallel(2)
        .with_on_error(OnError::Skip);

    let output = run(&mut ctx, &config, json!([1, 13, 2])).unwrap();

    assert_eq!(output, json!([10, 20]));
    let failed: Vec<usize> = recorder
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ForEachItemFailed { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(failed, [1]);
}

const KEY: &str = "grauwolf.for_each.each";

#[test]
fn a_failed_run_keeps_the_finished_elements_and_a_resumed_run_does_the_rest() {
    let finished = Log::default();
    let config = ForEach::new(body(&finished));
    let mut ctx = context();

    run(&mut ctx, &config, json!([1, 2, 13, 3])).unwrap_err();
    let progress: Value = ctx.load(KEY).unwrap().unwrap();
    assert_eq!(
        progress,
        json!({ "0": { "status": "done", "output": 10 }, "1": { "status": "done", "output": 20 } })
    );

    // The same run with a good third element: only the elements 2 and 3 run now.
    finished.lock().unwrap().clear();
    let output = run(&mut ctx, &config, json!([1, 2, 4, 3])).unwrap();

    assert_eq!(output, json!([10, 20, 40, 30]));
    assert_eq!(*finished.lock().unwrap(), [4, 3]);
    assert_eq!(ctx.load::<Value>(KEY).unwrap(), None);
}

#[test]
fn skipped_elements_are_not_run_again_when_resuming() {
    let finished = Log::default();
    let config = ForEach::new(body(&finished)).with_on_error(OnError::Skip);
    let mut ctx = context();
    ctx.store(KEY, &json!({ "0": { "status": "skipped" } }))
        .unwrap();

    let output = run(&mut ctx, &config, json!([13, 1])).unwrap();

    assert_eq!(output, json!([10]));
}

/// Answers and reports 15 tokens per request.
struct Counting;

impl grauwolf_llm::LlmClient for Counting {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [grauwolf_prompt::ToolDescriptor],
    ) -> futures_util::future::BoxFuture<'a, Result<Message, grauwolf_llm::LlmError>> {
        Box::pin(async { Ok(Message::assistant("ok")) })
    }

    fn execute_with_usage<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [grauwolf_prompt::ToolDescriptor],
    ) -> futures_util::future::BoxFuture<'a, Result<grauwolf_llm::Completion, grauwolf_llm::LlmError>>
    {
        Box::pin(async move {
            Ok(grauwolf_llm::Completion {
                model: None,
                message: self.execute(prompt, model, tools).await?,
                usage: Some(grauwolf_prompt::Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                }),
            })
        })
    }
}

#[test]
fn the_elements_share_the_budget_of_the_run() {
    let graph = TypedGraph::<AgentContext, String, String>::build(|g| {
        let ask = g.node("ask", |ctx: &mut AgentContext, _item: String| {
            Box::pin(async move {
                ctx.request_llm().await?;
                Ok("ok".to_owned())
            })
        });
        g.edge(&g.start(), &ask);
        g.edge(&ask, &g.finish());
    })
    .unwrap();
    let config = ForEach::new(Body::Graph {
        name: "ask".into(),
        graph: graph.into_graph(),
        scope: None,
    });
    let mut ctx = AgentContext::new(
        Arc::new(
            PromptExecutor::builder()
                .client("mock", Arc::new(Counting))
                .build(),
        ),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("Question").build(),
    )
    .with_budget(grauwolf_agents::Budget {
        max_tokens: Some(30),
        ..Default::default()
    });

    let error = run(&mut ctx, &config, json!(["a", "b", "c"])).unwrap_err();

    assert!(error.to_string().contains("element 2"), "{error}");
    assert_eq!(
        ctx.usage()
            .map(|usage| usage.input_tokens + usage.output_tokens),
        Some(30)
    );
}

#[test]
fn text_output_is_the_list_as_json_text() {
    let config = ForEach::new(body(&Log::default())).with_text_output();

    let output = run(&mut context(), &config, json!([1, 2])).unwrap();

    assert_eq!(output, json!("[10,20]"));
}
