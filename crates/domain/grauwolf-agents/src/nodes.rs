//! Implementations of the catalog nodes (ADR-0017, ADR-0018).

use futures_util::future::{BoxFuture, join_all};
use grauwolf_graph::{NodeData, NodeError};
use grauwolf_prompt::{Message, ToolCall, ToolChoice, ToolResult};

use grauwolf_llm::StructuredOutput;

use crate::structured::Structured;
use crate::{AgentContext, HistoryCompression};

/// `llm_request`: sends `input` as a user message and returns the LLM's answer.
pub fn llm_request(
    ctx: &mut AgentContext,
    input: String,
) -> BoxFuture<'_, Result<Message, NodeError>> {
    Box::pin(async move {
        ctx.append(Message::user(input));
        Ok(ctx.request_llm().await?)
    })
}

/// `llm_request_streaming`: like `llm_request`, but streams the answer to the context's
/// frame sink; the assembled answer goes out over the edge (ADR-0023).
pub fn llm_request_streaming(
    ctx: &mut AgentContext,
    input: String,
) -> BoxFuture<'_, Result<Message, NodeError>> {
    Box::pin(async move {
        ctx.append(Message::user(input));
        Ok(ctx.request_llm_streaming().await?)
    })
}

/// `execute_tool`: runs the tool call. The call itself is already in the history, because
/// `request_llm` appends every answer of the LLM.
pub fn execute_tool(
    ctx: &mut AgentContext,
    call: ToolCall,
) -> BoxFuture<'_, Result<ToolResult, NodeError>> {
    Box::pin(async move { Ok(ctx.execute_tool(&call).await) })
}

/// `get_field`: returns the field `field` of a structured value as text; other values as JSON
/// text, missing fields as empty text (ADR-0041).
pub fn get_field<T: NodeData>(
    field: &'static str,
) -> impl for<'a> Fn(&'a mut AgentContext, T) -> BoxFuture<'a, Result<String, NodeError>>
+ Send
+ Sync
+ 'static {
    move |_ctx, value| Box::pin(async move { Ok(field_text(&serde_json::to_value(value)?, field)) })
}

/// Field `field` of `value` as text.
pub(crate) fn field_text(value: &serde_json::Value, field: &str) -> String {
    match value.get(field) {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// Default instruction of `llm_verify`. Built-in model text. Deliberately not a literal
/// translation of the earlier German text (ADR-0091 addendum): it limits the verifier to the
/// content of the answer, so that a small model neither demands to see tool calls nor rejects a
/// correct answer for how it was obtained.
pub const DEFAULT_VERIFY_PROMPT: &str = "Judge only the content of the answer below against the task. \
    You cannot see how the answer was obtained, so do not ask for tool calls or working steps. \
    Check the facts and numbers in the answer yourself. Set correct to false only if the answer \
    is wrong or leaves part of the task unanswered, and say in feedback exactly what is wrong; \
    otherwise set correct to true and leave feedback empty.";

/// `llm_verify`: has the LLM check without tools whether `answer` solves the task, and returns
/// the verdict as a structured value of type `T` (fields `correct`, `feedback`, `answer`);
/// `answer` holds the checked answer (F05.13, ADR-0041).
pub fn llm_verify<T: Structured>(
    prompt: impl Into<String>,
) -> impl for<'a> Fn(&'a mut AgentContext, String) -> BoxFuture<'a, Result<T, NodeError>>
+ Send
+ Sync
+ 'static {
    let prompt = prompt.into();
    move |ctx, answer| {
        let prompt = prompt.clone();
        Box::pin(async move {
            let verdict = verify(ctx, &T::structured_output(), &prompt, answer).await?;
            Ok(serde_json::from_value(verdict)?)
        })
    }
}

/// Checks the input text for harmful content (F05.17, ADR-0066) with `model` or the active
/// model; returns `{ harmful, categories, text, message }` as `T`. The history stays unchanged.
pub fn llm_moderate<T: Structured>(
    model: Option<grauwolf_prompt::LlModel>,
) -> impl for<'a> Fn(&'a mut AgentContext, String) -> BoxFuture<'a, Result<T, NodeError>>
+ Send
+ Sync
+ 'static {
    move |ctx, text| {
        let model = model.clone();
        Box::pin(async move {
            let verdict = moderate(ctx, model.as_ref(), text).await?;
            Ok(serde_json::from_value(verdict)?)
        })
    }
}

