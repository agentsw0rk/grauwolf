//! The handles a caller gives a run of generated code (ADR-0119): the same behaviour as the
//! interpreter's `RunExtras` – prices, caller budget, approvals, conversations, attachments,
//! checkpoints and resume.

use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::approval::ToolApprover;
use grauwolf_agents::checkpoints::{CheckpointStore, MemoryCheckpointStore};
use grauwolf_agents::events::{AgentEvent, Feature, ToolDecision, TraceParent};
use grauwolf_agents::memory::{ChatHistoryStore, MemoryChatHistoryStore};
use grauwolf_agents::run_handles::{RunHandles, StartError};
use grauwolf_agents::{AgentContext, Budget, conditions, nodes};
use grauwolf_graph::TypedGraph;
use grauwolf_llm::{Completion, LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{
    Attachment, AttachmentData, LlModel, Message, ModelPrice, Prices, Prompt, ToolCall,
    ToolDescriptor, Usage,
};
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

/// Calls `add` first (when `calls` is set), then answers; fails after `fail_after` requests.
/// Every request uses one million tokens in and out. Remembers the prompts it saw.
struct Scripted {
    calls: bool,
    fail_after: Option<usize>,
    seen: Mutex<Vec<Prompt>>,
}

impl Scripted {
    fn new(calls: bool, fail_after: Option<usize>) -> Arc<Self> {
        Arc::new(Self {
            calls,
            fail_after,
            seen: Mutex::new(Vec::new()),
        })
    }
}

impl LlmClient for Scripted {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async move { Ok(self.execute_with_usage(prompt, model, tools).await?.message) })
    }

    fn execute_with_usage<'a>(
        &'a self,
        prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        let mut seen = self.seen.lock().unwrap();
        seen.push(prompt.clone());
        let count = seen.len();
        let answer = if self.fail_after.is_some_and(|limit| count > limit) {
            Err(LlmError::Client("Connection lost".into()))
        } else if self.calls && count == 1 {
            Ok(Message::tool_call("add", "add", json!({ "a": 2, "b": 3 })))
        } else {
            Ok(Message::assistant("The result is 5."))
        };
        Box::pin(async move {
            answer.map(|message| Completion {
                message,
                usage: Some(Usage {
                    input_tokens: 1_000_000,
                    output_tokens: 1_000_000,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                }),
                model: None,
            })
        })
    }
}

/// Keeps the events of a run.
#[derive(Default)]
struct Events(Mutex<Vec<AgentEvent>>);

impl Feature for Events {
    fn on_event(&self, event: &AgentEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

impl Events {
    fn cost(&self) -> Option<f64> {
        self.0.lock().unwrap().iter().find_map(|event| match event {
            AgentEvent::AgentCompleted { cost, .. } | AgentEvent::AgentFailed { cost, .. } => *cost,
            _ => None,
        })
    }

    fn trace_parent(&self) -> Option<TraceParent> {
        self.0.lock().unwrap().iter().find_map(|event| match event {
            AgentEvent::AgentStarting { trace_parent, .. } => trace_parent.clone(),
            _ => None,
        })
    }

    fn tool_results(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolCallCompleted { result } => Some(result.content.clone()),
                _ => None,
            })
            .collect()
    }
}

/// `llm_request` → `add` → `llm_send_tool_result`, as the generated calculator.
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

fn prices() -> Arc<Prices> {
    Arc::new(Prices {
        currency: "USD".into(),
        models: [(
            "mock:m".to_owned(),
            ModelPrice {
                input: 1.0,
                output: 2.0,
                cache_read: None,
                cache_write: None,
            },
        )]
        .into(),
    })
}

/// What `run_observed` of a generated crate does with the handles.
async fn run(
    client: &Arc<Scripted>,
    handles: RunHandles,
    input: Option<&str>,
    limits: Budget,
    approval_tools: &[&str],
) -> (
    Result<String, Box<dyn std::error::Error + Send + Sync>>,
    Arc<Events>,
) {
    let events = Arc::new(Events::default());
    let result = async {
        let executor = Arc::new(
            PromptExecutor::builder()
                .client("mock", client.clone())
                .build(),
        );
        let state = handles.resume_state().await?;
        let mut ctx = handles
            .context(
                executor,
                LlModel::new("mock", "m"),
                Prompt::builder("calc").build(),
                state.as_ref(),
                limits,
                approval_tools,
            )?
            .with_tools(ToolRegistry::builder().tool(Add::default()).build())
            .install(events.clone());
        let input = input.map(str::to_owned);
        let run = handles.start(input.as_ref(), state.as_ref())?;
        Ok(RunHandles::run_graph(run, &mut ctx, graph(), input, state).await?)
    }
    .await;
    (result, events)
}

