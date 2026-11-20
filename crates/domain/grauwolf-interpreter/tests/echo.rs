//! Interpreter runs specs from the example corpus (F13.10).

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;
use std::sync::Arc;

use futures_executor::block_on;
use grauwolf_interpreter::{Interpreter, InterpreterError};
use grauwolf_llm::PromptExecutor;
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;

fn example(name: &str) -> AgentSpec {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/specs")
        .join(name);
    AgentSpec::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn runs_the_echo_spec_against_a_mock_llm() {
    let executor = PromptExecutor::builder()
        .client(
            "ollama",
            Arc::new(MockLlmClient::new("?").answer_when_contains("Hello world", "Hello world")),
        )
        .build();
    let agent = Interpreter::new(Arc::new(executor))
        .load(&example("echo.json"))
        .unwrap();

    let output = block_on(agent.run("Hello world")).unwrap();

    assert_eq!(output, "Hello world");
}

#[test]
fn refuses_specs_that_do_not_validate() {
    let spec = AgentSpec::from_json(
        r#"{ "spec_version": 1, "name": "broken", "model": { "provider": "ollama", "id": "m" },
             "graph": { "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
                        "edges": [ { "from": "start", "to": "call_llm" }, { "from": "call_llm", "to": "finish" } ] } }"#,
    )
    .unwrap();
    let interpreter = Interpreter::new(Arc::new(PromptExecutor::builder().build()));

    let error = interpreter
        .load(&spec)
        .err()
        .expect("invalid spec was loaded");

    assert!(
        matches!(&error, InterpreterError::Invalid(errors) if errors.len() == 1),
        "unexpected error: {error}"
    );
}

#[test]
fn runs_agents_with_named_types_on_json_values() {
    let spec = AgentSpec::from_json(
        r#"{ "spec_version": 1, "name": "relay", "model": { "provider": "ollama", "id": "m" },
             "types": { "Answer": { "type": "object", "properties": { "text": { "type": "string" } } } },
             "input": "Answer", "output": "Answer",
             "graph": { "edges": [ { "from": "start", "to": "finish" } ] } }"#,
    )
    .unwrap();
    let executor = PromptExecutor::builder().build();
    let agent = Interpreter::new(Arc::new(executor)).load(&spec).unwrap();

    let output = block_on(agent.run_json(serde_json::json!({ "text": "hi" }))).unwrap();

    assert_eq!(output, serde_json::json!({ "text": "hi" }));
}

#[test]
fn rejects_inputs_that_do_not_match_the_declared_type() {
    let spec = AgentSpec::from_json(
        r#"{ "spec_version": 1, "name": "relay", "model": { "provider": "ollama", "id": "m" },
             "types": { "Answer": { "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] } },
             "input": "Answer", "output": "Answer",
             "graph": { "edges": [ { "from": "start", "to": "finish" } ] } }"#,
    )
    .unwrap();
    let agent = Interpreter::new(Arc::new(PromptExecutor::builder().build()))
        .load(&spec)
        .unwrap();

    let error = block_on(agent.run_json(serde_json::json!({ "txt": "hi" }))).unwrap_err();

    assert!(matches!(error, InterpreterError::Input { .. }), "{error:?}");
    assert_eq!(
        error.to_string(),
        "input does not fit type Answer: /text: missing required field"
    );
}

#[test]
fn passes_frames_of_streaming_nodes_to_the_frame_sink() {
    let spec = AgentSpec::from_json(
        r#"{ "spec_version": 1, "name": "live", "model": { "provider": "ollama", "id": "m" },
             "graph": { "nodes": [ { "id": "call_llm", "kind": "llm_request_streaming" } ],
                        "edges": [ { "from": "start", "to": "call_llm" },
                                   { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } } ] } }"#,
    )
    .unwrap();
    let executor = PromptExecutor::builder()
        .client(
            "ollama",
            Arc::new(MockLlmClient::new("?").answer_when_contains("Hello", "Hi")),
        )
        .build();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = seen.clone();
    let agent = Interpreter::new(Arc::new(executor))
        .with_frame_sink(Arc::new(move |frame: &grauwolf_llm::StreamFrame| {
            sink.lock().unwrap().push(frame.clone());
        }))
        .load(&spec)
        .unwrap();

    let output = block_on(agent.run("Hello")).unwrap();

    assert_eq!(output, "Hi");
    assert_eq!(
        seen.lock().unwrap().first(),
        Some(&grauwolf_llm::StreamFrame::TextDelta { text: "Hi".into() })
    );
}

#[test]
fn runs_subgraphs_from_the_spec() {
    let spec = AgentSpec::from_json(
        r#"{ "spec_version": 1, "name": "nested", "model": { "provider": "ollama", "id": "m" },
             "subgraphs": { "answer": { "graph": {
                 "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
                 "edges": [ { "from": "start", "to": "call_llm" },
                            { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } } ] } } },
             "graph": { "nodes": [ { "id": "answer", "kind": "subgraph", "params": { "subgraph": "answer" } } ],
                        "edges": [ { "from": "start", "to": "answer" }, { "from": "answer", "to": "finish" } ] } }"#,
    )
    .unwrap();
    let executor = PromptExecutor::builder()
        .client(
            "ollama",
            Arc::new(MockLlmClient::new("?").answer_when_contains("Hello", "Hi")),
        )
        .build();
    let agent = Interpreter::new(Arc::new(executor)).load(&spec).unwrap();

    let output = block_on(agent.run("Hello")).unwrap();

    assert_eq!(output, "Hi");
}

/// Appends a note to every answer.
struct Signature;

impl grauwolf_agents::events::Interceptor for Signature {
    fn after_llm_call(&self, answer: &mut grauwolf_prompt::Message) {
        if let grauwolf_prompt::Message::Assistant(answer) = answer {
            answer.parts.push(grauwolf_prompt::AssistantPart::Text {
                text: " (checked)".into(),
            });
        }
    }
}

#[test]
fn installs_interceptors_in_every_run() {
    let executor = PromptExecutor::builder()
        .client(
            "ollama",
            Arc::new(MockLlmClient::new("?").answer_when_contains("Hello world", "Hello world")),
        )
        .build();
    let agent = Interpreter::new(Arc::new(executor))
        .with_interceptor(Arc::new(Signature))
        .load(&example("echo.json"))
        .unwrap();

    assert_eq!(
        block_on(agent.run("Hello world")).unwrap(),
        "Hello world (checked)"
    );
    assert_eq!(
        block_on(agent.run("Hello world")).unwrap(),
        "Hello world (checked)"
    );
}
