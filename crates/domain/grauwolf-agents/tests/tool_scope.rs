//! Tool subsets of subgraphs (F05.6, ADR-0027, ADR-0034).

use std::sync::Arc;

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::AgentContext;
use grauwolf_agents::scopes::ToolSubset;
use grauwolf_graph::Scope;
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::{LlModel, Prompt, ToolCall};
use grauwolf_tools::{Tool, ToolError, ToolRegistry};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize, JsonSchema)]
struct Args {
    a: f64,
    b: f64,
}

struct Calc(&'static str);

impl Tool for Calc {
    type Args = Args;
    type Output = f64;

    fn name(&self) -> &str {
        self.0
    }

    fn description(&self) -> &str {
        "Calculates"
    }

    fn call(&self, args: Args) -> BoxFuture<'_, Result<f64, ToolError>> {
        let result = if self.0 == "add" {
            args.a + args.b
        } else {
            args.a * args.b
        };
        Box::pin(async move { Ok(result) })
    }
}

fn context() -> AgentContext {
    AgentContext::new(
        Arc::new(PromptExecutor::builder().build()),
        LlModel::new("mock", "m"),
        Prompt::builder("p").build(),
    )
    .with_tools(
        ToolRegistry::builder()
            .tool(Calc("add"))
            .tool(Calc("mul"))
            .build(),
    )
}

fn call(tool: &str) -> ToolCall {
    ToolCall {
        id: "c".into(),
        tool: tool.into(),
        args: json!({ "a": 2, "b": 3 }),
    }
}

#[test]
fn offers_and_runs_only_the_tools_of_the_active_subgraph() {
    let mut ctx = context();
    let scope = ToolSubset::new(["add"]);

    scope.enter(&mut ctx);
    let offered: Vec<String> = ctx
        .tool_descriptors()
        .into_iter()
        .map(|tool| tool.name)
        .collect();
    let allowed = block_on(ctx.execute_tool(&call("add")));
    let forbidden = block_on(ctx.execute_tool(&call("mul")));
    scope.exit(&mut ctx);
    let after: Vec<String> = ctx
        .tool_descriptors()
        .into_iter()
        .map(|tool| tool.name)
        .collect();

    assert_eq!(offered, ["add"]);
    assert_eq!(allowed.content, "5.0");
    assert_eq!(
        forbidden.content,
        "Error: tool 'mul' is not available in this section"
    );
    assert_eq!(after, ["add", "mul"]);
}

/// Self-built tools (addendum ADR-0075): `create_tool` and the tools of the level follow the
/// innermost build permission; a tool list further inside hides them.
#[test]
fn offers_self_made_tools_only_where_building_is_allowed() {
    use grauwolf_agents::scopes::SubgraphScope;

    let shelf = grauwolf_tools::ToolShelf::default();
    shelf.put(Arc::new(Calc("mul"))).unwrap();
    let mut ctx = AgentContext::new(
        Arc::new(PromptExecutor::builder().build()),
        LlModel::new("mock", "m"),
        Prompt::builder("p").build(),
    )
    .with_tools(
        ToolRegistry::builder()
            .tool(Calc("add"))
            .tool(Calc("create_tool"))
            .build()
            .with_shelf(shelf),
    )
    .with_self_tools("create_tool", false);
    let offered = |ctx: &AgentContext| -> Vec<String> {
        ctx.tool_descriptors()
            .into_iter()
            .map(|tool| tool.name)
            .collect()
    };
    let workshop = SubgraphScope::new()
        .with_tools(["add"])
        .with_self_tools(true);
    let silent = ToolSubset::new(Vec::<String>::new());

    let outside = offered(&ctx);
    workshop.enter(&mut ctx);
    let inside = offered(&ctx);
    let used = block_on(ctx.execute_tool(&call("mul")));
    silent.enter(&mut ctx);
    let reasoning = offered(&ctx);
    silent.exit(&mut ctx);
    workshop.exit(&mut ctx);
    let denied = block_on(ctx.execute_tool(&call("mul")));

    assert_eq!(outside, ["add"]);
    assert_eq!(inside, ["add", "create_tool", "mul"]);
    assert_eq!(used.content, "6.0");
    assert_eq!(reasoning, Vec::<String>::new());
    assert_eq!(offered(&ctx), ["add"]);
    assert!(
        denied.content.contains("not available"),
        "{}",
        denied.content
    );
}

/// Limit for tool calls (ADR-0078): beyond it no tool runs any more, not even in a fork;
/// the model learns of it as a tool result.
#[test]
fn stops_running_tools_once_the_limit_of_the_run_is_reached() {
    let ctx = context().with_budget(grauwolf_agents::Budget {
        max_tool_calls: Some(2),
        max_prompt_tokens: None,
        ..Default::default()
    });

    let first = block_on(ctx.execute_tool(&call("add")));
    let second = block_on(ctx.fork().execute_tool(&call("mul")));
    let third = block_on(ctx.execute_tool(&call("add")));

    assert_eq!(first.content, "5.0");
    assert_eq!(second.content, "6.0");
    assert_eq!(
        third.content,
        "Error: limit of 2 tool calls per run reached – answer now without further tools."
    );
}
