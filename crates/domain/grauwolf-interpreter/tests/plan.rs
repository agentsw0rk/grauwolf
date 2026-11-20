//! Nodes of kind `plan` (ADR-0039): planner with critic, steps in the subgraph.

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;
use std::sync::Arc;

use futures_executor::block_on;
use grauwolf_interpreter::Interpreter;
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
fn runs_the_planner_spec_step_by_step() {
    let mock = MockLlmClient::new("?")
        .answer_when_contains(
            "Create a plan",
            r#"{ "goal": "Trip", "steps": ["Choose destination", "Give the answer"] }"#,
        )
        .answer_when_contains(
            "Evaluate the plan",
            r#"{ "should_continue": true, "reason": "Fits." }"#,
        )
        .answer_when_contains("step 1 of 2: Choose destination", "Rügen")
        .answer_when_contains("step 2 of 2: Give the answer", "Head to Rügen.");
    let executor = PromptExecutor::builder()
        .client("ollama", Arc::new(mock))
        .build();
    let agent = Interpreter::new(Arc::new(executor))
        .load(&example("planner.json"))
        .unwrap();

    let output = block_on(agent.run("Where to go on vacation?")).unwrap();

    assert_eq!(output, "Head to Rügen.");
}

/// Answers like the planner mock, but fails at step 2 while `broken` is set,
/// and counts the executions of step 1.
struct Flaky {
    broken: std::sync::atomic::AtomicBool,
    first_step: std::sync::atomic::AtomicUsize,
}

impl grauwolf_llm::LlmClient for Flaky {
    fn execute<'a>(
        &'a self,
        prompt: &'a grauwolf_prompt::Prompt,
        _model: &'a grauwolf_prompt::LlModel,
        _tools: &'a [grauwolf_prompt::ToolDescriptor],
    ) -> futures_util::future::BoxFuture<'a, Result<grauwolf_prompt::Message, grauwolf_llm::LlmError>>
    {
        use std::sync::atomic::Ordering;
        let last = match prompt.messages.last() {
            Some(grauwolf_prompt::Message::User { content, .. }) => content.clone(),
            _ => String::new(),
        };
        let all = format!("{:?}", prompt.messages);
        let answer = if all.contains("Create a plan") {
            Ok(
                r#"{ "goal": "Trip", "steps": ["Choose destination", "Give the answer"] }"#
                    .to_owned(),
            )
        } else if all.contains("Evaluate the plan") {
            Ok(r#"{ "should_continue": true, "reason": "Fits." }"#.to_owned())
        } else if last.contains("step 1 of 2") {
            self.first_step.fetch_add(1, Ordering::Relaxed);
            Ok("Rügen".to_owned())
        } else if last.contains("step 2 of 2") && self.broken.load(Ordering::Relaxed) {
            Err(grauwolf_llm::LlmError::Client("Network down".into()))
        } else {
            Ok("Head to Rügen.".to_owned())
        };
        Box::pin(async move { answer.map(grauwolf_prompt::Message::assistant) })
    }
}

#[test]
fn resumes_a_plan_after_the_last_finished_step() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let client = Arc::new(Flaky {
        broken: AtomicBool::new(true),
        first_step: AtomicUsize::new(0),
    });
    let executor = PromptExecutor::builder()
        .client("ollama", client.clone())
        .build();
    let agent = Interpreter::new(Arc::new(executor))
        .load(&example("planner.json"))
        .unwrap();
    let store: Arc<dyn grauwolf_agents::checkpoints::CheckpointStore> =
        Arc::new(grauwolf_agents::checkpoints::MemoryCheckpointStore::default());

    let crashed =
        block_on(agent.run_checkpointed("Where to go on vacation?".into(), store.clone(), "reise"));
    client.broken.store(false, Ordering::Relaxed);
    let resumed = block_on(agent.resume(store, "reise")).unwrap();

    assert!(crashed.is_err());
    assert_eq!(resumed, serde_json::json!("Head to Rügen."));
    assert_eq!(
        client.first_step.load(Ordering::Relaxed),
        1,
        "Step 1 was repeated"
    );
}
