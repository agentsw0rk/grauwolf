//! The model as a decider (ADR-0100): structured requests with the allowed answers.

use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use grauwolf_agents::AgentContext;
use grauwolf_agents::decision::{ChoiceOption, Decision, DecisionKind};
use grauwolf_agents::events::{AgentEvent, Feature};
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::{LlModel, Message, Prompt};
use grauwolf_test::MockLlmClient;
use serde_json::json;

fn context(mock: MockLlmClient) -> AgentContext {
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(mock))
        .client(
            "other",
            Arc::new(MockLlmClient::new(r#"{ "answer": "technical" }"#)),
        )
        .build();
    AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").system("Be helpful.").build(),
    )
}

fn routing() -> Decision {
    Decision {
        kind: DecisionKind::Choice {
            options: vec![
                ChoiceOption {
                    name: "billing".into(),
                    description: Some("Invoices and payments".into()),
                },
                ChoiceOption {
                    name: "technical".into(),
                    description: None,
                },
            ],
        },
        question: "Which team handles the ticket?".into(),
        model: None,
        temperature: None,
        think: None,
    }
}

#[test]
fn a_choice_is_the_option_the_model_names() {
    let ctx = context(
        MockLlmClient::new("?").answer_when_contains("wrong invoice", r#"{ "answer": "billing" }"#),
    );

    let verdict =
        block_on(ctx.decide_with_model(&routing(), &json!("I got a wrong invoice"))).unwrap();

    assert_eq!(verdict.answer, "billing");
    assert_eq!(verdict.model.as_deref(), Some("m"));
}

#[test]
fn the_request_carries_the_question_the_options_and_the_state() {
    let ctx = context(
        MockLlmClient::new("?")
            .answer_when_contains("Invoices and payments", r#"{ "answer": "billing" }"#),
    );

    let verdict = block_on(ctx.decide_with_model(&routing(), &json!({ "text": "Hello" }))).unwrap();

    assert_eq!(
        verdict.answer, "billing",
        "options with description in the request"
    );
    let ctx = context(MockLlmClient::new("?").answer_when_contains(
        "Which team handles the ticket?",
        r#"{ "answer": "technical" }"#,
    ));
    let verdict = block_on(ctx.decide_with_model(&routing(), &json!("x"))).unwrap();
    assert_eq!(verdict.answer, "technical", "question in the request");
    let ctx = context(
        MockLlmClient::new("?")
            .answer_when_contains("\"text\":\"Hello\"", r#"{ "answer": "billing" }"#),
    );
    let verdict = block_on(ctx.decide_with_model(&routing(), &json!({ "text": "Hello" }))).unwrap();
    assert_eq!(verdict.answer, "billing", "other values as JSON");
}

#[test]
fn an_answer_outside_the_options_is_an_error() {
    let ctx = context(MockLlmClient::new(r#"{ "answer": "sales" }"#));

    let error = block_on(ctx.decide_with_model(&routing(), &json!("x"))).unwrap_err();

    assert!(error.to_string().contains("sales"), "{error}");
}

#[test]
fn a_yes_no_decision_applies_the_threshold_to_the_probability() {
    let ctx = context(MockLlmClient::new(r#"{ "probability": 0.6 }"#));
    let decision = |threshold| Decision {
        kind: DecisionKind::YesNo {
            threshold,
            yes: None,
            no: None,
        },
        question: "Is it urgent?".into(),
        model: None,
        temperature: None,
        think: None,
    };

    let low = block_on(ctx.decide_with_model(&decision(0.5), &json!("x"))).unwrap();
    let high = block_on(ctx.decide_with_model(&decision(0.8), &json!("x"))).unwrap();

    assert_eq!((low.answer.as_str(), low.probability), ("yes", Some(0.6)));
    assert_eq!((high.answer.as_str(), high.probability), ("no", Some(0.6)));
}

#[test]
fn a_score_is_the_level_the_model_names() {
    let ctx = context(MockLlmClient::new(r#"{ "answer": "high" }"#));
    let decision = Decision {
        kind: DecisionKind::Score {
            levels: vec!["low".into(), "high".into()],
        },
        question: "How urgent?".into(),
        model: None,
        temperature: None,
        think: None,
    };

    let verdict = block_on(ctx.decide_with_model(&decision, &json!("x"))).unwrap();

    assert_eq!(verdict.answer, "high");
}

#[test]
fn the_decision_model_replaces_the_active_model_and_the_history_stays_unchanged() {
    let ctx = context(MockLlmClient::new(r#"{ "answer": "billing" }"#));
    let decision = Decision {
        model: Some("other/big".into()),
        ..routing()
    };

    let verdict = block_on(ctx.decide_with_model(&decision, &json!("x"))).unwrap();

    assert_eq!(
        verdict.answer, "technical",
        "answered by the other provider"
    );
    assert_eq!(verdict.model.as_deref(), Some("big"));
    assert_eq!(ctx.prompt().messages.len(), 1);
    assert!(matches!(ctx.prompt().messages[0], Message::System { .. }));
}

#[derive(Default)]
struct Events(Mutex<Vec<AgentEvent>>);

impl Feature for Events {
    fn on_event(&self, event: &AgentEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

#[test]
fn the_request_is_reported_as_an_llm_call() {
    let events = Arc::new(Events::default());
    let ctx = context(MockLlmClient::new(r#"{ "answer": "billing" }"#)).install(events.clone());

    block_on(ctx.decide_with_model(&routing(), &json!("x"))).unwrap();

    let seen = events.0.lock().unwrap();
    assert!(matches!(seen[0], AgentEvent::LlmCallStarting { .. }));
    assert!(matches!(seen[1], AgentEvent::LlmCallCompleted { .. }));
}

#[test]
fn the_request_uses_the_temperature_and_think_of_the_decision() {
    let events = Arc::new(Events::default());
    let ctx = context(MockLlmClient::new(r#"{ "answer": "billing" }"#)).install(events.clone());
    let decision = Decision {
        temperature: Some(0.0),
        think: Some(false),
        ..routing()
    };

    block_on(ctx.decide_with_model(&decision, &json!("x"))).unwrap();

    let seen = events.0.lock().unwrap();
    let AgentEvent::LlmCallStarting { model, .. } = &seen[0] else {
        panic!("expected llm_call_starting");
    };
    assert_eq!(model.temperature, Some(0.0));
    assert_eq!(model.think, Some(false));
    assert_eq!(ctx.model().temperature, None, "the active model stays");
}
