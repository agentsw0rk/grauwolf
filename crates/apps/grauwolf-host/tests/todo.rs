//! The todo tools keep their list in the agent storage: it survives a checkpoint (ADR-0098).

use std::path::Path;
use std::sync::Arc;

use futures_executor::block_on;
use grauwolf_agents::AgentContext;
use grauwolf_host::config::MapEnv;
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::{LlModel, Prompt, ToolCall};
use grauwolf_spec::BuiltinToolsSpec;
use grauwolf_tools::ToolRegistry;
use serde_json::json;

mod support;

fn context(snapshot: Option<grauwolf_agents::checkpoints::ContextSnapshot>) -> AgentContext {
    let builtin = BuiltinToolsSpec {
        tools: ["add_task", "complete_task", "remove_task", "list_tasks"]
            .map(String::from)
            .to_vec(),
        ..Default::default()
    };
    let tools = support::tools_with_env(&builtin, Path::new("."), MapEnv::new([])).unwrap();
    let executor = Arc::new(PromptExecutor::builder().build());
    let ctx = match snapshot {
        Some(snapshot) => AgentContext::restore(executor, snapshot),
        None => AgentContext::new(
            executor,
            LlModel::new("mock", "m"),
            Prompt::builder("p").build(),
        ),
    };
    ctx.with_tools(
        tools
            .into_iter()
            .fold(ToolRegistry::builder(), |b, t| b.shared(t))
            .build(),
    )
}

fn run(ctx: &AgentContext, tool: &str, args: serde_json::Value) -> String {
    let call = ToolCall {
        id: "c".into(),
        tool: tool.into(),
        args,
    };
    block_on(ctx.execute_tool(&call)).content
}

#[test]
fn the_task_list_survives_a_resume() {
    let ctx = context(None);
    run(&ctx, "add_task", json!({ "title": "First" }));
    run(&ctx, "add_task", json!({ "title": "Second" }));
    run(&ctx, "complete_task", json!({ "id": 1 }));

    let resumed = context(Some(ctx.snapshot()));

    assert_eq!(
        run(&resumed, "list_tasks", json!({})),
        "#1 [x] First\n#2 [ ] Second"
    );
    assert_eq!(
        run(&resumed, "add_task", json!({ "title": "Third" })),
        "Added task #3: Third"
    );
}
