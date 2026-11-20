//! Loads specs and provides them as agents: builds tool code, sets up providers, integrates
//! external tools and sub-agents (recursively). For the CLI as well as for applications (ADR-0040,
//! ADR-0050).

use grauwolf_spec::AgentSpecExt;
use std::path::Path;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_agents::FrameSink;
use grauwolf_agents::events::{Feature, Interceptor};
use grauwolf_interpreter::{InterpretedAgent, Interpreter};
use grauwolf_sandbox::SandboxRuntime;
use grauwolf_spec::AgentSpec;
use grauwolf_toolbuild::ToolBuilder;
use tokio::sync::OnceCell;

use crate::agent_tree::{AgentTree, Child, ReadErrors, ResolveOptions};
use crate::builtin::{ConsoleUserIo, UserIo};
use crate::config::{Env, ProcessEnv};
use crate::mcp_observer::McpToolsObserver;
use crate::provisioning::{
    ProvisionOptions, ProvisionedAgent, SkippedToolReport, SubAgent, SubAgentSupplier,
};

pub use crate::app::AgentFeatures;
pub use grauwolf_agent_tree::{build_tools, read_spec, warning_lines};

/// A loaded agent: its spec, the interpreter and what it was provisioned with (ADR-0114).
pub type Loaded = (AgentSpec, InterpretedAgent, Arc<ProvisionedAgent>);

/// Loads specs as agents; settings also apply to their sub-agents.
#[derive(Clone)]
pub struct AgentLoader {
    builder: ToolBuilder,
    features: Vec<Arc<dyn Feature>>,
    agent_features: Option<AgentFeatures>,
    interceptors: Vec<Arc<dyn Interceptor>>,
    approval_tools: Vec<String>,
    prices: Option<Arc<grauwolf_prompt::Prices>>,
    budget: Option<grauwolf_agents::Budget>,
    decider: Option<Arc<dyn grauwolf_agents::decision::Decider>>,
    user_io: Arc<dyn UserIo>,
    llm_cache: Option<Arc<dyn grauwolf_llm::PromptCache>>,
    locked: bool,
    env: Arc<dyn Env>,
    /// The environment with the credential sources of the spec, made on the first load and
    /// shared by the clones of this loader (ADR-0110).
    credentials: Arc<std::sync::Mutex<Option<Arc<dyn Env>>>>,
    chat_api: bool,
    mcp_tools_observer: Option<McpToolsObserver>,
    skipped_tool_report: Option<SkippedToolReport>,
}

impl AgentLoader {
    /// Loader that builds tool code with `builder`; user tools talk via the console.
    pub fn new(builder: ToolBuilder) -> Self {
        Self {
            builder,
            features: Vec::new(),
            agent_features: None,
            interceptors: Vec::new(),
            approval_tools: Vec::new(),
            prices: None,
            budget: None,
            decider: None,
            user_io: Arc::new(ConsoleUserIo),
            llm_cache: None,
            locked: false,
            env: Arc::new(ProcessEnv),
            credentials: Arc::default(),
            chat_api: false,
            mcp_tools_observer: None,
            skipped_tool_report: None,
        }
    }

    /// A loader for the sub-agents of an agent provisioned with `options` (generated code): its
    /// tool packages and git skills must be in the lockfile (`--locked`, ADR-0115); it
    /// reads the same environment and passes the same user I/O, observer and features on.
    pub fn for_provisioning(builder: ToolBuilder, options: &ProvisionOptions) -> Self {
        let mut loader = Self::new(builder)
            .with_locked(true)
            .with_env(Arc::clone(options.env()))
            .with_user_io(Arc::clone(options.user_io()))
            .with_chat_api_mode(options.chat_api())
            .with_mcp_tools_observer(options.mcp_tools_observer().cloned());
        if let Some(features) = options.agent_features() {
            loader = loader.with_agent_features(Arc::clone(features));
        }
        if let Some(cache) = options.llm_cache() {
            loader = loader.with_llm_cache(Arc::clone(cache));
        }
        loader
    }

