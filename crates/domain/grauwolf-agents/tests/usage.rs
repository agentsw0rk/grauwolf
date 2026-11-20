//! Token usage in events (ADR-0067).

use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::AgentContext;
use grauwolf_agents::events::{AgentEvent, Feature};
use grauwolf_llm::{Completion, LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor, Usage};

const USAGE: Usage = Usage {
    input_tokens: 120,
    output_tokens: 7,
    cache_read_tokens: Some(100),
    cache_write_tokens: Some(0),
};

/// Reports the usage of every answer.
struct Counting;

impl LlmClient for Counting {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async { Ok(Message::assistant("Hello")) })
    }

    fn execute_with_usage<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async move {
            Ok(Completion {
                model: None,
                message: self.execute(prompt, model, tools).await?,
                usage: Some(USAGE),
            })
        })
    }
    fn execute_structured_with_usage<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _output: &'a grauwolf_llm::StructuredOutput,
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async {
            Ok(Completion {
                model: None,
                message: Message::assistant(r#"{ "ok": true }"#),
                usage: Some(USAGE),
            })
        })
    }
}

#[derive(Default)]
struct Recorder(Mutex<Vec<AgentEvent>>);

impl Feature for Recorder {
    fn on_event(&self, event: &AgentEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

#[test]
fn reports_the_token_usage_of_a_request() {
    let recorder = Arc::new(Recorder::default());
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(Counting))
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("Hello").build(),
    )
    .install(recorder.clone());

    block_on(ctx.request_llm()).unwrap();

    let events = recorder.0.lock().unwrap();
    assert_eq!(
        events.last(),
        Some(&AgentEvent::LlmCallCompleted {
            answer: Message::assistant("Hello"),
            usage: Some(USAGE),
            cost: None,
            model: None
        })
    );
    let json = serde_json::to_value(events.last().unwrap()).unwrap();
    assert_eq!(json["usage"]["cache_read_tokens"], 100);
}

/// Sum per run (ADR-0067): including branches that are not adopted.
#[test]
fn sums_the_usage_of_all_requests_including_forks() {
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(Counting))
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("Hello").build(),
    );

    assert_eq!(ctx.usage(), None);
    block_on(ctx.request_llm()).unwrap();
    let mut branch = ctx.fork();
    block_on(branch.request_llm()).unwrap();
    drop(branch);

    assert_eq!(
        ctx.usage(),
        Some(Usage {
            input_tokens: 240,
            output_tokens: 14,
            cache_read_tokens: Some(200),
            cache_write_tokens: Some(0),
        })
    );
}

/// Structured answers report their usage as well (ADR-0067).
#[test]
fn reports_the_token_usage_of_a_structured_request() {
    let recorder = Arc::new(Recorder::default());
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(Counting))
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("Hello").build(),
    )
    .install(recorder.clone());
    let output = grauwolf_llm::StructuredOutput {
        name: "Ok".into(),
        schema: serde_json::json!({
            "type": "object", "properties": { "ok": { "type": "boolean" } }, "required": ["ok"]
        }),
    };

    block_on(ctx.request_llm_structured(&output, 0)).unwrap();

    assert_eq!(ctx.usage(), Some(USAGE));
}

/// Streaming reports the usage as its own frame (ADR-0067); the default path without real
/// streaming passes on the usage of `execute_with_usage`.
#[test]
fn reports_the_token_usage_of_a_streamed_request() {
    let frames = Arc::new(Mutex::new(Vec::new()));
    let sink_frames = Arc::clone(&frames);
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(Counting))
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("Hello").build(),
    )
    .with_frame_sink(Arc::new(move |frame: &grauwolf_llm::StreamFrame| {
        sink_frames.lock().unwrap().push(frame.clone());
    }));

    let answer = block_on(ctx.request_llm_streaming()).unwrap();

    assert_eq!(answer, Message::assistant("Hello"));
    assert_eq!(ctx.usage(), Some(USAGE));
    assert!(
        frames
            .lock()
            .unwrap()
            .contains(&grauwolf_llm::StreamFrame::Usage(USAGE))
    );
}

