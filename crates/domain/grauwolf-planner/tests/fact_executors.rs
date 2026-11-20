//! GOAP actions with executors, input templates, success checks and failure effects
//! (addendum to ADR-0070).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::AgentContext;
use grauwolf_graph::{Graph, NodeError, TypedGraph};
use grauwolf_llm::PromptExecutor;
use grauwolf_planner::goap::fact_planner;
use grauwolf_planner::run_planner;
use grauwolf_prompt::{LlModel, Prompt, ToolDescriptor};
use grauwolf_test::MockLlmClient;
use grauwolf_tools::{DynTool, ToolError, ToolRegistry};
use serde_json::{Value, json};

fn context() -> AgentContext {
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(MockLlmClient::new("-")))
        .build();
    AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").build(),
    )
}

type Log = Arc<Mutex<Vec<String>>>;

/// Step graph that records its input in `log` and answers with the next of `answers` (the last
/// one repeats); an answer `{ "error": text }` makes the step fail.
fn step(log: &Log, answers: Vec<Value>) -> Arc<Graph<AgentContext>> {
    let log = Arc::clone(log);
    let calls = Arc::new(Mutex::new(0usize));
    Arc::new(
        TypedGraph::<AgentContext, String, Value>::build(|g| {
            let run = g.node("run", move |_ctx, input: String| {
                log.lock().unwrap().push(input);
                let mut calls = calls.lock().unwrap();
                let answer = answers[(*calls).min(answers.len() - 1)].clone();
                *calls += 1;
                Box::pin(async move {
                    match answer.get("error").and_then(Value::as_str) {
                        Some(error) => Err(NodeError::from(error.to_owned())),
                        None => Ok(answer),
                    }
                })
            });
            g.edge(&g.start(), &run);
            g.edge(&run, &g.finish());
        })
        .unwrap()
        .into_graph(),
    )
}

/// AgentWeaver's review loop: write → review → fix while `review_attempts < 3` → save.
fn review_loop(executor: impl Fn(&str) -> Value) -> Value {
    json!({
        "state": {
            "written": false, "reviewed": false, "approved": false, "saved": false,
            "review_attempts": 0, "task_text": "", "draft": "", "feedback": "", "result": ""
        },
        "input_fact": "task_text",
        "output_fact": "result",
        "actions": [
            { "name": "write", "input": "Write about: {{task_text}}", "output_fact": "draft",
              "pre": { "written": false }, "effects": { "written": true, "reviewed": false }, "executor": executor("write") },
            { "name": "review", "input": "Review:\n{{draft}}", "output_fact": "feedback",
              "pre": { "written": true, "reviewed": false },
              "effects": { "reviewed": true, "approved": true },
              "success_contains": "APPROVED",
              "on_failure": { "reviewed": true, "review_attempts": { "add": 1 } },
              "executor": executor("review") },
            { "name": "fix", "input": "Fix {{draft}} as told: {{feedback}}", "output_fact": "draft",
              "pre": { "reviewed": true, "approved": false, "review_attempts": { "lt": 3 } },
              "effects": { "reviewed": false }, "executor": executor("fix") },
            { "name": "save", "input": "Save {{draft}}", "output_fact": "result",
              "pre": { "approved": true }, "effects": { "saved": true },
              "executor": executor("save") }
        ],
        "goals": [
            { "name": "saved", "facts": { "saved": true }, "priority": 1 },
            { "name": "give_up", "facts": { "reviewed": true, "review_attempts": { "ge": 3 } } }
        ]
    })
}

/// Flattens the `executor` key (`{ "subgraph": … }` or `{ "agent": … }`) into the action.
fn flatten(mut params: Value) -> Value {
    for action in params["actions"].as_array_mut().unwrap() {
        let executor = action["executor"].take();
        for (key, value) in executor.as_object().unwrap() {
            action[key] = value.clone();
        }
        action.as_object_mut().unwrap().remove("executor");
    }
    params
}

fn graphs(log: &Log, reviews: Vec<Value>) -> HashMap<String, Arc<Graph<AgentContext>>> {
    HashMap::from([
        ("write".to_owned(), step(log, vec![json!("Draft 1")])),
        ("review".to_owned(), step(log, reviews)),
        ("fix".to_owned(), step(log, vec![json!("Draft 2")])),
        ("save".to_owned(), step(log, vec![json!("Saved.")])),
    ])
}

