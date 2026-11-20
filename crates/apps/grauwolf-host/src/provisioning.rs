//! Provisioning (ADR-0114): prepares everything an agent run needs from a loaded, resolved spec –
//! model access, the decider, the tools from outside the graph (MCP servers, knowledge, built-in
//! groups, skills, memory, sub-agents) and the long-term memory.
//!
//! Two stages:
//! - a [`ProvisionedAgent`] is built once per loaded agent and shared by all its runs;
//! - a [`ProvisionedRun`] is a cheap view per run and user: the user's memory tools and facts,
//!   the user's key-value store, the tools of `tools_dir` (reloaded per run) and the sub-agents
//!   that run on behalf of that user.
//!
//! Everything in here reads its configuration from the [`Env`] it is given
//! ([`ProvisionOptions::new`]); the process environment only appears at the entry points.

use std::path::Path;
#[cfg(feature = "builtin")]
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_agents::decision::Decider;
use grauwolf_agents::memory::FactMemory;
use grauwolf_interpreter::RunExtras;
use grauwolf_llm::{PromptCache, PromptExecutor};
use grauwolf_rag::Embedder;
use grauwolf_spec::{AgentSpec, ModelSpec};
use grauwolf_toolbuild::ToolBuilder;
use grauwolf_tools::{DynTool, ResolvedTool};

use crate::app::AgentFeatures;
#[cfg(feature = "builtin")]
use crate::builtin::{ConsoleUserIo, UserIo};
use crate::config::{Env, ProvidersConfig};
use crate::long_term_memory::LongTermMemory;
use crate::mcp_observer::McpToolsObserver;
use crate::providers::{embedder_with, executor_builder_with};

/// Where the sub-agents of a spec come from (ADR-0114): provisioning asks for them once when it
/// builds the agent and is handed everything it needs to run them for a user. The loader
/// implements this port (it reads the sub-agent specs and provisions them in turn).
pub trait SubAgentSupplier: Send + Sync {
    /// The sub-agents named in `spec.agent_tools` (paths relative to `base_dir`).
    fn supply<'a>(
        &'a self,
        spec: &'a AgentSpec,
        base_dir: &'a Path,
    ) -> BoxFuture<'a, Result<Vec<SubAgent>, String>>;
}

/// Reports a file of the tools directory that cannot be used.
pub type SkippedToolReport = Arc<dyn Fn(&str) + Send + Sync>;

/// The tool a sub-agent is for one user (without: the calling run has no user).
pub type SubAgentTool = Arc<dyn Fn(Option<&str>) -> Arc<dyn DynTool> + Send + Sync>;

/// A sub-agent as a tool that runs on behalf of the user of the calling run (ADR-0114).
#[derive(Clone)]
pub struct SubAgent {
    /// The name of the tool (the key in `agent_tools`).
    pub name: String,
    tool: SubAgentTool,
}

impl SubAgent {
    /// A sub-agent whose tool for a user is `tool(user)`.
    pub fn new(
        name: impl Into<String>,
        tool: impl Fn(Option<&str>) -> Arc<dyn DynTool> + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            tool: Arc::new(tool),
        }
    }

    /// A sub-agent whose tool does not depend on the user (e.g. a remote agent).
    pub fn fixed(tool: Arc<dyn DynTool>) -> Self {
        Self {
            name: tool.name().to_owned(),
            tool: Arc::new(move |_| Arc::clone(&tool)),
        }
    }

    /// The tool of this sub-agent for a run of `user`.
    pub fn tool_for(&self, user: Option<&str>) -> Arc<dyn DynTool> {
        (self.tool)(user)
    }
}

