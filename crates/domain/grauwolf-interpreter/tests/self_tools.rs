//! Tools that the agent creates itself during the run (ADR-0075).

use grauwolf_spec::AgentSpecExt;
use std::sync::Arc;

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_interpreter::{InterpretedAgent, Interpreter};
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::ToolDescriptor;
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use grauwolf_tools::{DynTool, NewTool, ToolError, ToolFactory};
use serde_json::{Value, json};

/// Built tool of the fake factory: returns its code as the result.
struct Echo(NewTool);

impl DynTool for Echo {
    fn name(&self) -> &str {
        &self.0.name
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: self.0.name.clone(),
            description: self.0.description.clone(),
            parameters: self.0.args.clone(),
        }
    }

    fn call_json(&self, _args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        let code = self.0.code.clone();
        Box::pin(async move { Ok(format!("Echo: {code}")) })
    }
}

/// Builds nothing, but turns each code into an [`Echo`].
struct FakeFactory;

impl ToolFactory for FakeFactory {
    fn create(&self, tool: NewTool) -> BoxFuture<'_, Result<Arc<dyn DynTool>, String>> {
        Box::pin(async move { Ok(Arc::new(Echo(tool)) as Arc<dyn DynTool>) })
    }
}

fn agent(self_tools: Value, mock: MockLlmClient) -> InterpretedAgent {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "toolmaker",
            "model": { "provider": "mock", "id": "m" },
            "self_tools": self_tools,
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(mock))
            .build(),
    ))
    .with_tool_factory(Arc::new(FakeFactory))
    .load(&spec)
    .unwrap()
}

#[test]
fn offers_a_created_tool_in_the_same_run() {
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains(
            "Build me",
            "create_tool",
            json!({ "name": "shout", "description": "Ruft laut", "args": { "type": "object" },
                    "code": "Ok(\"LOUD\".into())" }),
        )
        .call_tool_when_contains("available from now on", "shout", json!({}))
        .answer_when_contains("Echo:", "done");

    let output = block_on(agent(json!({ "confirm": false }), mock).run("Build me a tool")).unwrap();

    assert_eq!(output, "done");
}

#[test]
fn needs_approval_for_new_tools_by_default() {
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains(
            "Build me",
            "create_tool",
            json!({ "name": "shout", "description": "Ruft laut", "args": { "type": "object" },
                    "code": "Ok(\"LOUD\".into())" }),
        )
        .answer_when_contains("approval", "not allowed");

    let output = block_on(agent(json!({}), mock).run("Build me a tool")).unwrap();

    assert_eq!(output, "not allowed");
}

#[test]
fn refuses_more_tools_than_allowed_and_reports_it_to_the_model() {
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains(
            "Build me",
            "create_tool",
            json!({ "name": "shout", "description": "Ruft laut", "args": { "type": "object" },
                    "code": "Ok(\"LOUD\".into())" }),
        )
        .answer_when_contains("at most 0 new tools", "limit reached");

    let output =
        block_on(agent(json!({ "confirm": false, "max_tools": 0 }), mock).run("Build me a tool"))
            .unwrap();

    assert_eq!(output, "limit reached");
}

#[test]
fn refuses_to_load_self_tools_without_a_factory() {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "toolmaker",
            "model": { "provider": "mock", "id": "m" },
            "self_tools": {},
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();

    let error = Interpreter::new(Arc::new(PromptExecutor::builder().build()))
        .load(&spec)
        .err()
        .unwrap();

    assert!(
        matches!(
            error,
            grauwolf_interpreter::InterpreterError::MissingToolFactory
        ),
        "{error}"
    );
}

#[test]
fn accepts_a_whole_tool_function_instead_of_its_body() {
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains(
            "Build me",
            "create_tool",
            json!({ "name": "shout", "description": "Ruft laut", "args": { "type": "object" },
                    "code": "fn tool(args: serde_json::Value) -> Result<String, String> {\n    Ok(\"LOUD\".into())\n}" }),
        )
        .call_tool_when_contains("available from now on", "shout", json!({}))
        .answer_when_contains("}\ntool(args)", "called");

    let output = block_on(agent(json!({ "confirm": false }), mock).run("Build me a tool")).unwrap();

    assert_eq!(output, "called");
}

