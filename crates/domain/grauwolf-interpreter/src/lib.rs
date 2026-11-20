//! Interpreter (E13, F13.10): runs an AgentSpec as an agent without compiling it.

use grauwolf_spec::AgentSpecExt;
use std::sync::Arc;

mod self_tools;

pub use grauwolf_agents::approval::ToolApprover;

use futures_util::future::BoxFuture;
use grauwolf_agents::checkpoints::CheckpointStore;
use grauwolf_agents::code::{CodeModule, code_condition, code_node, code_transform};
use grauwolf_agents::decision::Decider;
use grauwolf_agents::events::{Feature, Interceptor};
use grauwolf_agents::flows::ParentRun;
use grauwolf_agents::for_each::{Body as ForEachRunBody, ForEach, OnError, run_for_each};
use grauwolf_agents::memory::{ChatHistoryStore, FactMemory};
use grauwolf_agents::nodes;
use grauwolf_agents::parallel::{Branch, Merge, progress_key, run_parallel_resumable};
use grauwolf_agents::run_lifecycle::{AgentRun, RunError};
use grauwolf_agents::scopes::SubgraphScope;
use grauwolf_agents::structured::{StructureError, normalize};
use grauwolf_agents::{AgentContext, FrameSink, implementations};
use grauwolf_graph::{EngineError, Graph, GraphError, NodeFn, Scope, erase_node};
use grauwolf_llm::PromptExecutor;
use grauwolf_planner::LlmPlanner;
use grauwolf_prompt::{LlModel, Prompt};
use grauwolf_spec::{
    AgentSpec, CODE_KIND, DEFAULT_PLAN_ITERATIONS, FOR_EACH_KIND, ForEachBody, ForEachOnError,
    ForEachParams, GOAP_KIND, GraphSpec, ON_CODE_KIND, PARALLEL_KIND, PLAN_KIND, SUBGRAPH_KIND,
    SubgraphSpec, ValidationError,
};
use grauwolf_tools::{
    DynTool, NewTool, ResolvedTool, ToolArtifact, ToolDescriptor, ToolError, ToolFactory,
    ToolRegistry, ToolRuntime, ToolShelf,
};
use serde_json::Value;

