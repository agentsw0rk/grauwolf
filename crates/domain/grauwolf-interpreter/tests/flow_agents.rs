//! Sub-agents as steps of a flow (F14.13, ADR-0101).

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::events::{AgentEvent, Feature};
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::{Completion, LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor, Usage};
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use serde_json::json;

fn example(name: &str) -> AgentSpec {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/specs")
        .join(name);
    AgentSpec::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Two steps of the same sub-agent one after the other.
fn two_steps() -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "flow",
            "model": { "provider": "ollama", "id": "m" },
            "agent_tools": { "translator": { "spec": "echo.json", "description": "Translates" } },
            "graph": {
                "nodes": [
                    { "id": "first", "kind": "agent", "params": { "agent": "translator" } },
                    { "id": "second", "kind": "agent", "params": { "agent": "translator" } }
                ],
                "edges": [
                    { "from": "start", "to": "first" },
                    { "from": "first", "to": "second" },
                    { "from": "second", "to": "finish" }
                ]
            }
        })
        .to_string(),
    )
    .unwrap()
}

fn sub_agent(client: impl LlmClient + 'static) -> Arc<dyn grauwolf_tools::DynTool> {
    let interpreter = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("ollama", Arc::new(client))
            .build(),
    ));
    Arc::new(interpreter.load(&example("echo.json")).unwrap()).into_tool("translator", "Translates")
}

fn flow(
    client: impl LlmClient + Clone + 'static,
    feature: Option<Arc<dyn Feature>>,
) -> grauwolf_interpreter::InterpretedAgent {
    let mut interpreter = Interpreter::new(Arc::new(PromptExecutor::builder().build()))
        .with_tools(vec![sub_agent(client)]);
    if let Some(feature) = feature {
        interpreter = interpreter.with_feature(feature);
    }
    interpreter.load(&two_steps()).unwrap()
}

#[test]
fn agent_nodes_run_the_sub_agent_one_step_after_the_other() {
    let mock = MockLlmClient::new("?")
        .answer_when_contains("Hola", "Hello")
        .answer_when_contains("Hello", "In English: Hello");

    let answer = block_on(flow(mock, None).run("Hola")).unwrap();

    assert_eq!(answer, "In English: Hello");
}

/// Answers "Hello" and reports 10 input and 2 output tokens.
#[derive(Clone)]
struct Counting;

impl LlmClient for Counting {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async { Ok(Message::assistant("Hello")) })
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
                usage: Some(Usage {
                    input_tokens: 10,
                    output_tokens: 2,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                }),
            })
        })
    }
}

#[derive(Default)]
struct Recorder(Mutex<Vec<AgentEvent>>);

impl Feature for Recorder {
    fn on_event(&self, event: &AgentEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

#[test]
fn the_parent_run_counts_the_usage_of_its_sub_agent_steps() {
    let recorder = Arc::new(Recorder::default());

    block_on(flow(Counting, Some(recorder.clone())).run("Hola")).unwrap();

    let events = recorder.0.lock().unwrap();
    let usage = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::AgentCompleted { usage, .. } => Some(*usage),
            _ => None,
        })
        .flatten()
        .expect("the parent reports a usage");
    assert_eq!((usage.input_tokens, usage.output_tokens), (20, 4));
}