/// What a [`ProvisionedAgent`] is built with: the environment, user input/output, the decider and
/// the observers, the supplier of sub-agents (ADR-0114).
#[derive(Clone)]
pub struct ProvisionOptions {
    env: Arc<dyn Env>,
    chat_api: bool,
    #[cfg(feature = "builtin")]
    user_io: Arc<dyn UserIo>,
    decider: Option<Arc<dyn Decider>>,
    mcp_tools_observer: Option<McpToolsObserver>,
    agent_features: Option<AgentFeatures>,
    llm_cache: Option<Arc<dyn PromptCache>>,
    tool_builder: Option<ToolBuilder>,
    on_skipped_tool: Option<SkippedToolReport>,
    sub_agents: Option<Arc<dyn SubAgentSupplier>>,
    executor: Option<Arc<PromptExecutor>>,
    embedder: Option<Arc<dyn Embedder>>,
    prices: Option<Arc<grauwolf_prompt::Prices>>,
}

impl ProvisionOptions {
    /// Options that read all configuration (provider settings, credentials, `${VAR}` in
    /// headers, the values the command tool passes on) from `env`.
    pub fn new(env: Arc<dyn Env>) -> Self {
        Self {
            env,
            chat_api: false,
            #[cfg(feature = "builtin")]
            user_io: Arc::new(ConsoleUserIo),
            decider: None,
            mcp_tools_observer: None,
            agent_features: None,
            llm_cache: None,
            tool_builder: None,
            on_skipped_tool: None,
            sub_agents: None,
            executor: None,
            embedder: None,
            prices: None,
        }
    }

    /// The environment of these options.
    pub fn env(&self) -> &Arc<dyn Env> {
        &self.env
    }

    /// Chat API mode: `OLLAMA_BASE_URL` stands in for a missing `OLLAMA_HOST` (ADR-0094).
    pub fn with_chat_api_mode(mut self, on: bool) -> Self {
        self.chat_api = on;
        self
    }

    /// The user tools (`say_to_user`, `ask_user`) talk via `io`.
    #[cfg(feature = "builtin")]
    pub fn with_user_io(mut self, io: Arc<dyn UserIo>) -> Self {
        self.user_io = io;
        self
    }

    /// The user I/O of these options.
    #[cfg(feature = "builtin")]
    pub fn user_io(&self) -> &Arc<dyn UserIo> {
        &self.user_io
    }

    /// The `system_one` decider to use instead of the service from the environment (ADR-0100).
    pub fn with_decider(mut self, decider: Option<Arc<dyn Decider>>) -> Self {
        self.decider = decider;
        self
    }

    /// Lets `observer` see the tool lists of the MCP servers (chat API, ADR-0094).
    pub fn with_mcp_tools_observer(mut self, observer: Option<McpToolsObserver>) -> Self {
        self.mcp_tools_observer = observer;
        self
    }

    /// The observer of the MCP tool lists.
    pub fn mcp_tools_observer(&self) -> Option<&McpToolsObserver> {
        self.mcp_tools_observer.as_ref()
    }

    /// Features for the sub-agents a loader supplies (ADR-0094).
    pub fn with_agent_features(mut self, features: Option<AgentFeatures>) -> Self {
        self.agent_features = features;
        self
    }

    /// The features for sub-agents.
    pub fn agent_features(&self) -> Option<&AgentFeatures> {
        self.agent_features.as_ref()
    }

    /// Answers identical LLM requests from `cache` (ADR-0086).
    pub fn with_llm_cache(mut self, cache: Option<Arc<dyn PromptCache>>) -> Self {
        self.llm_cache = cache;
        self
    }

    /// Builds the tools of the spec's `tools_dir` with `builder` (the tool cache, ADR-0074).
    pub fn with_tool_builder(mut self, builder: ToolBuilder) -> Self {
        self.tool_builder = Some(builder);
        self
    }

    /// Reports files of the tools directory that cannot be used (reason as text).
    pub fn with_skipped_tool_report(mut self, report: SkippedToolReport) -> Self {
        self.on_skipped_tool = Some(report);
        self
    }

    /// Where the sub-agents come from; without a supplier a spec with `agent_tools` is an error.
    pub fn with_sub_agents(mut self, supplier: Arc<dyn SubAgentSupplier>) -> Self {
        self.sub_agents = Some(supplier);
        self
    }