/// Error when loading or running a spec.
#[derive(Debug, thiserror::Error)]
pub enum InterpreterError {
    /// The spec is invalid; contains all errors found.
    #[error("invalid spec: {}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    Invalid(Vec<ValidationError>),
    /// No built artifact was passed for a tool of the spec.
    #[error("no artifact for tool '{tool}' – build the tools first (`grauwolf spec build`)")]
    MissingArtifact { tool: String },
    /// The spec has tools, but the interpreter lacks a tool runtime.
    #[error("no tool runtime configured (`Interpreter::with_tool_runtime`)")]
    MissingToolRuntime,
    /// A tool could not be loaded.
    #[error(transparent)]
    Tool(#[from] ToolError),
    /// A tool from outside (`AgentSpec::external_tools()`: MCP, knowledge, memory, sub-agents,
    /// built-in tools) was not provided via `with_tools`.
    #[error("tool '{tool}' was not provided (MCP server, knowledge, sub-agent or built-in tool)")]
    MissingExternalTool { tool: String },
    /// The spec allows self-created tools, but a factory is missing (ADR-0075).
    #[error("`self_tools` needs a tool factory (`Interpreter::with_tool_factory`)")]
    MissingToolFactory,
    /// There is no implementation for a kind from the spec.
    #[error("no implementation for '{kind}'")]
    MissingImplementation { kind: String },
    /// The spec has a decision node with the `system_one` decider, but none was provided.
    #[error(
        "the spec asks the 'system_one' decider; provide one with `Interpreter::with_decider` \
         (the CLI reads RKEV_BASE_URL)"
    )]
    MissingDecider,
    /// The graph could not be built.
    #[error(transparent)]
    Graph(#[from] GraphError),
    /// The execution failed.
    #[error(transparent)]
    Engine(#[from] EngineError),
    /// There is no stored checkpoint for the run.
    #[error("no checkpoint for run '{run_id}'")]
    NoCheckpoint { run_id: String },
    /// The chat store failed.
    #[error("chat store failed: {0}")]
    ChatHistory(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// The checkpoint store failed.
    #[error("checkpoint store failed: {0}")]
    Checkpoint(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// The input does not fit the input type of the spec.
    #[error("input does not fit type {name}: {source}")]
    Input {
        name: String,
        #[source]
        source: StructureError,
    },
    /// `limits.max_cost` without a price table: costs would be unknown, the limit ineffective.
    #[error("`limits.max_cost` needs a price table (`Interpreter::with_prices`, CLI `--prices`)")]
    CostLimitWithoutPrices,
    /// A self-made tool from the checkpoint could not be rebuilt (ADR-0077).
    #[error("self-made tool '{tool}' could not be rebuilt on resume: {reason}")]
    SelfToolRebuild { tool: String, reason: String },
    /// A checkpoint run with a conversation; the two together are not supported (ADR-0077).
    #[error("checkpoint runs cannot be combined with a conversation")]
    CheckpointConversation,
    /// The output of the graph is not text.
    #[error("output is not text: {0}")]
    Output(#[source] serde_json::Error),
}

impl From<RunError> for InterpreterError {
    fn from(error: RunError) -> Self {
        match error {
            RunError::Graph(error) => Self::Engine(error),
            RunError::Output(error) => Self::Output(error),
            RunError::ChatHistory(error) => Self::ChatHistory(error),
        }
    }
}

/// Creates executable agents from specs.
pub struct Interpreter {
    executor: Arc<PromptExecutor>,
    tool_runtime: Option<Arc<dyn ToolRuntime>>,
    frame_sink: Option<FrameSink>,
    features: Vec<Arc<dyn Feature>>,
    interceptors: Vec<Arc<dyn Interceptor>>,
    external_tools: Vec<Arc<dyn DynTool>>,
    package_tools: Vec<ResolvedTool>,
    approval_tools: Vec<String>,
    prices: Option<Arc<grauwolf_prompt::Prices>>,
    budget: Option<grauwolf_agents::Budget>,
    tool_factory: Option<Arc<dyn ToolFactory>>,
    decider: Option<Arc<dyn Decider>>,
}

impl Interpreter {
    /// Interpreter whose agents make LLM requests via `executor`.
    pub fn new(executor: Arc<PromptExecutor>) -> Self {
        Self {
            executor,
            tool_runtime: None,
            frame_sink: None,
            features: Vec::new(),
            interceptors: Vec::new(),
            external_tools: Vec::new(),
            package_tools: Vec::new(),
            approval_tools: Vec::new(),
            prices: None,
            budget: None,
            tool_factory: None,
            decider: None,
        }
    }

    /// Provides the `system_one` decider of decision nodes (ADR-0100); the `llm` decider needs
    /// none.
    pub fn with_decider(mut self, decider: Arc<dyn Decider>) -> Self {
        self.decider = Some(decider);
        self
    }

    /// Builds tools that agents with `self_tools` create during the run (ADR-0075).
    pub fn with_tool_factory(mut self, factory: Arc<dyn ToolFactory>) -> Self {
        self.tool_factory = Some(factory);
        self
    }

    /// Provides tools from outside, e.g. from MCP servers (ADR-0036), search, memory,
    /// sub-agent or built-in tools (ADR-0050). Only those that the spec
    /// declares are offered (`AgentSpec::external_tools()`).
    pub fn with_tools(mut self, tools: Vec<Arc<dyn DynTool>>) -> Self {
        self.external_tools.extend(tools);
        self
    }

    /// Provides the tools of resolved tool packages (`tool_packages`, ADR-0107); they run via the
    /// tool runtime like the tools of the spec and are all offered to the agent.
    pub fn with_package_tools(mut self, tools: Vec<ResolvedTool>) -> Self {
        self.package_tools.extend(tools);
        self
    }

    /// Installs `interceptor` in every run; it intervenes in LLM and tool calls (ADR-0047).
    pub fn with_interceptor(mut self, interceptor: Arc<dyn Interceptor>) -> Self {
        self.interceptors.push(interceptor);
        self
    }

    /// Makes `tools` require approval (ADR-0061): every run rejects them as long as no
    /// one can grant approval.
    pub fn with_tool_approval<S: Into<String>>(
        mut self,
        tools: impl IntoIterator<Item = S>,
    ) -> Self {
        self.approval_tools
            .extend(tools.into_iter().map(Into::into));
        self
    }

    /// Limits every run to `budget` (ADR-0072).
    pub fn with_budget(mut self, budget: grauwolf_agents::Budget) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Computes costs in every run with `prices` (ADR-0071).
    pub fn with_prices(mut self, prices: Arc<grauwolf_prompt::Prices>) -> Self {
        self.prices = Some(prices);
        self
    }

    /// Installs `feature` in every run; it receives all events (F06.1).
    pub fn with_feature(mut self, feature: Arc<dyn Feature>) -> Self {
        self.features.push(feature);
        self
    }

    /// Passes the frames of streaming nodes on to `sink` (ADR-0023), e.g. for live output.
    pub fn with_frame_sink(mut self, sink: FrameSink) -> Self {
        self.frame_sink = Some(sink);
        self
    }

    /// Runs spec tools via `runtime`, e.g. in the sandbox (ADR-0014).
    pub fn with_tool_runtime(mut self, runtime: Arc<dyn ToolRuntime>) -> Self {
        self.tool_runtime = Some(runtime);
        self
    }

    /// Limits from `limits` of the spec; fields from [`Interpreter::with_budget`] take precedence
    /// (ADR-0078).
    fn budget_for(&self, spec: &AgentSpec) -> Option<grauwolf_agents::Budget> {
        let limits = spec.limits.as_ref();
        let from_spec = grauwolf_agents::Budget {
            max_tokens: limits.and_then(|limits| limits.max_tokens),
            max_cost: limits.and_then(|limits| limits.max_cost),
            max_tool_calls: limits.and_then(|limits| limits.max_tool_calls),
            max_prompt_tokens: limits.and_then(|limits| limits.max_prompt_tokens),
        };
        grauwolf_agents::run_handles::merged_budget(self.budget, from_spec)
    }

    /// Validates `spec` and builds an agent from it without tools.
    pub fn load(&self, spec: &AgentSpec) -> Result<InterpretedAgent, InterpreterError> {
        self.load_with_artifacts(spec, &[])
    }

    /// Validates `spec` and builds an executable agent from it; the tools of the spec come
    /// as prebuilt `artifacts` (ADR-0024) and run via the tool runtime.
    pub fn load_with_artifacts(
        &self,
        spec: &AgentSpec,
        artifacts: &[ToolArtifact],
    ) -> Result<InterpretedAgent, InterpreterError> {
        spec.validate().map_err(InterpreterError::Invalid)?;
        let tools = self.tools(spec, artifacts)?;
        if spec.uses_system_one() && self.decider.is_none() {
            return Err(InterpreterError::MissingDecider);
        }
        let code = self.code_modules(spec, artifacts)?;
        let graph = build_graph(spec, &spec.graph, &code)?;
        let budget = self.budget_for(spec);
        if budget.is_some_and(|budget| budget.max_cost.is_some()) && self.prices.is_none() {
            return Err(InterpreterError::CostLimitWithoutPrices);
        }
        let self_tools = match (&spec.self_tools, &self.tool_factory) {
            (None, _) => None,
            (Some(_), None) => return Err(InterpreterError::MissingToolFactory),
            (Some(self_tools), Some(factory)) => Some((self_tools.clone(), Arc::clone(factory))),
        };
        let mut approval_tools = self.approval_tools.clone();
        approval_tools.extend(spec.approval_tools());

        let mut prompt = Prompt::builder(spec.name.clone());
        if let Some(system) = &spec.system_prompt {
            prompt = prompt.system(system.clone());
        }

        Ok(InterpretedAgent {
            graph: Arc::new(graph),
            executor: Arc::clone(&self.executor),
            model: llmodel(&spec.model),
            // The tools may extend the system prompt, e.g. with the skill list (ADR-0103).
            prompt: match tools.prompt_addition() {
                Some(addition) => prompt.build().extend_system(&addition),
                None => prompt.build(),
            },
            tools,
            frame_sink: self.frame_sink.clone(),
            features: self.features.clone(),
            interceptors: self.interceptors.clone(),
            decider: self.decider.clone(),
            approval_tools,
            self_tools,
            prices: self.prices.clone(),
            budget,
            knowledge: self.knowledge_context(spec)?,
            auto_recall: spec
                .long_term_memory
                .as_ref()
                .is_some_and(|memory| memory.auto_recall),
            auto_remember: spec
                .long_term_memory
                .as_ref()
                .is_some_and(|memory| memory.auto_remember),
            remember_compressed: spec
                .long_term_memory
                .as_ref()
                .is_some_and(|memory| memory.remember_compressed),
            input_type: spec
                .types
                .get(&spec.input)
                .map(|schema| (spec.input.clone(), schema.clone())),
        })
    }

    /// The knowledge context of a spec with `knowledge.inject`: the provided search tool of
    /// `knowledge`, offered to the model or not (ADR-0113).
    fn knowledge_context(
        &self,
        spec: &AgentSpec,
    ) -> Result<Option<Arc<dyn grauwolf_tools::KnowledgeContext>>, InterpreterError> {
        let Some(knowledge) = spec.knowledge.as_ref().filter(|knowledge| knowledge.inject) else {
            return Ok(None);
        };
        grauwolf_tools::knowledge_context_of(&self.external_tools, &knowledge.tool)
            .map(Some)
            .ok_or_else(|| InterpreterError::MissingExternalTool {
                tool: knowledge.tool.clone(),
            })
    }

    /// Loads the code of the graphs (ADR-0102) from the artifacts via the tool runtime, by the
    /// pointer of their code in the spec.
    fn code_modules(
        &self,
        spec: &AgentSpec,
        artifacts: &[ToolArtifact],
    ) -> Result<CodeModules, InterpreterError> {
        let mut modules = CodeModules::new();
        for piece in spec.code_pieces() {
            let artifact = artifacts
                .iter()
                .find(|artifact| artifact.name == piece.name)
                .ok_or_else(|| InterpreterError::MissingArtifact {
                    tool: piece.name.clone(),
                })?;
            let runtime = self
                .tool_runtime
                .as_ref()
                .ok_or(InterpreterError::MissingToolRuntime)?;
            modules.insert(piece.pointer, CodeModule::load(&**runtime, artifact)?);
        }
        Ok(modules)
    }

    /// Loads the tools of the spec from their artifacts via the tool runtime.
    fn tools(
        &self,
        spec: &AgentSpec,
        artifacts: &[ToolArtifact],
    ) -> Result<ToolRegistry, InterpreterError> {
        let mut registry = ToolRegistry::builder();
        for tool in &spec.tools {
            let artifact = artifacts
                .iter()
                .find(|artifact| artifact.name == tool.name)
                .ok_or_else(|| InterpreterError::MissingArtifact {
                    tool: tool.name.clone(),
                })?;
            let runtime = self
                .tool_runtime
                .as_ref()
                .ok_or(InterpreterError::MissingToolRuntime)?;
            let descriptor = ToolDescriptor {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.args.clone(),
            };
            registry = registry.shared(runtime.instantiate(artifact, descriptor)?);
        }
        for tool in &self.package_tools {
            let runtime = self
                .tool_runtime
                .as_ref()
                .ok_or(InterpreterError::MissingToolRuntime)?;
            let descriptor = ToolDescriptor {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.args.clone(),
            };
            registry = registry.shared(runtime.instantiate_limited(
                &tool.artifact,
                descriptor,
                &tool.limits,
            )?);
        }
        // External tools (MCP, knowledge, sub-agents, built-in; ADR-0050): only the declared
        // ones are offered.
        for name in spec.external_tools() {
            let tool = self
                .external_tools
                .iter()
                .find(|tool| tool.name() == name)
                .ok_or_else(|| InterpreterError::MissingExternalTool {
                    tool: name.to_owned(),
                })?;
            registry = registry.shared(Arc::clone(tool));
        }
        Ok(registry.build())
    }
}

/// An agent as a tool of another agent (ADR-0040).
struct AgentTool {
    descriptor: ToolDescriptor,
    agent: Arc<InterpretedAgent>,
    /// Handles per call (ADR-0089); without them the agent runs as with `run_json`.
    extras: Option<ExtrasProvider>,
}

impl DynTool for AgentTool {
    fn name(&self) -> &str {
        &self.descriptor.name
    }

    fn descriptor(&self) -> ToolDescriptor {
        self.descriptor.clone()
    }

    fn call_step(
        &self,
        input: Value,
        parent: Option<grauwolf_tools::StepHandles>,
    ) -> BoxFuture<'_, Result<Value, ToolError>> {
        Box::pin(async move {
            let mut extras = match &self.extras {
                Some(extras) => extras().await,
                None => RunExtras::default(),
            };
            let parent: Option<Arc<ParentRun>> = parent.and_then(|handles| handles.downcast().ok());
            // Approvals are the calling run's, unless the extras bring their own.
            if extras.approver.is_none() {
                extras.approver = parent
                    .as_ref()
                    .and_then(|parent| parent.handle::<ToolApprover>().cloned());
            }
            self.agent
                .run_as_step(input, extras, parent)
                .await
                .map_err(|error| ToolError::Execution(error.to_string()))
        })
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let input = if self.agent.input_type.is_some() {
                args
            } else {
                let request = args["request"]
                    .as_str()
                    .ok_or_else(|| ToolError::InvalidArgs {
                        tool: self.descriptor.name.clone(),
                        reason: "missing field 'request' (text)".into(),
                    })?;
                Value::String(request.to_owned())
            };
            let extras = match &self.extras {
                Some(extras) => extras().await,
                None => RunExtras::default(),
            };
            match self.agent.run_with(input, extras).await {
                Ok(Value::String(text)) => Ok(text),
                Ok(other) => Ok(other.to_string()),
                Err(error) => Err(ToolError::Execution(error.to_string())),
            }
        })
    }
}

/// Provides the handles for one call of the agent as a tool (ADR-0089).
pub type ExtrasProvider = Arc<dyn Fn() -> BoxFuture<'static, RunExtras> + Send + Sync>;

/// Additional runtime handles for a single run (ADR-0053).
#[derive(Default, Clone)]
pub struct RunExtras {
    /// Further feature that observes only this run.
    pub feature: Option<Arc<dyn Feature>>,
    /// Frame sink of this run; replaces the interpreter's.
    pub frame_sink: Option<FrameSink>,
    /// Conversation: memory and conversation ID (ADR-0037).
    pub conversation: Option<(Arc<dyn ChatHistoryStore>, String)>,
    /// Tools for this run only; they replace same-named tools of the agent, e.g. the
    /// memory of a user (ADR-0059).
    pub tools: Vec<Arc<dyn DynTool>>,
    /// Approver for the approval-required tools of this run (ADR-0061); without one they
    /// are rejected.
    pub approver: Option<ToolApprover>,
    /// Memory for automatic recall and storage, if the spec requires it (ADR-0069).
    pub memory: Option<Arc<dyn FactMemory>>,
    /// Attachments to the input of the run, e.g. images or PDFs (ADR-0081).
    pub attachments: Vec<grauwolf_prompt::Attachment>,
    /// The SPIFFE ID of the calling workload, reported as `caller` in `agent_starting`
    /// (ADR-0109); never a token.
    pub caller: Option<String>,
    /// The trace context the caller sent (`traceparent`, `tracestate`; ADR-0123): the tracing
    /// feature starts the agent's span as a child of it.
    pub trace_parent: Option<grauwolf_agents::events::TraceParent>,
    /// The person the run acts for, from a verified Txn-Token (ADR-0111); reported as
    /// `on_behalf_of` in `agent_starting`. The token itself stays with the service.
    pub on_behalf_of: Option<grauwolf_agents::events::OnBehalfOf>,
}

/// An executable agent generated from a spec.
pub struct InterpretedAgent {
    graph: Arc<Graph<AgentContext>>,
    executor: Arc<PromptExecutor>,
    model: LlModel,
    prompt: Prompt,
    tools: ToolRegistry,
    frame_sink: Option<FrameSink>,
    features: Vec<Arc<dyn Feature>>,
    interceptors: Vec<Arc<dyn Interceptor>>,
    /// Decider of the decision nodes that name `system_one` (ADR-0100).
    decider: Option<Arc<dyn Decider>>,
    /// Tools requiring approval (ADR-0061).
    approval_tools: Vec<String>,
    /// Self-created tools: settings and factory (ADR-0075).
    self_tools: Option<(grauwolf_spec::SelfToolsSpec, Arc<dyn ToolFactory>)>,
    /// Price table for costs (ADR-0071).
    prices: Option<Arc<grauwolf_prompt::Prices>>,
    /// Limits of each run (ADR-0072).
    budget: Option<grauwolf_agents::Budget>,
    /// The knowledge base whose best sections are added to each run (`knowledge.inject`,
    /// ADR-0113).
    knowledge: Option<Arc<dyn grauwolf_tools::KnowledgeContext>>,
    /// Automatically recall facts before each run (ADR-0069).
    auto_recall: bool,
    /// Automatically store facts after each successful run (ADR-0069).
    auto_remember: bool,
    /// Store lasting facts from history compression (ADR-0079).
    remember_compressed: bool,
    /// Name and schema of the input type, if it is a named type.
    input_type: Option<(String, Value)>,
}

impl InterpretedAgent {
    /// Runs an agent with text input and output (`string`) in a fresh context.
    pub async fn run(&self, input: impl Into<String>) -> Result<String, InterpreterError> {
        let output = self.run_json(Value::String(input.into())).await?;
        serde_json::from_value(output).map_err(InterpreterError::Output)
    }

    /// Runs the agent with a JSON value; for named input/output types (ADR-0022).
    ///
    /// Inputs of named types are checked and normalized as in the generated crate
    /// (ADR-0031).
    pub async fn run_json(&self, input: Value) -> Result<Value, InterpreterError> {
        self.run_with(input, RunExtras::default()).await
    }

    /// Runs the agent with additional handles for this run only: another
    /// feature (e.g. for the events of an HTTP request), an own frame sink, optionally a
    /// conversation and tools that replace same-named ones (ADR-0037, ADR-0053, ADR-0059).
    pub async fn run_with(
        &self,
        input: Value,
        extras: RunExtras,
    ) -> Result<Value, InterpreterError> {
        self.run_as_step(input, extras, None).await
    }

    /// Like [`InterpretedAgent::run_with`]; with `parent` the run is a step of a flow: usage,
    /// cost and tool calls count on the parent's counters and its limits apply (ADR-0101).
    pub async fn run_as_step(
        &self,
        input: Value,
        extras: RunExtras,
        parent: Option<Arc<ParentRun>>,
    ) -> Result<Value, InterpreterError> {
        let input = self.normalize_input(input)?;
        let ctx = self.equip(
            AgentContext::new(
                Arc::clone(&self.executor),
                self.model.clone(),
                self.prompt.clone(),
            ),
            extras.approver.clone(),
            extras.tools,
            Vec::new(),
        );
        let ctx = match &parent {
            Some(parent) => ctx.adopt_run(parent),
            None => ctx,
        };
        let mut ctx = with_handles(ctx, extras.frame_sink, extras.feature);
        let mut run = self
            .lifecycle(input.clone(), extras.memory)
            .with_caller(extras.caller)
            .with_trace_parent(extras.trace_parent)
            .with_on_behalf_of(extras.on_behalf_of)
            .with_attachments(extras.attachments);
        if let Some((store, id)) = &extras.conversation {
            run = run.with_conversation(Arc::clone(store), id);
        }
        let graph = Arc::clone(&self.graph);
        Ok(run
            .run(&mut ctx, move |ctx, observer| {
                Box::pin(async move { graph.run_observed(ctx, input, observer).await })
            })
            .await?)
    }

    /// The run lifecycle of this agent for `input` with the automatic memory and the knowledge
    /// context of the spec (ADR-0116).
    fn lifecycle(&self, input: Value, memory: Option<Arc<dyn FactMemory>>) -> AgentRun {
        let mut run = AgentRun::new(input);
        if let Some(memory) = memory {
            run = self.with_memory(run, memory);
        }
        if let Some(knowledge) = &self.knowledge {
            run = run.with_knowledge(Arc::clone(knowledge));
        }
        run
    }

    /// The memory options of the spec for `run` (ADR-0069, ADR-0079).
    fn with_memory(&self, run: AgentRun, memory: Arc<dyn FactMemory>) -> AgentRun {
        let run = run.with_memory(memory, self.auto_recall, self.auto_remember);
        if self.remember_compressed {
            run.remember_compressed()
        } else {
            run
        }
    }

    /// JSON schema of the input if it is a named type; `None` for text input (ADR-0093).
    pub fn input_schema(&self) -> Option<&Value> {
        self.input_type.as_ref().map(|(_, schema)| schema)
    }

    /// Offers the agent as a tool `name` (F03.12, ADR-0040): every call runs like
    /// [`InterpretedAgent::run_json`] in a fresh context. With text input the
    /// argument is `{ "request": "<text>" }`, with a named type its schema; structured
    /// outputs come back as JSON text.
    pub fn into_tool(
        self: Arc<Self>,
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Arc<dyn DynTool> {
        self.agent_tool(name.into(), description.into(), None)
    }

    /// Like [`InterpretedAgent::into_tool`]; every call runs with the handles from `extras`,
    /// e.g. the tools and memory of the user of an MCP request (ADR-0089).
    pub fn into_tool_with(
        self: Arc<Self>,
        name: impl Into<String>,
        description: impl Into<String>,
        extras: ExtrasProvider,
    ) -> Arc<dyn DynTool> {
        self.agent_tool(name.into(), description.into(), Some(extras))
    }

    fn agent_tool(
        self: Arc<Self>,
        name: String,
        description: String,
        extras: Option<ExtrasProvider>,
    ) -> Arc<dyn DynTool> {
        let parameters = match &self.input_type {
            Some((_, schema)) => schema.clone(),
            None => serde_json::json!({
                "type": "object",
                "properties": {
                    "request": { "type": "string", "description": "Request to the agent" }
                },
                "required": ["request"]
            }),
        };
        Arc::new(AgentTool {
            descriptor: ToolDescriptor {
                name,
                description,
                parameters,
            },
            agent: self,
            extras,
        })
    }

    /// Like [`InterpretedAgent::run_json`], but saves the run state before each node under
    /// `run_id` in `store`, so that [`InterpretedAgent::resume`] can continue after an abort
    /// (ADR-0020).
    pub async fn run_checkpointed(
        &self,
        input: Value,
        store: Arc<dyn CheckpointStore>,
        run_id: &str,
    ) -> Result<Value, InterpreterError> {
        self.run_checkpointed_with(input, store, run_id, RunExtras::default())
            .await
    }

    /// Like [`InterpretedAgent::run_checkpointed`] with handles for this run only, as with
    /// [`InterpretedAgent::run_with`]: tools, approver, memory, feature, frame sink. A
    /// conversation is not possible with checkpoints (ADR-0077).
    pub async fn run_checkpointed_with(
        &self,
        input: Value,
        store: Arc<dyn CheckpointStore>,
        run_id: &str,
        extras: RunExtras,
    ) -> Result<Value, InterpreterError> {
        if extras.conversation.is_some() {
            return Err(InterpreterError::CheckpointConversation);
        }
        let input = self.normalize_input(input)?;
        let ctx = self.equip(
            AgentContext::new(
                Arc::clone(&self.executor),
                self.model.clone(),
                self.prompt.clone(),
            ),
            extras.approver.clone(),
            extras.tools,
            Vec::new(),
        );
        let mut ctx = with_handles(ctx, extras.frame_sink, extras.feature);
        // Attachments attach to the input that the first LLM node attaches (ADR-0081).
        let run = self
            .lifecycle(input.clone(), extras.memory)
            .with_caller(extras.caller)
            .with_trace_parent(extras.trace_parent)
            .with_on_behalf_of(extras.on_behalf_of)
            .with_attachments(extras.attachments)
            .with_checkpoints(store, run_id);
        let graph = Arc::clone(&self.graph);
        Ok(run
            .run(&mut ctx, move |ctx, observer| {
                Box::pin(async move { graph.run_observed(ctx, input, observer).await })
            })
            .await?)
    }

    /// Rebuilds the self-made tools of a checkpoint via the factory (ADR-0077).
    async fn rebuild(
        &self,
        definitions: &[NewTool],
    ) -> Result<Vec<(Arc<dyn DynTool>, NewTool)>, InterpreterError> {
        let mut made = Vec::new();
        for definition in definitions {
            let failed = |reason: String| InterpreterError::SelfToolRebuild {
                tool: definition.name.clone(),
                reason,
            };
            let factory = self
                .self_tools
                .as_ref()
                .map(|(_, factory)| Arc::clone(factory))
                .ok_or_else(|| failed("the spec does not allow self-made tools".into()))?;
            let tool = factory.create(definition.clone()).await.map_err(failed)?;
            made.push((tool, definition.clone()));
        }
        Ok(made)
    }

    /// Runs the agent as part of the conversation `conversation_id` (F07.1, ADR-0037): the
    /// stored history follows the system prompt; after a successful run, the history is
    /// stored without the system prompt.
    pub async fn run_conversation(
        &self,
        input: Value,
        store: Arc<dyn ChatHistoryStore>,
        conversation_id: &str,
    ) -> Result<Value, InterpreterError> {
        let extras = RunExtras {
            conversation: Some((store, conversation_id.to_owned())),
            ..RunExtras::default()
        };
        self.run_with(input, extras).await
    }

    /// Continues the run `run_id` at the last checkpoint in `store`; work before it is not
    /// repeated (ADR-0034).
    pub async fn resume(
        &self,
        store: Arc<dyn CheckpointStore>,
        run_id: &str,
    ) -> Result<Value, InterpreterError> {
        self.resume_with(store, run_id, RunExtras::default()).await
    }

    /// Like [`InterpretedAgent::resume`] with handles for the continued run, as with
    /// [`InterpretedAgent::run_checkpointed_with`] (ADR-0077).
    pub async fn resume_with(
        &self,
        store: Arc<dyn CheckpointStore>,
        run_id: &str,
        extras: RunExtras,
    ) -> Result<Value, InterpreterError> {
        if extras.conversation.is_some() {
            return Err(InterpreterError::CheckpointConversation);
        }
        let state = store
            .load(run_id)
            .await
            .map_err(InterpreterError::Checkpoint)?
            .ok_or_else(|| InterpreterError::NoCheckpoint {
                run_id: run_id.to_owned(),
            })?;
        let made = self.rebuild(&state.context.self_tools).await?;
        let ctx = self.equip(
            AgentContext::restore(Arc::clone(&self.executor), state.context),
            extras.approver.clone(),
            extras.tools,
            made,
        );
        let mut ctx = with_handles(ctx, extras.frame_sink, extras.feature);
        let mut run = AgentRun::new(state.checkpoint.input.clone())
            .with_caller(extras.caller)
            .with_trace_parent(extras.trace_parent)
            .with_on_behalf_of(extras.on_behalf_of)
            .with_checkpoints(store, run_id)
            .resumed();
        if let Some(memory) = extras.memory {
            run = self.with_memory(run, memory);
        }
        let checkpoint = state.checkpoint;
        let graph = Arc::clone(&self.graph);
        Ok(run
            .run(&mut ctx, move |ctx, observer| {
                Box::pin(async move { graph.resume(ctx, checkpoint, observer).await })
            })
            .await?)
    }

    /// Checks and normalizes inputs of named types (ADR-0031).
    fn normalize_input(&self, input: Value) -> Result<Value, InterpreterError> {
        match &self.input_type {
            Some((name, schema)) => {
                normalize(schema, &input).map_err(|source| InterpreterError::Input {
                    name: name.clone(),
                    source,
                })
            }
            None => Ok(input),
        }
    }

    /// Equips a context with the runtime handles: tools, frame sink and features.
    /// Equips a fresh context; `tools` replace same-named ones only for this run
    /// (ADR-0059), with `self_tools`, `create_tool` comes along with its own tool level (ADR-0075).
    fn equip(
        &self,
        ctx: AgentContext,
        approver: Option<ToolApprover>,
        tools: Vec<Arc<dyn DynTool>>,
        made: Vec<(Arc<dyn DynTool>, NewTool)>,
    ) -> AgentContext {
        let mut registry = self.tools.clone().extended(tools);
        let mut self_tools = None;
        if let Some((spec, factory)) = &self.self_tools {
            self_tools = Some((spec.tool.clone(), spec.enabled));
            let shelf = ToolShelf::default();
            for (tool, definition) in made {
                // Names are unique in the checkpoint; an error is not possible here.
                let _ = shelf.put_made(tool, definition);
            }
            let create = self_tools::CreateTool {
                spec: spec.clone(),
                factory: Arc::clone(factory),
                shelf: shelf.clone(),
                taken: registry
                    .descriptors()
                    .into_iter()
                    .map(|tool| tool.name)
                    .collect(),
            };
            registry = registry
                .extended([Arc::new(create) as Arc<dyn DynTool>])
                .with_shelf(shelf);
        }
        let mut ctx = ctx.with_tools(registry);
        // Sub-agents of a flow run with the approver of this run (ADR-0101).
        if let Some(approver) = &approver {
            ctx = ctx.with_run_handle(Arc::new(approver.clone()));
        }
        if let Some((tool, enabled)) = self_tools {
            ctx = ctx.with_self_tools(tool, enabled);
        }
        if let Some(prices) = &self.prices {
            ctx = ctx.with_prices(Arc::clone(prices));
        }
        if let Some(budget) = self.budget {
            ctx = ctx.with_budget(budget);
        }
        if let Some(decider) = &self.decider {
            ctx = ctx.with_decider(Arc::clone(decider));
        }
        if let Some(sink) = &self.frame_sink {
            ctx = ctx.with_frame_sink(Arc::clone(sink));
        }
        let mut ctx = self
            .features
            .iter()
            .fold(ctx, |ctx, feature| ctx.install(Arc::clone(feature)));
        for interceptor in &self.interceptors {
            ctx.intercept(Arc::clone(interceptor));
        }
        if !self.approval_tools.is_empty() {
            ctx.intercept(Arc::new(grauwolf_agents::approval::ApprovalGate {
                tools: self.approval_tools.clone(),
                approver,
            }));
        }
        ctx
    }
}

/// Gives the context the frame sink and feature of a single run.
fn with_handles(
    mut ctx: AgentContext,
    sink: Option<FrameSink>,
    feature: Option<Arc<dyn Feature>>,
) -> AgentContext {
    if let Some(sink) = sink {
        ctx = ctx.with_frame_sink(sink);
    }
    if let Some(feature) = feature {
        ctx = ctx.install(feature);
    }
    ctx
}

/// Node function of a `plan` node: an LLM planner whose steps the subgraph from
/// `params.subgraph` runs with its tool subset (ADR-0039).
fn plan_node(
    spec: &AgentSpec,
    node_id: &str,
    params: &Value,
    code: &CodeModules,
) -> Result<NodeFn<AgentContext>, InterpreterError> {
    let subgraph = params
        .get("subgraph")
        .and_then(Value::as_str)
        .and_then(|name| spec.subgraphs.get(name))
        .ok_or_else(|| InterpreterError::MissingImplementation {
            kind: PLAN_KIND.into(),
        })?;
    let mut planner = LlmPlanner::new(Arc::new(build_graph(spec, &subgraph.graph, code)?));
    if let Some(scope) = subgraph_scope(subgraph) {
        planner = planner.with_step_scope(scope);
    }
    if params["critic"].as_bool() == Some(true) {
        planner = planner.with_critic();
    }
    let max_iterations = params["max_iterations"]
        .as_u64()
        .unwrap_or(DEFAULT_PLAN_ITERATIONS);
    Ok(erase_node(grauwolf_planner::plan_node(
        Arc::new(planner),
        usize::try_from(max_iterations).unwrap_or(usize::MAX),
        node_id,
    )))
}

/// Node function of a `goap` node (ADR-0070): for each action with a subgraph, its graph with
/// its scope (an action with an agent runs the sub-agent as a step); `grauwolf_planner::goap::fact_planner` builds the planner as in the generated code.
fn goap_node(
    spec: &AgentSpec,
    node_id: &str,
    params: &Value,
    code: &CodeModules,
) -> Result<NodeFn<AgentContext>, InterpreterError> {
    let missing = || InterpreterError::MissingImplementation {
        kind: GOAP_KIND.into(),
    };
    let mut steps = std::collections::HashMap::new();
    for action in params["actions"].as_array().into_iter().flatten() {
        // Actions with an `agent` run it as a step and need no graph.
        let Some(name) = action["subgraph"].as_str() else {
            continue;
        };
        if steps.contains_key(name) {
            continue;
        }
        let subgraph = spec.subgraphs.get(name).ok_or_else(missing)?;
        let graph = build_graph(spec, &subgraph.graph, code)?;
        // The scope of the subgraph (tools, model) applies only during the action.
        let graph = match subgraph_scope(subgraph) {
            Some(scope) => Graph::builder()
                .subgraph("step", graph, Some(scope))
                .edge(grauwolf_graph::START, "step")
                .edge("step", grauwolf_graph::FINISH)
                .build()?,
            None => graph,
        };
        steps.insert(name.to_owned(), Arc::new(graph));
    }
    let planner = grauwolf_planner::goap::fact_planner(params, &steps).map_err(|_| missing())?;
    let max_iterations = params["max_iterations"]
        .as_u64()
        .unwrap_or(DEFAULT_PLAN_ITERATIONS);
    Ok(erase_node(grauwolf_planner::planner_node(
        Arc::new(planner),
        usize::try_from(max_iterations).unwrap_or(usize::MAX),
        node_id,
    )))
}

/// Node function of a `parallel` node: branches from `params.subgraphs` with their scopes,
/// merged according to `params.merge` (ADR-0045).
fn parallel_node(
    spec: &AgentSpec,
    node_id: &str,
    params: &Value,
    code: &CodeModules,
) -> Result<NodeFn<AgentContext>, InterpreterError> {
    let missing = || InterpreterError::MissingImplementation {
        kind: PARALLEL_KIND.into(),
    };
    let mut branches = Vec::new();
    for name in params["subgraphs"].as_array().into_iter().flatten() {
        let name = name.as_str().ok_or_else(missing)?;
        let subgraph = spec.subgraphs.get(name).ok_or_else(missing)?;
        let mut branch = Branch::new(name, build_graph(spec, &subgraph.graph, code)?);
        if let Some(scope) = subgraph_scope(subgraph) {
            branch = branch.with_scope(scope);
        }
        branches.push(branch);
    }
    let merge = match params["merge"].as_str() {
        Some("concat") => Merge::Concat,
        Some("index") => Merge::Index(
            usize::try_from(params["index"].as_u64().ok_or_else(missing)?)
                .map_err(|_| missing())?,
        ),
        Some("llm_select") => Merge::LlmSelect {
            criteria: params["criteria"]
                .as_str()
                .unwrap_or(nodes::DEFAULT_BEST_OF_CRITERIA)
                .to_owned(),
        },
        _ => return Err(missing()),
    };
    let branches = Arc::new(branches);
    let key = progress_key(node_id);
    Ok(Box::new(move |ctx: &mut AgentContext, input: Value| {
        let branches = Arc::clone(&branches);
        let (merge, key) = (merge.clone(), key.clone());
        Box::pin(async move { run_parallel_resumable(ctx, &branches, input, &merge, &key).await })
    }))
}

/// Node function of a `for_each` node: the body per element, with the parallelism and the error
/// handling from the parameters (ADR-0101).
fn for_each_node(
    spec: &AgentSpec,
    node_id: &str,
    params: &Value,
    code: &CodeModules,
) -> Result<NodeFn<AgentContext>, InterpreterError> {
    let missing = || InterpreterError::MissingImplementation {
        kind: FOR_EACH_KIND.into(),
    };
    let parsed = ForEachParams::parse(Some(params)).map_err(|_| missing())?;
    let body = match parsed.body {
        ForEachBody::Agent(name) => ForEachRunBody::Agent(name),
        ForEachBody::Subgraph(name) => {
            let subgraph = spec.subgraphs.get(&name).ok_or_else(missing)?;
            ForEachRunBody::Graph {
                name,
                graph: build_graph(spec, &subgraph.graph, code)?,
                scope: subgraph_scope(subgraph),
            }
        }
    };
    let mut config = ForEach::new(body)
        .with_max_parallel(parsed.max_parallel)
        .with_on_error(match parsed.on_error {
            ForEachOnError::Fail => OnError::Fail,
            ForEachOnError::Skip => OnError::Skip,
        });
    if parsed.output == "string" {
        config = config.with_text_output();
    }
    let config = Arc::new(config);
    let node_id = node_id.to_owned();
    Ok(Box::new(move |ctx: &mut AgentContext, input: Value| {
        let (config, node_id) = (Arc::clone(&config), node_id.clone());
        Box::pin(async move { run_for_each(ctx, &config, &node_id, input).await })
    }))
}

/// Scope of a subgraph: tool subset, permission to build, and own model (ADR-0034,
/// ADR-0043, ADR-0075).
fn subgraph_scope(subgraph: &SubgraphSpec) -> Option<Box<dyn Scope<AgentContext>>> {
    if subgraph.tools.is_none() && subgraph.model.is_none() && subgraph.self_tools.is_none() {
        return None;
    }
    let mut scope = SubgraphScope::new();
    if let Some(tools) = &subgraph.tools {
        scope = scope.with_tools(tools.iter().cloned());
    }
    if let Some(allowed) = subgraph.self_tools {
        scope = scope.with_self_tools(allowed);
    }
    if let Some(model) = &subgraph.model {
        scope = scope.with_model(llmodel(model));
    }
    Some(Box::new(scope))
}

/// The loaded code of the graphs by the JSON pointer of its code in the spec (ADR-0102).
type CodeModules = std::collections::HashMap<String, CodeModule>;

/// Builds the graph `graph` of the spec; nodes of kind `subgraph` are attached recursively
/// with their tool subset as scope (ADR-0034). The spec is validated, so there are no cycles.
fn build_graph(
    spec: &AgentSpec,
    graph: &GraphSpec,
    code: &CodeModules,
) -> Result<Graph<AgentContext>, InterpreterError> {
    let path = spec.graph_pointer(graph).unwrap_or_default();
    let module = |pointer: String, kind: &str| {
        code.get(&pointer)
            .cloned()
            .ok_or_else(|| InterpreterError::MissingImplementation { kind: kind.into() })
    };
    let mut builder = Graph::builder();
    // Step limit per graph run, also for subgraphs (ADR-0078).
    if let Some(steps) = spec.limits.as_ref().and_then(|limits| limits.max_steps) {
        builder = builder.max_steps(steps);
    }
    for (index, node) in graph.nodes.iter().enumerate() {
        let params = node.params.clone().unwrap_or(Value::Null);
        if node.kind == CODE_KIND {
            let module = module(format!("{path}/nodes/{index}/params/code"), CODE_KIND)?;
            builder = builder.node(node.id.clone(), code_node(module));
            continue;
        }
        if node.kind == SUBGRAPH_KIND {
            let subgraph = params
                .get("subgraph")
                .and_then(Value::as_str)
                .and_then(|name| spec.subgraphs.get(name))
                .ok_or_else(|| InterpreterError::MissingImplementation {
                    kind: node.kind.clone(),
                })?;
            let scope = subgraph_scope(subgraph);
            builder = builder.subgraph(
                node.id.clone(),
                build_graph(spec, &subgraph.graph, code)?,
                scope,
            );
            continue;
        }
        if node.kind == PLAN_KIND {
            builder = builder.node(node.id.clone(), plan_node(spec, &node.id, &params, code)?);
            continue;
        }
        if node.kind == GOAP_KIND {
            builder = builder.node(node.id.clone(), goap_node(spec, &node.id, &params, code)?);
            continue;
        }
        if node.kind == PARALLEL_KIND {
            builder = builder.node(
                node.id.clone(),
                parallel_node(spec, &node.id, &params, code)?,
            );
            continue;
        }
        if node.kind == FOR_EACH_KIND {
            builder = builder.node(
                node.id.clone(),
                for_each_node(spec, &node.id, &params, code)?,
            );
            continue;
        }
        // Decisions report their events under the node's ID (ADR-0100).
        let implemented = if node.kind == "decide" {
            implementations::decide(&node.id, &params)
        } else {
            implementations::node(&node.kind, &params, &spec.types)
        };
        let run = implemented.ok_or_else(|| InterpreterError::MissingImplementation {
            kind: node.kind.clone(),
        })?;
        builder = builder.node(node.id.clone(), run);
    }
    for (index, edge) in graph.edges.iter().enumerate() {
        let (from, to) = (edge.from.clone(), edge.to.clone());
        let condition = match &edge.when {
            None => None,
            Some(when) if when.kind == ON_CODE_KIND => Some(code_condition(module(
                format!("{path}/edges/{index}/when/params/code"),
                ON_CODE_KIND,
            )?)),
            Some(when) => {
                let params = when.params.clone().unwrap_or(Value::Null);
                Some(
                    implementations::condition(&when.kind, &params).ok_or_else(|| {
                        InterpreterError::MissingImplementation {
                            kind: when.kind.clone(),
                        }
                    })?,
                )
            }
        };
        let transform = match &edge.transform {
            None => None,
            Some(_) => Some(code_transform(module(
                format!("{path}/edges/{index}/transform/code"),
                "transform",
            )?)),
        };
        builder = match (condition, transform) {
            (None, None) => builder.edge(from, to),
            (Some(condition), None) => builder.edge_when(from, to, condition),
            (None, Some(transform)) => builder.edge_transformed(from, to, transform),
            (Some(condition), Some(transform)) => {
                builder.edge_when_transformed(from, to, condition, transform)
            }
        };
    }
    Ok(builder.build()?)
}

/// The model of a spec together with its fallback models and sampling settings (ADR-0088,
/// ADR-0097).
fn llmodel(model: &grauwolf_spec::ModelSpec) -> LlModel {
    let sampled = |provider: &str, id: &str, temperature: Option<f64>, think: Option<bool>| {
        let mut model = LlModel::new(provider, id);
        model.temperature = temperature;
        model.think = think;
        model
    };
    sampled(&model.provider, &model.id, model.temperature, model.think).with_fallbacks(
        model
            .fallbacks
            .iter()
            .map(|fallback| {
                sampled(
                    &fallback.provider,
                    &fallback.id,
                    fallback.temperature,
                    fallback.think,
                )
            })
            .collect(),
    )
}
