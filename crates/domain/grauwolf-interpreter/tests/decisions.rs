//! Node kind `decide` and condition `on_decision` in the interpreter (ADR-0100).

use grauwolf_spec::AgentSpecExt;
use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::decision::{DecideError, Decider, Decision, Verdict};
use grauwolf_agents::events::{AgentEvent, Feature};
use grauwolf_interpreter::{Interpreter, InterpreterError};
use grauwolf_llm::PromptExecutor;
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use serde_json::{Value, json};

fn spec(decider: &str) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "router",
            "model": { "provider": "mock", "id": "m" },
            "graph": {
                "nodes": [
                    { "id": "route", "kind": "decide", "params": {
                        "value": "string", "kind": "choice", "question": "Which team?",
                        "options": [{ "name": "billing" }, { "name": "technical" }],
                        "decider": decider } },
                    { "id": "billing", "kind": "llm_request" },
                    { "id": "technical", "kind": "llm_reason", "params": { "value": "string" } }
                ],
                "edges": [
                    { "from": "start", "to": "route" },
                    { "from": "route", "to": "billing",
                      "when": { "kind": "on_decision", "params": { "answers": ["billing"] } } },
                    { "from": "route", "to": "technical",
                      "when": { "kind": "on_decision", "params": { "answers": ["technical"] } } },
                    { "from": "billing", "to": "finish", "when": { "kind": "on_assistant_message" } },
                    { "from": "technical", "to": "finish" }
                ]
            }
        })
        .to_string(),
    )
    .unwrap()
}

fn executor() -> Arc<PromptExecutor> {
    let mock = MockLlmClient::new("Looked into it.")
        .answer_when_contains("Which team?", r#"{ "answer": "billing" }"#)
        .answer_when_contains("invoice", "We refund the invoice.");
    Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(mock))
            .build(),
    )
}

#[derive(Default)]
struct Events(Mutex<Vec<AgentEvent>>);

impl Feature for Events {
    fn on_event(&self, event: &AgentEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

#[test]
fn the_model_decides_which_branch_runs_and_the_events_carry_the_node_id() {
    let events = Arc::new(Events::default());
    let agent = Interpreter::new(executor())
        .with_feature(events.clone())
        .load(&spec("llm"))
        .unwrap();

    let reply = block_on(agent.run("The invoice is wrong")).unwrap();

    assert_eq!(reply, "We refund the invoice.");
    let seen = events.0.lock().unwrap();
    assert!(seen.iter().any(|event| matches!(
        event,
        AgentEvent::DecisionCompleted { node, answer, .. } if node == "route" && answer == "billing"
    )));
}

struct Technical;

impl Decider for Technical {
    fn decide<'a>(
        &'a self,
        _decision: &'a Decision,
        _state: &'a Value,
    ) -> BoxFuture<'a, Result<Verdict, DecideError>> {
        Box::pin(std::future::ready(Ok(Verdict::new("technical"))))
    }
}

#[test]
fn an_external_decider_answers_for_system_one_nodes() {
    let agent = Interpreter::new(executor())
        .with_decider(Arc::new(Technical))
        .load(&spec("system_one"))
        .unwrap();

    let reply = block_on(agent.run("The printer is broken")).unwrap();

    assert_eq!(reply, "Looked into it.");
}

#[test]
fn a_spec_that_needs_system_one_cannot_load_without_a_decider() {
    let error = Interpreter::new(executor())
        .load(&spec("system_one"))
        .err()
        .unwrap();

    assert!(matches!(error, InterpreterError::MissingDecider), "{error}");
}

#[test]
fn the_decision_request_carries_temperature_and_think_and_later_requests_do_not() {
    let mut spec = serde_json::from_str::<Value>(&spec("llm").to_json()).unwrap();
    spec["model"]["provider"] = json!("ollama");
    spec["graph"]["nodes"][0]["params"]["temperature"] = json!(0);
    spec["graph"]["nodes"][0]["params"]["think"] = json!(false);
    let spec = AgentSpec::from_json(&spec.to_string()).unwrap();
    let mock = MockLlmClient::new("Looked into it.")
        .answer_when_contains("Which team?", r#"{ "answer": "billing" }"#)
        .answer_when_contains("invoice", "We refund the invoice.");
    let executor = Arc::new(
        PromptExecutor::builder()
            .client("ollama", Arc::new(mock))
            .build(),
    );
    let events = Arc::new(Events::default());
    let agent = Interpreter::new(executor)
        .with_feature(events.clone())
        .load(&spec)
        .unwrap();

    block_on(agent.run("The invoice is wrong")).unwrap();

    let seen = events.0.lock().unwrap();
    let samplings: Vec<_> = seen
        .iter()
        .filter_map(|event| match event {
            AgentEvent::LlmCallStarting { model, .. } => Some((model.temperature, model.think)),
            _ => None,
        })
        .collect();
    assert_eq!(samplings, [(Some(0.0), Some(false)), (None, None)]);
}
