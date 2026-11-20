//! Requests to the LLM: fallbacks, plain, streaming, structured, JSON, choices and moderation
//! requests with their events (ADR-0023, ADR-0031, ADR-0033, ADR-0042, ADR-0066, ADR-0088).

use std::sync::Arc;

use futures_util::StreamExt;
use grauwolf_llm::{LlmError, PromptExecutor, StreamFrame, StructuredOutput, aggregate};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolChoice, ToolDescriptor, Usage};

use crate::accounting::Accounting;
use crate::context::FrameSink;
use crate::event_bus::EventBus;
use crate::events::AgentEvent;
use crate::model_selection::ModelSelection;

/// View on the parts of an [`crate::AgentContext`] that an LLM request needs. The history is
/// not part of it: each request gets the `prompt` it is about to send, and the caller appends
/// the answer.
pub(crate) struct LlmGateway<'a> {
    pub(crate) executor: &'a Arc<PromptExecutor>,
    pub(crate) model: &'a ModelSelection,
    pub(crate) bus: &'a EventBus,
    pub(crate) accounting: &'a Accounting,
    pub(crate) frame_sink: Option<&'a FrameSink>,
}

impl LlmGateway<'_> {
    /// Reports `event` to all installed features.
    pub(crate) fn emit(&self, event: AgentEvent) {
        self.bus.emit(event);
    }

    /// Reports the start of an LLM request with the offered `tools`.
    pub(crate) fn llm_call_starting(&self, tools: &[ToolDescriptor]) {
        self.emit(AgentEvent::LlmCallStarting {
            model: self.model.model().clone(),
            tools: tools.iter().map(|tool| tool.name.clone()).collect(),
        });
    }

    /// Reports the result of an LLM request; costs at the price of `model`, the model that
    /// answered (ADR-0088).
    pub(crate) fn llm_call_finished_by(
        &self,
        model: &LlModel,
        result: &Result<Message, LlmError>,
        usage: Option<Usage>,
        reported: Option<String>,
    ) {
        let cost = usage.and_then(|usage| self.accounting.add_usage_for(model, usage));
        self.emit(match result {
            Ok(answer) => AgentEvent::LlmCallCompleted {
                answer: answer.clone(),
                usage,
                cost,
                model: reported,
            },
            Err(error) => AgentEvent::LlmCallFailed {
                error: error.to_string(),
            },
        });
    }

    /// Asks `model` via `request` and, if that fails, its fallback models one after another
    /// (ADR-0088); no switch when the budget is exhausted. Returns the result and the
    /// model asked last.
    pub(crate) async fn with_fallbacks<T, F, Fut>(
        &self,
        model: &LlModel,
        request: F,
    ) -> (Result<T, LlmError>, LlModel)
    where
        F: Fn(LlModel) -> Fut,
        Fut: std::future::Future<Output = Result<T, LlmError>>,
    {
        let mut asked = model.clone();
        let mut result = request(asked.clone()).await;
        for fallback in &model.fallbacks {
            match &result {
                Err(error) if !matches!(error, LlmError::BudgetExceeded { .. }) => {
                    self.emit(AgentEvent::LlmFallback {
                        from: asked.without_fallbacks(),
                        to: fallback.without_fallbacks(),
                        error: error.to_string(),
                    });
                }
                _ => break,
            }
            asked = fallback.clone();
            result = request(asked.clone()).await;
        }
        (result, asked)
    }

    /// Error if the run has reached a limit of its budget (ADR-0072); `prompt` is the history.
    pub(crate) fn check_budget(&self, prompt: &Prompt) -> Result<(), LlmError> {
        self.accounting.check_budget(prompt)
    }

    /// Can the executor force a tool call with `model` (ADR-0063)?
    pub(crate) fn forces_tool_choice(&self) -> bool {
        self.executor.forces_tool_choice(self.model.model())
    }

    /// Sends `prompt` to the active model, offers `tools` and returns the answer after the
    /// interceptors; the caller decides whether to keep it.
    pub(crate) async fn request(
        &self,
        prompt: &Prompt,
        tools: Vec<ToolDescriptor>,
    ) -> Result<Message, LlmError> {
        self.check_budget(prompt)?;
        let (prompt, tools) = self.bus.prepare(prompt, tools);
        self.llm_call_starting(&tools);
        let executor = Arc::clone(self.executor);
        let (prompt, tools) = (&prompt, &tools);
        let (completion, answered) = self
            .with_fallbacks(self.model.model(), |model| {
                let executor = Arc::clone(&executor);
                async move { executor.execute_with_usage(prompt, &model, tools).await }
            })
            .await;
        let usage = completion
            .as_ref()
            .ok()
            .and_then(|completion| completion.usage);
        let reported = completion
            .as_ref()
            .ok()
            .and_then(|completion| completion.model.clone());
        let result = self
            .bus
            .finish(completion.map(|completion| completion.message));
        self.llm_call_finished_by(&answered, &result, usage, reported);
        result
    }

    /// Streams the answer to `prompt`, passes each frame to the frame sink and returns the
    /// assembled answer (ADR-0033); the caller decides whether to keep it.
    pub(crate) async fn request_streaming(
        &self,
        prompt: &Prompt,
        tools: Vec<ToolDescriptor>,
    ) -> Result<Message, LlmError> {
        self.check_budget(prompt)?;
        let (prompt, tools) = self.bus.prepare(prompt, tools);
        self.llm_call_starting(&tools);
        let mut frames = Vec::new();
        let mut answered = self.model.model().clone();
        let mut streamed: Result<(), LlmError> = Ok(());
        let mut reported: Option<String> = None;
        // Fall back only while no frame has been passed on yet (ADR-0088).
        let candidates =
            std::iter::once(self.model.model().clone()).chain(self.model.model().fallbacks.clone());
        for (index, model) in candidates.enumerate() {
            if index > 0 {
                match &streamed {
                    Err(error)
                        if frames.is_empty()
                            && !matches!(error, LlmError::BudgetExceeded { .. }) =>
                    {
                        self.emit(AgentEvent::LlmFallback {
                            from: answered.without_fallbacks(),
                            to: model.without_fallbacks(),
                            error: error.to_string(),
                        });
                    }
                    _ => break,
                }
            }
            answered = model;
            // The model belongs to the candidate that answered.
            reported = None;
            streamed = async {
                let mut stream = self.executor.execute_streaming(&prompt, &answered, &tools);
                while let Some(frame) = stream.next().await {
                    let frame = frame?;
                    if let Some(sink) = self.frame_sink {
                        sink(&frame);
                    }
                    // Metadata, not output: does not stop a fallback.
                    if let StreamFrame::Model(model) = frame {
                        reported = Some(model);
                        continue;
                    }
                    frames.push(frame);
                }
                Ok(())
            }
            .await;
        }
        // Frames go unchanged to the sink; the interceptors see the whole answer.
        let usage = frames.iter().rev().find_map(|frame| match frame {
            StreamFrame::Usage(usage) => Some(*usage),
            _ => None,
        });
        let result = self.bus.finish(streamed.map(|()| aggregate(frames)));
        self.llm_call_finished_by(&answered, &result, usage, reported);
        result
    }

    /// Requests an answer for `output` to `prompt` (without tools); the caller keeps it and
    /// parses it.
    pub(crate) async fn request_structured(
        &self,
        prompt: &Prompt,
        output: &StructuredOutput,
    ) -> Result<Message, LlmError> {
        self.check_budget(prompt)?;
        let (prompt, _) = self.bus.prepare(prompt, Vec::new());
        self.llm_call_starting(&[]);
        let executor = Arc::clone(self.executor);
        let prompt = &prompt;
        let (completion, answered) = self
            .with_fallbacks(self.model.model(), |model| {
                let executor = Arc::clone(&executor);
                async move {
                    executor
                        .execute_structured_with_usage(prompt, &model, output)
                        .await
                }
            })
            .await;
        let usage = completion
            .as_ref()
            .ok()
            .and_then(|completion| completion.usage);
        let reported = completion
            .as_ref()
            .ok()
            .and_then(|completion| completion.model.clone());
        let result = self
            .bus
            .finish(completion.map(|completion| completion.message));
        self.llm_call_finished_by(&answered, &result, usage, reported);
        result
    }

    /// Requests an answer as JSON for `output.schema` (without tools); unlike
    /// [`LlmGateway::request_structured`] without a budget check.
    pub(crate) async fn request_json(
        &self,
        prompt: &Prompt,
        output: &StructuredOutput,
    ) -> Result<Message, LlmError> {
        let (prompt, _) = self.bus.prepare(prompt, Vec::new());
        self.llm_call_starting(&[]);
        let executor = Arc::clone(self.executor);
        let prompt = &prompt;
        let (result, answered) = self
            .with_fallbacks(self.model.model(), |model| {
                let executor = Arc::clone(&executor);
                async move { executor.execute_structured(prompt, &model, output).await }
            })
            .await;
        let result = self.bus.finish(result);
        self.llm_call_finished_by(&answered, &result, None, None);
        result
    }

    /// Requests `n` alternative answers to `prompt` with `tools` (ADR-0042).
    pub(crate) async fn request_choices(
        &self,
        prompt: &Prompt,
        tools: Vec<ToolDescriptor>,
        n: usize,
    ) -> Result<Vec<Message>, LlmError> {
        self.check_budget(prompt)?;
        let (prompt, tools) = self.bus.prepare(prompt, tools);
        self.llm_call_starting(&tools);
        let executor = Arc::clone(self.executor);
        let (prompt, tools) = (&prompt, &tools);
        let (result, answered) = self
            .with_fallbacks(self.model.model(), |model| {
                let executor = Arc::clone(&executor);
                async move { executor.execute_choices(prompt, &model, tools, n).await }
            })
            .await;
        let result = result.and_then(|choices| {
            let answers = choices
                .messages
                .into_iter()
                .map(|answer| self.bus.finish(Ok(answer)))
                .collect::<Result<Vec<Message>, LlmError>>()?;
            Ok((answers, choices.usage))
        });
        match &result {
            Ok((answers, usage)) => {
                // The shared usage and costs are on the event of the first answer.
                let cost = usage.and_then(|usage| self.accounting.add_usage_for(&answered, usage));
                for (index, answer) in answers.iter().enumerate() {
                    let first = index == 0;
                    self.emit(AgentEvent::LlmCallCompleted {
                        answer: answer.clone(),
                        usage: usage.filter(|_| first),
                        cost: cost.filter(|_| first),
                        model: None,
                    });
                }
            }
            Err(error) => self.emit(AgentEvent::LlmCallFailed {
                error: error.to_string(),
            }),
        }
        result.map(|(answers, _)| answers)
    }

    /// Checks `text` with `model` or the active model for harmful content (ADR-0066);
    /// `prompt` is the history, for the budget check.
    pub(crate) async fn moderate(
        &self,
        prompt: &Prompt,
        text: &str,
        model: Option<&LlModel>,
    ) -> Result<grauwolf_llm::Moderation, LlmError> {
        self.check_budget(prompt)?;
        let model = model.unwrap_or(self.model.model());
        let text = self.bus.intercepted_moderation_text(text);
        self.emit(AgentEvent::ModerationStarting {
            model: model.clone(),
        });
        let executor = Arc::clone(self.executor);
        let text = &text;
        let (result, answered) = self
            .with_fallbacks(model, |model| {
                let executor = Arc::clone(&executor);
                async move { executor.moderate(text, &model).await }
            })
            .await;
        self.emit(match &result {
            Ok(moderation) => AgentEvent::ModerationCompleted {
                harmful: moderation.harmful,
                categories: moderation.categories.clone(),
                usage: moderation.usage,
                cost: moderation
                    .usage
                    .and_then(|usage| self.accounting.add_usage_for(&answered, usage)),
            },
            Err(error) => AgentEvent::ModerationFailed {
                error: error.to_string(),
            },
        });
        result
    }
}

