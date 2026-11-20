//! `SystemOneClient` against recorded System One exchanges (ADR-0100); the request and
//! response shapes follow the worked example of the RKEV README.

use std::time::Duration;

use grauwolf_agents::decision::{ChoiceOption, Decider, Decision, DecisionKind};
use grauwolf_systemone::SystemOneClient;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const STATE: &str = "Shoes arrived two weeks late and in the wrong size. \
                     Also I see two charges on my card.";

fn departments() -> Decision {
    let option = |name: &str, description: &str| ChoiceOption {
        name: name.into(),
        description: Some(description.into()),
    };
    Decision {
        kind: DecisionKind::Choice {
            options: vec![
                option("returns", "Exchanges, refunds, wrong or damaged items"),
                option("shipping", "Delivery status, delays, lost packages"),
                option("billing", "Charges, invoices, payment problems"),
            ],
        },
        question: "Which team should handle this?".into(),
        model: Some("kev-latest".into()),
        temperature: None,
        think: None,
    }
}

fn escalation(threshold: f64) -> Decision {
    Decision {
        kind: DecisionKind::YesNo {
            threshold,
            yes: None,
            no: None,
        },
        question: "Does this need urgent human attention?".into(),
        model: None,
        temperature: None,
        think: None,
    }
}

fn frustration() -> Decision {
    Decision {
        kind: DecisionKind::Score {
            levels: vec!["Calm".into(), "Frustrated".into(), "Very angry".into()],
        },
        question: "How frustrated is the customer?".into(),
        model: None,
        temperature: None,
        think: None,
    }
}

/// The answers of the README response as sent, so that the order of their fields survives.
const DEPARTMENT: &str = r#"{ "type": "choice", "choice": "returns", "confidence": 0.21,
    "probabilities": { "returns": 0.47, "shipping": 0.28, "billing": 0.25 } }"#;
const ESCALATE: &str = r#"{ "type": "noul", "noul": 0.93 }"#;
const FRUSTRATION: &str = r#"{ "type": "score", "score": 1.44, "confidence": 0.34,
    "legend": { "0": "Calm", "1": "Frustrated", "2": "Very angry" },
    "probabilities": { "0": 0.00, "1": 0.56, "2": 0.44 } }"#;

/// A response that answers the question `decision` with `answer`.
fn readme_answer(question: &str) -> String {
    let answer = match question {
        "department" => DEPARTMENT,
        "escalate" => ESCALATE,
        _ => FRUSTRATION,
    };
    format!(
        r#"{{ "model": "kev-latest", "answers": {{ "decision": {answer} }},
            "usage": {{ "input_tokens": 101, "output_tokens": 161 }}, "latency_ms": 495 }}"#
    )
}

async fn server_answering(body: impl ToString) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body.to_string(), "application/json"))
        .mount(&server)
        .await;
    server
}

async fn sent(server: &MockServer) -> (Value, String, Vec<(String, String)>) {
    let request = server.received_requests().await.unwrap().remove(0);
    let headers = request
        .headers
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_str().unwrap().to_owned()))
        .collect();
    (
        serde_json::from_slice(&request.body).unwrap(),
        String::from_utf8(request.body.clone()).unwrap(),
        headers,
    )
}

#[tokio::test]
async fn asks_one_question_with_the_id_decision() {
    let server = server_answering(readme_answer("department")).await;

    SystemOneClient::new(server.uri())
        .decide(&departments(), &json!(STATE))
        .await
        .unwrap();

    let (body, _, _) = sent(&server).await;
    assert_eq!(
        body,
        json!({
            "state": STATE,
            "model": "kev-latest",
            "questions": { "decision": {
                "type": "choice",
                "instructions": "Which team should handle this?",
                "criteria": {
                    "returns": "Exchanges, refunds, wrong or damaged items",
                    "shipping": "Delivery status, delays, lost packages",
                    "billing": "Charges, invoices, payment problems"
                }
            } }
        })
    );
}

