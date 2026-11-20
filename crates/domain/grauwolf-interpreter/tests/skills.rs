//! Skills (ADR-0103): the skill list reaches the system prompt of the interpreted agent.

use std::sync::Arc;

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::{LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};
use grauwolf_spec::{AgentSpec, AgentSpecExt};
use grauwolf_tools::{DynTool, ToolError};
use serde_json::{Value, json};

/// Stands in for the skill tools: carries the prompt addition like `load_skill`.
struct Skill(&'static str);

impl DynTool for Skill {
    fn name(&self) -> &str {
        self.0
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: self.0.into(),
            description: String::new(),
            parameters: json!({ "type": "object" }),
        }
    }

    fn call_json(&self, _args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async { Ok(String::new()) })
    }

    fn prompt_addition(&self) -> Option<String> {
        (self.0 == "load_skill").then(|| "Available skills: haiku-writer".to_owned())
    }
}

fn spec(system_prompt: Option<&str>) -> AgentSpec {
    let mut spec = json!({
        "spec_version": 1, "name": "writer",
        "model": { "provider": "mock", "id": "m" },
        "skills": [{ "dir": "skills" }],
        "strategy": { "kind": "single_run" }
    });
    if let Some(prompt) = system_prompt {
        spec["system_prompt"] = json!(prompt);
    }
    AgentSpec::from_json(&spec.to_string()).unwrap()
}

/// Answers with the system message it received.
struct Echo;

impl LlmClient for Echo {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        let system = prompt
            .messages
            .iter()
            .find_map(|message| match message {
                Message::System { content } => Some(content.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "no system message".into());
        Box::pin(async move { Ok(Message::assistant(system)) })
    }
}

fn run(spec: &AgentSpec, with_skills: bool) -> String {
    let tools: Vec<Arc<dyn DynTool>> = if with_skills {
        vec![
            Arc::new(Skill("load_skill")),
            Arc::new(Skill("read_skill_file")),
        ]
    } else {
        Vec::new()
    };
    let spec = if with_skills {
        spec.clone()
    } else {
        let mut spec = spec.clone();
        spec.skills.clear();
        spec
    };
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(Echo))
            .build(),
    ))
    .with_tools(tools)
    .load(&spec)
    .unwrap();
    block_on(agent.run("Write")).unwrap()
}

#[test]
fn the_skill_list_is_the_system_prompt_when_the_spec_has_none() {
    assert_eq!(run(&spec(None), true), "Available skills: haiku-writer");
}

#[test]
fn the_skill_list_follows_the_system_prompt_of_the_spec() {
    assert_eq!(
        run(&spec(Some("Be kind.")), true),
        "Be kind.\n\nAvailable skills: haiku-writer"
    );
}

#[test]
fn without_skills_the_system_prompt_stays_as_it_is() {
    assert_eq!(run(&spec(Some("Be kind.")), false), "Be kind.");
}
