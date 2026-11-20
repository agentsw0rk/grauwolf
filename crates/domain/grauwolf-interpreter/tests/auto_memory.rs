//! Automatic memory (ADR-0069).

use grauwolf_spec::AgentSpecExt;
use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::events::{AgentEvent, Feature};
use grauwolf_agents::memory::FactMemory;
use grauwolf_interpreter::{Interpreter, RunExtras};
use grauwolf_llm::{LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};
use grauwolf_spec::AgentSpec;
use grauwolf_tools::{DynTool, ToolError};
use serde_json::{Value, json};

/// Records every prompt and answers with a fixed reply.
#[derive(Default)]
struct Recorder(Mutex<Vec<Prompt>>);

impl LlmClient for Recorder {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        self.0.lock().unwrap().push(prompt.clone());
        Box::pin(async { Ok(Message::assistant("You drink green tea.")) })
    }
}

/// Memory with fixed facts; records what is stored.
#[derive(Default)]
struct Facts(Mutex<Vec<String>>);

impl FactMemory for Facts {
    fn recall<'a>(
        &'a self,
        _query: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, grauwolf_agents::memory::MemoryError>> {
        Box::pin(async { Ok(vec!["Anna likes green tea best.".to_owned()]) })
    }

    fn remember<'a>(
        &'a self,
        facts: &'a [String],
    ) -> BoxFuture<'a, Result<(), grauwolf_agents::memory::MemoryError>> {
        self.0.lock().unwrap().extend(facts.iter().cloned());
        Box::pin(async { Ok(()) })
    }
}

/// Stands in for the memory tools that the spec requires as external tools.
struct Placeholder(&'static str);

impl DynTool for Placeholder {
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
}

#[derive(Default)]
struct Events(Mutex<Vec<AgentEvent>>);

impl Feature for Events {
    fn on_event(&self, event: &AgentEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

fn spec(auto: Value) -> AgentSpec {
    let mut memory = json!({
        "store": "m.json",
        "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" }
    });
    for (key, value) in auto.as_object().unwrap() {
        memory[key] = value.clone();
    }
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "assistant",
            "model": { "provider": "mock", "id": "m" },
            "system_prompt": "Be helpful.",
            "long_term_memory": memory,
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap()
}

#[test]
fn recalls_facts_before_the_input_when_asked_to() {
    let client = Arc::new(Recorder::default());
    let events = Arc::new(Events::default());
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", client.clone())
            .build(),
    ))
    .with_tools(vec![
        Arc::new(Placeholder("remember")) as Arc<dyn DynTool>,
        Arc::new(Placeholder("recall")),
    ])
    .with_feature(events.clone())
    .load(&spec(json!({ "auto_recall": true })))
    .unwrap();

    let output = block_on(agent.run_with(
        json!("What do I like to drink?"),
        RunExtras {
            memory: Some(Arc::new(Facts::default())),
            ..RunExtras::default()
        },
    ))
    .unwrap();

    assert_eq!(output, json!("You drink green tea."));
    let prompt = client.0.lock().unwrap()[0].clone();
    let n = prompt.messages.len();
    assert!(
        matches!(&prompt.messages[n - 2], Message::System { content } if content.contains("Anna likes green tea best.")),
        "{:?}",
        prompt.messages
    );
    assert_eq!(
        prompt.messages[n - 1],
        Message::user("What do I like to drink?")
    );
    assert!(
        events
            .0
            .lock()
            .unwrap()
            .contains(&AgentEvent::MemoryRecalled {
                facts: vec!["Anna likes green tea best.".into()]
            })
    );
}

/// Answers the fact request with a fact, otherwise with a greeting.
struct Extracting;

impl LlmClient for Extracting {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        let extraction = prompt.messages.iter().any(|message| {
            matches!(message, Message::System { content } if content.contains("durable facts"))
        });
        let answer = if extraction {
            r#"{ "facts": ["Anna likes green tea best."] }"#
        } else {
            "Nice to meet you, Anna!"
        };
        Box::pin(async move { Ok(Message::assistant(answer)) })
    }
}