/// Core of `llm_moderate` on `Value`, also used by the interpreter.
pub(crate) async fn moderate(
    ctx: &AgentContext,
    model: Option<&grauwolf_prompt::LlModel>,
    text: String,
) -> Result<serde_json::Value, NodeError> {
    let moderation = ctx.moderate(&text, model).await?;
    let message = if moderation.harmful {
        format!(
            "I cannot process this request: the content check rejected it \
             (categories: {}).",
            moderation.categories.join(", ")
        )
    } else {
        String::new()
    };
    Ok(serde_json::json!({
        "harmful": moderation.harmful,
        "categories": moderation.categories,
        "text": text,
        "message": message,
    }))
}

/// Core of `llm_verify` on `Value`, also used by the interpreter.
pub(crate) async fn verify(
    ctx: &mut AgentContext,
    output: &StructuredOutput,
    prompt: &str,
    answer: String,
) -> Result<serde_json::Value, NodeError> {
    ctx.append(Message::user(format!("{prompt}\n\nAnswer:\n{answer}")));
    let mut verdict = ctx.request_llm_structured(output, VERIFY_RETRIES).await?;
    if let Some(fields) = verdict.as_object_mut() {
        fields.insert("answer".into(), serde_json::Value::String(answer));
    }
    Ok(verdict)
}

/// Retries when the verdict is unusable.
const VERIFY_RETRIES: usize = 2;

/// Default instruction of `llm_reason`. Built-in model text.
pub const DEFAULT_REASONING_PROMPT: &str =
    "Think about the task and plan the next steps. Do not call a tool yet.";

/// Values that `llm_reason` adds to the history: text as a user message, tool results as
/// tool result messages.
pub trait ReasonInput: NodeData {
    /// Messages for the history.
    fn into_messages(self) -> Vec<Message>;
}

impl ReasonInput for String {
    fn into_messages(self) -> Vec<Message> {
        vec![Message::user(self)]
    }
}

impl ReasonInput for ToolResult {
    fn into_messages(self) -> Vec<Message> {
        vec![Message::ToolResult(self)]
    }
}

impl ReasonInput for Vec<ToolResult> {
    fn into_messages(self) -> Vec<Message> {
        self.into_iter().map(Message::ToolResult).collect()
    }
}

/// `llm_reason`: appends the value and `prompt`, asks **without tools** and returns the
/// thought as text (ReAct, ADR-0041).
pub fn llm_reason<T: ReasonInput>(
    prompt: impl Into<String>,
) -> impl for<'a> Fn(&'a mut AgentContext, T) -> BoxFuture<'a, Result<String, NodeError>>
+ Send
+ Sync
+ 'static {
    let prompt = prompt.into();
    move |ctx, value| {
        let prompt = prompt.clone();
        Box::pin(async move {
            for message in value.into_messages() {
                ctx.append(message);
            }
            ctx.append(Message::user(prompt));
            // An empty tool subset hides all tools.
            ctx.enter_tool_scope(Vec::new());
            let answer = ctx.request_llm().await;
            ctx.leave_tool_scope();
            Ok(match answer? {
                Message::Assistant(answer) => answer.text(),
                _ => String::new(),
            })
        })
    }
}

/// `llm_request_only_tools`: like `llm_request`, but the LLM must call a tool (ADR-0042).
pub fn llm_request_only_tools(
    ctx: &mut AgentContext,
    input: String,
) -> BoxFuture<'_, Result<Message, NodeError>> {
    Box::pin(async move {
        ctx.append(Message::user(input));
        Ok(ctx.request_llm_with(ToolChoice::Required).await?)
    })
}

