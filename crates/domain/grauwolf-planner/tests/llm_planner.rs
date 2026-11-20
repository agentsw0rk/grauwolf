//! LLM planner (F10.2): plan as a structured answer, steps through a graph.

use std::sync::Arc;

use futures_executor::block_on;
use grauwolf_agents::{AgentContext, implementations};
use grauwolf_graph::{FINISH, Graph, START};
use grauwolf_llm::PromptExecutor;
use grauwolf_planner::{LlmPlanner, run_planner};
use grauwolf_prompt::{LlModel, Message, Prompt};
use grauwolf_test::MockLlmClient;
use serde_json::Value;

/// One step: an LLM request with the step order, text as the result.
fn step_graph() -> Graph<AgentContext> {
    Graph::builder()
        .node(
            "call_llm",
            implementations::node("llm_request", &Value::Null, &Default::default()).unwrap(),
        )
        .edge(START, "call_llm")
        .edge_when(
            "call_llm",
            FINISH,
            implementations::condition("on_assistant_message", &Value::Null).unwrap(),
        )
        .build()
        .unwrap()
}

fn context(mock: MockLlmClient) -> AgentContext {
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(mock))
        .build();
    AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").system("You are a cook.").build(),
    )
}

#[test]
fn builds_a_plan_and_executes_each_step_through_the_step_graph() {
    let mock = MockLlmClient::new("?")
        .answer_when_contains(
            "Create a plan",
            r#"{ "goal": "pancakes", "steps": ["Stir the batter", "Bake"] }"#,
        )
        .answer_when_contains("step 1 of 2: Stir the batter", "The batter is ready.")
        .answer_when_contains("step 2 of 2: Bake", "Three pancakes baked.");
    let mut ctx = context(mock);
    let planner = LlmPlanner::new(Arc::new(step_graph()));

    let output = block_on(run_planner(&planner, &mut ctx, "Make pancakes".into(), 10)).unwrap();

    assert_eq!(output, "Three pancakes baked.");
    // The history contains the system prompt and both steps, but not the planning request.
    let messages = &ctx.prompt().messages;
    assert_eq!(messages[0], Message::system("You are a cook."));
    assert!(
        messages
            .iter()
            .all(|message| !format!("{message:?}").contains("Create a plan")),
        "{messages:?}"
    );
    assert_eq!(
        messages.last(),
        Some(&Message::assistant("Three pancakes baked."))
    );
}

#[test]
fn replans_with_the_reason_when_the_critic_rejects_the_plan() {
    let mock = MockLlmClient::new("?")
        .answer_when_contains(
            "The search was unsuccessful.",
            r#"{ "goal": "Name a number", "steps": ["Guess"] }"#,
        )
        .answer_when_contains(
            "Create a plan",
            r#"{ "goal": "Find the number", "steps": ["Search", "Answer"] }"#,
        )
        .answer_when_contains(
            "Evaluate the plan",
            r#"{ "should_continue": false, "reason": "The search was unsuccessful." }"#,
        )
        .answer_when_contains("step 1 of 2: Suchen", "Nothing found.")
        .answer_when_contains("step 1 of 1: Guess", "42");
    let mut ctx = context(mock);
    let planner = LlmPlanner::new(Arc::new(step_graph())).with_critic();

    let output = block_on(run_planner(&planner, &mut ctx, "Which number?".into(), 10)).unwrap();

    assert_eq!(output, "42");
}

#[test]
fn runs_as_a_node_inside_a_graph() {
    let mock = MockLlmClient::new("?")
        .answer_when_contains(
            "Create a plan",
            r#"{ "goal": "Greeting", "steps": ["Greet"] }"#,
        )
        .answer_when_contains("step 1 of 1: Greet", "Hello!");
    let planner = Arc::new(LlmPlanner::new(Arc::new(step_graph())));
    let graph = grauwolf_graph::TypedGraph::<AgentContext, String, String>::build(|g| {
        let plan = g.node("plan", grauwolf_planner::plan_node(planner, 5, "plan"));
        g.edge(&g.start(), &plan);
        g.edge(&plan, &g.finish());
    })
    .unwrap();

    let output = block_on(graph.run(&mut context(mock), "Greet me".into())).unwrap();

    assert_eq!(output, "Hello!");
}

/// Records entering and leaving.
#[derive(Clone, Default)]
struct Trace(Arc<std::sync::Mutex<Vec<&'static str>>>);

impl grauwolf_graph::Scope<AgentContext> for Trace {
    fn enter(&self, _ctx: &mut AgentContext) {
        self.0.lock().unwrap().push("enter");
    }

    fn exit(&self, _ctx: &mut AgentContext) {
        self.0.lock().unwrap().push("exit");
    }
}

#[test]
fn runs_each_step_inside_the_step_scope() {
    let mock = MockLlmClient::new("?").answer_when_contains(
        "Create a plan",
        r#"{ "goal": "Two", "steps": ["One", "Two"] }"#,
    );
    let trace = Trace::default();
    let planner = LlmPlanner::new(Arc::new(step_graph())).with_step_scope(Box::new(trace.clone()));

    block_on(run_planner(&planner, &mut context(mock), "Count".into(), 5)).unwrap();

    assert_eq!(*trace.0.lock().unwrap(), ["enter", "exit", "enter", "exit"]);
}