    /// The price table of the agent: events carry the cost, `limits.max_cost` can end a run
    /// (ADR-0071, ADR-0119).
    pub fn with_prices(mut self, prices: Option<Arc<grauwolf_prompt::Prices>>) -> Self {
        self.prices = prices;
        self
    }

    /// Uses `executor` instead of building one from the providers of the spec (own providers,
    /// tests).
    pub fn with_executor(mut self, executor: Arc<PromptExecutor>) -> Self {
        self.executor = Some(executor);
        self
    }

    /// Uses `embedder` for every embedding model of the spec (own providers, tests).
    pub fn with_embedder(mut self, embedder: Arc<dyn Embedder>) -> Self {
        self.embedder = Some(embedder);
        self
    }

    #[cfg_attr(
        not(all(
            feature = "mcp",
            feature = "builtin",
            feature = "sandbox",
            feature = "a2a"
        )),
        allow(dead_code)
    )]
    pub(crate) fn chat_api(&self) -> bool {
        self.chat_api
    }

    #[cfg_attr(
        not(all(
            feature = "mcp",
            feature = "builtin",
            feature = "sandbox",
            feature = "a2a"
        )),
        allow(dead_code)
    )]
    pub(crate) fn llm_cache(&self) -> Option<&Arc<dyn PromptCache>> {
        self.llm_cache.as_ref()
    }

    fn providers(&self) -> ProvidersConfig {
        let providers = ProvidersConfig::from_env(&*self.env);
        let on = providers.chat_api_mode || self.chat_api;
        providers.with_chat_api_mode(on)
    }

    fn embedder_for(&self, model: &ModelSpec) -> Result<Arc<dyn Embedder>, String> {
        match &self.embedder {
            Some(embedder) => Ok(Arc::clone(embedder)),
            None => embedder_with(&model.provider, &model.id, &self.providers())
                .map_err(|error| error.to_string()),
        }
    }
}

/// An agent provisioned once and shared by all its runs (ADR-0114).
pub struct ProvisionedAgent {
    executor: Arc<PromptExecutor>,
    decider: Option<Arc<dyn Decider>>,
    /// MCP, knowledge, built-in groups and skills: the same for every run.
    tools: Vec<Arc<dyn DynTool>>,
    memory: Option<Arc<LongTermMemory>>,
    memory_tools: bool,
    automatic_memory: bool,
    #[cfg(feature = "builtin")]
    key_value: Option<(grauwolf_spec::BuiltinToolsSpec, PathBuf)>,
    sub_agents: Vec<SubAgent>,
    #[cfg(feature = "sandbox")]
    tools_dir: Option<Arc<crate::tools_dir::ToolsDir>>,
    on_skipped_tool: Option<SkippedToolReport>,
    package_tools: Vec<ResolvedTool>,
    prices: Option<Arc<grauwolf_prompt::Prices>>,
}