/// `llm_request_force_tool`: like `llm_request`, but the LLM must call the tool `tool`
/// (ADR-0042).
pub fn llm_request_force_tool(
    tool: impl Into<String>,
) -> impl for<'a> Fn(&'a mut AgentContext, String) -> BoxFuture<'a, Result<Message, NodeError>>
+ Send
+ Sync
+ 'static {
    let tool = tool.into();
    move |ctx, input| {
        let tool = tool.clone();
        Box::pin(async move {
            ctx.append(Message::user(input));
            Ok(ctx.request_llm_with(ToolChoice::Tool(tool)).await?)
        })
    }
}

/// `llm_request_best_of`: appends the input, requests `choices` answers and lets an evaluation
/// without tools pick the best by `criteria`; only that one goes into the history (ADR-0042).
pub fn llm_request_best_of(
    choices: usize,
    criteria: impl Into<String>,
) -> impl for<'a> Fn(&'a mut AgentContext, String) -> BoxFuture<'a, Result<Message, NodeError>>
+ Send
+ Sync
+ 'static {
    let criteria = criteria.into();
    move |ctx, input| {
        let criteria = criteria.clone();
        Box::pin(async move {
            ctx.append(Message::user(input.clone()));
            let mut candidates = ctx.request_llm_choices(choices.max(1)).await?;
            let best = if candidates.len() > 1 {
                let texts: Vec<String> = candidates.iter().map(describe).collect();
                pick_best(ctx, &input, &texts, &criteria).await?
            } else {
                0
            };
            let answer = candidates.swap_remove(best.min(candidates.len() - 1));
            ctx.append(answer.clone());
            Ok(answer)
        })
    }
}

/// Default criteria of `llm_request_best_of`. Built-in model text.
pub const DEFAULT_BEST_OF_CRITERIA: &str =
    "Choose the answer that solves the task most completely and correctly.";

/// Index of the best answer after an evaluation on its own history; outside the list → 0.
pub(crate) async fn pick_best(
    ctx: &mut AgentContext,
    task: &str,
    candidates: &[String],
    criteria: &str,
) -> Result<usize, NodeError> {
    let mut request = format!("Task:\n{task}\n\nCriterion: {criteria}\n");
    for (index, candidate) in candidates.iter().enumerate() {
        request.push_str(&format!("\nAnswer {}:\n{candidate}\n", index + 1));
    }
    request.push_str("\nName the number of the best answer in `best`.");
    let judge = grauwolf_prompt::Prompt::builder("best_of")
        .system("You compare answers and choose the best one.")
        .user(request)
        .build();
    let original = ctx.replace_prompt(judge);
    let verdict = ctx
        .request_llm_structured(
            &StructuredOutput {
                name: "BestChoice".into(),
                schema: serde_json::json!({
                    "type": "object",
                    "properties": { "best": { "type": "integer" } },
                    "required": ["best"]
                }),
            },
            2,
        )
        .await;
    ctx.replace_prompt(original);
    let best = verdict?["best"].as_u64().unwrap_or(1);
    Ok(usize::try_from(best.saturating_sub(1)).unwrap_or(0))
}

/// Text of an answer including its tool calls, for the evaluation.
fn describe(message: &Message) -> String {
    let Message::Assistant(answer) = message else {
        return String::new();
    };
    let mut text = answer.text();
    for call in answer.tool_calls() {
        text.push_str(&format!("\n[Tool call {}({})]", call.tool, call.args));
    }
    text
}

/// `set_model`: switches the model for the rest of the run and passes the value through
/// (F04.9, ADR-0043).
pub fn set_model<T: NodeData>(
    provider: impl Into<String>,
    id: impl Into<String>,
) -> impl for<'a> Fn(&'a mut AgentContext, T) -> BoxFuture<'a, Result<T, NodeError>>
+ Send
+ Sync
+ 'static {
    let model = grauwolf_prompt::LlModel::new(provider, id);
    move |ctx, value| {
        ctx.set_model(model.clone());
        Box::pin(async move { Ok(value) })
    }
}

/// Like [`set_model`], with a ready-made model, e.g. including fallback models (ADR-0088).
pub fn switch_model<T: NodeData>(
    model: grauwolf_prompt::LlModel,
) -> impl for<'a> Fn(&'a mut AgentContext, T) -> BoxFuture<'a, Result<T, NodeError>>
+ Send
+ Sync
+ 'static {
    move |ctx, value| {
        ctx.set_model(model.clone());
        Box::pin(async move { Ok(value) })
    }
}