#[test]
fn remembers_facts_from_a_successful_run_when_asked_to() {
    let facts = Arc::new(Facts::default());
    let events = Arc::new(Events::default());
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(Extracting))
            .build(),
    ))
    .with_tools(vec![
        Arc::new(Placeholder("remember")) as Arc<dyn DynTool>,
        Arc::new(Placeholder("recall")),
    ])
    .with_feature(events.clone())
    .load(&spec(json!({ "auto_remember": true })))
    .unwrap();

    let output = block_on(agent.run_with(
        json!("I am Anna and I like green tea best."),
        RunExtras {
            memory: Some(facts.clone()),
            ..RunExtras::default()
        },
    ))
    .unwrap();

    assert_eq!(output, json!("Nice to meet you, Anna!"));
    assert_eq!(*facts.0.lock().unwrap(), ["Anna likes green tea best."]);
    let events = events.0.lock().unwrap();
    let remembered = events
        .iter()
        .position(|event| matches!(event, AgentEvent::MemoryRemembered { .. }))
        .expect("memory_remembered is missing");
    let completed = events
        .iter()
        .position(|event| matches!(event, AgentEvent::AgentCompleted { .. }))
        .unwrap();
    assert!(remembered < completed);
}

/// Returns no fact for the fact request only if the known fact is already in it.
struct KnowsKnownFacts;

impl LlmClient for KnowsKnownFacts {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        let extraction = prompt.messages.iter().any(|message| {
            matches!(message, Message::System { content } if content.contains("durable facts"))
        });
        let knows = prompt.messages.iter().any(|message| {
            matches!(message, Message::User { content, .. }
                if content.contains("Already known") && content.contains("Anna likes green tea best."))
        });
        let answer = match (extraction, knows) {
            (true, true) => r#"{ "facts": [] }"#,
            (true, false) => r#"{ "facts": ["Anna likes green tea best of all."] }"#,
            (false, _) => "I know that.",
        };
        Box::pin(async move { Ok(Message::assistant(answer)) })
    }
}

#[test]
fn remembers_only_facts_that_are_not_known_yet() {
    let facts = Arc::new(Facts::default());
    let events = Arc::new(Events::default());
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(KnowsKnownFacts))
            .build(),
    ))
    .with_feature(events.clone())
    .load(&spec(json!({ "auto_remember": true, "tools": false })))
    .unwrap();

    block_on(agent.run_with(
        json!("I like green tea best."),
        RunExtras {
            memory: Some(facts.clone()),
            ..RunExtras::default()
        },
    ))
    .unwrap();

    assert!(
        facts.0.lock().unwrap().is_empty(),
        "{:?}",
        facts.0.lock().unwrap()
    );
    assert!(
        !events
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, AgentEvent::MemoryRemembered { .. }))
    );
}

/// LLM that fails immediately.
struct Down;

impl LlmClient for Down {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async { Err(LlmError::Client("Connection lost".into())) })
    }
}

/// Facts only if the fact request contains the original input.
struct ExtractingFromInput;

impl LlmClient for ExtractingFromInput {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        let extraction_with_input = prompt.messages.iter().any(|message| {
            matches!(message, Message::User { content, .. } if content.contains("User message:\nI drink green tea."))
        });
        let answer = if extraction_with_input {
            r#"{ "facts": ["Anna drinks green tea."] }"#
        } else {
            "Noted."
        };
        Box::pin(async move { Ok(Message::assistant(answer)) })
    }
}

#[test]
fn remembers_facts_from_the_original_input_after_resuming() {
    let facts = Arc::new(Facts::default());
    let store = Arc::new(grauwolf_agents::checkpoints::MemoryCheckpointStore::default());
    let agent = |client: Arc<dyn LlmClient>| {
        Interpreter::new(Arc::new(
            PromptExecutor::builder().client("mock", client).build(),
        ))
        .with_tools(vec![
            Arc::new(Placeholder("remember")) as Arc<dyn DynTool>,
            Arc::new(Placeholder("recall")),
        ])
        .load(&spec(json!({ "auto_remember": true })))
        .unwrap()
    };
    let extras = || RunExtras {
        memory: Some(facts.clone()),
        ..RunExtras::default()
    };

    let crashed = block_on(agent(Arc::new(Down)).run_checkpointed_with(
        json!("I drink green tea."),
        store.clone(),
        "run-tea",
        extras(),
    ));
    assert!(crashed.is_err());
    let output =
        block_on(agent(Arc::new(ExtractingFromInput)).resume_with(store, "run-tea", extras()))
            .unwrap();

    assert_eq!(output, json!("Noted."));
    assert_eq!(*facts.0.lock().unwrap(), ["Anna drinks green tea."]);
}
