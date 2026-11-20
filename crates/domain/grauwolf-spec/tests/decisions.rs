//! Decision nodes in the spec (ADR-0100).

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, ValidationError, ValidationErrorKind};
use serde_json::{Value, json};

fn routing(edges: Value) -> Value {
    json!({
        "spec_version": 1, "name": "router",
        "model": { "provider": "ollama", "id": "m" },
        "graph": {
            "nodes": [
                { "id": "route", "kind": "decide", "params": {
                    "value": "string", "kind": "choice", "question": "Which team?",
                    "options": [{ "name": "billing" }, { "name": "technical" }] } },
                { "id": "billing", "kind": "llm_request" },
                { "id": "technical", "kind": "llm_request" }
            ],
            "edges": edges
        }
    })
}

fn good_edges() -> Value {
    json!([
        { "from": "start", "to": "route" },
        { "from": "route", "to": "billing", "when": { "kind": "on_decision", "params": { "answers": ["billing"] } } },
        { "from": "route", "to": "technical", "when": { "kind": "on_decision", "params": { "answers": ["technical"] } } },
        { "from": "billing", "to": "finish", "when": { "kind": "on_assistant_message" } },
        { "from": "technical", "to": "finish", "when": { "kind": "on_assistant_message" } }
    ])
}

fn errors(spec: Value) -> Vec<ValidationError> {
    AgentSpec::from_json(&spec.to_string())
        .unwrap()
        .validate()
        .err()
        .unwrap_or_default()
}

#[test]
fn a_decision_with_one_edge_per_answer_is_valid() {
    assert_eq!(errors(routing(good_edges())), []);
}

#[test]
fn on_decision_may_only_leave_a_decide_node() {
    let mut edges = good_edges();
    edges[3]["when"] = json!({ "kind": "on_decision", "params": { "answers": ["billing"] } });

    let found = errors(routing(edges));

    assert!(
        found.iter().any(|error| error.path == "/graph/edges/3/when"
            && matches!(&error.kind, ValidationErrorKind::DecisionEdgeSource)),
        "{found:?}"
    );
}

#[test]
fn an_edge_leaving_a_decide_node_needs_on_decision() {
    let mut edges = good_edges();
    edges.as_array_mut().unwrap()[2]
        .as_object_mut()
        .unwrap()
        .remove("when");

    let found = errors(routing(edges));

    assert!(found.iter().any(|error| error.path == "/graph/edges/2"
        && matches!(&error.kind, ValidationErrorKind::DecisionEdgeCondition { node } if node == "route")), "{found:?}");
}

#[test]
fn an_answer_the_decision_does_not_have_is_an_error() {
    let mut edges = good_edges();
    edges[2]["when"]["params"]["answers"] = json!(["technical", "sales"]);

    let found = errors(routing(edges));

    assert_eq!(
        found,
        [ValidationError {
            path: "/graph/edges/2/when/params/answers/1".into(),
            kind: ValidationErrorKind::UnknownDecisionAnswer {
                node: "route".into(),
                answer: "sales".into()
            }
        }]
    );
}

#[test]
fn every_answer_needs_exactly_one_edge() {
    let mut missing = good_edges();
    missing.as_array_mut().unwrap().remove(2);
    let mut twice = good_edges();
    twice[2]["when"]["params"]["answers"] = json!(["technical", "billing"]);

    let missing = errors(routing(missing));
    let twice = errors(routing(twice));

    assert!(missing.iter().any(|error| error.path == "/graph/nodes/0"
        && matches!(&error.kind, ValidationErrorKind::DecisionAnswerUncovered { answer, .. } if answer == "technical")), "{missing:?}");
    assert!(twice.iter().any(|error| error.path == "/graph/edges/2"
        && matches!(&error.kind, ValidationErrorKind::DecisionAnswerDuplicated { answer, .. } if answer == "billing")), "{twice:?}");
}