#[test]
fn prices_put_the_cost_on_the_completion_event() {
    let client = Scripted::new(false, None);
    let handles = RunHandles {
        prices: Some(prices()),
        ..RunHandles::default()
    };
    let (result, events) = block_on(run(&client, handles, Some("hi"), Budget::default(), &[]));
    assert_eq!(result.unwrap(), "The result is 5.");
    assert_eq!(events.cost(), Some(3.0));
}

/// ADR-0123: the trace context of the caller reaches `agent_starting`, as the interpreter's
/// `RunExtras.trace_parent` does.
#[test]
fn the_trace_context_of_the_caller_is_reported_in_agent_starting() {
    let client = Scripted::new(false, None);
    let parent = TraceParent {
        traceparent: "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".into(),
        tracestate: None,
    };
    let handles = RunHandles {
        trace_parent: Some(parent.clone()),
        ..RunHandles::default()
    };

    let (_, events) = block_on(run(&client, handles, Some("hi"), Budget::default(), &[]));

    assert_eq!(events.trace_parent(), Some(parent));
}

#[test]
fn without_prices_there_is_no_cost() {
    let client = Scripted::new(false, None);
    let (_, events) = block_on(run(
        &client,
        RunHandles::default(),
        Some("hi"),
        Budget::default(),
        &[],
    ));
    assert_eq!(events.cost(), None);
}

#[test]
fn the_caller_budget_ends_a_run_by_cost() {
    let client = Scripted::new(true, None);
    let handles = RunHandles {
        prices: Some(prices()),
        budget: Some(Budget {
            max_cost: Some(2.0),
            ..Budget::default()
        }),
        ..RunHandles::default()
    };
    let (result, events) = block_on(run(&client, handles, Some("hi"), Budget::default(), &[]));
    assert!(result.unwrap_err().to_string().contains("budget"));
    assert_eq!(events.cost(), Some(3.0));
    assert_eq!(client.seen.lock().unwrap().len(), 1);
}

#[test]
fn the_caller_budget_wins_over_the_limits_of_the_spec() {
    let client = Scripted::new(true, None);
    let handles = RunHandles {
        budget: Some(Budget {
            max_tokens: Some(1_000_000_000),
            ..Budget::default()
        }),
        ..RunHandles::default()
    };
    let limits = Budget {
        max_tokens: Some(1),
        ..Budget::default()
    };
    let (result, _) = block_on(run(&client, handles, Some("hi"), limits, &[]));
    assert_eq!(result.unwrap(), "The result is 5.");
}

#[test]
fn the_limits_of_the_spec_apply_without_a_caller_budget() {
    let client = Scripted::new(true, None);
    let limits = Budget {
        max_tokens: Some(1_000),
        ..Budget::default()
    };
    let (result, _) = block_on(run(&client, RunHandles::default(), Some("hi"), limits, &[]));
    assert!(result.unwrap_err().to_string().contains("budget"));
}

#[test]
fn an_approval_required_tool_is_refused_without_an_approver() {
    let client = Scripted::new(true, None);
    let (result, events) = block_on(run(
        &client,
        RunHandles::default(),
        Some("hi"),
        Budget::default(),
        &["add"],
    ));
    result.unwrap();
    assert_eq!(
        events.tool_results(),
        ["tool 'add' needs an approval that nobody can give here"]
    );
}

#[test]
fn an_approval_required_tool_asks_the_approver() {
    let client = Scripted::new(true, None);
    let asked = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&asked);
    let approver: ToolApprover = Arc::new(move |call: &ToolCall| {
        log.lock().unwrap().push(call.tool.clone());
        Box::pin(std::future::ready(ToolDecision::Proceed))
    });
    let handles = RunHandles {
        approver: Some(approver),
        ..RunHandles::default()
    };
    let (result, events) = block_on(run(
        &client,
        handles,
        Some("hi"),
        Budget::default(),
        &["add"],
    ));
    result.unwrap();
    assert_eq!(*asked.lock().unwrap(), ["add"]);
    assert_eq!(events.tool_results(), ["5.0"]);
}

