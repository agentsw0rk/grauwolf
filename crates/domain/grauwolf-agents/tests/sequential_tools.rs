//! `execute_tools_in_order`: several tool calls of one answer run one after another.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::{AgentContext, implementations, nodes};
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::{LlModel, Prompt, ToolCall};
use grauwolf_test::MockLlmClient;
use grauwolf_tools::{Tool, ToolError, ToolRegistry};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

/// Returns `Pending` once, so that concurrently polled futures interleave.
struct YieldOnce(bool);

impl Future for YieldOnce {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            return Poll::Ready(());
        }
        self.0 = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

struct Logged(Arc<Mutex<Vec<String>>>);

#[derive(Deserialize, JsonSchema)]
struct Args {
    n: u32,
}

impl Tool for Logged {
    type Args = Args;
    type Output = String;

    fn name(&self) -> &str {
        "logged"
    }

    fn description(&self) -> &str {
        "Logs its start and end"
    }

    fn call(&self, args: Args) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(format!("start {}", args.n));
            YieldOnce(false).await;
            self.0.lock().unwrap().push(format!("end {}", args.n));
            Ok(format!("done {}", args.n))
        })
    }
}

fn context(log: &Arc<Mutex<Vec<String>>>) -> AgentContext {
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(MockLlmClient::new("?")))
        .build();
    AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("t").build(),
    )
    .with_tools(
        ToolRegistry::builder()
            .tool(Logged(Arc::clone(log)))
            .build(),
    )
}

fn call(id: &str, n: u32) -> ToolCall {
    ToolCall {
        id: id.into(),
        tool: "logged".into(),
        args: json!({ "n": n }),
    }
}

#[test]
fn runs_the_calls_one_after_another_and_returns_the_results_in_call_order() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut ctx = context(&log);

    let results = block_on(nodes::execute_tools_in_order(
        &mut ctx,
        vec![call("a", 1), call("b", 2)],
    ))
    .unwrap();

    assert_eq!(
        *log.lock().unwrap(),
        ["start 1", "end 1", "start 2", "end 2"]
    );
    assert_eq!(
        results
            .iter()
            .map(|r| r.content.as_str())
            .collect::<Vec<_>>(),
        ["\"done 1\"", "\"done 2\""]
    );
}

#[test]
fn the_interpreter_runs_the_calls_in_order_only_with_the_sequential_flag() {
    let run = |params: serde_json::Value| {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut ctx = context(&log);
        let node =
            implementations::node("execute_multiple_tools", &params, &Default::default()).unwrap();
        let calls = serde_json::to_value(vec![call("a", 1), call("b", 2)]).unwrap();
        block_on(node(&mut ctx, calls)).unwrap();
        #[allow(clippy::let_and_return)]
        let events = log.lock().unwrap().clone();
        events
    };

    assert_eq!(
        run(json!({ "sequential": true })),
        ["start 1", "end 1", "start 2", "end 2"]
    );
    assert_eq!(run(json!(null)), ["start 1", "start 2", "end 1", "end 2"]);
}
