//! Sub-agents as steps of a flow (ADR-0101).

use std::sync::Arc;

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::AgentContext;
use grauwolf_agents::flows::ParentRun;
use grauwolf_llm::{Completion, LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor, Usage};
use grauwolf_tools::{DynTool, StepHandles, ToolError, ToolRegistry};
use serde_json::{Value, json};

const USAGE: Usage = Usage {
    input_tokens: 10,
    output_tokens: 5,
    cache_read_tokens: None,
    cache_write_tokens: None,
};

/// Answers "done" and reports a fixed usage.
struct Counting;

impl LlmClient for Counting {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async { Ok(Message::assistant("done")) })
    }

    fn execute_with_usage<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async move {
            Ok(Completion {
                model: None,
                message: self.execute(prompt, model, tools).await?,
                usage: Some(USAGE),
            })
        })
    }
}

fn executor() -> Arc<PromptExecutor> {
    Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(Counting))
            .build(),
    )
}

/// A sub-agent: runs one LLM request on its own history and adopts the calling run.
struct Worker;

impl DynTool for Worker {
    fn name(&self) -> &str {
        "worker"
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "worker".into(),
            description: "Works".into(),
            parameters: json!({}),
        }
    }

    fn call_json(&self, _args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async { Err(ToolError::Execution("not as a tool".into())) })
    }

    fn call_step(
        &self,
        input: Value,
        parent: Option<StepHandles>,
    ) -> BoxFuture<'_, Result<Value, ToolError>> {
        Box::pin(async move {
            let parent = parent
                .and_then(|handles| handles.downcast::<ParentRun>().ok())
                .expect("the run is handed to the step");
            let mut child = AgentContext::new(
                executor(),
                LlModel::new("mock", "m"),
                Prompt::builder("worker").build(),
            )
            .adopt_run(&parent);
            child.append(Message::user(input.as_str().unwrap_or_default()));
            child
                .request_llm()
                .await
                .map_err(|error| ToolError::Execution(error.to_string()))?;
            Ok(
                json!({ "history": child.prompt().messages.len(), "handle": parent.handle::<u32>() }),
            )
        })
    }
}

fn parent() -> AgentContext {
    AgentContext::new(
        executor(),
        LlModel::new("mock", "m"),
        Prompt::builder("parent").user("Parent history").build(),
    )
    .with_tools(ToolRegistry::builder().tool(Worker).build())
    .with_run_handle(Arc::new(7u32))
}

#[test]
fn a_sub_agent_step_has_its_own_history_and_shares_the_counters_of_the_run() {
    let ctx = parent();

    let output = block_on(ctx.run_agent_step("worker", json!("Task"))).unwrap();

    // User message and answer: the parent's history is not in it.
    assert_eq!(output["history"], 2);
    assert_eq!(output["handle"], 7);
    assert_eq!(ctx.prompt().messages.len(), 1);
    assert_eq!(ctx.usage(), Some(USAGE));
}

#[test]
fn an_unknown_sub_agent_is_a_node_error() {
    let error = block_on(parent().run_agent_step("ghost", json!("Task"))).unwrap_err();

    assert!(error.to_string().contains("agent 'ghost'"), "{error}");
}

#[test]
fn the_default_step_of_a_tool_sends_text_as_a_request() {
    struct Echo;
    impl DynTool for Echo {
        fn name(&self) -> &str {
            "echo"
        }
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: "echo".into(),
                description: String::new(),
                parameters: json!({}),
            }
        }
        fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
            Box::pin(async move { Ok(args["request"].as_str().unwrap_or("?").to_owned()) })
        }
    }
    let ctx = parent().with_tools(ToolRegistry::builder().tool(Echo).build());

    let output = block_on(ctx.run_agent_step("echo", json!("Hi"))).unwrap();

    assert_eq!(output, json!("Hi"));
}
