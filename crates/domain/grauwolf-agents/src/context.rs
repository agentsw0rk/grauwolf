//! Laufzeitkontext eines Agenten.

use std::collections::BTreeMap;
use std::sync::Arc;

use grauwolf_graph::NodeError;
use grauwolf_llm::{LlmError, PromptExecutor, StreamFrame, StructuredOutput};
use grauwolf_prompt::{
    LlModel, Message, Prices, Prompt, Tokenizer, ToolCall, ToolChoice, ToolDescriptor, ToolResult,
    Usage,
};
use grauwolf_tools::ToolRegistry;
use serde_json::Value;

use crate::accounting::{Accounting, Budget};
use crate::checkpoints::{ContextSnapshot, ProgressSaver};
use crate::compression::{self, HistoryCompression};
use crate::conversation::Conversation;
use crate::decision::{self, DecideError, Decider, Decision, Verdict};
use crate::event_bus::EventBus;
use crate::events::{AgentEvent, Feature, Interceptor};
use crate::facts;
use crate::llm_gateway::{LlmGateway, forced_call_instruction, makes_call};
use crate::model_selection::ModelSelection;
use crate::storage::Storage;
use crate::structured::parse_answer;
use crate::tool_access::ToolAccess;
use crate::tool_execution::ToolExecution;

/// Context in which an agent's nodes run: LLM access, model and history.
pub struct AgentContext {
    executor: Arc<PromptExecutor>,
    /// Active model and the models of the entered subgraphs (ADR-0043).
    model: ModelSelection,
    /// History and the attachments of the run's input (ADR-0081).
    conversation: Conversation,
    /// Registry, tool subsets and build permissions (ADR-0034, ADR-0075).
    pub(crate) tools: ToolAccess,
    frame_sink: Option<FrameSink>,
    /// Features and interceptors (F06.1, ADR-0047).
    bus: EventBus,
    /// Long-term memory for durable facts from the compression (ADR-0079).
    fact_memory: Option<Arc<dyn crate::memory::FactMemory>>,
    /// Decider of the decisions that name an external one (ADR-0100).
    decider: Option<Arc<dyn Decider>>,
    /// Usage, cost, tool calls, limits, prices and tokenizer of the run (ADR-0067, ADR-0071,
    /// ADR-0072, ADR-0078, ADR-0087).
    pub(crate) accounting: Accounting,
    /// Agent storage and progress hook (ADR-0044, ADR-0052).
    storage: Storage,
    /// Opaque handle of the runtime that sub-agents of this run adopt (ADR-0101).
    pub(crate) run_handle: Option<Arc<dyn std::any::Any + Send + Sync>>,
}

/// Receiver of the frames from streaming nodes (side channel, ADR-0023).
pub type FrameSink = Arc<dyn Fn(&StreamFrame) + Send + Sync>;

impl AgentContext {
    /// Context with `executor`, active `model` and the initial `prompt`.
    pub fn new(executor: Arc<PromptExecutor>, model: LlModel, prompt: Prompt) -> Self {
        Self {
            executor,
            model: ModelSelection::new(model),
            conversation: Conversation::new(prompt),
            tools: ToolAccess::new(),
            frame_sink: None,
            bus: EventBus::new(),
            fact_memory: None,
            decider: None,
            accounting: Accounting::new(),
            storage: Storage::new(BTreeMap::new()),
            run_handle: None,
        }
    }

    /// Puts an opaque handle of the runtime into the context; sub-agents run as steps find it
    /// in their [`crate::flows::ParentRun`] (ADR-0101). Forks inherit it.
    pub fn with_run_handle(mut self, handle: Arc<dyn std::any::Any + Send + Sync>) -> Self {
        self.run_handle = Some(handle);
        self
    }

    /// Makes this context part of the run `parent`: it counts usage, cost and tool calls on
    /// the parent's counters and obeys its limits, and it sees the parent's handle
    /// (ADR-0101). History, tools, model and storage stay its own.
    pub fn adopt_run(mut self, parent: &crate::flows::ParentRun) -> Self {
        self.accounting = parent.accounting.fork();
        self.run_handle = parent.handle.clone().or(self.run_handle.take());
        self
    }

