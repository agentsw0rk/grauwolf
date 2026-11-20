//! Tool abstraction and tool registry (E03).
//!
//! Users implement the typed [`Tool`]; internally the registry works with
//! object-safe [`DynTool`]s (JSON in, text out), so that sandbox tools
//! (ADR-0014) and tools from specs share the same interface.

use std::collections::BTreeMap;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

pub mod catalog;
mod delegation;
mod package;
mod policy;
mod sources;
mod trace_headers;

pub use delegation::{
    Delegation, TXN_TOKEN_HEADER, current_delegation, delegation_header, with_delegation,
};
pub use grauwolf_prompt::ToolDescriptor;
pub use package::{
    MANIFEST_FILE, PACKAGE_FORMAT, PackageError, PackageManifest, PackageTool, ResolvedTool,
    TOOL_ABI, ToolLimits,
};
pub use policy::{PolicyError, PolicyViolation, ToolPolicy};
pub use sources::{
    TOOL_SOURCES_FILE, TOOL_SOURCES_FORMAT, ToolSourceCrate, ToolSourceEntry, ToolSourceKind,
    ToolSourcesError, ToolSourcesManifest,
};
pub use trace_headers::{current_trace_headers, with_trace_headers};

/// Error when calling a tool; it is reported to the LLM as the tool result (F03.8).
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// There is no tool with this name.
    #[error("unknown tool '{name}'")]
    UnknownTool { name: String },
    /// The arguments do not match the schema of the tool.
    #[error("invalid arguments for '{tool}': {reason}")]
    InvalidArgs { tool: String, reason: String },
    /// The tool failed during execution.
    #[error("tool failed: {0}")]
    Execution(String),
}

/// Typed tool that an LLM can call (F03.1).
pub trait Tool: Send + Sync + 'static {
    /// Arguments; their JSON schema is passed to the LLM.
    type Args: DeserializeOwned + JsonSchema + Send;
    /// Result; by default it is given to the LLM as JSON.
    type Output: Serialize + Send;

    /// Unique name under which the LLM calls the tool.
    fn name(&self) -> &str;

    /// Description for the LLM.
    fn description(&self) -> &str;

    /// Runs the tool.
    fn call(&self, args: Self::Args) -> BoxFuture<'_, Result<Self::Output, ToolError>>;
}

/// Name of the built-in tool that loads a skill (ADR-0103); the agent reports `skill_loaded`
/// when a call of it succeeds.
pub const LOAD_SKILL_TOOL: &str = "load_skill";

/// Handles of the calling run for [`DynTool::call_step`]; the agent runtime defines what is in
/// them.
pub type StepHandles = Arc<dyn std::any::Any + Send + Sync>;

/// A section of a knowledge base with its source (ADR-0113).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeHit {
    /// Where the section comes from: a file path or a URL.
    pub source: String,
    /// Text of the section.
    pub text: String,
}

/// A knowledge base that is searched without the model asking: the agent adds the best sections
/// for the user input to the prompt (`knowledge.inject`, ADR-0113).
pub trait KnowledgeContext: Send + Sync {
    /// The best sections for `query`, best first; empty when nothing is relevant.
    fn retrieve<'a>(
        &'a self,
        query: &'a str,
    ) -> BoxFuture<'a, Result<Vec<KnowledgeHit>, ToolError>>;
}

/// The knowledge base of the search tool called `name` among `tools`; `None` when there is no
/// such tool or it is not the search tool of a knowledge base. The interpreter and generated
/// code look the knowledge context of `knowledge.inject` up this way (ADR-0116).
pub fn knowledge_context_of(
    tools: &[Arc<dyn DynTool>],
    name: &str,
) -> Option<Arc<dyn KnowledgeContext>> {
    tools
        .iter()
        .find(|tool| tool.name() == name)
        .and_then(|tool| tool.knowledge_context())
}

/// Object-safe form of a tool: JSON arguments in, text for the LLM out (F03.7).
pub trait DynTool: Send + Sync {
    /// Name of the tool.
    fn name(&self) -> &str;

    /// Description for the LLM.
    fn descriptor(&self) -> ToolDescriptor;

    /// Runs the tool with JSON arguments.
    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>>;

