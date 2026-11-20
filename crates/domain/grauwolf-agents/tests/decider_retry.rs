//! Transient decider failures are retried with the retry policy (ADR-0026, ADR-0100).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::decision::{
    DecideError, Decider, Decision, DecisionKind, RetryingDecider, Verdict,
};
use grauwolf_llm::{Clock, RetryPolicy};
use serde_json::{Value, json};

/// Clock that does not wait but records the wait times.
#[derive(Default)]
struct RecordingClock(Mutex<Vec<Duration>>);

impl Clock for RecordingClock {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        self.0.lock().unwrap().push(duration);
        Box::pin(async {})
    }
}

/// Decider that returns the given errors in turn and then answers `yes`.
struct Flaky {
    errors: Mutex<Vec<DecideError>>,
    asked: Mutex<usize>,
}

impl Flaky {
    fn failing(errors: Vec<DecideError>) -> Arc<Self> {
        Arc::new(Self {
            errors: Mutex::new(errors.into_iter().rev().collect()),
            asked: Mutex::new(0),
        })
    }
}

impl Decider for Flaky {
    fn decide<'a>(
        &'a self,
        _decision: &'a Decision,
        _state: &'a Value,
    ) -> BoxFuture<'a, Result<Verdict, DecideError>> {
        *self.asked.lock().unwrap() += 1;
        let next = self.errors.lock().unwrap().pop();
        Box::pin(std::future::ready(match next {
            Some(error) => Err(error),
            None => Ok(Verdict::new("yes")),
        }))
    }
}

fn decision() -> Decision {
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

fn busy() -> DecideError {
    DecideError::Transient("busy".into())
}

fn retrying(inner: &Arc<Flaky>) -> (RetryingDecider, Arc<RecordingClock>) {
    let clock = Arc::new(RecordingClock::default());
    let decider = RetryingDecider::new(
        inner.clone(),
        RetryPolicy::default(),
        clock.clone() as Arc<dyn Clock>,
    );
    (decider, clock)
}

#[test]
fn a_transient_failure_is_asked_again_after_the_waits_of_the_policy() {
    let inner = Flaky::failing(vec![busy(), busy()]);
    let (decider, clock) = retrying(&inner);

    let verdict = block_on(decider.decide(&decision(), &json!("x"))).unwrap();

    assert_eq!(verdict.answer, "yes");
    assert_eq!(*inner.asked.lock().unwrap(), 3);
    assert_eq!(
        *clock.0.lock().unwrap(),
        [Duration::from_secs(1), Duration::from_secs(2)]
    );
}

#[test]
fn it_gives_up_after_the_retries_of_the_policy() {
    let inner = Flaky::failing(vec![busy(); 10]);
    let (decider, _) = retrying(&inner);

    let error = block_on(decider.decide(&decision(), &json!("x"))).unwrap_err();

    assert!(error.is_transient());
    assert_eq!(
        *inner.asked.lock().unwrap(),
        4,
        "first try plus three retries"
    );
}

#[test]
fn a_permanent_failure_is_not_retried() {
    let inner = Flaky::failing(vec![DecideError::Failed("bad request".into())]);
    let (decider, clock) = retrying(&inner);

    let error = block_on(decider.decide(&decision(), &json!("x"))).unwrap_err();

    assert_eq!(error, DecideError::Failed("bad request".into()));
    assert_eq!(*inner.asked.lock().unwrap(), 1);
    assert!(clock.0.lock().unwrap().is_empty());
}