    /// Installs `feature`; from now on it receives all events.
    pub fn install(mut self, feature: Arc<dyn Feature>) -> Self {
        self.bus.install(feature);
        self
    }

    /// Installs `interceptor`; from now on it intervenes in LLM and tool calls (ADR-0047).
    pub fn intercept(&mut self, interceptor: Arc<dyn Interceptor>) {
        self.bus.intercept(interceptor);
    }

    /// Reports `event` to all installed features.
    pub fn emit(&self, event: AgentEvent) {
        self.bus.emit(event);
    }

    /// The trace headers for a call out of the run (ADR-0123); `call` is the tool call that
    /// makes it, if any.
    pub(crate) fn trace_headers(&self, call: Option<&str>) -> Vec<(String, String)> {
        self.bus.trace_headers(call)
    }

    /// View for LLM requests on the executor, model, bus, accounting and frame sink.
    fn gateway(&self) -> LlmGateway<'_> {
        LlmGateway {
            executor: &self.executor,
            model: &self.model,
            bus: &self.bus,
            accounting: &self.accounting,
            frame_sink: self.frame_sink.as_ref(),
        }
    }

    /// Context from a saved state (ADR-0020); tools and frame sink are set anew, as in
    /// [`AgentContext::new`].
    pub fn restore(executor: Arc<PromptExecutor>, snapshot: ContextSnapshot) -> Self {
        // Consumption continues, so that limits apply to the whole run (ADR-0077 addendum).
        Self {
            storage: Storage::new(snapshot.storage),
            accounting: Accounting::resumed(snapshot.usage, snapshot.cost, snapshot.tool_calls),
            ..Self::new(executor, snapshot.model, snapshot.prompt)
        }
    }

    /// Copy of the context for a parallel branch (ADR-0045).
    pub fn fork(&self) -> Self {
        Self {
            executor: Arc::clone(&self.executor),
            model: self.model.clone(),
            conversation: self.conversation.fork(),
            tools: self.tools.clone(),
            frame_sink: self.frame_sink.clone(),
            bus: self.bus.clone(),
            fact_memory: self.fact_memory.clone(),
            decider: self.decider.clone(),
            accounting: self.accounting.fork(),
            storage: self.storage.fork(),
            run_handle: self.run_handle.clone(),
        }
    }

    /// Sets history, model and storage from a saved state (ADR-0052).
    pub fn apply(&mut self, snapshot: ContextSnapshot) {
        self.conversation.replace_prompt(snapshot.prompt);
        self.model.set_model(snapshot.model);
        self.storage.replace(snapshot.storage);
    }

    /// Takes over history, model and storage of a branch (ADR-0045).
    pub fn absorb(&mut self, branch: AgentContext) {
        self.conversation
            .replace_prompt(branch.conversation.into_prompt());
        self.model.set_model(branch.model.into_model());
        self.storage.replace(branch.storage.into_values());
    }

    /// Additionally stores durable facts from the compression with `HistoryCompression::Facts` in
    /// `memory` (ADR-0079).
    pub fn with_fact_memory(mut self, memory: Arc<dyn crate::memory::FactMemory>) -> Self {
        self.set_fact_memory(memory);
        self
    }

    /// Like [`AgentContext::with_fact_memory`] on a context that is already in use.
    pub fn set_fact_memory(&mut self, memory: Arc<dyn crate::memory::FactMemory>) {
        self.fact_memory = Some(memory);
    }

    /// Sets the external decider (`system_one`) for decision nodes (ADR-0100); the model-based
    /// one (`llm`) needs none.
    pub fn with_decider(mut self, decider: Arc<dyn Decider>) -> Self {
        self.decider = Some(decider);
        self
    }

    /// The external decider, if one is set.
    pub(crate) fn external_decider(&self) -> Option<&Arc<dyn Decider>> {
        self.decider.as_ref()
    }

    /// Limits the run to `budget` (ADR-0072).
    pub fn with_budget(mut self, budget: Budget) -> Self {
        self.accounting.set_budget(budget);
        self
    }

    /// Error if the run has reached a limit of its budget (ADR-0072).
    fn check_budget(&self) -> Result<(), LlmError> {
        self.accounting.check_budget(self.conversation.prompt())
    }

    /// Why the run budget is exhausted (the error text of the next request), or `None` while it
    /// still allows requests (ADR-0072). Counters are shared with forks and sub-agent steps, so
    /// callers that catch step failures (GOAP `on_failure`) can tell an exhausted budget apart.
    pub fn budget_exhausted(&self) -> Option<String> {
        self.check_budget().err().map(|error| error.to_string())
    }

    /// Counts tokens with `tokenizer` instead of the built-in estimate (ADR-0087).
    pub fn with_tokenizer(mut self, tokenizer: Arc<dyn Tokenizer>) -> Self {
        self.accounting.set_tokenizer(tokenizer);
        self
    }

    /// Tokens of the history so far, as the tokenizer counts them (ADR-0087).
    pub fn prompt_tokens(&self) -> u64 {
        self.accounting.prompt_tokens(self.conversation.prompt())
    }

    /// Calculates costs with `prices` (ADR-0071).
    pub fn with_prices(mut self, prices: Arc<Prices>) -> Self {
        self.accounting.set_prices(prices);
        self
    }

    /// Sum of the costs of all requests of this context and its forks, in the currency of the
    /// price table; `None` without a table or a known model (ADR-0071).
    pub fn cost(&self) -> Option<f64> {
        self.accounting.cost()
    }

    /// Asks the active model, with its own structured request, for durable facts
    /// from `input` and `output` that are not yet in `known` (ADR-0069); the history stays
    /// unchanged, and the usage counts toward the sum of the run.
    pub async fn extract_facts(
        &self,
        input: &str,
        output: &str,
        known: &[String],
    ) -> Result<Vec<String>, NodeError> {
        self.check_budget()?;
        facts::extract_facts(&self.gateway(), input, output, known).await
    }

    /// Answers `decision` about `state` with a model (ADR-0100): `decision.model` as
    /// `provider/id` or the active model, asked for an enum of the allowed answers or for a
    /// probability. The history stays unchanged; the request counts like any other (events,
    /// usage, costs, budget, fallbacks).
    pub async fn decide_with_model(
        &self,
        decision: &Decision,
        state: &Value,
    ) -> Result<Verdict, DecideError> {
        decision::decide_with_model(&self.gateway(), self.check_budget(), decision, state).await
    }

    /// Sum of the token usage of all requests of this context and its forks; `None` as long as
    /// no provider has reported numbers (ADR-0067).
    pub fn usage(&self) -> Option<Usage> {
        self.accounting.usage()
    }

    /// Connects the context to the save hook of a checkpoint observer (ADR-0052).
    pub fn set_progress_saver(&mut self, saver: Option<ProgressSaver>) {
        self.storage.set_progress_saver(saver);
    }

    /// Writes the current context as the new state of the running checkpoint, e.g. after
    /// each step of a long node; without checkpoints it does nothing (ADR-0052).
    pub async fn save_progress(&self) -> Result<(), NodeError> {
        self.storage.save_progress(|| self.snapshot()).await
    }

    /// Removes the value under `key` from the agent storage.
    pub fn remove(&mut self, key: &str) {
        self.storage.remove(key);
    }

    /// Stores `value` under `key` in the agent storage (ADR-0044).
    pub fn store(
        &mut self,
        key: impl Into<String>,
        value: &impl serde::Serialize,
    ) -> Result<(), serde_json::Error> {
        self.storage.store(key, value)
    }

    /// Value under `key` as `T`; `None` if nothing is stored, error on the wrong type.
    pub fn load<T: serde::de::DeserializeOwned>(
        &self,
        key: &str,
    ) -> Result<Option<T>, serde_json::Error> {
        self.storage.load(key)
    }

    /// Serializable state for checkpoints. The model is the one outside all subgraphs:
    /// when resuming, the scopes entered again set their models anew (ADR-0043).
    pub fn snapshot(&self) -> ContextSnapshot {
        ContextSnapshot {
            prompt: self.conversation.prompt().clone(),
            model: self.model.outermost().clone(),
            storage: self.storage.values(),
            self_tools: self.tools.self_made(),
            usage: self.usage(),
            cost: self.cost(),
            tool_calls: self.accounting.tool_calls(),
        }
    }

    /// Active model.
    pub fn model(&self) -> &LlModel {
        self.model.model()
    }

    /// Switches the model and returns the previous one; the history stays (F04.9, ADR-0043).
    pub fn set_model(&mut self, model: LlModel) -> LlModel {
        self.model.set_model(model)
    }

    /// Enters a subgraph with its own model; the previous one is remembered.
    pub fn enter_model_scope(&mut self, model: LlModel) {
        self.model.enter_scope(model);
    }

    /// Leaves the innermost subgraph with its own model and restores the outer one.
    pub fn leave_model_scope(&mut self) {
        self.model.leave_scope();
    }

    /// Forwards the frames of streaming nodes to `sink`, e.g. for live output.
    pub fn with_frame_sink(mut self, sink: FrameSink) -> Self {
        self.frame_sink = Some(sink);
        self
    }

    /// Streams the answer into the history, passes each frame to the frame sink, appends the
    /// assembled answer and returns it (ADR-0033).
    pub async fn request_llm_streaming(&mut self) -> Result<Message, LlmError> {
        let answer = self
            .gateway()
            .request_streaming(self.conversation.prompt(), self.tool_descriptors())
            .await?;
        self.append(answer.clone());
        Ok(answer)
    }

    /// Makes `tools` available to the agent.
    pub fn with_tools(mut self, tools: ToolRegistry) -> Self {
        self.tools.set_registry(tools);
        self
    }

    /// Shortens the history by `strategy` (F05.14, ADR-0037). System messages at the start
    /// are kept, as is a pending tool call at the end.
    pub async fn compress_history(&mut self, strategy: HistoryCompression) -> Result<(), LlmError> {
        if !matches!(strategy, HistoryCompression::KeepLast(_)) {
            self.check_budget()?;
        }
        // The view is built from the fields, so that the history can be borrowed mutably.
        let gateway = LlmGateway {
            executor: &self.executor,
            model: &self.model,
            bus: &self.bus,
            accounting: &self.accounting,
            frame_sink: self.frame_sink.as_ref(),
        };
        compression::compress(
            &mut self.conversation,
            &gateway,
            self.fact_memory.as_ref(),
            strategy,
        )
        .await
    }

    /// History so far.
    pub fn prompt(&self) -> &Prompt {
        self.conversation.prompt()
    }

    /// Replaces the history with `prompt` and returns the previous one, e.g. for a
    /// side request on its own history (planner, ADR-0039).
    pub fn replace_prompt(&mut self, prompt: Prompt) -> Prompt {
        self.conversation.replace_prompt(prompt)
    }

    /// Appends `message` to the history.
    pub fn append(&mut self, message: Message) {
        self.conversation.append(message);
    }

    /// Attaches `attachments` to the next user message, the input of the run, which the first
    /// LLM node appends (ADR-0081).
    pub fn attach_to_input(&mut self, attachments: Vec<grauwolf_prompt::Attachment>) {
        self.conversation.attach_to_input(attachments);
    }

    /// Tools offered in the active section.
    pub fn tool_descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools.descriptors()
    }

    /// Restricts the tools to `tools` until the matching [`AgentContext::leave_tool_scope`].
    pub fn enter_tool_scope(&mut self, tools: Vec<String>) {
        self.tools.enter_tool_scope(tools);
    }

    /// Allows or forbids building and using self-built tools until the matching
    /// [`AgentContext::leave_tool_scope`], also against an outer tool subset (ADR-0075).
    pub fn enter_building_scope(&mut self, allowed: bool) {
        self.tools.enter_building_scope(allowed);
    }

    /// Lifts the most recently set tool subset or build permission.
    pub fn leave_tool_scope(&mut self) {
        self.tools.leave_scope();
    }

    /// `tool` builds new tools; outside subgraphs only when `enabled` (ADR-0075).
    pub fn with_self_tools(mut self, tool: impl Into<String>, enabled: bool) -> Self {
        self.tools.set_self_tools(tool.into(), enabled);
        self
    }

    /// Runs `call`. Errors, including a tool not available in the active section,
    /// are reported to the LLM as result text (F03.8, ADR-0027).
    pub async fn execute_tool(&self, call: &ToolCall) -> ToolResult {
        ToolExecution {
            tools: &self.tools,
            bus: &self.bus,
            accounting: &self.accounting,
            storage: &self.storage,
        }
        .execute(call)
        .await
    }

    /// Requests an answer for `output` from the active model (without tools), appends it
    /// and returns it as a normalized value (ADR-0031).
    ///
    /// If the answer is unusable, the error is reported as a user message up to `retries` times
    /// and asked again (F01.11); all attempts stay in the history.
    pub async fn request_llm_structured(
        &mut self,
        output: &StructuredOutput,
        retries: usize,
    ) -> Result<Value, NodeError> {
        let mut attempt = 0;
        loop {
            let answer = self
                .gateway()
                .request_structured(self.conversation.prompt(), output)
                .await?;
            self.append(answer.clone());
            match parse_answer(&answer, output) {
                Ok(value) => return Ok(value),
                Err(error) if attempt < retries => {
                    attempt += 1;
                    self.append(Message::user(format!(
                        "Your answer is invalid: {error}\n\
                         Answer again with nothing but a JSON value of type {} according to the schema.",
                        output.name
                    )));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// Requests an answer as JSON for `output.schema` (without tools), appends it and
    /// returns it unchecked; unlike [`AgentContext::request_llm_structured`], also for
    /// schemas outside the subset of the spec types, e.g. the schema of the DSL itself.
    pub async fn request_llm_json(
        &mut self,
        output: &StructuredOutput,
    ) -> Result<Message, LlmError> {
        let answer = self
            .gateway()
            .request_json(self.conversation.prompt(), output)
            .await?;
        self.append(answer.clone());
        Ok(answer)
    }

    /// Requests `n` alternative answers with the tools, without appending them (ADR-0042).
    pub async fn request_llm_choices(&mut self, n: usize) -> Result<Vec<Message>, LlmError> {
        self.gateway()
            .request_choices(self.conversation.prompt(), self.tool_descriptors(), n)
            .await
    }

    /// Checks `text` with `model` or the active model for harmful content (ADR-0066); the
    /// history stays unchanged.
    pub async fn moderate(
        &self,
        text: &str,
        model: Option<&LlModel>,
    ) -> Result<grauwolf_llm::Moderation, LlmError> {
        self.gateway()
            .moderate(self.conversation.prompt(), text, model)
            .await
    }

    /// Like [`AgentContext::request_llm`], but with `tool_choice` only for this request (ADR-0042).
    ///
    /// If the model cannot force a tool call, an instruction stays permanently in the history
    /// instead and the request runs with `Auto`; if the requested call is missing, the answer is
    /// discarded and asked again exactly once (ADR-0063).
    pub async fn request_llm_with(&mut self, tool_choice: ToolChoice) -> Result<Message, LlmError> {
        if let Some(instruction) = forced_call_instruction(&tool_choice)
            && !self.gateway().forces_tool_choice()
        {
            self.append(Message::system(instruction));
            let mut answer = self.request_unkept().await?;
            if !makes_call(&answer, &tool_choice) {
                answer = self.request_unkept().await?;
            }
            self.append(answer.clone());
            return Ok(answer);
        }
        let previous = std::mem::replace(
            &mut self.conversation.prompt_mut().params.tool_choice,
            tool_choice,
        );
        let answer = self.request_llm().await;
        self.conversation.prompt_mut().params.tool_choice = previous;
        answer
    }

    /// Sends the history to the active model, offers the tools, appends the answer and returns it.
    pub async fn request_llm(&mut self) -> Result<Message, LlmError> {
        let answer = self.request_unkept().await?;
        self.append(answer.clone());
        Ok(answer)
    }

    /// Like [`AgentContext::request_llm`], but does not append the answer.
    async fn request_unkept(&mut self) -> Result<Message, LlmError> {
        self.gateway()
            .request(self.conversation.prompt(), self.tool_descriptors())
            .await
    }
}