impl ProvisionedAgent {
    /// Provisions `spec` (loaded and resolved: code files read, git skills localised, packages
    /// resolved); paths in it are relative to `base_dir`. Everything that can fail – a missing
    /// credential, a feature that is not compiled in, an unreachable MCP server, a knowledge
    /// source that cannot be read – fails here, before the first run.
    pub async fn new(
        spec: &AgentSpec,
        base_dir: &Path,
        options: ProvisionOptions,
    ) -> Result<Self, String> {
        // The price table: the one handed in, otherwise the file `GRAUWOLF_PRICES` names
        // (relative to `base_dir`), as `--prices` does for the CLI (ADR-0119).
        let prices = match &options.prices {
            Some(prices) => Some(Arc::clone(prices)),
            None => crate::config::PricesConfig::from_env(&*options.env)
                .file
                .map(|file| crate::prices::load(&base_dir.join(file)).map(Arc::new))
                .transpose()?,
        };
        if spec
            .limits
            .as_ref()
            .is_some_and(|limits| limits.max_cost.is_some())
            && prices.is_none()
        {
            return Err(
                "`limits.max_cost` needs a price table (`GRAUWOLF_PRICES`, CLI `--prices`)"
                    .to_owned(),
            );
        }
        let providers = options.providers();
        let executor = match &options.executor {
            Some(executor) => Arc::clone(executor),
            None => {
                let mut builder = executor_builder_with(spec.providers(), &providers)
                    .map_err(|error| error.to_string())?;
                if let Some(cache) = &options.llm_cache {
                    builder = builder.cache(Arc::clone(cache));
                }
                Arc::new(builder.build())
            }
        };
        let decider = decider(spec, options.decider.clone(), &*options.env)?;
        let mut tools = mcp_tools(spec, &options).await?;
        // Knowledge is re-indexed on every start; the index lives in memory (ADR-0038).
        if let Some(knowledge) = &spec.knowledge {
            let embedder = options.embedder_for(&knowledge.embedding_model)?;
            tools.push(
                crate::knowledge::search_tool_in(
                    &spec.name,
                    knowledge,
                    base_dir,
                    embedder,
                    &*options.env,
                )
                .await?,
            );
        }
        let memory = match &spec.long_term_memory {
            Some(memory) => {
                let embedder = options.embedder_for(&memory.embedding_model)?;
                Some(
                    LongTermMemory::open_in(&spec.name, memory, base_dir, embedder, &*options.env)
                        .await?,
                )
            }
            None => None,
        };
        #[cfg(feature = "builtin")]
        let key_value = spec
            .builtin_tools
            .as_ref()
            .map(|builtin| (builtin.clone(), base_dir.to_path_buf()));
        tools.extend(builtin_tools(spec, base_dir, &options).await?);
        tools.extend(skill_tools(spec, base_dir)?);
        let sub_agents = match (spec.agent_tools.is_empty(), &options.sub_agents) {
            (true, _) => Vec::new(),
            (false, Some(supplier)) => supplier.supply(spec, base_dir).await?,
            (false, None) => {
                return Err(
                    "the spec has `agent_tools`, but no supplier of sub-agents is given".to_owned(),
                );
            }
        };
        #[cfg(feature = "sandbox")]
        let tools_dir = match (&spec.tools_dir, &options.tool_builder) {
            (Some(dir), Some(builder)) => Some(Arc::new(crate::tools_dir::ToolsDir::new(
                base_dir.join(dir),
                builder.clone(),
                crate::tools_dir::taken_names(spec),
            )?)),
            _ => None,
        };
        let long_term = spec.long_term_memory.as_ref();
        Ok(Self {
            executor,
            decider,
            tools,
            memory_tools: long_term.is_some_and(|memory| memory.tools),
            automatic_memory: long_term.is_some_and(|memory| {
                memory.auto_recall || memory.auto_remember || memory.remember_compressed
            }),
            memory,
            #[cfg(feature = "builtin")]
            key_value,
            sub_agents,
            #[cfg(feature = "sandbox")]
            tools_dir,
            on_skipped_tool: options.on_skipped_tool.clone(),
            package_tools: Vec::new(),
            prices,
        })
    }

    /// Hands the resolved tools of the spec's `tool_packages` on to the interpreter (ADR-0107).
    pub fn with_package_tools(mut self, tools: Vec<ResolvedTool>) -> Self {
        self.package_tools = tools;
        self
    }

    /// The executor with the clients of all providers of the spec.
    pub fn executor(&self) -> Arc<PromptExecutor> {
        Arc::clone(&self.executor)
    }

    /// The price table of the agent (ADR-0071): generated code installs it in every run.
    pub fn prices(&self) -> Option<Arc<grauwolf_prompt::Prices>> {
        self.prices.clone()
    }

    /// The decider of the decision nodes that name `system_one` (ADR-0100).
    pub fn decider(&self) -> Option<Arc<dyn Decider>> {
        self.decider.clone()
    }

    /// The resolved tools of the spec's packages.
    pub fn package_tools(&self) -> Vec<ResolvedTool> {
        self.package_tools.clone()
    }

