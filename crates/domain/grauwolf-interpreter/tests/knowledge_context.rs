//! Knowledge as context (ADR-0113): the best sections are added before the user input.

use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::events::{AgentEvent, Feature};
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::{LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};
use grauwolf_spec::{AgentSpec, AgentSpecExt};
use grauwolf_tools::{DynTool, KnowledgeContext, KnowledgeHit, ToolError};
use serde_json::{Value, json};

#[derive(Default)]
struct Recorder(Mutex<Vec<(Prompt, Vec<String>)>>);

impl LlmClient for Recorder {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        _model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        let names = tools.iter().map(|tool| tool.name.clone()).collect();
        self.0.lock().unwrap().push((prompt.clone(), names));
        Box::pin(async { Ok(Message::assistant("Minka.")) })
    }
}

/// A knowledge base with fixed sections (or a failing one).
struct Fixed(Vec<(&'static str, &'static str)>, bool);

impl KnowledgeContext for Fixed {
    fn retrieve<'a>(
        &'a self,
        _query: &'a str,
    ) -> BoxFuture<'a, Result<Vec<KnowledgeHit>, ToolError>> {
        let failing = self.1;
        let hits = self
            .0
            .iter()
            .map(|(source, text)| KnowledgeHit {
                source: (*source).into(),
                text: (*text).into(),
            })
            .collect();
        Box::pin(async move {
            if failing {
                Err(ToolError::Execution("index unavailable".into()))
            } else {
                Ok(hits)
            }
        })
    }
}

/// The search tool of `knowledge`, backed by `Fixed`.
struct SearchTool(Arc<Fixed>);

impl DynTool for SearchTool {
    fn name(&self) -> &str {
        "search_knowledge"
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "search_knowledge".into(),
            description: String::new(),
            parameters: json!({ "type": "object" }),
        }
    }

    fn call_json(&self, _args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async { Ok(String::new()) })
    }

    fn knowledge_context(&self) -> Option<Arc<dyn KnowledgeContext>> {
        Some(self.0.clone())
    }
}

#[derive(Default)]
struct Events(Mutex<Vec<AgentEvent>>);

impl Feature for Events {
    fn on_event(&self, event: &AgentEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

fn spec(tool_enabled: bool) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "assistant",
            "model": { "provider": "mock", "id": "m" },
            "knowledge": {
                "sources": ["docs"], "inject": true, "tool_enabled": tool_enabled,
                "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" }
            },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap()
}

fn run(spec: &AgentSpec, knowledge: Fixed) -> (Vec<(Prompt, Vec<String>)>, Vec<AgentEvent>) {
    let client = Arc::new(Recorder::default());
    let events = Arc::new(Events::default());
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", client.clone())
            .build(),
    ))
    .with_tools(vec![
        Arc::new(SearchTool(Arc::new(knowledge))) as Arc<dyn DynTool>
    ])
    .with_feature(events.clone())
    .load(spec)
    .unwrap();
    block_on(agent.run_json(json!("What is the cat called?"))).unwrap();
    let prompts = client.0.lock().unwrap().clone();
    let events = events.0.lock().unwrap().clone();
    (prompts, events)
}

#[test]
fn adds_the_best_sections_with_their_sources_before_the_input_and_reports_it() {
    let (prompts, events) = run(
        &spec(true),
        Fixed(
            vec![
                ("pets.md", "The cat is named Minka."),
                ("https://example.com/p", "Minka is a cat."),
                ("pets.md", "Cats sleep a lot."),
            ],
            false,
        ),
    );

    let (prompt, tools) = &prompts[0];
    let n = prompt.messages.len();
    assert!(
        matches!(&prompt.messages[n - 2], Message::System { content }
            if content.contains("[pets.md] The cat is named Minka.")
            && content.contains("[https://example.com/p] Minka is a cat.")),
        "{:?}",
        prompt.messages
    );
    assert_eq!(
        prompt.messages[n - 1],
        Message::user("What is the cat called?")
    );
    assert_eq!(tools, &["search_knowledge"]);
    assert!(events.contains(&AgentEvent::KnowledgeInjected {
        sources: vec!["pets.md".into(), "https://example.com/p".into()],
        count: 3
    }));
}

#[test]
fn context_only_knowledge_does_not_offer_the_search_tool() {
    let (prompts, _) = run(
        &spec(false),
        Fixed(vec![("pets.md", "The cat is Minka.")], false),
    );

    let (prompt, tools) = &prompts[0];
    assert!(tools.is_empty(), "{tools:?}");
    assert!(prompt.messages.iter().any(
        |message| matches!(message, Message::System { content } if content.contains("[pets.md]"))
    ));
}

#[test]
fn adds_nothing_without_hits_and_reports_a_failure_without_ending_the_run() {
    let (empty, events) = run(&spec(true), Fixed(vec![], false));
    assert_eq!(empty[0].0.messages.len(), 1);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::KnowledgeInjected { .. }))
    );

    let (failed, events) = run(&spec(true), Fixed(vec![], true));
    assert_eq!(failed[0].0.messages.len(), 1);
    assert!(events.contains(&AgentEvent::KnowledgeFailed {
        error: "tool failed: index unavailable".into()
    }));
}