#[test]
fn yes_no_and_score_decisions_have_their_own_answers() {
    let mut spec = routing(good_edges());
    spec["graph"]["nodes"][0]["params"] = json!({
        "value": "string", "kind": "yes_no", "question": "Urgent?" });
    spec["graph"]["edges"][1]["when"]["params"]["answers"] = json!(["yes"]);
    spec["graph"]["edges"][2]["when"]["params"]["answers"] = json!(["no"]);
    assert_eq!(errors(spec), []);

    let mut spec = routing(good_edges());
    spec["graph"]["nodes"][0]["params"] = json!({
        "value": "string", "kind": "score", "question": "How urgent?",
        "levels": ["low", "medium", "high"] });
    spec["graph"]["edges"][1]["when"]["params"]["answers"] = json!(["high", "medium"]);
    spec["graph"]["edges"][2]["when"]["params"]["answers"] = json!(["low"]);
    assert_eq!(errors(spec), []);
}

#[test]
fn impossible_parameters_are_reported_at_the_node() {
    let mut spec = routing(good_edges());
    spec["graph"]["nodes"][0]["params"]["options"] = json!([{ "name": "a" }, { "name": "a" }]);

    let found = errors(spec);

    assert!(found.iter().any(|error| error.path == "/graph/nodes/0/params"
        && matches!(&error.kind, ValidationErrorKind::InvalidParams { kind, .. } if kind == "decide")), "{found:?}");
}

#[test]
fn the_model_decider_needs_the_provider_of_its_model() {
    let mut spec = routing(good_edges());
    spec["graph"]["nodes"][0]["params"]["model"] = json!("openai/gpt-5-mini");
    let model_decider = AgentSpec::from_json(&spec.to_string()).unwrap();
    spec["graph"]["nodes"][0]["params"]["decider"] = json!("system_one");
    spec["graph"]["nodes"][0]["params"]["model"] = json!("kev-1");
    let external = AgentSpec::from_json(&spec.to_string()).unwrap();

    assert_eq!(model_decider.providers(), ["ollama", "openai"]);
    assert!(!model_decider.uses_system_one());
    assert_eq!(external.providers(), ["ollama"]);
    assert!(external.uses_system_one());
}

#[test]
fn the_provider_of_a_decision_model_is_listed_for_the_provider_check() {
    let mut spec = routing(good_edges());
    spec["graph"]["nodes"][0]["params"]["model"] = json!("openai/gpt-5-mini");
    let spec = AgentSpec::from_json(&spec.to_string()).unwrap();

    assert!(
        spec.provider_uses()
            .contains(&("/graph/nodes/0/params/model".to_owned(), "openai"))
    );
}

fn with_decision_params(extra: Value) -> Vec<(String, ValidationErrorKind)> {
    let mut spec = routing(good_edges());
    for (name, value) in extra.as_object().unwrap() {
        spec["graph"]["nodes"][0]["params"][name] = value.clone();
    }
    errors(spec)
        .into_iter()
        .map(|error| (error.path, error.kind))
        .collect()
}

#[test]
fn temperature_and_think_of_a_decision_follow_the_model_rules() {
    assert_eq!(
        with_decision_params(json!({ "temperature": 0, "think": false })),
        []
    );
    assert_eq!(
        with_decision_params(json!({ "model": "openai/gpt-5-mini", "temperature": 2 })),
        []
    );
    assert_eq!(
        with_decision_params(json!({ "temperature": 2.5 })),
        [(
            "/graph/nodes/0/params/temperature".to_owned(),
            ValidationErrorKind::TemperatureOutOfRange
        )]
    );
    assert_eq!(
        with_decision_params(json!({ "model": "openai/gpt-5-mini", "think": true })),
        [(
            "/graph/nodes/0/params/think".to_owned(),
            ValidationErrorKind::ThinkNotSupported {
                provider: "openai".into()
            }
        )]
    );
}

#[test]
fn temperature_and_think_of_a_decision_need_the_llm_decider() {
    assert_eq!(
        with_decision_params(
            json!({ "decider": "system_one", "model": "kev-1", "temperature": 0, "think": false })
        ),
        [
            (
                "/graph/nodes/0/params/temperature".to_owned(),
                ValidationErrorKind::SamplingWithSystemOne
            ),
            (
                "/graph/nodes/0/params/think".to_owned(),
                ValidationErrorKind::SamplingWithSystemOne
            )
        ]
    );
}
