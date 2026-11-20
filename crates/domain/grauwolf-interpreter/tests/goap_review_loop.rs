//! The review loop of `examples/specs/review_loop.json` (addendum to ADR-0070): GOAP with
//! sub-agents as executors, a success check, a failure counter and goal priorities.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use grauwolf_agents::events::{AgentEvent, Feature};
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::PromptExecutor;
use grauwolf_spec::{AgentSpec, AgentSpecExt};
use grauwolf_test::MockLlmClient;

fn example(name: &str) -> AgentSpec {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/specs")
        .join(name);
    AgentSpec::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn agent(
    spec: &str,
    name: &str,
    description: &str,
    mock: MockLlmClient,
) -> Arc<dyn grauwolf_tools::DynTool> {
    let executor = PromptExecutor::builder()
        .client("ollama", Arc::new(mock))
        .build();
    Arc::new(
        Interpreter::new(Arc::new(executor))
            .load(&example(spec))
            .unwrap(),
    )
    .into_tool(name, description)
}

/// Collects the plans of the planner: the first action of each is the one that runs.
#[derive(Default)]
struct Plans(Mutex<Vec<String>>);

impl Feature for Plans {
    fn on_event(&self, event: &AgentEvent) {
        if let AgentEvent::PlanBuilt { plan, .. } = event
            && let Some(first) = plan.get(0).and_then(|action| action.as_str())
        {
            self.0.lock().unwrap().push(first.to_owned());
        }
    }
}

/// `reviewer` answers for the given texts: it approves only the revised draft.
fn run_loop(reviewer: MockLlmClient) -> (String, Vec<String>) {
    let writer = MockLlmClient::new("First draft")
        .answer_when_contains("Revise the text", "Title\nSecond draft");
    let main = MockLlmClient::new("Saved");
    let plans = Arc::new(Plans::default());
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("ollama", Arc::new(main))
            .build(),
    ))
    .with_tools(vec![
        agent("draft_writer.json", "draft_writer", "Writes", writer),
        agent("draft_reviewer.json", "draft_reviewer", "Reviews", reviewer),
    ])
    .with_feature(plans.clone())
    .load(&example("review_loop.json"))
    .unwrap();

    let output = block_on(agent.run("Autumn")).unwrap();

    let steps = plans.0.lock().unwrap().clone();
    (output, steps)
}

#[test]
fn a_rejected_review_is_counted_and_the_draft_revised_until_it_is_approved() {
    let reviewer = MockLlmClient::new("REJECTED: add a title")
        .answer_when_contains("Second draft", "APPROVED");

    let (output, steps) = run_loop(reviewer);

    assert_eq!(output, "Title\nSecond draft");
    assert_eq!(steps, ["write", "review", "fix", "review", "save"]);
}

#[test]
fn the_loop_gives_up_after_three_rejected_reviews_and_delivers_the_last_draft() {
    let (output, steps) = run_loop(MockLlmClient::new("REJECTED: not good enough"));

    assert_eq!(output, "Title\nSecond draft");
    assert_eq!(steps, ["write", "review", "fix", "review", "fix", "review"]);
}