    /// Installs `feature` in every loaded agent.
    pub fn with_feature(mut self, feature: Arc<dyn Feature>) -> Self {
        self.features.push(feature);
        self
    }

    /// Installs in every agent the features that `features` returns for its spec.
    pub fn with_agent_features(mut self, features: AgentFeatures) -> Self {
        self.agent_features = Some(features);
        self
    }

    /// Installs `interceptor` in every loaded agent.
    pub fn with_interceptor(mut self, interceptor: Arc<dyn Interceptor>) -> Self {
        self.interceptors.push(interceptor);
        self
    }

    /// Provides the `system_one` decider to every loaded agent whose spec has decision nodes
    /// that name it (ADR-0100).
    pub fn with_decider(mut self, decider: Arc<dyn grauwolf_agents::decision::Decider>) -> Self {
        self.decider = Some(decider);
        self
    }

    /// Limits every run of every loaded agent to `budget` (ADR-0072); sub-agents have
    /// their own budget, their usage does not count towards that of the calling agent.
    pub fn with_budget(mut self, budget: grauwolf_agents::Budget) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Answers identical LLM requests of every loaded agent from `cache` (ADR-0086).
    pub fn with_llm_cache(mut self, cache: Arc<dyn grauwolf_llm::PromptCache>) -> Self {
        self.llm_cache = Some(cache);
        self
    }

    /// With `locked`, tool packages (ADR-0107) must be in the lockfile: loading fails instead of
    /// changing `grauwolf.lock`.
    pub fn with_locked(mut self, locked: bool) -> Self {
        self.locked = locked;
        self
    }

    /// Reads the environment (provider settings, credentials, `${VAR}` in headers) from `env`
    /// instead of the process (tests, embedding applications).
    pub fn with_env(mut self, env: Arc<dyn Env>) -> Self {
        self.env = env;
        self.credentials = Arc::default();
        self
    }

    /// Switches chat API mode on for every loaded agent (`OLLAMA_BASE_URL` stands in for a
    /// missing `OLLAMA_HOST`, ADR-0094); `GRAUWOLF_CHAT_API` in the environment does so too.
    pub fn with_chat_api_mode(mut self, on: bool) -> Self {
        self.chat_api = on;
        self
    }

    /// Lets `observer` (if any) see the tool lists of the MCP servers of every loaded agent and
    /// sub-agent (chat API, ADR-0094).
    pub fn with_mcp_tools_observer(mut self, observer: Option<McpToolsObserver>) -> Self {
        self.mcp_tools_observer = observer;
        self
    }

    /// Reports files of the tools directory that cannot be used (default: on stderr).
    pub fn with_skipped_tool_report(mut self, report: SkippedToolReport) -> Self {
        self.skipped_tool_report = Some(report);
        self
    }

    /// Computes costs with `prices` in every loaded agent (ADR-0071).
    pub fn with_prices(mut self, prices: Arc<grauwolf_prompt::Prices>) -> Self {
        self.prices = Some(prices);
        self
    }

    /// Makes `tools` require approval in every loaded agent (ADR-0061); sub-agents
    /// have no approver and therefore reject them.
    pub fn with_tool_approval(mut self, tools: Vec<String>) -> Self {
        self.approval_tools.extend(tools);
        self
    }

    /// User tools (`say_to_user`, `ask_user`) talk via `io`.
    pub fn with_user_io(mut self, io: Arc<dyn UserIo>) -> Self {
        self.user_io = io;
        self
    }

    /// Loads the spec `file` as an agent; only it receives `sink` (live output).
    pub async fn load(
        &self,
        file: &Path,
        sink: Option<FrameSink>,
    ) -> Result<(AgentSpec, InterpretedAgent), String> {
        let (spec, agent, _) = self.load_provisioned(file, sink).await?;
        Ok((spec, agent))
    }