/// `decide`: asks a decider a typed question about the passing value, stores the verdict
/// and passes the value on unchanged (ADR-0100). `node` is the node ID for the events;
/// `params` are the node parameters as JSON text.
pub fn decide<T: NodeData>(
    node: &'static str,
    params: &'static str,
) -> impl for<'a> Fn(&'a mut AgentContext, T) -> BoxFuture<'a, Result<T, NodeError>>
+ Send
+ Sync
+ 'static {
    let parsed = serde_json::from_str(params)
        .map_err(|error| error.to_string())
        .and_then(|params| crate::decision::DecisionNode::from_params(&params));
    move |ctx, value| {
        let parsed = parsed.clone();
        Box::pin(async move {
            let decision =
                parsed.map_err(|reason| format!("invalid decide parameters: {reason}"))?;
            let state = serde_json::to_value(&value)?;
            crate::decision::decide(ctx, node, &decision, &state).await?;
            Ok(value)
        })
    }
}

/// `agent`: runs the sub-agent `agent` from `agent_tools` as a step with the passing value
/// and returns its result as `O` (ADR-0101).
pub fn agent_step<I: NodeData, O: NodeData>(
    agent: &'static str,
) -> impl for<'a> Fn(&'a mut AgentContext, I) -> BoxFuture<'a, Result<O, NodeError>>
+ Send
+ Sync
+ 'static {
    move |ctx, input| {
        Box::pin(async move {
            let output = ctx
                .run_agent_step(agent, serde_json::to_value(input)?)
                .await?;
            Ok(serde_json::from_value(output)?)
        })
    }
}

/// `store`: saves the passing value under `key` in the agent storage and passes it on
/// (ADR-0044).
pub fn store<T: NodeData>(
    key: &'static str,
) -> impl for<'a> Fn(&'a mut AgentContext, T) -> BoxFuture<'a, Result<T, NodeError>>
+ Send
+ Sync
+ 'static {
    move |ctx, value| {
        Box::pin(async move {
            ctx.store(key, &value)?;
            Ok(value)
        })
    }
}

/// `load`: returns the value under `key` from the agent storage; the input is not read
/// (ADR-0044).
pub fn load<I: NodeData, O: NodeData>(
    key: &'static str,
) -> impl for<'a> Fn(&'a mut AgentContext, I) -> BoxFuture<'a, Result<O, NodeError>>
+ Send
+ Sync
+ 'static {
    move |ctx, _input| Box::pin(async move { load_value(ctx, key) })
}

/// Value under `key`, or an error that names the key.
pub(crate) fn load_value<T: serde::de::DeserializeOwned>(
    ctx: &AgentContext,
    key: &str,
) -> Result<T, NodeError> {
    ctx.load(key)?
        .ok_or_else(|| format!("no value under '{key}' in the agent storage").into())
}

/// Instruction that `llm_continue` appends when the history ends with an answer. Built-in model
/// text.
pub const CONTINUE_PROMPT: &str = "Act now: call a tool or give the final answer.";

/// `llm_continue`: asks the LLM with tools without appending the input – it is already in the
/// history, e.g. as a thought from `llm_reason` (ADR-0041). If the history ends with an answer,
/// [`CONTINUE_PROMPT`] is appended first: providers such as Ollama or Anthropic would otherwise
/// continue a trailing answer as a prefill instead of answering anew.
pub fn llm_continue(
    ctx: &mut AgentContext,
    _input: String,
) -> BoxFuture<'_, Result<Message, NodeError>> {
    Box::pin(async move {
        if matches!(ctx.prompt().messages.last(), Some(Message::Assistant(_))) {
            ctx.append(Message::user(CONTINUE_PROMPT));
        }
        Ok(ctx.request_llm().await?)
    })
}

/// `llm_send_tool_result`: passes the tool result to the LLM and returns its answer.
pub fn llm_send_tool_result(
    ctx: &mut AgentContext,
    result: ToolResult,
) -> BoxFuture<'_, Result<Message, NodeError>> {
    Box::pin(async move {
        ctx.append(Message::ToolResult(result));
        Ok(ctx.request_llm().await?)
    })
}