#[tokio::test]
async fn options_keep_their_order_and_a_missing_description_is_null() {
    let server = server_answering(readme_answer("department")).await;
    let decision = Decision {
        kind: DecisionKind::Choice {
            options: vec![
                ChoiceOption {
                    name: "shipping".into(),
                    description: Some("Delivery".into()),
                },
                ChoiceOption {
                    name: "returns".into(),
                    description: Some("Refunds".into()),
                },
                ChoiceOption {
                    name: "billing".into(),
                    description: None,
                },
            ],
        },
        question: "Which team?".into(),
        model: None,
        temperature: None,
        think: None,
    };

    SystemOneClient::new(server.uri())
        .decide(&decision, &json!("x"))
        .await
        .unwrap();

    let (body, text, _) = sent(&server).await;
    assert!(
        text.contains(r#""criteria":{"shipping":"Delivery","returns":"Refunds","billing":null}"#),
        "{text}"
    );
    assert!(body.get("model").is_none(), "no model unless one is named");
}

#[tokio::test]
async fn yes_no_and_score_questions_have_their_own_shape() {
    let noul = server_answering(readme_answer("escalate")).await;
    let score = server_answering(readme_answer("frustration")).await;
    let described = Decision {
        kind: DecisionKind::YesNo {
            threshold: 0.5,
            yes: Some("Needs a human today".into()),
            no: Some("Can wait for the queue".into()),
        },
        ..escalation(0.5)
    };

    SystemOneClient::new(noul.uri())
        .decide(&escalation(0.5), &json!("x"))
        .await
        .unwrap();
    SystemOneClient::new(score.uri())
        .decide(&frustration(), &json!("x"))
        .await
        .unwrap();
    let plain = sent(&noul).await.0["questions"]["decision"].clone();
    let levels = sent(&score).await.0["questions"]["decision"].clone();
    let with_criteria = server_answering(readme_answer("escalate")).await;
    SystemOneClient::new(with_criteria.uri())
        .decide(&described, &json!("x"))
        .await
        .unwrap();
    let criteria = sent(&with_criteria).await.0["questions"]["decision"].clone();

    assert_eq!(
        plain,
        json!({ "type": "noul", "instructions": "Does this need urgent human attention?" })
    );
    assert_eq!(
        levels,
        json!({
            "type": "score",
            "instructions": "How frustrated is the customer?",
            "criteria": ["Calm", "Frustrated", "Very angry"]
        })
    );
    assert_eq!(
        criteria["criteria"],
        json!({ "true": "Needs a human today", "false": "Can wait for the queue" })
    );
}

#[tokio::test]
async fn a_structured_state_is_sent_as_json() {
    let server = server_answering(readme_answer("department")).await;
    let state = json!({ "subject": "Wrong size", "body": "Shoes arrived late." });

    SystemOneClient::new(server.uri())
        .decide(&departments(), &state)
        .await
        .unwrap();

    assert_eq!(sent(&server).await.0["state"], state);
}

#[tokio::test]
async fn the_bearer_token_is_sent_only_with_a_key() {
    let with = server_answering(readme_answer("department")).await;
    let without = server_answering(readme_answer("department")).await;

    SystemOneClient::new(with.uri())
        .with_api_key("secret")
        .decide(&departments(), &json!("x"))
        .await
        .unwrap();
    SystemOneClient::new(without.uri())
        .decide(&departments(), &json!("x"))
        .await
        .unwrap();

    let authorization = |headers: Vec<(String, String)>| {
        headers
            .into_iter()
            .find(|(name, _)| name == "authorization")
            .map(|(_, value)| value)
    };
    assert_eq!(
        authorization(sent(&with).await.2).as_deref(),
        Some("Bearer secret")
    );
    assert_eq!(authorization(sent(&without).await.2), None);
}

#[tokio::test]
async fn a_choice_answer_is_the_option_with_confidence_and_distribution() {
    let server = server_answering(readme_answer("department")).await;

    let verdict = SystemOneClient::new(server.uri())
        .decide(&departments(), &json!(STATE))
        .await
        .unwrap();

    assert_eq!(verdict.answer, "returns");
    assert_eq!(verdict.confidence, Some(0.21));
    assert_eq!(verdict.model.as_deref(), Some("kev-latest"));
    assert_eq!(
        verdict.probabilities,
        [
            ("returns".to_owned(), 0.47),
            ("shipping".to_owned(), 0.28),
            ("billing".to_owned(), 0.25)
        ],
        "in the order the service sent them"
    );
}

#[tokio::test]
async fn a_yes_no_answer_applies_the_threshold_to_the_probability_of_yes() {
    let server = server_answering(readme_answer("escalate")).await;
    let client = SystemOneClient::new(server.uri());

    let yes = client.decide(&escalation(0.9), &json!("x")).await.unwrap();
    let no = client.decide(&escalation(0.95), &json!("x")).await.unwrap();

    assert_eq!((yes.answer.as_str(), yes.probability), ("yes", Some(0.93)));
    assert_eq!((no.answer.as_str(), no.probability), ("no", Some(0.93)));
    assert_eq!(yes.confidence, None);
}

#[tokio::test]
async fn a_score_answer_is_the_level_description_of_the_rounded_mean_index() {
    let server = server_answering(readme_answer("frustration")).await;

    let verdict = SystemOneClient::new(server.uri())
        .decide(&frustration(), &json!("x"))
        .await
        .unwrap();

    assert_eq!(verdict.answer, "Frustrated");
    assert_eq!(verdict.confidence, Some(0.34));
    assert_eq!(
        verdict.probabilities,
        [
            ("Calm".to_owned(), 0.0),
            ("Frustrated".to_owned(), 0.56),
            ("Very angry".to_owned(), 0.44)
        ]
    );
}

#[tokio::test]
async fn a_score_beyond_the_scale_is_clamped_to_its_last_level() {
    let server = server_answering(json!({
        "model": "kev-latest",
        "answers": { "decision": { "type": "score", "score": 7.2 } }
    }))
    .await;

    let verdict = SystemOneClient::new(server.uri())
        .decide(&frustration(), &json!("x"))
        .await
        .unwrap();

    assert_eq!(verdict.answer, "Very angry");
}

#[tokio::test]
async fn optional_fields_of_the_answer_may_be_missing() {
    let server = server_answering(json!({
        "answers": { "decision": { "type": "choice", "choice": "billing" } }
    }))
    .await;

    let verdict = SystemOneClient::new(server.uri())
        .decide(&departments(), &json!("x"))
        .await
        .unwrap();

    assert_eq!(verdict.answer, "billing");
    assert_eq!(verdict.confidence, None);
    assert!(verdict.probabilities.is_empty());
    assert_eq!(verdict.model, None);
}

#[tokio::test]
async fn a_busy_service_is_a_transient_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(503).set_body_json(
                json!({ "error": { "type": "busy", "message": "all slots are busy" } }),
            ),
        )
        .mount(&server)
        .await;

    let error = SystemOneClient::new(server.uri())
        .decide(&departments(), &json!("x"))
        .await
        .unwrap_err();

    assert!(error.is_transient(), "{error}");
    assert!(error.to_string().contains("all slots are busy"), "{error}");
}