fn run(
    params: &Value,
    steps: &HashMap<String, Arc<Graph<AgentContext>>>,
    task: &str,
) -> Result<String, String> {
    let planner = fact_planner(params, steps)?;
    block_on(run_planner(&planner, &mut context(), task.into(), 30))
        .map_err(|error| error.to_string())
}

fn subgraph_review_loop() -> Value {
    flatten(review_loop(|name| json!({ "subgraph": name })))
}

#[test]
fn a_failed_success_check_applies_on_failure_and_the_loop_continues_until_approved() {
    let log = Log::default();
    let reviews = vec![
        json!("REJECTED: too short"),
        json!("REJECTED: no title"),
        json!("APPROVED"),
    ];

    let result = run(&subgraph_review_loop(), &graphs(&log, reviews), "Rust").unwrap();

    assert_eq!(result, "Saved.");
    assert_eq!(
        *log.lock().unwrap(),
        [
            "Write about: Rust",
            "Review:\nDraft 1",
            "Fix Draft 1 as told: REJECTED: too short",
            "Review:\nDraft 2",
            "Fix Draft 2 as told: REJECTED: no title",
            "Review:\nDraft 2",
            "Save Draft 2",
        ]
    );
}

#[test]
fn stops_at_the_other_goal_when_the_attempts_are_used_up() {
    let log = Log::default();

    let result = run(
        &subgraph_review_loop(),
        &graphs(&log, vec![json!("REJECTED")]),
        "Rust",
    )
    .unwrap();

    // Three reviews, two fixes in between; the output is the last answer to `result`
    // (nothing was saved, so the fact is empty).
    assert_eq!(log.lock().unwrap().len(), 6);
    assert_eq!(result, "");
}

#[test]
fn a_failing_executor_counts_as_a_failure_with_on_failure() {
    let log = Log::default();

    run(
        &subgraph_review_loop(),
        &graphs(&log, vec![json!({ "error": "boom" })]),
        "Rust",
    )
    .unwrap();

    let reviews = |input: &&String| input.starts_with("Review");
    assert_eq!(log.lock().unwrap().iter().filter(reviews).count(), 3);
}

#[test]
fn a_failing_executor_without_on_failure_ends_the_run() {
    let mut params = subgraph_review_loop();
    params["actions"][1]
        .as_object_mut()
        .unwrap()
        .remove("on_failure");

    let error = run(
        &params,
        &graphs(&Log::default(), vec![json!({ "error": "boom" })]),
        "Rust",
    )
    .unwrap_err();

    assert!(
        error.contains("review") && error.contains("boom"),
        "{error}"
    );
}

#[test]
fn a_failed_success_check_without_on_failure_ends_the_run() {
    let mut params = subgraph_review_loop();
    params["actions"][1]
        .as_object_mut()
        .unwrap()
        .remove("on_failure");

    let error = run(
        &params,
        &graphs(&Log::default(), vec![json!("REJECTED")]),
        "Rust",
    )
    .unwrap_err();

    assert!(
        error.contains("review") && error.contains("APPROVED"),
        "{error}"
    );
}

#[test]
fn on_success_replaces_the_effects_that_apply_after_the_run() {
    let log = Log::default();
    let mut params = subgraph_review_loop();
    // The review would be approved, but `on_success` says only that it was done: the loop has to
    // review and fix again (until the attempts are used up – the attempt is not counted, so the
    // run is bounded by the iteration limit).
    params["actions"][1]["on_success"] = json!({ "reviewed": true });

    let error = run(&params, &graphs(&log, vec![json!("APPROVED")]), "Rust").unwrap_err();

    assert!(error.contains("30 steps"), "{error}");
}