    /// Runs the tool as a deterministic step of a flow (ADR-0101): `input` is the value of the
    /// node, `parent` the handles of the calling run, which a tool that is an agent itself may
    /// adopt (shared budget). By default text becomes `{ "request": text }`, other values are
    /// the arguments; the result is the tool's text.
    fn call_step(
        &self,
        input: Value,
        parent: Option<StepHandles>,
    ) -> BoxFuture<'_, Result<Value, ToolError>> {
        let _ = parent;
        Box::pin(async move {
            let args = match input {
                Value::String(request) => serde_json::json!({ "request": request }),
                other => other,
            };
            self.call_json(args).await.map(Value::String)
        })
    }

    /// The knowledge base behind this tool, if it is the search tool of `knowledge`: the agent
    /// uses it for `knowledge.inject`, also when the tool itself is not offered (ADR-0113).
    fn knowledge_context(&self) -> Option<Arc<dyn KnowledgeContext>> {
        None
    }

    /// Text that the agent appends to its system prompt while it offers this tool (ADR-0103:
    /// the list of skills); `None` for most tools.
    fn prompt_addition(&self) -> Option<String> {
        None
    }

    /// Key in the agent storage under which the tool keeps its state of the run (ADR-0098);
    /// `None` for tools without state. The agent context then calls
    /// [`DynTool::call_with_state`] with the stored value, so that the state is part of
    /// checkpoints and survives a resume.
    fn state_key(&self) -> Option<&str> {
        None
    }

    /// Runs the tool on its state: `state` is the value stored under [`DynTool::state_key`]
    /// (`null` if nothing is stored yet); the tool may change it. The state is only kept when
    /// the call succeeds. Only called for tools with a state key.
    fn call_with_state(&self, args: Value, state: &mut Value) -> Result<String, ToolError> {
        let _ = (args, state);
        Err(ToolError::Execution("this tool keeps no state".to_owned()))
    }
}

impl<T: Tool> DynTool for T {
    fn name(&self) -> &str {
        Tool::name(self)
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: Tool::name(self).to_owned(),
            description: self.description().to_owned(),
            parameters: schemars::schema_for!(T::Args).to_value(),
        }
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: T::Args =
                serde_json::from_value(args).map_err(|error| ToolError::InvalidArgs {
                    tool: Tool::name(self).to_owned(),
                    reason: error.to_string(),
                })?;
            let output = self.call(args).await?;
            serde_json::to_string(&output).map_err(|error| ToolError::Execution(error.to_string()))
        })
    }
}

/// Built tool code, ready to run in a runtime (ADR-0024).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolArtifact {
    /// Name of the tool.
    pub name: String,
    /// Content, e.g. a WASM module following tool ABI v1 (ADR-0029).
    pub bytes: Vec<u8>,
    /// Content hash, key of the build cache.
    pub hash: String,
}

/// Port: turns an artifact into a callable tool (ADR-0014). Implemented e.g. by
/// `grauwolf-sandbox`; never accepts source code.
pub trait ToolRuntime: Send + Sync {
    /// Loads `artifact` and describes it to the LLM with `descriptor`.
    fn instantiate(
        &self,
        artifact: &ToolArtifact,
        descriptor: ToolDescriptor,
    ) -> Result<Arc<dyn DynTool>, ToolError>;

    /// Like [`instantiate`](Self::instantiate) for a tool whose package asks for `limits`
    /// (ADR-0124): a runtime with limits of its own applies the stricter of both. The default
    /// ignores them, for runtimes without limits.
    fn instantiate_limited(
        &self,
        artifact: &ToolArtifact,
        descriptor: ToolDescriptor,
        limits: &ToolLimits,
    ) -> Result<Arc<dyn DynTool>, ToolError> {
        let _ = limits;
        self.instantiate(artifact, descriptor)
    }
}

/// A tool that an agent wants to create during the run (ADR-0075); serializable for
/// checkpoints (ADR-0077).
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct NewTool {
    /// Name under which the LLM calls the tool.
    pub name: String,
    /// Description for the LLM.
    pub description: String,
    /// JSON schema of the arguments.
    pub args: Value,
    /// Body of `fn tool(args: Value) -> Result<String, String>` (ADR-0024).
    pub code: String,
}

/// Port: turns tool code into a callable tool (ADR-0075), e.g. via a build into the
/// sandbox. Errors are text for the model, e.g. compile errors with line and column.
pub trait ToolFactory: Send + Sync {
    /// Builds `tool`.
    fn create(&self, tool: NewTool) -> BoxFuture<'_, Result<Arc<dyn DynTool>, String>>;
}

/// Set of tools that are available to an agent (F03.6).
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn DynTool>>,
    /// Tools that are only added during a run (ADR-0075).
    shelf: Option<ToolShelf>,
}

/// Shared, growing set of tools of a run (ADR-0075): clones see the same tools,
/// e.g. the registry in parallel branches.
#[derive(Clone, Default)]
pub struct ToolShelf {
    tools: Arc<std::sync::RwLock<BTreeMap<String, Arc<dyn DynTool>>>>,
    /// Definitions of built tools, so that checkpoints can restore them (ADR-0077).
    definitions: Arc<std::sync::RwLock<Vec<NewTool>>>,
}

impl ToolShelf {
    /// Stores the `tool` built from `definition` and remembers the definition (ADR-0077).
    pub fn put_made(&self, tool: Arc<dyn DynTool>, definition: NewTool) -> Result<(), String> {
        self.put(tool)?;
        self.definitions
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(definition);
        Ok(())
    }