/// Costs (ADR-0071): per call in the event, and as the sum of the context including forks.
#[test]
fn reports_the_cost_of_requests_with_a_price_table() {
    let recorder = Arc::new(Recorder::default());
    let prices: grauwolf_prompt::Prices = serde_json::from_value(serde_json::json!({
        "currency": "USD",
        "models": { "mock:m": { "input": 1.0, "output": 10.0, "cache_read": 0.1 } }
    }))
    .unwrap();
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(Counting))
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("Hello").build(),
    )
    .with_prices(Arc::new(prices))
    .install(recorder.clone());

    block_on(ctx.request_llm()).unwrap();
    let mut branch = ctx.fork();
    block_on(branch.request_llm()).unwrap();

    // Per call: 20 uncached × 1 + 100 read × 0.1 + 0 written × 1 + 7 × 10, per million.
    let per_call = (20.0 + 10.0 + 70.0) / 1e6;
    let events = recorder.0.lock().unwrap();
    let AgentEvent::LlmCallCompleted { cost, .. } = events.last().unwrap() else {
        panic!("{events:?}");
    };
    assert!((cost.unwrap() - per_call).abs() < 1e-12);
    assert!((ctx.cost().unwrap() - 2.0 * per_call).abs() < 1e-12);
}

/// Budget (ADR-0072): once the limit is reached, no further request starts, not even in
/// a fork.
#[test]
fn stops_requesting_once_the_budget_is_used_up() {
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(Counting))
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("Hello").build(),
    )
    .with_budget(grauwolf_agents::Budget {
        max_tokens: Some(127),
        max_cost: None,
        max_tool_calls: None,
        max_prompt_tokens: None,
    });

    // The first call uses 120 + 7 = 127 tokens and thereby reaches the limit.
    block_on(ctx.request_llm()).unwrap();
    let second = block_on(ctx.request_llm());
    let in_branch = block_on(ctx.fork().request_llm());

    assert!(
        matches!(&second, Err(LlmError::BudgetExceeded { .. })),
        "{second:?}"
    );
    assert!(second.unwrap_err().to_string().contains("127"));
    assert!(matches!(in_branch, Err(LlmError::BudgetExceeded { .. })));
    assert_eq!(ctx.usage().unwrap().input_tokens, 120);
}

/// Budget (ADR-0072): applies to every kind of request, not only ordinary requests.
#[test]
fn checks_the_budget_before_every_kind_of_request() {
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(Counting))
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("Hello").build(),
    )
    .with_budget(grauwolf_agents::Budget {
        max_tokens: Some(1),
        max_cost: None,
        max_tool_calls: None,
        max_prompt_tokens: None,
    });
    block_on(ctx.request_llm()).unwrap();
    let output = grauwolf_llm::StructuredOutput {
        name: "Ok".into(),
        schema: serde_json::json!({ "type": "object" }),
    };

    let streamed = block_on(ctx.request_llm_streaming());
    let structured = block_on(ctx.request_llm_structured(&output, 0));
    let choices = block_on(ctx.request_llm_choices(2));
    let facts = block_on(ctx.extract_facts("a", "b", &[]));
    let moderation = block_on(ctx.moderate("Hello", None));

    assert!(
        matches!(streamed, Err(LlmError::BudgetExceeded { .. })),
        "{streamed:?}"
    );
    assert!(structured.unwrap_err().to_string().contains("budget"));
    assert!(matches!(choices, Err(LlmError::BudgetExceeded { .. })));
    assert!(facts.unwrap_err().to_string().contains("budget"));
    assert!(matches!(moderation, Err(LlmError::BudgetExceeded { .. })));
}

/// Budget (ADR-0072): the message names costs rounded, not with rounding artifacts.
#[test]
fn names_the_costs_of_an_exceeded_budget_rounded() {
    let prices: grauwolf_prompt::Prices = serde_json::from_value(serde_json::json!({
        "currency": "USD", "models": { "mock:m": { "input": 0.1, "output": 0.5 } }
    }))
    .unwrap();
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(Counting))
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("Hello").build(),
    )
    .with_prices(Arc::new(prices))
    .with_budget(grauwolf_agents::Budget {
        max_tokens: None,
        max_cost: Some(0.000_005),
        max_tool_calls: None,
        max_prompt_tokens: None,
    });
    block_on(ctx.request_llm()).unwrap();

    let message = block_on(ctx.request_llm()).unwrap_err().to_string();

    // 20 × 0.1 + 100 × 0.1 + 7 × 0.5 = 15.5 per million tokens.
    assert!(message.contains("0.000016"), "{message}");
    assert!(message.contains("0.000005"), "{message}");
}

/// Moderates harmlessly and answers; reports [`USAGE`] for every request.
struct Moderating;