#[test]
fn refuses_unknown_placeholders_and_executor_mistakes() {
    let steps = graphs(&Log::default(), vec![json!("APPROVED")]);
    let error_of = |edit: &dyn Fn(&mut Value)| {
        let mut params = subgraph_review_loop();
        edit(&mut params);
        fact_planner(&params, &steps).err().expect("refused")
    };

    let unknown = error_of(&|params| params["actions"][0]["input"] = json!("Write {{topic}}"));
    let unclosed = error_of(&|params| params["actions"][0]["input"] = json!("Write {{task"));
    let both = error_of(&|params| params["actions"][0]["agent"] = json!("writer"));
    let neither = error_of(&|params| {
        params["actions"][0]
            .as_object_mut()
            .unwrap()
            .remove("subgraph");
    });
    let output = error_of(&|params| params["actions"][0]["output_fact"] = json!("written"));
    let input = error_of(&|params| params["input_fact"] = json!("approved"));

    assert!(unknown.contains("topic"), "{unknown}");
    assert!(unclosed.contains("{{"), "{unclosed}");
    assert!(
        both.contains("actions[0]") && both.contains("agent"),
        "{both}"
    );
    assert!(neither.contains("actions[0]"), "{neither}");
    assert!(output.contains("written"), "{output}");
    assert!(input.contains("approved"), "{input}");
}

/// A sub-agent that answers with the text of its request, upper-cased, prefixed by its name.
struct Agent(&'static str, Log);

impl DynTool for Agent {
    fn name(&self) -> &str {
        self.0
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: self.0.into(),
            description: "Agent".into(),
            parameters: json!({}),
        }
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        self.1
            .lock()
            .unwrap()
            .push(args["request"].as_str().unwrap().to_owned());
        let answer = match self.0 {
            "reviewer" => "APPROVED".to_owned(),
            name => format!("{name} done"),
        };
        Box::pin(async move { Ok(answer) })
    }
}

#[test]
fn runs_actions_with_an_agent_as_executor() {
    let log = Log::default();
    let params = flatten(review_loop(|name| match name {
        "write" => json!({ "agent": "writer" }),
        "review" => json!({ "agent": "reviewer" }),
        _ => json!({ "agent": "saver" }),
    }));
    let mut ctx = context().with_tools(
        ToolRegistry::builder()
            .tool(Agent("writer", Arc::clone(&log)))
            .tool(Agent("reviewer", Arc::clone(&log)))
            .tool(Agent("saver", Arc::clone(&log)))
            .build(),
    );
    let planner = fact_planner(&params, &HashMap::new()).unwrap();

    let result = block_on(run_planner(&planner, &mut ctx, "Rust".into(), 30)).unwrap();

    assert_eq!(result, "saver done");
    assert_eq!(
        *log.lock().unwrap(),
        [
            "Write about: Rust",
            "Review:\nwriter done",
            "Save writer done"
        ]
    );
}

#[test]
fn without_an_input_template_the_executor_gets_the_task_and_the_action() {
    let log = Log::default();
    let mut params = subgraph_review_loop();
    params["actions"][0]
        .as_object_mut()
        .unwrap()
        .remove("input");
    params["actions"][0]["description"] = json!("Write it.");

    run(&params, &graphs(&log, vec![json!("APPROVED")]), "Rust").unwrap();

    assert_eq!(log.lock().unwrap()[0], "Rust\n\nAction: write\nWrite it.");
}

/// A context whose history is already over `max_prompt_tokens`, so the run budget is exhausted.
fn exhausted_context() -> AgentContext {
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(MockLlmClient::new("-")))
        .build();
    AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p")
            .user("a history that is far longer than the single token the budget allows")
            .build(),
    )
    .with_budget(grauwolf_agents::Budget {
        max_prompt_tokens: Some(1),
        ..Default::default()
    })
}

#[test]
fn an_exhausted_run_budget_ends_the_run_even_with_on_failure() {
    let log = Log::default();
    let planner = fact_planner(
        &subgraph_review_loop(),
        &graphs(&log, vec![json!({ "error": "the model refused: budget" })]),
    )
    .unwrap();

    let error = block_on(run_planner(
        &planner,
        &mut exhausted_context(),
        "Rust".into(),
        30,
    ))
    .unwrap_err()
    .to_string();

    assert!(error.contains("run budget exhausted"), "{error}");
    let reviews = |input: &&String| input.starts_with("Review");
    assert_eq!(log.lock().unwrap().iter().filter(reviews).count(), 1);
}