    /// Like [`load`](Self::load), and returns the provisioned agent too: its runs bring the
    /// memory, key-value store and tools directory of their user (ADR-0114).
    pub async fn load_provisioned(
        &self,
        file: &Path,
        sink: Option<FrameSink>,
    ) -> Result<Loaded, String> {
        // Credential sources of the spec (`[credentials]`, ADR-0110): from here on this loader,
        // its sub-agents and the provisioned agent read the variables through them.
        let loader = self.with_credentials(file)?;
        let run = loader
            .prepare_tree(crate::agent_tree::read(file), true)
            .await?;
        loader.build_node(&run, 0, sink).await
    }

    /// The environment of this loader with the credential sources of the spec `file`
    /// ([`crate::credentials::env_for`]); made once, then shared. Entry points that read
    /// variables themselves (the service token) use it too.
    pub fn credential_env(&self, file: &Path) -> Result<Arc<dyn Env>, String> {
        let mut cached = self.credentials.lock().expect("credentials lock");
        if let Some(env) = &*cached {
            return Ok(Arc::clone(env));
        }
        let env = crate::credentials::env_for(Arc::clone(&self.env), Some(file))?;
        *cached = Some(Arc::clone(&env));
        Ok(env)
    }

    /// Uses `env` as the environment with the credential sources (already made by
    /// [`crate::credentials::env_for`]), so the loader does not make a second one.
    pub fn with_credential_env(self, env: Arc<dyn Env>) -> Self {
        *self.credentials.lock().expect("credentials lock") = Some(env);
        self
    }

    fn with_credentials(&self, file: &Path) -> Result<Self, String> {
        let mut loader = self.clone();
        loader.env = self.credential_env(file)?;
        Ok(loader)
    }

    /// The options for provisioning an agent the way this loader does.
    pub fn provision_options(&self) -> ProvisionOptions {
        ProvisionOptions::new(Arc::clone(&self.env))
            .with_chat_api_mode(self.chat_api)
            .with_user_io(Arc::clone(&self.user_io))
            .with_decider(self.decider.clone())
            .with_mcp_tools_observer(self.mcp_tools_observer.clone())
            .with_agent_features(self.agent_features.clone())
            .with_llm_cache(self.llm_cache.clone())
            .with_prices(self.prices.clone())
    }

