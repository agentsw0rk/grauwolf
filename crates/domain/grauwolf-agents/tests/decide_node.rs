//! Node `decide` and condition `on_decision` (ADR-0100).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::decision::{DecideError, Decider, Decision, DecisionKind, Verdict};
use grauwolf_agents::events::{AgentEvent, Feature};
use grauwolf_agents::{AgentContext, conditions, implementations, nodes};
use grauwolf_graph::TypedGraph;
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::{LlModel, Prompt};
use grauwolf_test::MockLlmClient;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Decider that answers `result` and remembers what it was asked.
struct Fixed {
    result: Result<Verdict, DecideError>,
    asked: Mutex<Vec<(Decision, Value)>>,
}

impl Fixed {
    fn answering(answer: &str) -> Arc<Self> {
        Self::with(Ok(Verdict {
            answer: answer.into(),
            confidence: Some(0.9),
            model: Some("kev-1".into()),
            ..Verdict::default()
        }))
    }

    fn with(result: Result<Verdict, DecideError>) -> Arc<Self> {
        Arc::new(Self {
            result,
            asked: Mutex::default(),
        })
    }
}

impl Decider for Fixed {
    fn decide<'a>(
        &'a self,
        decision: &'a Decision,
        state: &'a Value,
    ) -> BoxFuture<'a, Result<Verdict, DecideError>> {
        self.asked
            .lock()
            .unwrap()
            .push((decision.clone(), state.clone()));
        Box::pin(std::future::ready(self.result.clone()))
    }
}

#[derive(Default)]
struct Events(Mutex<Vec<AgentEvent>>);