    /// Definitions of the tools stored with [`ToolShelf::put_made`], in order.
    pub fn definitions(&self) -> Vec<NewTool> {
        self.definitions
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Stores `tool`; a name is only assigned once.
    pub fn put(&self, tool: Arc<dyn DynTool>) -> Result<(), String> {
        let mut tools = self
            .tools
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let name = tool.name().to_owned();
        if tools.contains_key(&name) {
            return Err(format!("tool '{name}' already exists"));
        }
        tools.insert(name, tool);
        Ok(())
    }

    /// Number of stored tools.
    pub fn len(&self) -> usize {
        self.tools().len()
    }

    /// Has no tool been stored yet?
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn tools(&self) -> std::sync::RwLockReadGuard<'_, BTreeMap<String, Arc<dyn DynTool>>> {
        self.tools
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn get(&self, name: &str) -> Option<Arc<dyn DynTool>> {
        self.tools().get(name).cloned()
    }
}

impl ToolRegistry {
    /// Starts building a registry.
    pub fn builder() -> ToolRegistryBuilder {
        ToolRegistryBuilder::default()
    }

    /// Descriptors of all tools, sorted by name; tools on the shelf follow afterwards.
    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        let mut descriptors: Vec<_> = self.tools.values().map(|tool| tool.descriptor()).collect();
        if let Some(shelf) = &self.shelf {
            descriptors.extend(
                shelf
                    .tools()
                    .iter()
                    .filter(|(name, _)| !self.tools.contains_key(*name))
                    .map(|(_, tool)| tool.descriptor()),
            );
        }
        descriptors
    }

    /// The system prompt additions of the tools (see [`DynTool::prompt_addition`]), sorted by
    /// tool name and joined by a blank line; `None` if no tool has one.
    pub fn prompt_addition(&self) -> Option<String> {
        let parts: Vec<String> = self
            .tools
            .values()
            .filter_map(|tool| tool.prompt_addition())
            .collect();
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    }

    /// Definitions of the self-made tools on the shelf (ADR-0077); empty without a shelf.
    pub fn self_made(&self) -> Vec<NewTool> {
        self.shelf
            .as_ref()
            .map(ToolShelf::definitions)
            .unwrap_or_default()
    }

    /// This registry with a shelf for tools that are added during the run (ADR-0075).
    pub fn with_shelf(mut self, shelf: ToolShelf) -> Self {
        self.shelf = Some(shelf);
        self
    }

    /// Is the tool `name` on the shelf, i.e. was it added during the run?
    pub fn is_on_shelf(&self, name: &str) -> bool {
        !self.tools.contains_key(name)
            && self
                .shelf
                .as_ref()
                .is_some_and(|shelf| shelf.get(name).is_some())
    }

    /// Is there a tool `name` (also on the shelf)?
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
            || self
                .shelf
                .as_ref()
                .is_some_and(|shelf| shelf.get(name).is_some())
    }

    /// This registry with `tools`; tools with the same name are replaced, e.g. for a single
    /// run (ADR-0059).
    pub fn extended(mut self, tools: impl IntoIterator<Item = Arc<dyn DynTool>>) -> Self {
        for tool in tools {
            self.tools.insert(tool.name().to_owned(), tool);
        }
        self
    }

    /// The tool `name` (also on the shelf), if there is one.
    pub fn find(&self, name: &str) -> Option<Arc<dyn DynTool>> {
        self.tools
            .get(name)
            .cloned()
            .or_else(|| self.shelf.as_ref().and_then(|shelf| shelf.get(name)))
    }

    /// Calls the tool `name` with `args`.
    pub async fn call(&self, name: &str, args: Value) -> Result<String, ToolError> {
        let tool = self
            .tools
            .get(name)
            .cloned()
            .or_else(|| self.shelf.as_ref().and_then(|shelf| shelf.get(name)))
            .ok_or_else(|| ToolError::UnknownTool {
                name: name.to_owned(),
            })?;
        tool.call_json(args).await
    }
}

/// Builds a [`ToolRegistry`].
#[derive(Default)]
pub struct ToolRegistryBuilder {
    registry: ToolRegistry,
}

impl ToolRegistryBuilder {
    /// Registers a tool.
    pub fn tool(mut self, tool: impl DynTool + 'static) -> Self {
        self.registry
            .tools
            .insert(tool.name().to_owned(), Arc::new(tool));
        self
    }

    /// Registers an already shared tool, e.g. from a [`ToolRuntime`].
    pub fn shared(mut self, tool: Arc<dyn DynTool>) -> Self {
        self.registry.tools.insert(tool.name().to_owned(), tool);
        self
    }

    /// Finishes the build.
    pub fn build(self) -> ToolRegistry {
        self.registry
    }
}

/// For code from `#[tool]` (grauwolf-macros): users do not need serde, schemars and
/// futures-util as dependencies themselves. Not a stable API.
#[doc(hidden)]
pub mod __private {
    pub use futures_util::future::BoxFuture;
    pub use schemars;
    pub use serde;
}