    /// Does the spec ask for automatic recall or storing in the memory (ADR-0069)?
    pub fn has_automatic_memory(&self) -> bool {
        self.memory.is_some() && self.automatic_memory
    }

    /// Does the spec have a long-term memory?
    pub fn has_memory(&self) -> bool {
        self.memory.is_some()
    }

    /// The tools the agent itself carries: those that are the same for every run plus the shared
    /// area of the memory and the key-value store and the sub-agents without a user. Runs of a
    /// user bring their own with [`ProvisionedAgent::run`].
    pub fn tools(&self) -> Vec<Arc<dyn DynTool>> {
        self.tools_for(None).unwrap_or_default()
    }

    /// All tools of a run of `user` in one list (without: the shared area): the tools every run
    /// has, the memory tools and key-value store of the user and the sub-agents for the user.
    /// For callers that hand the tools to the agent themselves, such as generated code.
    pub fn tools_for(&self, user: Option<&str>) -> Result<Vec<Arc<dyn DynTool>>, String> {
        let mut tools = self.tools.clone();
        tools.extend(self.user_tools(user)?);
        tools.extend(self.sub_agents.iter().map(|agent| agent.tool_for(user)));
        Ok(tools)
    }

    /// The view of one run for `user` (without: the shared area): memory tools and facts, the
    /// key-value store and the sub-agents of that user, and the tools directory.
    pub fn run(&self, user: Option<&str>) -> Result<ProvisionedRun, String> {
        let mut tools = self.user_tools(user)?;
        if user.is_some() {
            tools.extend(self.sub_agents.iter().map(|agent| agent.tool_for(user)));
        }
        Ok(ProvisionedRun {
            tools,
            facts: self
                .memory
                .as_ref()
                .filter(|_| self.automatic_memory)
                .map(|memory| memory.facts(user)),
            #[cfg(feature = "sandbox")]
            tools_dir: self.tools_dir.clone(),
            on_skipped_tool: self.on_skipped_tool.clone(),
        })
    }

    /// The memory tools and the key-value store of `user`.
    fn user_tools(&self, user: Option<&str>) -> Result<Vec<Arc<dyn DynTool>>, String> {
        let mut tools = Vec::new();
        if let Some(memory) = self.memory.as_ref().filter(|_| self.memory_tools) {
            tools.extend(memory.tools(user));
        }
        #[cfg(feature = "builtin")]
        if let Some((builtin, base_dir)) = &self.key_value {
            tools.extend(crate::builtin::kv_tools(builtin, base_dir, user)?);
        }
        Ok(tools)
    }
}

/// What one run of a [`ProvisionedAgent`] brings along for its user (ADR-0114).
pub struct ProvisionedRun {
    tools: Vec<Arc<dyn DynTool>>,
    facts: Option<Arc<dyn FactMemory>>,
    #[cfg(feature = "sandbox")]
    tools_dir: Option<Arc<crate::tools_dir::ToolsDir>>,
    #[cfg_attr(not(feature = "sandbox"), allow(dead_code))]
    on_skipped_tool: Option<SkippedToolReport>,
}

impl ProvisionedRun {
    /// The memory of the user for automatic recall and storing, if the spec asks for it
    /// (ADR-0069).
    pub fn facts(&self) -> Option<Arc<dyn FactMemory>> {
        self.facts.clone()
    }

    /// The tools of the user: memory, key-value store and sub-agents; the same for the whole run.
    pub fn user_tools(&self) -> Vec<Arc<dyn DynTool>> {
        self.tools.clone()
    }

    /// The tools of `tools_dir`, freshly loaded, so that tools built in an earlier run are
    /// available (ADR-0074); files that cannot be used are reported.
    pub async fn directory_tools(&self) -> Vec<Arc<dyn DynTool>> {
        #[cfg(feature = "sandbox")]
        if let Some(dir) = &self.tools_dir {
            let loaded = dir.load().await;
            for skipped in &loaded.skipped {
                match &self.on_skipped_tool {
                    Some(report) => report(skipped),
                    None => eprintln!("tool folder: {skipped}"),
                }
            }
            return loaded.tools;
        }
        Vec::new()
    }