/// Instruction that replaces a forced tool call (ADR-0063); `None` if nothing
/// is forced.
pub(crate) fn forced_call_instruction(tool_choice: &ToolChoice) -> Option<String> {
    match tool_choice {
        ToolChoice::Required => Some(
            "In your next answer, call at least one of the offered tools and \
             begin the answer with that call."
                .to_owned(),
        ),
        ToolChoice::Tool(name) => Some(format!(
            "In your next answer, call the tool `{name}` and begin the answer with that call."
        )),
        ToolChoice::Auto | ToolChoice::None => None,
    }
}

/// Does `answer` contain the call that `tool_choice` requires?
pub(crate) fn makes_call(answer: &Message, tool_choice: &ToolChoice) -> bool {
    let Message::Assistant(answer) = answer else {
        return false;
    };
    match tool_choice {
        ToolChoice::Tool(name) => answer.tool_calls().any(|call| &call.tool == name),
        _ => answer.tool_calls().next().is_some(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use futures_executor::block_on;
    use futures_util::future::BoxFuture;
    use grauwolf_llm::{Completion, LlmClient};

    use super::*;
    use crate::accounting::Budget;
    use crate::events::Feature;

    /// Fails for the models named in `failing`, otherwise answers with the model's id.
    struct Client {
        failing: Vec<&'static str>,
        asked: Mutex<Vec<String>>,
    }

    impl LlmClient for Client {
        fn execute<'a>(
            &'a self,
            _prompt: &'a Prompt,
            model: &'a LlModel,
            _tools: &'a [ToolDescriptor],
        ) -> BoxFuture<'a, Result<Message, LlmError>> {
            self.asked.lock().unwrap().push(model.id.clone());
            let result = if self.failing.contains(&model.id.as_str()) {
                Err(LlmError::Client("down".into()))
            } else {
                Ok(Message::assistant(model.id.clone()))
            };
            Box::pin(std::future::ready(result))
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
                    usage: Some(Usage {
                        input_tokens: 10,
                        output_tokens: 5,
                        cache_read_tokens: None,
                        cache_write_tokens: None,
                    }),
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

    /// The parts a gateway views, with a client that fails for `failing`.
    struct Parts {
        client: Arc<Client>,
        recorder: Arc<Recorder>,
        executor: Arc<PromptExecutor>,
        model: ModelSelection,
        bus: EventBus,
        accounting: Accounting,
    }

    impl Parts {
        fn new(model: LlModel, failing: Vec<&'static str>) -> Self {
            let client = Arc::new(Client {
                failing,
                asked: Mutex::default(),
            });
            let recorder = Arc::new(Recorder::default());
            let mut bus = EventBus::new();
            bus.install(recorder.clone());
            Self {
                executor: Arc::new(
                    PromptExecutor::builder()
                        .client("p", client.clone())
                        .build(),
                ),
                client,
                recorder,
                model: ModelSelection::new(model),
                bus,
                accounting: Accounting::new(),
            }
        }

        fn gateway(&self) -> LlmGateway<'_> {
            LlmGateway {
                executor: &self.executor,
                model: &self.model,
                bus: &self.bus,
                accounting: &self.accounting,
                frame_sink: None,
            }
        }

        fn asked(&self) -> Vec<String> {
            self.client.asked.lock().unwrap().clone()
        }

        fn events(&self) -> Vec<AgentEvent> {
            self.recorder.0.lock().unwrap().clone()
        }
    }

    fn prompt() -> Prompt {
        Prompt::builder("p").user("Hello").build()
    }

    #[test]
    fn a_request_reports_start_and_completion_and_counts_the_usage() {
        let parts = Parts::new(LlModel::new("p", "main"), vec![]);
        let answer = block_on(parts.gateway().request(&prompt(), Vec::new())).unwrap();
        assert_eq!(answer, Message::assistant("main"));
        assert!(matches!(
            parts.events().as_slice(),
            [
                AgentEvent::LlmCallStarting { .. },
                AgentEvent::LlmCallCompleted { usage: Some(_), .. }
            ]
        ));
        assert_eq!(parts.accounting.usage().unwrap().input_tokens, 10);
    }

    #[test]
    fn fallbacks_are_asked_in_order_and_each_switch_is_reported() {
        let model = LlModel::new("p", "a")
            .with_fallbacks(vec![LlModel::new("p", "b"), LlModel::new("p", "c")]);
        let parts = Parts::new(model, vec!["a", "b"]);
        let answer = block_on(parts.gateway().request(&prompt(), Vec::new())).unwrap();
        assert_eq!(answer, Message::assistant("c"));
        assert_eq!(parts.asked(), ["a", "b", "c"]);
        let events = parts.events();
        assert!(matches!(events[0], AgentEvent::LlmCallStarting { .. }));
        assert!(matches!(&events[1], AgentEvent::LlmFallback { to, .. } if to.id == "b"));
        assert!(matches!(&events[2], AgentEvent::LlmFallback { to, .. } if to.id == "c"));
        assert!(matches!(events[3], AgentEvent::LlmCallCompleted { .. }));
    }

    #[test]
    fn without_a_working_fallback_the_last_error_is_reported_as_failure() {
        let model = LlModel::new("p", "a").with_fallbacks(vec![LlModel::new("p", "b")]);
        let parts = Parts::new(model, vec!["a", "b"]);
        assert!(block_on(parts.gateway().request(&prompt(), Vec::new())).is_err());
        assert_eq!(parts.asked(), ["a", "b"]);
        assert!(matches!(
            parts.events().last(),
            Some(AgentEvent::LlmCallFailed { .. })
        ));
    }

    #[test]
    fn an_exhausted_budget_stops_before_any_request_or_event() {
        let mut parts = Parts::new(LlModel::new("p", "main"), vec![]);
        parts.accounting.set_budget(Budget {
            max_tokens: Some(1),
            ..Budget::default()
        });
        let _ = block_on(parts.gateway().request(&prompt(), Vec::new())).unwrap();
        let before = parts.events().len();
        let error = block_on(parts.gateway().request(&prompt(), Vec::new())).unwrap_err();
        assert!(matches!(error, LlmError::BudgetExceeded { .. }));
        assert_eq!(parts.asked(), ["main"]);
        assert_eq!(parts.events().len(), before);
    }

    #[test]
    fn a_budget_error_does_not_switch_to_a_fallback() {
        let parts = Parts::new(
            LlModel::new("p", "a").with_fallbacks(vec![LlModel::new("p", "b")]),
            vec![],
        );
        let (result, asked) = block_on(
            parts
                .gateway()
                .with_fallbacks(parts.model.model(), |_| async {
                    Err::<(), _>(LlmError::BudgetExceeded { reason: "x".into() })
                }),
        );
        assert!(result.is_err());
        assert_eq!(asked.id, "a");
        assert!(parts.events().is_empty());
    }

    #[test]
    fn a_forced_call_is_instructed_and_recognized() {
        assert_eq!(forced_call_instruction(&ToolChoice::Auto), None);
        assert_eq!(forced_call_instruction(&ToolChoice::None), None);
        assert!(forced_call_instruction(&ToolChoice::Required).is_some());
        assert!(
            forced_call_instruction(&ToolChoice::Tool("t".into()))
                .unwrap()
                .contains("`t`")
        );
        assert!(!makes_call(
            &Message::assistant("text"),
            &ToolChoice::Required
        ));
        assert!(!makes_call(&Message::user("text"), &ToolChoice::Required));
    }
}