impl LlmClient for Moderating {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async { Ok(Message::assistant("Hello")) })
    }

    fn execute_with_usage<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async move {
            Ok(Completion {
                model: None,
                message: self.execute(prompt, model, tools).await?,
                usage: Some(USAGE),
            })
        })
    }

    fn execute_structured_with_usage<'a>(
        &'a self,
        _prompt: &'a Prompt,
        _model: &'a LlModel,
        _output: &'a grauwolf_llm::StructuredOutput,
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async {
            Ok(Completion {
                model: None,
                message: Message::assistant(r#"{ "harmful": false, "categories": [] }"#),
                usage: Some(USAGE),
            })
        })
    }
}

/// Moderation and several answers count toward usage and costs (ADR-0067 addendum); the
/// moderation at the price of its own model.
#[test]
fn counts_moderation_and_several_choices_with_their_prices() {
    let recorder = Arc::new(Recorder::default());
    let prices: grauwolf_prompt::Prices = serde_json::from_value(serde_json::json!({
        "currency": "USD",
        "models": {
            "mock:m": { "input": 1.0, "output": 10.0, "cache_read": 0.1 },
            "mock:guard": { "input": 2.0, "output": 20.0, "cache_read": 0.2 }
        }
    }))
    .unwrap();
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(Moderating))
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("Hello").build(),
    )
    .with_prices(Arc::new(prices))
    .install(recorder.clone());

    block_on(ctx.moderate("Hello", Some(&LlModel::new("mock", "guard")))).unwrap();
    block_on(ctx.request_llm_choices(2)).unwrap();

    // Moderation: (20 × 2 + 100 × 0.2 + 7 × 20) / 1e6; per answer (20 + 10 + 70) / 1e6.
    let moderation = (40.0 + 20.0 + 140.0) / 1e6;
    let per_choice = 100.0 / 1e6;
    assert_eq!(ctx.usage().unwrap().input_tokens, 360);
    assert!((ctx.cost().unwrap() - (moderation + 2.0 * per_choice)).abs() < 1e-12);
    let events = recorder.0.lock().unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ModerationCompleted { cost: Some(cost), .. } if (cost - moderation).abs() < 1e-12
    )));
    let first_choice = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::LlmCallCompleted { usage, .. } => Some(*usage),
            _ => None,
        })
        .unwrap();
    assert_eq!(first_choice.unwrap().input_tokens, 240);
}

/// Resuming (ADR-0077 addendum): sum, costs and tool calls are in the snapshot, and
/// limits keep applying to the whole run after `restore`.
#[test]
fn keeps_usage_cost_and_tool_calls_across_a_restore() {
    let prices: grauwolf_prompt::Prices = serde_json::from_value(serde_json::json!({
        "currency": "USD",
        "models": { "mock:m": { "input": 1.0, "output": 10.0, "cache_read": 0.1 } }
    }))
    .unwrap();
    let executor = Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(Counting))
            .build(),
    );
    let budget = grauwolf_agents::Budget {
        max_tokens: Some(200),
        max_cost: None,
        max_tool_calls: Some(1),
        max_prompt_tokens: None,
    };
    let mut ctx = AgentContext::new(
        Arc::clone(&executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("Hello").build(),
    )
    .with_prices(Arc::new(prices.clone()))
    .with_budget(budget);
    block_on(ctx.request_llm()).unwrap();
    let call = grauwolf_prompt::ToolCall {
        id: "c1".into(),
        tool: "missing".into(),
        args: serde_json::json!({}),
    };
    block_on(ctx.execute_tool(&call));

    let snapshot = serde_json::to_value(ctx.snapshot()).unwrap();
    let mut restored = AgentContext::restore(executor, serde_json::from_value(snapshot).unwrap())
        .with_prices(Arc::new(prices))
        .with_budget(budget);

    assert_eq!(restored.usage().unwrap().input_tokens, 120);
    assert!((restored.cost().unwrap() - 100.0 / 1e6).abs() < 1e-12);
    // 127 tokens used: one more request is allowed, after that the limit is reached.
    block_on(restored.request_llm()).unwrap();
    assert!(matches!(
        block_on(restored.request_llm()),
        Err(LlmError::BudgetExceeded { .. })
    ));
    let second_tool = block_on(restored.execute_tool(&call));
    assert!(
        second_tool.content.contains("limit of 1"),
        "{}",
        second_tool.content
    );
}
