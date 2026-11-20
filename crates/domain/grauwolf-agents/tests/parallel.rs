//! Parallel branches with a merge strategy (F05.11, ADR-0045).

use std::sync::Arc;

use futures_executor::block_on;
use grauwolf_agents::AgentContext;
use grauwolf_agents::parallel::{Branch, Merge, run_parallel};
use grauwolf_graph::TypedGraph;
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::{LlModel, Message, Prompt};
use serde_json::json;

fn context() -> AgentContext {
    AgentContext::new(
        Arc::new(PromptExecutor::builder().build()),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("Question").build(),
    )
}

/// Branch that writes its name into its own history and returns `answer`.
fn branch(name: &'static str, answer: &'static str) -> Branch {
    let graph = TypedGraph::<AgentContext, String, String>::build(|g| {
        let work = g.node("work", move |ctx: &mut AgentContext, _input: String| {
            ctx.append(Message::user(name));
            ctx.store("branch", &name).unwrap();
            Box::pin(async move { Ok(answer.to_owned()) })
        });
        g.edge(&g.start(), &work);
        g.edge(&work, &g.finish());
    })
    .unwrap();
    Branch::new(name, graph.into_graph())
}

#[test]
fn concat_joins_text_results_and_keeps_the_callers_history() {
    let branches = [branch("short", "A"), branch("long", "BB")];
    let mut ctx = context();

    let output = block_on(run_parallel(
        &mut ctx,
        &branches,
        json!("los"),
        &Merge::Concat,
    ))
    .unwrap();

    assert_eq!(output, json!("## short\nA\n\n## long\nBB"));
    assert_eq!(ctx.prompt().messages, [Message::user("Question")]);
}

#[test]
fn index_adopts_result_and_context_of_one_branch() {
    let branches = [branch("short", "A"), branch("long", "BB")];
    let mut ctx = context();

    let output = block_on(run_parallel(
        &mut ctx,
        &branches,
        json!("los"),
        &Merge::Index(1),
    ))
    .unwrap();

    assert_eq!(output, json!("BB"));
    assert_eq!(
        ctx.prompt().messages,
        [Message::user("Question"), Message::user("long")]
    );
    assert_eq!(ctx.load::<String>("branch").unwrap(), Some("long".into()));
}

/// Answers the evaluation (prompt `best_of`) with branch 1.
struct Judge;

impl grauwolf_llm::LlmClient for Judge {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [grauwolf_prompt::ToolDescriptor],
    ) -> futures_util::future::BoxFuture<'a, Result<Message, grauwolf_llm::LlmError>> {
        let text = format!("{:?}", prompt.messages);
        assert!(
            text.contains("Answer 2:\\nBB") && text.contains("The longest"),
            "{text}"
        );
        Box::pin(async { Ok(Message::assistant(r#"{ "best": 2 }"#)) })
    }
}

#[test]
fn llm_select_lets_a_judge_pick_the_branch() {
    let branches = [branch("short", "A"), branch("long", "BB")];
    let mut ctx = AgentContext::new(
        Arc::new(
            PromptExecutor::builder()
                .client("mock", Arc::new(Judge))
                .build(),
        ),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("Question").build(),
    );
    let merge = Merge::LlmSelect {
        criteria: "The longest".into(),
    };

    let output = block_on(run_parallel(&mut ctx, &branches, json!("los"), &merge)).unwrap();

    assert_eq!(output, json!("BB"));
    assert_eq!(ctx.prompt().messages.last(), Some(&Message::user("long")));
}

#[test]
fn resumes_only_the_branches_that_did_not_finish() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let runs = Arc::new(AtomicUsize::new(0));
    let broken = Arc::new(AtomicBool::new(true));
    let counted = {
        let runs = Arc::clone(&runs);
        TypedGraph::<AgentContext, String, String>::build(move |g| {
            let work = g.node("work", move |ctx: &mut AgentContext, _input: String| {
                runs.fetch_add(1, Ordering::Relaxed);
                ctx.append(Message::user("stable"));
                Box::pin(async { Ok("A".to_owned()) })
            });
            g.edge(&g.start(), &work);
            g.edge(&work, &g.finish());
        })
        .unwrap()
    };
    let flaky = {
        let broken = Arc::clone(&broken);
        TypedGraph::<AgentContext, String, String>::build(move |g| {
            let work = g.node("work", move |_ctx: &mut AgentContext, _input: String| {
                let failed = broken.load(Ordering::Relaxed);
                Box::pin(async move {
                    if failed {
                        Err("Network down".into())
                    } else {
                        Ok("BB".to_owned())
                    }
                })
            });
            g.edge(&g.start(), &work);
            g.edge(&work, &g.finish());
        })
        .unwrap()
    };
    let branches = [
        Branch::new("stable", counted.into_graph()),
        Branch::new("wobbly", flaky.into_graph()),
    ];
    let mut ctx = context();

    let crashed = block_on(grauwolf_agents::parallel::run_parallel_resumable(
        &mut ctx,
        &branches,
        json!("los"),
        &Merge::Index(0),
        "grauwolf.parallel.views",
    ));
    broken.store(false, Ordering::Relaxed);
    let resumed = block_on(grauwolf_agents::parallel::run_parallel_resumable(
        &mut ctx,
        &branches,
        json!("los"),
        &Merge::Index(0),
        "grauwolf.parallel.views",
    ))
    .unwrap();

    assert!(crashed.is_err());
    assert_eq!(resumed, json!("A"));
    assert_eq!(
        runs.load(Ordering::Relaxed),
        1,
        "a finished branch was run again"
    );
    // The saved context of the finished branch is adopted, without the progress.
    assert_eq!(ctx.prompt().messages.last(), Some(&Message::user("stable")));
    assert_eq!(
        ctx.load::<serde_json::Value>("grauwolf.parallel.views")
            .unwrap(),
        None
    );
}
