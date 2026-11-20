//! Tool loop: llm_request → execute_tool → llm_send_tool_result (F05.9).

use std::sync::Arc;

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::{AgentContext, conditions, nodes};
use grauwolf_graph::TypedGraph;
use grauwolf_llm::{LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};
use grauwolf_test::MockLlmClient;
use grauwolf_tools::{Tool, ToolError, ToolRegistry};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

struct Add;

#[derive(Deserialize, JsonSchema)]
struct AddArgs {
    a: f64,
    b: f64,
}

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
        Box::pin(async move { Ok(args.a + args.b) })
    }
}

#[test]
fn computes_an_answer_through_a_tool_call() {
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains("2+3", "add", json!({ "a": 2, "b": 3 }))
        .answer_when_contains("5.0", "The result is 5.");
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(mock))
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("calc").build(),
    )
    .with_tools(ToolRegistry::builder().tool(Add).build());
    let graph = TypedGraph::<AgentContext, String, String>::build(|g| {
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
    .unwrap();

    let answer = block_on(graph.run(&mut ctx, "What is 2+3?".to_string())).unwrap();

    assert_eq!(answer, "The result is 5.");
    let tool_calls = ctx
        .prompt()
        .messages
        .iter()
        .filter(|message| matches!(message, Message::Assistant(answer) if answer.tool_calls().next().is_some()))
        .count();
    assert_eq!(
        tool_calls,
        1,
        "tool call duplicated in the history: {:?}",
        ctx.prompt().messages
    );
    assert!(
        ctx.prompt()
            .messages
            .contains(&Message::tool_result("add", "add", "5.0"))
    );
}

/// Client that answers with the names of the offered tools.
struct ToolEcho;

impl LlmClient for ToolEcho {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
        let answer = Message::assistant(names.join(","));
        Box::pin(async move { Ok(answer) })
    }
}

#[test]
fn offers_the_agent_tools_to_the_llm() {
    let executor = PromptExecutor::builder()
        .client("echo", Arc::new(ToolEcho))
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(executor),
        LlModel::new("echo", "m"),
        Prompt::builder("calc").build(),
    )
    .with_tools(ToolRegistry::builder().tool(Add).build());

    let answer = block_on(nodes::llm_request(
        &mut ctx,
        "Which tools are there?".into(),
    ))
    .unwrap();

    assert_eq!(answer, Message::assistant("add"));
}

#[test]
fn executes_several_tool_calls_of_one_answer_together() {
    let mock = MockLlmClient::new("?")
        .call_tools_when_contains(
            "both",
            [
                ("add", json!({ "a": 2, "b": 3 })),
                ("add", json!({ "a": 4, "b": 5 })),
            ],
        )
        .answer_when_contains("9.0", "5 and 9");
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(mock))
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("calc").build(),
    )
    .with_tools(ToolRegistry::builder().tool(Add).build());
    let graph = TypedGraph::<AgentContext, String, String>::build(|g| {
        let call_llm = g.node("call_llm", nodes::llm_request);
        let execute_tools = g.node("execute_tools", nodes::execute_multiple_tools);
        let send_results = g.node("send_results", nodes::llm_send_multiple_tool_results);
        g.edge(&g.start(), &call_llm);
        g.edge_when(
            &call_llm,
            &execute_tools,
            conditions::on_multiple_tool_calls,
        );
        g.edge_when(&call_llm, &g.finish(), conditions::on_assistant_message);
        g.edge(&execute_tools, &send_results);
        g.edge_when(
            &send_results,
            &execute_tools,
            conditions::on_multiple_tool_calls,
        );
        g.edge_when(&send_results, &g.finish(), conditions::on_assistant_message);
    })
    .unwrap();

    let answer = block_on(graph.run(&mut ctx, "Calculate both".to_string())).unwrap();

    assert_eq!(answer, "5 and 9");
    assert_eq!(
        ctx.prompt().messages[2..4],
        [
            Message::tool_result("add_1", "add", "5.0"),
            Message::tool_result("add_2", "add", "9.0"),
        ]
    );
}