    /// Builds the agent of the node `index` of the prepared tree: its sub-agents come from the
    /// tree's other nodes (each loaded once, however many parents name it, ADR-0115).
    fn build_node(
        &self,
        run: &Arc<TreeRun>,
        index: usize,
        sink: Option<FrameSink>,
    ) -> BoxFuture<'_, Result<Loaded, String>> {
        let run = Arc::clone(run);
        Box::pin(async move {
            let node = run.tree.node(index);
            let file = node.file();
            let fail = |error: &dyn std::fmt::Display| format!("{}: {error}", file.display());
            let resolution = node.resolution().expect("the tree is resolved");
            let spec = node.spec().clone();
            // Tool packages and git skills are resolved already (ADR-0107, ADR-0103), code built.
            let has_code = !spec.code_pieces().is_empty();
            let tool_runtime =
                if spec.tools.is_empty() && !has_code && resolution.package_tools.is_empty() {
                    None
                } else {
                    let sandbox = SandboxRuntime::new().map_err(|error| fail(&error))?;
                    Some(Arc::new(sandbox))
                };
            let base_dir = node.base_dir();
            // Everything an agent run needs from outside the graph (ADR-0114). The tools
            // directory belongs to the loaded agent, not to its sub-agents.
            let mut options = self
                .provision_options()
                .with_sub_agents(Arc::new(TreeSupplier {
                    loader: self.clone(),
                    run: Arc::clone(&run),
                    node: index,
                }));
            if index == 0 {
                options = options.with_tool_builder(self.builder.clone());
                if let Some(report) = &self.skipped_tool_report {
                    options = options.with_skipped_tool_report(Arc::clone(report));
                }
            }
            let provisioned = ProvisionedAgent::new(&resolution.spec, base_dir, options)
                .await
                .map_err(|error| fail(&error))?
                .with_package_tools(resolution.package_tools.clone());
            let mut interpreter = Interpreter::new(provisioned.executor());
            if let Some(sink) = sink {
                interpreter = interpreter.with_frame_sink(sink);
            }
            let per_agent = self
                .agent_features
                .as_ref()
                .map(|features| features(&spec))
                .unwrap_or_default();
            for feature in self.features.iter().chain(&per_agent) {
                interpreter = interpreter.with_feature(Arc::clone(feature));
            }
            for interceptor in &self.interceptors {
                interpreter = interpreter.with_interceptor(Arc::clone(interceptor));
            }
            interpreter = interpreter.with_tool_approval(self.approval_tools.iter().cloned());
            if let Some(prices) = &self.prices {
                interpreter = interpreter.with_prices(Arc::clone(prices));
            }
            if let Some(budget) = self.budget {
                interpreter = interpreter.with_budget(budget);
            }
            if let Some(decider) = provisioned.decider() {
                interpreter = interpreter.with_decider(decider);
            }
            if let Some(runtime) = tool_runtime {
                interpreter = interpreter.with_tool_runtime(runtime);
            }
            interpreter = interpreter.with_package_tools(provisioned.package_tools());
            // Self-made tools (ADR-0075); with `save` they end up in the tools directory.
            if let Some(self_tools) = &spec.self_tools {
                let mut factory = crate::self_tools::BuildingToolFactory::new(self.builder.clone())
                    .map_err(|error| fail(&error))?;
                if let (true, Some(dir)) = (self_tools.save, &spec.tools_dir) {
                    factory = factory.with_save_dir(base_dir.join(dir));
                }
                interpreter = interpreter.with_tool_factory(Arc::new(factory));
            }
            let agent = interpreter
                .with_tools(provisioned.tools())
                .load_with_artifacts(&spec, &resolution.artifacts)
                .map_err(|error| fail(&error))?;
            Ok((spec, agent, Arc::new(provisioned)))
        })
    }

    /// The sub-agent `child` of a node: a local one is loaded once (shared by all parents), a
    /// remote one connects over A2A (ADR-0055); headers with `${VAR}` from the environment.
    async fn sub_agent(
        &self,
        run: &Arc<TreeRun>,
        parent: &AgentSpec,
        child: &Child,
    ) -> Result<SubAgent, String> {
        let description = |name: &str| {
            parent
                .agent_tools
                .get(name)
                .and_then(|tool| tool.description.clone())
        };
        match child {
            Child::Local { name, node } => {
                let (_, agent, provisioned) = run.loaded[*node]
                    .get_or_try_init(|| async {
                        let (spec, agent, provisioned) = self.build_node(run, *node, None).await?;
                        Ok::<_, String>((spec, Arc::new(agent), provisioned))
                    })
                    .await?;
                Ok(local_sub_agent(
                    name,
                    description(name).unwrap_or_default(),
                    Arc::clone(agent),
                    Arc::clone(provisioned),
                ))
            }
            Child::Remote { name, url } => {
                let headers = parent.agent_tools[name]
                    .headers
                    .iter()
                    .map(|(header, value)| {
                        Ok((header.clone(), crate::env::expand_with(value, &*self.env)?))
                    })
                    .collect::<Result<Vec<_>, String>>()
                    .map_err(|error| format!("sub-agent '{name}': {error}"))?;
                let tool = &parent.agent_tools[name];
                let auth = match &tool.auth {
                    Some(auth) => Some(
                        crate::spiffe::client_auth(
                            &*self.env,
                            &format!("sub-agent '{name}'"),
                            auth,
                        )
                        .await?,
                    ),
                    None => None,
                };
                let client = grauwolf_a2a::A2aClient::connect_with_auth(
                    url,
                    &headers,
                    grauwolf_a2a::DEFAULT_READ_TIMEOUT,
                    auth,
                )
                .await
                .map_err(|error| format!("sub-agent '{name}': {error}"))?;
                Ok(SubAgent::fixed(
                    Arc::new(client).tool(name, description(name).as_deref()),
                ))
            }
        }
    }

    /// Reads and resolves the tree of `file` (or of the spec in memory) for loading it.
    async fn prepare_tree(
        &self,
        tree: Result<AgentTree, ReadErrors>,
        resolve_root: bool,
    ) -> Result<Arc<TreeRun>, String> {
        let mut tree = tree.map_err(|errors| errors.to_string())?;
        let config = crate::config::RegistryConfig::from_env(&*self.env);
        tree.resolve(&ResolveOptions {
            builder: &self.builder,
            locked: self.locked,
            config: &config,
            build_code: true,
            resolve_root,
        })
        .await?;
        for warning in tree.warnings() {
            eprintln!("{warning}");
        }
        let loaded = (0..tree.nodes().len()).map(|_| OnceCell::new()).collect();
        Ok(Arc::new(TreeRun { tree, loaded }))
    }
}