#[test]
fn refuses_to_build_outside_subgraphs_when_switched_off() {
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains(
            "Build me",
            "create_tool",
            json!({ "name": "shout", "description": "Ruft laut", "args": { "type": "object" },
                    "code": "Ok(\"LOUD\".into())" }),
        )
        .answer_when_contains("not available", "forbidden");

    let output =
        block_on(agent(json!({ "confirm": false, "enabled": false }), mock).run("Build me a tool"))
            .unwrap();

    assert_eq!(output, "forbidden");
}

#[test]
fn lets_a_permitted_subgraph_build_and_use_tools_despite_its_tool_list() {
    let tool_loop = json!({
        "nodes": [ { "id": "call_llm", "kind": "llm_request" },
                   { "id": "execute_tool", "kind": "execute_tool" },
                   { "id": "send_result", "kind": "llm_send_tool_result" } ],
        "edges": [ { "from": "start", "to": "call_llm" },
                   { "from": "call_llm", "to": "execute_tool", "when": { "kind": "on_tool_call" } },
                   { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } },
                   { "from": "execute_tool", "to": "send_result" },
                   { "from": "send_result", "to": "execute_tool", "when": { "kind": "on_tool_call" } },
                   { "from": "send_result", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
    });
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "toolmaker",
            "model": { "provider": "mock", "id": "m" },
            "self_tools": { "confirm": false, "enabled": false },
            "subgraphs": { "werkstatt": { "tools": [], "self_tools": true, "graph": tool_loop } },
            "graph": {
                "nodes": [ { "id": "werkstatt", "kind": "subgraph", "params": { "subgraph": "werkstatt" } } ],
                "edges": [ { "from": "start", "to": "werkstatt" }, { "from": "werkstatt", "to": "finish" } ]
            }
        })
        .to_string(),
    )
    .unwrap();
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains(
            "Build me",
            "create_tool",
            json!({ "name": "shout", "description": "Ruft laut", "args": { "type": "object" },
                    "code": "Ok(\"LOUD\".into())" }),
        )
        .call_tool_when_contains("available from now on", "shout", json!({}))
        .answer_when_contains("Echo:", "done");
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(mock))
            .build(),
    ))
    .with_tool_factory(Arc::new(FakeFactory))
    .load(&spec)
    .unwrap();

    let output = block_on(agent.run("Build me a tool")).unwrap();

    assert_eq!(output, "done");
}

/// LLM that first calls `create_tool` and then fails.
#[derive(Default)]
struct BuildThenDown(std::sync::Mutex<usize>);

impl grauwolf_llm::LlmClient for BuildThenDown {
    fn execute<'a>(
        &'a self,
        _prompt: &'a grauwolf_prompt::Prompt,
        _model: &'a grauwolf_prompt::LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<grauwolf_prompt::Message, grauwolf_llm::LlmError>> {
        let mut calls = self.0.lock().unwrap();
        *calls += 1;
        let answer = match *calls {
            1 => Ok(grauwolf_prompt::Message::tool_call(
                "c1",
                "create_tool",
                json!({ "name": "shout", "description": "Ruft laut", "args": { "type": "object" },
                        "code": "Ok(\"LOUD\".into())" }),
            )),
            _ => Err(grauwolf_llm::LlmError::Client("Connection lost".into())),
        };
        Box::pin(async move { answer })
    }
}

#[test]
fn rebuilds_self_made_tools_when_resuming_a_run() {
    let spec = |_: ()| {
        AgentSpec::from_json(
            &json!({
                "spec_version": 1, "name": "toolmaker",
                "model": { "provider": "mock", "id": "m" },
                "self_tools": { "confirm": false },
                "strategy": { "kind": "single_run" }
            })
            .to_string(),
        )
        .unwrap()
    };
    let load = |client: Arc<dyn grauwolf_llm::LlmClient>| {
        Interpreter::new(Arc::new(
            PromptExecutor::builder().client("mock", client).build(),
        ))
        .with_tool_factory(Arc::new(FakeFactory))
        .load(&spec(()))
        .unwrap()
    };
    let store = Arc::new(grauwolf_agents::checkpoints::MemoryCheckpointStore::default());
    let crashed = block_on(load(Arc::new(BuildThenDown::default())).run_checkpointed(
        json!("Build me a tool"),
        store.clone(),
        "werkstatt",
    ));
    assert!(crashed.is_err());
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains("available from now on", "shout", json!({}))
        .answer_when_contains("Echo:", "done");

    let output = block_on(load(Arc::new(mock)).resume(store, "werkstatt")).unwrap();

    assert_eq!(output, json!("done"));
}
