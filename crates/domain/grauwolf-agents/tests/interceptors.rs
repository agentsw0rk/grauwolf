//! Intervening interceptors (F06.4, ADR-0047).

use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::AgentContext;
use grauwolf_agents::events::Interceptor;
use grauwolf_llm::{LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};

/// Records what arrives and answers with a secret.
#[derive(Default)]
struct Recorder(Mutex<Vec<(Vec<Message>, Vec<String>)>>);

impl LlmClient for Recorder {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        _model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        self.0.lock().unwrap().push((
            prompt.messages.clone(),
            tools.iter().map(|tool| tool.name.clone()).collect(),
        ));
        Box::pin(async { Ok(Message::assistant("The password is secret123.")) })
    }
}

/// Adds an instruction, hides `delete` and redacts secrets.
struct Guard;

impl Interceptor for Guard {
    fn before_llm_call(&self, prompt: &mut Prompt, tools: &mut Vec<ToolDescriptor>) {
        prompt
            .messages
            .insert(0, Message::system("Answer politely."));
        tools.retain(|tool| tool.name != "delete");
    }

    fn after_llm_call(&self, answer: &mut Message) {
        if let Message::Assistant(answer) = answer {
            for part in &mut answer.parts {
                if let grauwolf_prompt::AssistantPart::Text { text } = part {
                    *text = text.replace("secret123", "[removed]");
                }
            }
        }
    }
}

/// Tool that only returns its name and the arguments.
struct Named(&'static str);

impl grauwolf_tools::DynTool for Named {
    fn name(&self) -> &str {
        self.0
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: self.0.into(),
            description: String::new(),
            parameters: serde_json::json!({ "type": "object" }),
        }
    }

    fn call_json(
        &self,
        args: serde_json::Value,
    ) -> BoxFuture<'_, Result<String, grauwolf_tools::ToolError>> {
        Box::pin(async move { Ok(format!("{} with {args}", self.0)) })
    }
}

#[test]
fn changes_the_request_only_for_the_call_and_the_answer_before_it_is_kept() {
    let client = Arc::new(Recorder::default());
    let executor = PromptExecutor::builder()
        .client("mock", client.clone())
        .build();
    let tools = grauwolf_tools::ToolRegistry::builder()
        .tool(Named("add"))
        .tool(Named("delete"))
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("What is the password?").build(),
    )
    .with_tools(tools);
    ctx.intercept(Arc::new(Guard));

    let answer = block_on(ctx.request_llm()).unwrap();

    let (sent, offered) = client.0.lock().unwrap()[0].clone();
    assert_eq!(sent[0], Message::system("Answer politely."));
    assert_eq!(offered, ["add"]);
    assert_eq!(answer, Message::assistant("The password is [removed]."));
    assert_eq!(
        ctx.prompt().messages,
        [
            Message::user("What is the password?"),
            Message::assistant("The password is [removed].")
        ]
    );
}

/// Denies `delete`, doubles the argument `a` and truncates results to 20 characters.
struct ToolGuard;

impl Interceptor for ToolGuard {
    fn before_tool_call(
        &self,
        call: &mut grauwolf_prompt::ToolCall,
    ) -> grauwolf_agents::events::ToolDecision {
        if call.tool == "delete" {
            return grauwolf_agents::events::ToolDecision::Deny("Deleting is not allowed.".into());
        }
        if let Some(a) = call.args["a"].as_i64() {
            call.args["a"] = serde_json::json!(a * 2);
        }
        grauwolf_agents::events::ToolDecision::Proceed
    }

    fn after_tool_call(&self, result: &mut grauwolf_prompt::ToolResult) {
        result.content = result.content.chars().take(20).collect();
    }
}

#[test]
fn rewrites_or_denies_tool_calls_and_edits_their_results() {
    let tools = grauwolf_tools::ToolRegistry::builder()
        .tool(Named("add"))
        .tool(Named("delete"))
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(PromptExecutor::builder().build()),
        LlModel::new("mock", "m"),
        Prompt::builder("p").build(),
    )
    .with_tools(tools);
    ctx.intercept(Arc::new(ToolGuard));
    let call = |tool: &str| grauwolf_prompt::ToolCall {
        id: format!("{tool}_1"),
        tool: tool.into(),
        args: serde_json::json!({ "a": 2 }),
    };

    let added = block_on(ctx.execute_tool(&call("add")));
    let deleted = block_on(ctx.execute_tool(&call("delete")));

    assert_eq!(added.content, "add with {\"a\":4}");
    assert_eq!(deleted.content, "Deleting is not allo");
    assert_eq!(deleted.id, "delete_1");
}

/// Asks asynchronously (ADR-0061): `write` is denied, everything else is approved.
struct Approver;

impl Interceptor for Approver {
    fn approve_tool_call<'a>(
        &'a self,
        call: &'a grauwolf_prompt::ToolCall,
    ) -> BoxFuture<'a, grauwolf_agents::events::ToolDecision> {
        Box::pin(async move {
            if call.tool == "write" {
                grauwolf_agents::events::ToolDecision::Deny("Not approved.".into())
            } else {
                grauwolf_agents::events::ToolDecision::Proceed
            }
        })
    }
}

#[test]
fn runs_only_tool_calls_that_were_approved() {
    let tools = grauwolf_tools::ToolRegistry::builder()
        .tool(Named("read"))
        .tool(Named("write"))
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(PromptExecutor::builder().build()),
        LlModel::new("mock", "m"),
        Prompt::builder("p").build(),
    )
    .with_tools(tools);
    ctx.intercept(Arc::new(Approver));
    let call = |tool: &str| grauwolf_prompt::ToolCall {
        id: format!("{tool}_1"),
        tool: tool.into(),
        args: serde_json::json!({}),
    };

    let read = block_on(ctx.execute_tool(&call("read")));
    let written = block_on(ctx.execute_tool(&call("write")));

    assert_eq!(read.content, "read with {}");
    assert_eq!(written.content, "Not approved.");
}
