//! Decision types and answer rules (ADR-0100).

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::decision::{
    ChoiceOption, DecideError, Decider, Decision, DecisionKind, Verdict,
};
use serde_json::{Value, json};

fn decision(kind: DecisionKind) -> Decision {
    Decision {
        kind,
        question: "How urgent is the ticket?".into(),
        model: None,
        temperature: None,
        think: None,
    }
}

fn levels() -> DecisionKind {
    DecisionKind::Score {
        levels: vec!["low".into(), "medium".into(), "high".into()],
    }
}

#[test]
fn yes_no_answers_yes_when_the_probability_reaches_the_threshold() {
    let kind = DecisionKind::YesNo {
        threshold: 0.7,
        yes: None,
        no: None,
    };

    assert_eq!(kind.answer_for_probability(0.7), "yes");
    assert_eq!(kind.answer_for_probability(0.9), "yes");
    assert_eq!(kind.answer_for_probability(0.69), "no");
}

#[test]
fn score_rounds_the_mean_index_and_clamps_it_to_the_scale() {
    let kind = levels();

    assert_eq!(kind.answer_for_score(0.4), "low");
    assert_eq!(kind.answer_for_score(0.5), "medium");
    assert_eq!(kind.answer_for_score(1.6), "high");
    assert_eq!(kind.answer_for_score(7.0), "high");
    assert_eq!(kind.answer_for_score(-3.0), "low");
}

#[test]
fn the_possible_answers_follow_the_kind() {
    let choice = DecisionKind::Choice {
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
    };
    let yes_no = DecisionKind::YesNo {
        threshold: 0.5,
        yes: None,
        no: None,
    };

    assert_eq!(choice.answers(), ["billing", "technical"]);
    assert_eq!(yes_no.answers(), ["yes", "no"]);
    assert_eq!(levels().answers(), ["low", "medium", "high"]);
}

#[test]
fn a_verdict_of_a_choice_must_name_one_of_the_options() {
    let decision = decision(DecisionKind::Choice {
        options: vec![ChoiceOption {
            name: "billing".into(),
            description: None,
        }],
    });

    let verdict = Verdict::new("billing");

    assert!(decision.accepts(&verdict));
    assert!(!decision.accepts(&Verdict::new("sales")));
}

struct Fixed(Result<Verdict, DecideError>);

impl Decider for Fixed {
    fn decide<'a>(
        &'a self,
        _decision: &'a Decision,
        _state: &'a Value,
    ) -> BoxFuture<'a, Result<Verdict, DecideError>> {
        Box::pin(std::future::ready(self.0.clone()))
    }
}

#[test]
fn a_decider_is_used_through_the_port_and_reports_whether_a_failure_is_transient() {
    let decision = decision(levels());
    let state = json!("The printer is on fire");

    let answered = block_on(Fixed(Ok(Verdict::new("high"))).decide(&decision, &state));
    let busy =
        block_on(Fixed(Err(DecideError::Transient("busy".into()))).decide(&decision, &state));

    assert_eq!(answered.unwrap().answer, "high");
    assert!(busy.unwrap_err().is_transient());
    assert!(!DecideError::Failed("bad request".into()).is_transient());
}
