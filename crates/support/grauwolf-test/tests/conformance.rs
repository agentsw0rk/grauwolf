//! LlmClient conformance suite (ADR-0005) against mocks: a conforming client passes, a
//! non-conforming one fails.

use futures_executor::block_on;
use grauwolf_prompt::LlModel;
use grauwolf_test::MockLlmClient;
use grauwolf_test::conformance::check_llm_client;
use serde_json::json;

fn model() -> LlModel {
    LlModel::new("mock", "m")
}

#[test]
fn a_conforming_client_passes_every_check() {
    let client = MockLlmClient::new("?")
        .answer_when_contains("Hello", "Hello")
        .call_tool_when_contains("2+3", "add", json!({ "a": 2, "b": 3 }))
        .answer_when_contains("5", "The result is 5.")
        .answer_when_contains(
            "Login",
            r#"{ "title": "Login button does not react", "priority": 1 }"#,
        );

    let failures = block_on(check_llm_client(&client, &model()));

    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn a_client_that_ignores_the_request_fails_every_check() {
    let client = MockLlmClient::new("?");

    let failures = block_on(check_llm_client(&client, &model()));

    assert_eq!(
        failures
            .iter()
            .map(|failure| failure.check)
            .collect::<Vec<_>>(),
        [
            "text",
            "tool_call",
            "tool_result",
            "structured",
            "streaming"
        ]
    );
}
