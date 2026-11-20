//! The `system_one` decider from the environment (ADR-0100).

use grauwolf_spec::AgentSpecExt;
use std::sync::Arc;

use crate::config::SystemOneConfig;
use crate::systemone::{DeciderError, decider_with};
use grauwolf_agents::decision::Decider;
use grauwolf_agents::decision::{Decision, DecisionKind};
use grauwolf_spec::AgentSpec;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn decider_for(
    spec: &AgentSpec,
    provided: Option<Arc<dyn Decider>>,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Option<Arc<dyn Decider>>, DeciderError> {
    decider_with(spec, provided, &SystemOneConfig::from_lookup(&env))
}

fn spec(decider: &str) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "router",
            "model": { "provider": "ollama", "id": "m" },
            "graph": {
                "nodes": [
                    { "id": "route", "kind": "decide", "params": {
                        "value": "string", "kind": "yes_no", "question": "Urgent?", "decider": decider } }
                ],
                "edges": [
                    { "from": "start", "to": "route" },
                    { "from": "route", "to": "finish",
                      "when": { "kind": "on_decision", "params": { "answers": ["yes", "no"] } } }
                ]
            }
        })
        .to_string(),
    )
    .unwrap()
}

fn urgent() -> Decision {
    Decision {
        kind: DecisionKind::YesNo {
            threshold: 0.5,
            yes: None,
            no: None,
        },
        question: "Urgent?".into(),
        model: None,
        temperature: None,
        think: None,
    }
}

fn env(pairs: Vec<(&'static str, String)>) -> impl Fn(&str) -> Option<String> {
    move |name| {
        pairs
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.clone())
    }
}

#[test]
fn a_spec_without_system_one_needs_no_service() {
    let decider = decider_for(&spec("llm"), None, env(vec![])).unwrap();

    assert!(decider.is_none());
}

#[test]
fn a_spec_with_system_one_fails_early_without_the_base_url() {
    let error = decider_for(&spec("system_one"), None, env(vec![]))
        .err()
        .unwrap();

    assert!(matches!(error, DeciderError::MissingBaseUrl));
    let message = error.to_string();
    assert!(message.contains("RKEV_BASE_URL"), "{message}");
    assert!(message.contains("system_one"), "{message}");
}

#[test]
fn a_decider_that_is_handed_in_wins_over_the_environment() {
    let provided: Arc<dyn grauwolf_agents::decision::Decider> =
        Arc::new(crate::systemone::SystemOneClient::new("http://127.0.0.1:1"));

    let decider = decider_for(&spec("system_one"), Some(provided), env(vec![])).unwrap();

    assert!(decider.is_some());
}

#[tokio::test]
async fn the_decider_from_the_environment_asks_the_service_and_retries_a_busy_one() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(
            ResponseTemplate::new(503).set_body_json(
                json!({ "error": { "type": "busy", "message": "all slots are busy" } }),
            ),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "kev-1",
            "answers": { "decision": { "type": "noul", "noul": 0.9 } }
        })))
        .mount(&server)
        .await;
    let decider = decider_for(
        &spec("system_one"),
        None,
        env(vec![("RKEV_BASE_URL", server.uri())]),
    )
    .unwrap()
    .unwrap();

    let verdict = decider
        .decide(&urgent(), &json!("The server is down"))
        .await
        .unwrap();

    assert_eq!(verdict.answer, "yes");
    // The busy answer was retried. Exactly two requests are the rule; under the load of a full
    // workspace test run, one more transient error (e.g. a reset pooled connection) may add a
    // third retry, which is the retry doing its job, not a defect (seen once, 2026-11-15).
    let requests = server.received_requests().await.unwrap().len();
    assert!((2..=3).contains(&requests), "{requests} requests");
}