    /// All tools of this run: those of the user and those of the tools directory.
    pub async fn tools(&self) -> Vec<Arc<dyn DynTool>> {
        let mut tools = self.user_tools();
        tools.extend(self.directory_tools().await);
        tools
    }

    /// The handles for the run: tools and memory.
    pub async fn extras(&self) -> RunExtras {
        RunExtras {
            tools: self.tools().await,
            memory: self.facts(),
            ..RunExtras::default()
        }
    }
}

/// The decider for the decisions of `spec` that name `system_one`.
#[cfg(feature = "systemone")]
fn decider(
    spec: &AgentSpec,
    provided: Option<Arc<dyn Decider>>,
    env: &dyn Env,
) -> Result<Option<Arc<dyn Decider>>, String> {
    crate::systemone::decider_with(
        spec,
        provided,
        &crate::config::SystemOneConfig::from_env(env),
    )
    .map_err(|error| error.to_string())
}

#[cfg(not(feature = "systemone"))]
fn decider(
    _: &AgentSpec,
    provided: Option<Arc<dyn Decider>>,
    _: &dyn Env,
) -> Result<Option<Arc<dyn Decider>>, String> {
    Ok(provided)
}

#[cfg(feature = "mcp")]
async fn mcp_tools(
    spec: &AgentSpec,
    options: &ProvisionOptions,
) -> Result<Vec<Arc<dyn DynTool>>, String> {
    crate::mcp::start_servers_with(spec, &*options.env, options.mcp_tools_observer.as_ref()).await
}

#[cfg(not(feature = "mcp"))]
async fn mcp_tools(
    spec: &AgentSpec,
    _: &ProvisionOptions,
) -> Result<Vec<Arc<dyn DynTool>>, String> {
    if spec.mcp_servers.is_empty() {
        Ok(Vec::new())
    } else {
        Err("this build has no MCP servers (feature `mcp`)".to_owned())
    }
}

#[cfg(feature = "builtin")]
async fn builtin_tools(
    spec: &AgentSpec,
    base_dir: &Path,
    options: &ProvisionOptions,
) -> Result<Vec<Arc<dyn DynTool>>, String> {
    let Some(builtin) = &spec.builtin_tools else {
        return Ok(Vec::new());
    };
    let (fetch_auth, git_auth) = crate::builtin::resolve_auth(builtin, &*options.env).await?;
    crate::builtin::agent_tools(
        builtin,
        base_dir,
        Arc::clone(&options.user_io),
        &crate::builtin::Settings {
            atlassian: &crate::config::AtlassianConfig::from_env(&*options.env),
            git_env: &crate::config::GitEnvConfig::from_env(&*options.env),
            command_env: &*options.env,
            fetch_auth,
            git_auth,
        },
    )
}

#[cfg(not(feature = "builtin"))]
async fn builtin_tools(
    spec: &AgentSpec,
    _: &Path,
    _: &ProvisionOptions,
) -> Result<Vec<Arc<dyn DynTool>>, String> {
    if spec.builtin_tools.is_none() {
        Ok(Vec::new())
    } else {
        Err("this build has no built-in tools (feature `builtin`)".to_owned())
    }
}

#[cfg(feature = "builtin")]
fn skill_tools(spec: &AgentSpec, base_dir: &Path) -> Result<Vec<Arc<dyn DynTool>>, String> {
    crate::skills::tools(spec, base_dir)
}

#[cfg(not(feature = "builtin"))]
fn skill_tools(spec: &AgentSpec, _: &Path) -> Result<Vec<Arc<dyn DynTool>>, String> {
    if spec.skills.is_empty() {
        Ok(Vec::new())
    } else {
        Err("this build has no skills (feature `builtin`)".to_owned())
    }
}