/// `execute_multiple_tools`: runs all tool calls of an answer concurrently and returns the
/// results in the order of the calls.
pub fn execute_multiple_tools(
    ctx: &mut AgentContext,
    calls: Vec<ToolCall>,
) -> BoxFuture<'_, Result<Vec<ToolResult>, NodeError>> {
    Box::pin(async move {
        let ctx = &*ctx;
        Ok(join_all(calls.iter().map(|call| ctx.execute_tool(call))).await)
    })
}

/// `execute_multiple_tools` with `sequential: true`: runs the tool calls of an answer one
/// after another in call order (not concurrently) and returns the results in that order.
/// Tool events, interceptors, approvals and the tool call limit apply per call.
pub fn execute_tools_in_order(
    ctx: &mut AgentContext,
    calls: Vec<ToolCall>,
) -> BoxFuture<'_, Result<Vec<ToolResult>, NodeError>> {
    Box::pin(async move {
        let ctx = &*ctx;
        let mut results = Vec::with_capacity(calls.len());
        for call in &calls {
            results.push(ctx.execute_tool(call).await);
        }
        Ok(results)
    })
}

/// `llm_send_multiple_tool_results`: passes all tool results to the LLM and returns its
/// answer.
pub fn llm_send_multiple_tool_results(
    ctx: &mut AgentContext,
    results: Vec<ToolResult>,
) -> BoxFuture<'_, Result<Message, NodeError>> {
    Box::pin(async move {
        for result in results {
            ctx.append(Message::ToolResult(result));
        }
        Ok(ctx.request_llm().await?)
    })
}

/// `llm_request_structured`: sends `input` as a user message and returns the answer as `T`
/// (ADR-0031); an unusable answer is an error.
pub fn llm_request_structured<T: Structured>(
    ctx: &mut AgentContext,
    input: String,
) -> BoxFuture<'_, Result<T, NodeError>> {
    llm_request_structured_with_retries::<T, 0>(ctx, input)
}

/// Like [`llm_request_structured`], but on an unusable answer it asks again up to `RETRIES`
/// times with the error message (F01.11).
pub fn llm_request_structured_with_retries<T: Structured, const RETRIES: usize>(
    ctx: &mut AgentContext,
    input: String,
) -> BoxFuture<'_, Result<T, NodeError>> {
    Box::pin(async move {
        ctx.append(Message::user(input));
        let value = ctx
            .request_llm_structured(&T::structured_output(), RETRIES)
            .await?;
        Ok(serde_json::from_value(value)?)
    })
}

/// `compress_history` with `strategy: summarize`: summarizes the history and passes `value`
/// on unchanged (ADR-0037).
pub fn compress_history_summarize<T: NodeData>(
    ctx: &mut AgentContext,
    value: T,
) -> BoxFuture<'_, Result<T, NodeError>> {
    Box::pin(async move {
        ctx.compress_history(HistoryCompression::Summarize).await?;
        Ok(value)
    })
}

/// `compress_history` with `strategy: facts`: keeps the last `KEEP` messages, replaces the
/// older ones with their facts and passes `value` on unchanged (ADR-0076).
pub fn compress_history_facts<T: NodeData, const KEEP: usize>(
    ctx: &mut AgentContext,
    value: T,
) -> BoxFuture<'_, Result<T, NodeError>> {
    Box::pin(async move {
        ctx.compress_history(HistoryCompression::Facts(KEEP))
            .await?;
        Ok(value)
    })
}

/// `compress_history` with `strategy: keep_last`: keeps the last `KEEP` messages and passes
/// `value` on unchanged (ADR-0037).
pub fn compress_history_keep_last<T: NodeData, const KEEP: usize>(
    ctx: &mut AgentContext,
    value: T,
) -> BoxFuture<'_, Result<T, NodeError>> {
    Box::pin(async move {
        ctx.compress_history(HistoryCompression::KeepLast(KEEP))
            .await?;
        Ok(value)
    })
}
