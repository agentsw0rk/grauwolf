//! Checkpoints and resuming an agent (ADR-0020, ADR-0034).

use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::checkpoints::{CheckpointStore, Checkpointing, MemoryCheckpointStore};
use grauwolf_agents::{AgentContext, conditions, nodes};
use grauwolf_graph::TypedGraph;
use grauwolf_llm::{LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};
use grauwolf_test::MockLlmClient;
use grauwolf_tools::{Tool, ToolError, ToolRegistry};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize, JsonSchema)]
struct AddArgs {
    a: f64,
    b: f64,
}

/// Adds and counts its calls.
#[derive(Clone, Default)]
struct Add(Arc<Mutex<usize>>);

impl Tool for Add {
    type Args = AddArgs;
    type Output = f64;

    fn name(&self) -> &str {
        "add"
    }

    fn description(&self) -> &str {
        "Adds two numbers"
    }

    fn call(&self, args: AddArgs) -> BoxFuture<'_, Result<f64, ToolError>> {
        *self.0.lock().unwrap() += 1;
        Box::pin(async move { Ok(args.a + args.b) })
    }
}

/// LLM that first calls the tool and fails on the second call.
#[derive(Default)]
struct BreaksAfterToolCall(Mutex<usize>);

impl LlmClient for BreaksAfterToolCall {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        let mut calls = self.0.lock().unwrap();
        *calls += 1;
        let answer = match *calls {
            1 => Ok(Message::tool_call("add", "add", json!({ "a": 2, "b": 3 }))),
            _ => Err(LlmError::Client("Connection lost".into())),
        };
        Box::pin(async move { answer })
    }
}

fn graph() -> TypedGraph<AgentContext, String, String> {
    TypedGraph::build(|g| {
        let call_llm = g.node("call_llm", nodes::llm_request);
        let execute_tool = g.node("execute_tool", nodes::execute_tool);
        let send_result = g.node("send_result", nodes::llm_send_tool_result);
        g.edge(&g.start(), &call_llm);
        g.edge_when(&call_llm, &execute_tool, conditions::on_tool_call);
        g.edge_when(&call_llm, &g.finish(), conditions::on_assistant_message);
        g.edge(&execute_tool, &send_result);
        g.edge_when(&send_result, &execute_tool, conditions::on_tool_call);
        g.edge_when(&send_result, &g.finish(), conditions::on_assistant_message);
    })
    .unwrap()
}

fn executor(client: Arc<dyn LlmClient>) -> Arc<PromptExecutor> {
    Arc::new(PromptExecutor::builder().client("mock", client).build())
}

#[test]
fn resumes_after_a_crash_without_repeating_finished_work() {
    let store = Arc::new(MemoryCheckpointStore::default());
    let checkpointing = Checkpointing::new(store.clone(), "run-1");
    let add = Add::default();
    let tools = || ToolRegistry::builder().tool(add.clone()).build();

    let mut ctx = AgentContext::new(
        executor(Arc::new(BreaksAfterToolCall::default())),
        LlModel::new("mock", "m"),
        Prompt::builder("calc").build(),
    )
    .with_tools(tools());
    let crashed = block_on(graph().run_observed(&mut ctx, "What is 2+3?".into(), &checkpointing));
    assert!(crashed.is_err());

    let state = block_on(store.load("run-1")).unwrap().unwrap();
    assert_eq!(state.checkpoint.path, ["send_result"]);
    let working = Arc::new(MockLlmClient::new("?").answer_when_contains("5.0", "The result is 5."));
    let mut ctx = AgentContext::restore(executor(working), state.context).with_tools(tools());
    let answer = block_on(graph().resume(&mut ctx, state.checkpoint, &checkpointing)).unwrap();

    assert_eq!(answer, "The result is 5.");
    assert_eq!(*add.0.lock().unwrap(), 1, "the tool must not run again");
    assert_eq!(
        ctx.prompt().messages,
        [
            Message::user("What is 2+3?"),
            Message::tool_call("add", "add", json!({ "a": 2, "b": 3 })),
            Message::tool_result("add", "add", "5.0"),
            Message::assistant("The result is 5."),
        ]
    );
}