#[test]
fn a_conversation_remembers_the_first_request() {
    let store = Arc::new(MemoryChatHistoryStore::default());
    let handles = || RunHandles {
        conversation: Some((store.clone() as Arc<dyn ChatHistoryStore>, "chat".into())),
        ..RunHandles::default()
    };
    let client = Scripted::new(false, None);
    block_on(run(
        &client,
        handles(),
        Some("my name is Ada"),
        Budget::default(),
        &[],
    ))
    .0
    .unwrap();
    block_on(run(
        &client,
        handles(),
        Some("who am I?"),
        Budget::default(),
        &[],
    ))
    .0
    .unwrap();
    let seen = client.seen.lock().unwrap();
    assert_eq!(
        seen[1].messages,
        [
            Message::user("my name is Ada"),
            Message::assistant("The result is 5."),
            Message::user("who am I?"),
        ]
    );
}

#[test]
fn attachments_go_with_the_input() {
    let client = Scripted::new(false, None);
    let attachment = Attachment {
        media_type: "image/png".into(),
        data: AttachmentData::Url("https://example.com/a.png".into()),
    };
    let handles = RunHandles {
        attachments: vec![attachment.clone()],
        ..RunHandles::default()
    };
    block_on(run(
        &client,
        handles,
        Some("what is this?"),
        Budget::default(),
        &[],
    ))
    .0
    .unwrap();
    assert_eq!(
        client.seen.lock().unwrap()[0].messages,
        [Message::user_with("what is this?", vec![attachment])]
    );
}

#[test]
fn a_crashed_run_resumes_from_its_checkpoint_without_repeating_work() {
    let store = Arc::new(MemoryCheckpointStore::default());
    let checkpoints = || {
        Some((
            store.clone() as Arc<dyn CheckpointStore>,
            "run-1".to_owned(),
        ))
    };
    let crashing = Scripted::new(true, Some(1));
    let first = RunHandles {
        checkpoints: checkpoints(),
        ..RunHandles::default()
    };
    let (result, _) = block_on(run(&crashing, first, Some("2+3?"), Budget::default(), &[]));
    assert!(result.is_err());

    let working = Scripted::new(false, None);
    let second = RunHandles {
        checkpoints: checkpoints(),
        resume: true,
        ..RunHandles::default()
    };
    let (result, events) = block_on(run(&working, second, None, Budget::default(), &[]));
    assert_eq!(result.unwrap(), "The result is 5.");
    assert!(
        events.tool_results().is_empty(),
        "the tool must not run again"
    );
}

#[test]
fn resuming_without_a_checkpoint_is_an_error_with_the_interpreters_text() {
    let handles = RunHandles {
        checkpoints: Some((Arc::new(MemoryCheckpointStore::default()), "nope".into())),
        resume: true,
        ..RunHandles::default()
    };
    let error = block_on(handles.resume_state()).unwrap_err();
    assert_eq!(error.to_string(), "no checkpoint for run 'nope'");
}

#[test]
fn checkpoints_and_a_conversation_do_not_go_together() {
    let handles = RunHandles {
        checkpoints: Some((Arc::new(MemoryCheckpointStore::default()), "r".into())),
        conversation: Some((Arc::new(MemoryChatHistoryStore::default()), "c".into())),
        ..RunHandles::default()
    };
    assert!(matches!(
        block_on(handles.resume_state()),
        Err(StartError::CheckpointConversation)
    ));
}

#[test]
fn a_cost_limit_without_prices_is_refused_like_the_interpreter_does() {
    let handles = RunHandles {
        budget: Some(Budget {
            max_cost: Some(1.0),
            ..Budget::default()
        }),
        ..RunHandles::default()
    };
    let executor = Arc::new(PromptExecutor::builder().build());
    let error = handles
        .context(
            executor,
            LlModel::new("mock", "m"),
            Prompt::builder("calc").build(),
            None,
            Budget::default(),
            &[],
        )
        .err()
        .unwrap();
    assert!(matches!(error, StartError::CostLimitWithoutPrices));
}