impl Feature for Events {
    fn on_event(&self, event: &AgentEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

fn context() -> AgentContext {
    let mock =
        MockLlmClient::new("?").answer_when_contains("invoice", r#"{ "answer": "billing" }"#);
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(mock))
        .build();
    AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").build(),
    )
}

fn routing(extra: Value) -> Value {
    let mut params = json!({
        "value": "string",
        "kind": "choice",
        "question": "Which team handles the ticket?",
        "options": [{ "name": "billing", "description": "Invoices" }, { "name": "technical" }]
    });
    params
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    params
}

fn decide(ctx: &mut AgentContext, params: &Value, input: Value) -> Result<Value, String> {
    let node = implementations::node("decide", params, &BTreeMap::new()).unwrap();
    block_on(node(ctx, input)).map_err(|error| error.to_string())
}

#[test]
fn decide_passes_the_value_on_and_asks_the_external_decider_about_it() {
    let decider = Fixed::answering("technical");
    let mut ctx = context().with_decider(decider.clone());
    let params = routing(json!({ "decider": "system_one", "model": "kev-1" }));

    let output = decide(&mut ctx, &params, json!("The printer is on fire")).unwrap();

    assert_eq!(output, json!("The printer is on fire"));
    let asked = decider.asked.lock().unwrap();
    let (decision, state) = &asked[0];
    assert_eq!(state, &json!("The printer is on fire"));
    assert_eq!(decision.question, "Which team handles the ticket?");
    assert_eq!(decision.model.as_deref(), Some("kev-1"));
    assert_eq!(decision.kind.answers(), ["billing", "technical"]);
}

#[test]
fn the_verdict_is_stored_in_the_agent_storage() {
    let mut ctx = context().with_decider(Fixed::answering("technical"));

    decide(
        &mut ctx,
        &routing(json!({ "decider": "system_one" })),
        json!("x"),
    )
    .unwrap();

    let stored: Value = ctx.load("grauwolf.decision").unwrap().unwrap();
    assert_eq!(stored["node"], "decide");
    assert_eq!(stored["answer"], "technical");
    assert_eq!(stored["confidence"], 0.9);
    assert_eq!(stored["model"], "kev-1");
}

#[test]
fn the_model_decides_by_default() {
    let mut ctx = context();

    decide(&mut ctx, &routing(json!({})), json!("My invoice is wrong")).unwrap();

    let stored: Value = ctx.load("grauwolf.decision").unwrap().unwrap();
    assert_eq!(stored["answer"], "billing");
}

#[test]
fn a_yes_no_decision_uses_the_threshold_of_the_node() {
    let decider = Fixed::with(Ok(Verdict {
        answer: "no".into(),
        probability: Some(0.6),
        ..Verdict::default()
    }));
    let mut ctx = context().with_decider(decider.clone());
    let params = json!({
        "value": "string", "kind": "yes_no", "question": "Urgent?", "threshold": 0.8,
        "yes": "Needs a reply today", "decider": "system_one"
    });

    decide(&mut ctx, &params, json!("x")).unwrap();

    let asked = decider.asked.lock().unwrap();
    assert_eq!(
        asked[0].0.kind,
        DecisionKind::YesNo {
            threshold: 0.8,
            yes: Some("Needs a reply today".into()),
            no: None
        }
    );
}

#[test]
fn structured_values_are_the_state_as_json() {
    let decider = Fixed::answering("billing");
    let mut ctx = context().with_decider(decider.clone());

    decide(
        &mut ctx,
        &routing(json!({ "decider": "system_one" })),
        json!({ "title": "Invoice", "priority": 2 }),
    )
    .unwrap();

    assert_eq!(
        decider.asked.lock().unwrap()[0].1,
        json!({ "title": "Invoice", "priority": 2 })
    );
}

#[test]
fn the_external_decider_is_required_when_the_node_names_it() {
    let mut ctx = context();

    let error = decide(
        &mut ctx,
        &routing(json!({ "decider": "system_one" })),
        json!("x"),
    )
    .unwrap_err();

    assert!(error.contains("system_one"), "{error}");
}

#[test]
fn an_answer_that_the_decision_does_not_allow_fails_the_node() {
    let mut ctx = context().with_decider(Fixed::answering("sales"));

    let error = decide(
        &mut ctx,
        &routing(json!({ "decider": "system_one" })),
        json!("x"),
    )
    .unwrap_err();

    assert!(error.contains("sales"), "{error}");
}

#[test]
fn decision_events_name_the_node_and_never_carry_the_state() {
    let events = Arc::new(Events::default());
    let mut ctx = context()
        .with_decider(Fixed::answering("technical"))
        .install(events.clone());

    decide(
        &mut ctx,
        &routing(json!({ "decider": "system_one" })),
        json!("secret text"),
    )
    .unwrap();

    let seen = events.0.lock().unwrap();
    assert_eq!(
        *seen,
        [
            AgentEvent::DecisionStarting {
                node: "decide".into()
            },
            AgentEvent::DecisionCompleted {
                node: "decide".into(),
                answer: "technical".into(),
                probability: None,
                confidence: Some(0.9),
                model: Some("kev-1".into()),
            }
        ]
    );
    assert!(
        !serde_json::to_string(&*seen)
            .unwrap()
            .contains("secret text")
    );
}

#[test]
fn a_failing_decider_is_reported_and_fails_the_node() {
    let events = Arc::new(Events::default());
    let mut ctx = context()
        .with_decider(Fixed::with(Err(DecideError::Failed(
            "model not found".into(),
        ))))
        .install(events.clone());

    let error = decide(
        &mut ctx,
        &routing(json!({ "decider": "system_one" })),
        json!("x"),
    )
    .unwrap_err();

    assert_eq!(error, "model not found");
    assert_eq!(
        events.0.lock().unwrap()[1],
        AgentEvent::DecisionFailed {
            node: "decide".into(),
            error: "model not found".into()
        }
    );
}

#[test]
fn on_decision_passes_the_value_when_the_latest_answer_is_listed() {
    let mut ctx = context().with_decider(Fixed::answering("technical"));
    let before =
        implementations::condition("on_decision", &json!({ "answers": ["technical"] })).unwrap();
    let other =
        implementations::condition("on_decision", &json!({ "answers": ["billing", "sales"] }))
            .unwrap();

    assert_eq!(
        block_on(before(&ctx, json!("x"))).unwrap(),
        None,
        "no verdict yet"
    );
    decide(
        &mut ctx,
        &routing(json!({ "decider": "system_one" })),
        json!("x"),
    )
    .unwrap();

    assert_eq!(
        block_on(before(&ctx, json!("x"))).unwrap(),
        Some(json!("x"))
    );
    assert_eq!(block_on(other(&ctx, json!("x"))).unwrap(), None);
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Ticket {
    title: String,
}

const PARAMS: &str = r#"{ "value": "Ticket", "kind": "choice", "question": "Which team?",
    "options": [{ "name": "billing" }, { "name": "technical" }], "decider": "system_one" }"#;

#[test]
fn typed_graphs_route_by_the_decision() {
    let graph = TypedGraph::<AgentContext, Ticket, String>::build(|g| {
        let route = g.node("route", nodes::decide::<Ticket>("route", PARAMS));
        let billing = g.node("billing", |_, ticket: Ticket| {
            Box::pin(async move { Ok(format!("billing: {}", ticket.title)) })
        });
        let technical = g.node("technical", |_, ticket: Ticket| {
            Box::pin(async move { Ok(format!("technical: {}", ticket.title)) })
        });
        g.edge(&g.start(), &route);
        g.edge_when(&route, &billing, |ctx, ticket| {
            conditions::on_decision(ctx, &["billing"], ticket)
        });
        g.edge_when(&route, &technical, |ctx, ticket| {
            conditions::on_decision(ctx, &["technical"], ticket)
        });
        g.edge(&billing, &g.finish());
        g.edge(&technical, &g.finish());
    })
    .unwrap();
    let mut ctx = context().with_decider(Fixed::answering("technical"));

    let output = block_on(graph.run(
        &mut ctx,
        Ticket {
            title: "Printer".into(),
        },
    ))
    .unwrap();

    assert_eq!(output, "technical: Printer");
}