impl SubAgentSupplier for AgentLoader {
    fn supply<'a>(
        &'a self,
        spec: &'a AgentSpec,
        base_dir: &'a Path,
    ) -> BoxFuture<'a, Result<Vec<SubAgent>, String>> {
        Box::pin(async move {
            let run = self
                .prepare_tree(crate::agent_tree::read_spec(spec.clone(), base_dir), false)
                .await?;
            TreeSupplier {
                loader: self.clone(),
                run,
                node: 0,
            }
            .supply(spec, base_dir)
            .await
        })
    }
}

/// A loaded node as its parents share it.
type SharedAgent = (AgentSpec, Arc<InterpretedAgent>, Arc<ProvisionedAgent>);

/// A prepared tree and the agents loaded from it so far: a node that several parents name is
/// loaded once (index 0, the root, is built by the caller).
struct TreeRun {
    tree: AgentTree,
    loaded: Vec<OnceCell<SharedAgent>>,
}

/// The supplier a node is provisioned with: gives the sub-agents of that node of the tree.
struct TreeSupplier {
    loader: AgentLoader,
    run: Arc<TreeRun>,
    node: usize,
}

impl SubAgentSupplier for TreeSupplier {
    fn supply<'a>(
        &'a self,
        spec: &'a AgentSpec,
        _base_dir: &'a Path,
    ) -> BoxFuture<'a, Result<Vec<SubAgent>, String>> {
        Box::pin(async move {
            let mut agents = Vec::new();
            for child in self.run.tree.node(self.node).children() {
                agents.push(self.loader.sub_agent(&self.run, spec, child).await?);
            }
            Ok(agents)
        })
    }
}

/// A loaded sub-agent as a tool: for a calling run with a user it runs with that user's memory,
/// key-value store and facts (ADR-0114); without a user as it is.
fn local_sub_agent(
    name: &str,
    description: String,
    agent: Arc<InterpretedAgent>,
    provisioned: Arc<ProvisionedAgent>,
) -> SubAgent {
    let tool_name = name.to_owned();
    SubAgent::new(name, move |user| {
        let Some(run) = user.and_then(|user| provisioned.run(Some(user)).ok()) else {
            return Arc::clone(&agent).into_tool(&tool_name, &description);
        };
        let run = Arc::new(run);
        Arc::clone(&agent).into_tool_with(
            &tool_name,
            &description,
            Arc::new(move || {
                let run = Arc::clone(&run);
                Box::pin(async move { run.extras().await })
            }),
        )
    })
}

/// Like [`read_spec`], but also validates the spec afterwards (errors with file and JSON
/// pointer). Warnings of a valid spec go to stderr.
pub fn load_spec(file: &Path) -> Result<AgentSpec, String> {
    let spec = read_spec(file)?;
    spec.validate().map_err(|errors| {
        errors
            .iter()
            .map(|error| format!("{}:{error}", file.display()))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    for line in warning_lines(file, &spec) {
        eprintln!("{line}");
    }
    Ok(spec)
}