#[tokio::test]
async fn an_unreachable_service_is_a_transient_error() {
    // Nothing listens on port 1.
    let error = SystemOneClient::new("http://127.0.0.1:1")
        .decide(&departments(), &json!("x"))
        .await
        .unwrap_err();

    assert!(error.is_transient(), "{error}");
}

#[tokio::test]
async fn a_slow_service_times_out_as_a_transient_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(500)))
        .mount(&server)
        .await;

    let error = SystemOneClient::new(server.uri())
        .with_request_timeout(Duration::from_millis(50))
        .decide(&departments(), &json!("x"))
        .await
        .unwrap_err();

    assert!(error.is_transient(), "{error}");
}

#[tokio::test]
async fn other_errors_fail_with_the_message_of_the_service() {
    for (status, kind, message) in [
        (
            400,
            "invalid_request",
            "criteria must have 1 to 255 options",
        ),
        (401, "unauthorized", "invalid API key"),
        (404, "not_found", "no model 'kev-x'"),
        (413, "too_large", "state is too long"),
        (500, "internal", "engine failed"),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(status)
                    .set_body_json(json!({ "error": { "type": kind, "message": message } })),
            )
            .mount(&server)
            .await;

        let error = SystemOneClient::new(server.uri())
            .decide(&departments(), &json!("x"))
            .await
            .unwrap_err();

        assert!(!error.is_transient(), "{status}: {error}");
        assert!(error.to_string().contains(message), "{status}: {error}");
        assert!(error.to_string().contains(&status.to_string()), "{error}");
    }
}

#[tokio::test]
async fn an_error_without_the_service_format_shows_its_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(502).set_body_string("Bad gateway"))
        .mount(&server)
        .await;

    let error = SystemOneClient::new(server.uri())
        .decide(&departments(), &json!("x"))
        .await
        .unwrap_err();

    assert!(error.to_string().contains("Bad gateway"), "{error}");
}

#[tokio::test]
async fn an_answer_of_the_wrong_type_or_without_the_question_is_an_error() {
    let wrong = server_answering(readme_answer("escalate")).await;
    let missing = server_answering(json!({ "model": "m", "answers": {} })).await;

    let wrong = SystemOneClient::new(wrong.uri())
        .decide(&departments(), &json!("x"))
        .await
        .unwrap_err();
    let missing = SystemOneClient::new(missing.uri())
        .decide(&departments(), &json!("x"))
        .await
        .unwrap_err();

    assert!(wrong.to_string().contains("noul"), "{wrong}");
    assert!(missing.to_string().contains("decision"), "{missing}");
}

#[test]
fn the_client_is_built_from_the_environment() {
    let env = |pairs: &'static [(&str, &str)]| {
        move |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    };

    assert!(SystemOneClient::from_env(env(&[])).is_none());
    assert!(SystemOneClient::from_env(env(&[("RKEV_BASE_URL", " ")])).is_none());
    assert!(
        SystemOneClient::from_env(env(&[("RKEV_BASE_URL", "http://localhost:8090/")])).is_some()
    );
    let shown = format!(
        "{:?}",
        SystemOneClient::from_env(env(&[
            ("RKEV_BASE_URL", "http://h"),
            ("RKEV_API_KEY", "secret")
        ]))
        .unwrap()
    );
    assert!(!shown.contains("secret"), "{shown}");
}

#[test]
fn the_timeouts_are_ten_seconds_to_connect_and_a_minute_per_request() {
    assert_eq!(grauwolf_systemone::CONNECT_TIMEOUT, Duration::from_secs(10));
    assert_eq!(grauwolf_systemone::REQUEST_TIMEOUT, Duration::from_secs(60));
}
