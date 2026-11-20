//! Retry policy for LLM requests (F02.11, ADR-0026).

use std::time::Duration;

use grauwolf_llm::RetryPolicy;

#[test]
fn backs_off_exponentially_up_to_the_limit_and_stops_after_max_retries() {
    let policy = RetryPolicy {
        max_retries: 4,
        initial_delay: Duration::from_secs(1),
        max_delay: Duration::from_secs(5),
    };

    let delays: Vec<Option<Duration>> = (0..5)
        .map(|attempt| policy.next_delay(attempt, None))
        .collect();

    assert_eq!(
        delays,
        [
            Some(Duration::from_secs(1)),
            Some(Duration::from_secs(2)),
            Some(Duration::from_secs(4)),
            Some(Duration::from_secs(5)),
            None,
        ]
    );
}

#[test]
fn prefers_the_delay_requested_by_the_server_within_the_limit() {
    let policy = RetryPolicy::default();

    assert_eq!(
        policy.next_delay(0, Some(Duration::from_secs(7))),
        Some(Duration::from_secs(7))
    );
    assert_eq!(
        policy.next_delay(0, Some(Duration::from_secs(3600))),
        Some(policy.max_delay)
    );
}

use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_llm::{Clock, LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};

/// Clock that does not wait but records the wait times.
#[derive(Default)]
struct RecordingClock(Mutex<Vec<Duration>>);

impl Clock for RecordingClock {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        self.0.lock().unwrap().push(duration);
        Box::pin(async {})
    }
}

/// Client that first returns the given errors and then answers.
struct FlakyClient(Mutex<Vec<LlmError>>);

impl LlmClient for FlakyClient {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        let next = self.0.lock().unwrap().pop();
        Box::pin(async move {
            match next {
                Some(error) => Err(error),
                None => Ok(Message::assistant("finally")),
            }
        })
    }
}

fn transient(retry_after: Option<Duration>) -> LlmError {
    LlmError::Transient {
        source: "overloaded".into(),
        retry_after,
    }
}

fn executor(errors: Vec<LlmError>, clock: Arc<RecordingClock>) -> PromptExecutor {
    PromptExecutor::builder()
        .client("p", Arc::new(FlakyClient(Mutex::new(errors))))
        .retry(RetryPolicy::default(), clock)
        .build()
}

#[test]
fn retries_transient_errors_and_waits_on_the_clock() {
    let clock = Arc::new(RecordingClock::default());
    // `pop` takes from the end: first without a hint, then with 7 s.
    let executor = executor(
        vec![transient(Some(Duration::from_secs(7))), transient(None)],
        clock.clone(),
    );

    let answer = block_on(executor.execute(
        &Prompt::builder("x").user("Hello").build(),
        &LlModel::new("p", "m"),
        &[],
    ))
    .unwrap();

    assert_eq!(answer, Message::assistant("finally"));
    assert_eq!(
        *clock.0.lock().unwrap(),
        [Duration::from_secs(1), Duration::from_secs(7)]
    );
}

#[test]
fn does_not_retry_other_errors() {
    let clock = Arc::new(RecordingClock::default());
    let executor = executor(vec![LlmError::Client("invalid".into())], clock.clone());

    let error = block_on(executor.execute(
        &Prompt::builder("x").user("Hello").build(),
        &LlModel::new("p", "m"),
        &[],
    ))
    .unwrap_err();

    assert!(matches!(error, LlmError::Client(_)), "{error}");
    assert!(clock.0.lock().unwrap().is_empty());
}

#[test]
fn retries_streams_that_fail_before_the_first_frame() {
    use futures_util::StreamExt;

    let clock = Arc::new(RecordingClock::default());
    let executor = executor(vec![transient(None)], clock.clone());
    let prompt = Prompt::builder("x").user("Hello").build();
    let model = LlModel::new("p", "m");

    let frames: Vec<_> = block_on(
        executor
            .execute_streaming(&prompt, &model, &[])
            .map(Result::unwrap)
            .collect(),
    );

    assert_eq!(
        grauwolf_llm::aggregate(frames),
        Message::assistant("finally")
    );
    assert_eq!(*clock.0.lock().unwrap(), [Duration::from_secs(1)]);
}
