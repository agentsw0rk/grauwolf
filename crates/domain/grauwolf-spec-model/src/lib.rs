//! The data model of the agent spec DSL (E13, ADR-0108 stage 6): the spec types, parsing into them
//! (strategies stay unexpanded), the JSON schema and the helpers that only read a spec. No
//! strategy expansion and no validation – those live in `grauwolf-strategies` and `grauwolf-spec`.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

mod builtin_groups;
mod code;
mod for_each;
mod git;
mod harness;
mod mermaid;
mod namespace;
mod oci;

pub use builtin_groups::{BUILTIN_GROUPS, BuiltinGroup, BuiltinGroupInfo};
pub use code::{
    CODE_KIND, CodeNodeParams, CodePiece, CodePieceKind, ON_CODE_KIND, OnCodeParams, TransformSpec,
};
pub use for_each::{ForEachBody, ForEachOnError, ForEachParams, MAX_PARALLEL_LIMIT};
pub use git::{GitRef, GitSource, GitSourceProblem};
pub use grauwolf_graph::{FINISH, START};
pub use harness::{Harness, SUPPORTED_HARNESS_VERSION};
pub use namespace::{ToolCollision, ToolEntry, ToolNamespace, ToolOrigin};
pub use oci::{OciReference, is_sha256_digest};

/// Complete description of an agent as a JSON document.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentSpec {
    /// Version of the spec format.
    pub spec_version: u32,
    /// Name of the agent.
    pub name: String,
    /// LLM the agent works with.
    pub model: ModelSpec,
    /// Optional system prompt.
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Named types as JSON schemas; edges are checked nominally (ADR-0022).
    #[serde(default)]
    pub types: BTreeMap<String, serde_json::Value>,
    /// Input type of the agent: `string` or a name from `types`.
    #[serde(default = "string_type")]
    pub input: String,
    /// Output type of the agent: `string` or a name from `types`.
    #[serde(default = "string_type")]
    pub output: String,
    /// Tools whose code runs in the sandbox (ADR-0014, ADR-0029).
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    /// Tool packages from OCI registries (ADR-0107); their tools run in the sandbox like `tools`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_packages: Vec<ToolPackageSpec>,
    /// Skills: folders with a `SKILL.md` that the agent loads on demand (ADR-0103), from local
    /// directories and git repositories.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<SkillSourceSpec>,
    /// MCP servers whose tools the agent uses (ADR-0036); they run outside the sandbox.
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerSpec>,
    /// Other agents (specs) that this agent calls as tools (ADR-0040).
    #[serde(default)]
    pub agent_tools: BTreeMap<String, AgentToolSpec>,
    /// Built-in tools (files, user input/output; ADR-0046).
    #[serde(default)]
    pub builtin_tools: Option<BuiltinToolsSpec>,
    /// Documents the agent can look up via a search tool (RAG, ADR-0038).
    #[serde(default)]
    pub knowledge: Option<KnowledgeSpec>,
    /// Long-term memory via the tools `remember` and `recall` (ADR-0058).
    #[serde(default)]
    pub long_term_memory: Option<LongTermMemorySpec>,
    /// Directory (relative to the spec) whose tool files are loaded before each run
    /// (ADR-0074).
    #[serde(default)]
    pub tools_dir: Option<String>,
    /// Limits per run: steps, tool calls, tokens, cost (ADR-0078).
    #[serde(default)]
    pub limits: Option<LimitsSpec>,
    /// Lets the agent build its own tools during the run (ADR-0075).
    #[serde(default)]
    pub self_tools: Option<SelfToolsSpec>,
    /// Named subgraphs, included via nodes of kind `subgraph` (ADR-0034).
    #[serde(default)]
    pub subgraphs: BTreeMap<String, SubgraphSpec>,
    /// Strategy template instead of a written-out graph (ADR-0041); expanded to
    /// `graph` when loaded.
    #[serde(default)]
    pub strategy: Option<StrategySpec>,
    /// Strategy graph of the agent; if missing, it is empty.
    #[serde(default)]
    pub graph: GraphSpec,
}

/// A tool with code that runs as a WASM module in the sandbox (tool ABI v1, ADR-0029).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolSpec {
    /// Name under which the LLM calls the tool.
    pub name: String,
    /// Description for the LLM.
    pub description: String,
    /// JSON schema of the arguments.
    pub args: serde_json::Value,
    /// Body of a Rust function `fn(args: serde_json::Value) -> Result<String, String>`.
    pub code: String,
}

/// A tool package that the spec imports (ADR-0107): exactly one source, `oci` (a reference
/// `registry/repository[:tag][@sha256:…]`) or `git` (a repository URL with `path` and exactly one
/// of `rev`, `tag`, `branch`).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolPackageSpec {
    /// OCI reference of the package, e.g. `ghcr.io/acme/tools/math:1.0`.
    #[serde(default)]
    pub oci: Option<String>,
    /// Git repository URL (`https://`, `http://` only for localhost), e.g.
    /// `https://github.com/acme/grauwolf-tools.git`. Authentication: `GIT_TOKEN`.
    #[serde(default)]
    pub git: Option<String>,
    /// Git only: folder of the package in the repository (default: the repository root).
    #[serde(default)]
    pub path: Option<String>,
    /// Git only: commit, full or abbreviated (7 to 40 lowercase hex digits).
    #[serde(default)]
    pub rev: Option<String>,
    /// Git only: tag, resolved to its commit and locked.
    #[serde(default)]
    pub tag: Option<String>,
    /// Git only: branch, resolved to its tip commit and locked.
    #[serde(default)]
    pub branch: Option<String>,
    /// Tools of the package to import; default: all.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    /// Renames imported tools: package name → name in this spec.
    #[serde(default, rename = "as")]
    pub renames: BTreeMap<String, String>,
}

/// A source of skills (ADR-0103): exactly one of `dir` (a local directory relative to the spec)
/// and `git` (a repository URL with `path` and exactly one of `rev`, `tag`, `branch`). The
/// directory (or the `path` in the repository) holds one sub-directory per skill with a
/// `SKILL.md`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillSourceSpec {
    /// Local directory relative to the spec.
    #[serde(default)]
    pub dir: Option<String>,
    /// Git repository URL (`https://`, `http://` only for localhost). Authentication: `GIT_TOKEN`.
    #[serde(default)]
    pub git: Option<String>,
    /// Git only: folder with the skills in the repository (default: the repository root).
    #[serde(default)]
    pub path: Option<String>,
    /// Git only: commit, full or abbreviated (7 to 40 lowercase hex digits).
    #[serde(default)]
    pub rev: Option<String>,
    /// Git only: tag, resolved to its commit and locked.
    #[serde(default)]
    pub tag: Option<String>,
    /// Git only: branch, resolved to its tip commit and locked.
    #[serde(default)]
    pub branch: Option<String>,
    /// Skills of the source to use; default: every sub-directory with a `SKILL.md`.
    #[serde(default)]
    pub names: Option<Vec<String>>,
}

impl ToolPackageSpec {
    /// Name under which the tool `name` of the package appears in the spec.
    pub fn local_name<'a>(&'a self, name: &'a str) -> &'a str {
        self.renames.get(name).map_or(name, String::as_str)
    }

    /// Names in the spec of the tools that `tools` selects; `None` if all tools are imported
    /// (then the names are only known from the package).
    pub fn selected_names(&self) -> Option<Vec<String>> {
        self.tools.as_ref().map(|tools| {
            tools
                .iter()
                .map(|tool| self.local_name(tool).to_owned())
                .collect()
        })
    }
}

/// Transport of a remote MCP server (`url`, ADR-0112).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpHttpTransport {
    /// Streamable HTTP (protocol 2025-06-18, ADR-0051); the default.
    #[default]
    StreamableHttp,
    /// The older HTTP+SSE transport (protocol 2024-11-05, ADR-0112): `GET` opens an event
    /// stream, messages go by `POST` to the endpoint it announces.
    Sse,
}

/// An MCP server: locally as a process (`command`, stdio, ADR-0036) or remotely via
/// Streamable HTTP (`url`, ADR-0051) or HTTP+SSE (`url` with `transport: "sse"`, ADR-0112) –
/// exactly one of `command` and `url`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpServerSpec {
    /// Program that starts the server, e.g. `npx`.
    #[serde(default)]
    pub command: Option<String>,
    /// Arguments of the program.
    #[serde(default)]
    pub args: Vec<String>,
    /// Endpoint of a remote server, e.g. `https://mcp.example.com/mcp`.
    #[serde(default)]
    pub url: Option<String>,
    /// Transport of the remote server: `streamable_http` (default) or `sse` (the older
    /// HTTP+SSE transport); only together with `url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<McpHttpTransport>,
    /// Headers for `url`, e.g. `Authorization`; `${VAR}` is replaced from the environment.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Tools of the server that the agent uses; only these are offered.
    pub tools: Vec<String>,
    /// Offers the server's resources as the tools `<server>_list_resources` and
    /// `<server>_read_resource`.
    #[serde(default)]
    pub resources: bool,
    /// Workload identity towards `url` (ADR-0109); only for remote servers, not together with
    /// an `Authorization` header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthSpec>,
    /// Passes `SPIFFE_ENDPOINT_SOCKET` (and `SPIFFE_FETCH_TIMEOUT` if set) to the child process
    /// of a stdio server (ADR-0110). The child gets the identity of the agent itself.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub workload_api: bool,
}

/// How a client authenticates towards a remote agent or MCP server (ADR-0109).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthSpec {
    /// SPIFFE workload identity of this agent.
    pub spiffe: SpiffeAuthSpec,
}

/// SPIFFE authentication: mutual TLS with the X.509-SVID (default) or a JWT-SVID as bearer.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpiffeAuthSpec {
    /// `mtls` (default) or `jwt` (for targets behind a TLS-terminating ingress).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<SpiffeMode>,
    /// The SPIFFE ID the server must have, e.g. `spiffe://agentswork.de/ns/reviewer/sa/reviewer`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// SPIFFE IDs of which the server must have one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ids: Vec<String>,
    /// The server may have any SPIFFE ID of this trust domain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_domain: Option<String>,
    /// Audience of the JWT-SVID; required with `mode: "jwt"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
}

impl SpiffeAuthSpec {
    /// The mode, by default mutual TLS.
    pub fn mode(&self) -> SpiffeMode {
        self.mode.unwrap_or_default()
    }
}

/// Mode of SPIFFE authentication.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SpiffeMode {
    /// The client presents its X.509-SVID; the server is verified against the SPIFFE bundle.
    #[default]
    Mtls,
    /// The client sends a JWT-SVID as `Authorization: Bearer`.
    Jwt,
}

impl McpServerSpec {
    /// The transport of a remote server: the chosen one, by default Streamable HTTP.
    pub fn http_transport(&self) -> McpHttpTransport {
        self.transport.unwrap_or_default()
    }

    /// Names of the resource tools of server `name`; empty without `resources`.
    pub fn resource_tools(&self, name: &str) -> Vec<String> {
        if self.resources {
            vec![
                format!("{name}_list_resources"),
                format!("{name}_read_resource"),
            ]
        } else {
            Vec::new()
        }
    }
}

/// Built-in tools provided by the runtime (ADR-0046, ADR-0104).
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BuiltinToolsSpec {
    /// Names of the tools: `read_file`, `list_directory`, `write_file`, `edit_file`,
    /// `search_files`, `say_to_user`, `ask_user`, `think`, `fetch`, `store_value`/`retrieve_value`/`list_keys`, `add_task`/`complete_task`/`remove_task`/`list_tasks`, the `jira_*`, `confluence_*`, `git_*` and `run_command`
    /// tools (each group needs its section).
    pub tools: Vec<String>,
    /// Directory relative to the spec under which file tools and the git directory work and
    /// where `kv.json` lives; required for file tools and the key-value tools.
    #[serde(default)]
    pub root: Option<String>,
    /// Atlassian Cloud site of the `jira_*` tools (ADR-0104).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jira: Option<AtlassianSpec>,
    /// Atlassian Cloud site of the `confluence_*` tools (ADR-0104).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confluence: Option<AtlassianSpec>,
    /// The one repository of the `git_*` tools (ADR-0104).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitSpec>,
    /// The hosts the `fetch` tool may call (ADR-0098).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetch: Option<FetchSpec>,
    /// The programs of the `run_command` tool and its limits (ADR-0099).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandSpec>,
}

/// The `run_command` tool (ADR-0099): allowlist mode by default, shell mode only explicitly.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandSpec {
    /// Programs the tool may start: a bare name (looked up in the `PATH` of the cleared
    /// environment) or an absolute path. In shell mode exactly `["*"]`.
    pub allowed: Vec<String>,
    /// Shell mode: the tool takes a `command` line and runs it with `sh -c`. Needs
    /// `allowed: ["*"]` and `i_understand_shell: true`.
    #[serde(default)]
    pub shell: bool,
    /// Acknowledges that shell mode gives the model a shell with the permissions of the process.
    #[serde(default)]
    pub i_understand_shell: bool,
    /// Every call in shell mode needs an approval (default `true`; ignored in allowlist mode).
    #[serde(default = "default_true")]
    pub approve_each_call: bool,
    /// Seconds after which the process group is killed (1 to 3600, default 60).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Bytes of output kept (1 to 10485760, default 65536); the rest is cut off with a note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_bytes: Option<u64>,
    /// Environment variables handed to the program; everything else is cleared (default
    /// `["PATH"]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Vec<String>>,
    /// Passes `SPIFFE_ENDPOINT_SOCKET` (and `SPIFFE_FETCH_TIMEOUT` if set) to the allowlisted
    /// programs (ADR-0110); not in shell mode.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub workload_api: bool,
}

impl CommandSpec {
    /// The variables the program gets: `env`, otherwise only `PATH`.
    pub fn env_or_default(&self) -> Vec<String> {
        self.env.clone().unwrap_or_else(|| vec!["PATH".to_owned()])
    }

    /// Whether every call needs an approval: shell mode with `approve_each_call`.
    pub fn needs_approval(&self) -> bool {
        self.shell && self.approve_each_call
    }
}

/// The hosts of the `fetch` tool (ADR-0098).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FetchSpec {
    /// Hosts that may be called: an exact host name or IP address, or `*.domain` for all its
    /// subdomains. Private, loopback and link-local addresses are refused unless the host is
    /// listed exactly.
    pub allowed_hosts: Vec<String>,
    /// Also allow plain `http://` (default `false`: HTTPS only).
    #[serde(default)]
    pub allow_http: bool,
    /// Workload identity per host (ADR-0110): the key is an entry of `allowed_hosts`, the value
    /// has the form of `auth` of an MCP server. Requests to such a host use mTLS or a JWT-SVID
    /// and need `https://`; other hosts get no identity.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub auth: BTreeMap<String, AuthSpec>,
}

/// An Atlassian Cloud site; credentials come from `ATLASSIAN_EMAIL` and `ATLASSIAN_API_TOKEN`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AtlassianSpec {
    /// `https://<site>.atlassian.net`; a path such as `/wiki` is ignored.
    pub base_url: String,
}

/// Which branches `git_push` may push.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PushPolicy {
    /// Only branches that the agent created with `git_create_branch`.
    #[default]
    NewBranches,
    /// Every branch.
    Any,
}

/// The repository of the git tools.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitSpec {
    /// `https://` URL of the remote, without user info.
    pub repo_url: String,
    /// Working directory of the clone, relative to `root` (default `repo`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    /// Which branches may be pushed (default `new_branches`).
    #[serde(default)]
    pub push_policy: PushPolicy,
    /// Authenticate with `GIT_TOKEN` (default `false`).
    #[serde(default)]
    pub use_token: bool,
    /// Workload identity per host (ADR-0110): the key is the host of `repo_url` (or a pattern
    /// that matches it); clone, fetch and push use mTLS or a JWT-SVID instead of `GIT_TOKEN`.
    /// Excludes `use_token`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub auth: BTreeMap<String, AuthSpec>,
}

/// Whether `host` (lower case) is matched by `pattern`: exactly, or `*.domain` for subdomains
/// (the rule of `builtin_tools.fetch.allowed_hosts`, ADR-0098).
pub fn host_matches(pattern: &str, host: &str) -> bool {
    let (pattern, host) = (
        pattern.trim().to_ascii_lowercase(),
        host.to_ascii_lowercase(),
    );
    match pattern.strip_prefix("*.") {
        Some(domain) => host.len() > domain.len() + 1 && host.ends_with(&format!(".{domain}")),
        None => pattern == host,
    }
}

impl GitSpec {
    /// `directory`, or `repo`.
    pub fn directory_or_default(&self) -> &str {
        self.directory
            .as_deref()
            .map(str::trim)
            .filter(|directory| !directory.is_empty())
            .unwrap_or("repo")
    }
}

/// Built-in file tools; they need `builtin_tools.root`.
pub const BUILTIN_FILE_TOOLS: &[&str] = &[
    "read_file",
    "list_directory",
    "write_file",
    "edit_file",
    "search_files",
];

/// Built-in user tools.
pub const BUILTIN_USER_TOOLS: &[&str] = &["say_to_user", "ask_user"];

/// Built-in Jira tools; they need `builtin_tools.jira` (ADR-0104).
pub const BUILTIN_JIRA_TOOLS: &[&str] = &[
    "jira_search_issues",
    "jira_get_issue",
    "jira_create_issue",
    "jira_add_comment",
    "jira_transition_issue",
];

/// Built-in Confluence tools; they need `builtin_tools.confluence` (ADR-0104).
pub const BUILTIN_CONFLUENCE_TOOLS: &[&str] = &[
    "confluence_search_pages",
    "confluence_get_page",
    "confluence_create_page",
    "confluence_update_page",
];

/// Built-in git tools; they need `builtin_tools.git` (ADR-0104).
pub const BUILTIN_GIT_TOOLS: &[&str] = &[
    "git_checkout",
    "git_list_branches",
    "git_create_branch",
    "git_commit",
    "git_push",
];

/// Built-in tool `fetch`; it needs `builtin_tools.fetch` (ADR-0098).
pub const BUILTIN_FETCH_TOOLS: &[&str] = &["fetch"];

/// Built-in tool `run_command`; it needs `builtin_tools.command` and a root (ADR-0099).
pub const BUILTIN_COMMAND_TOOLS: &[&str] = &["run_command"];

/// Built-in key-value tools; they need `builtin_tools.root`, where `kv.json` lives (ADR-0098).
pub const BUILTIN_KV_TOOLS: &[&str] = &["store_value", "retrieve_value", "list_keys"];

/// Built-in task list tools; the list lives in the agent storage of the run (ADR-0098).
pub const BUILTIN_TODO_TOOLS: &[&str] = &["add_task", "complete_task", "remove_task", "list_tasks"];

/// Built-in tool `think` (ADR-0098).
pub const BUILTIN_THINK_TOOLS: &[&str] = &["think"];

/// A sub-agent as a tool: local (`spec`) or remote via A2A (`url`, ADR-0055) – exactly one
/// of the two; each request runs with its own history (ADR-0040).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentToolSpec {
    /// Spec file of a local sub-agent, relative to this spec.
    #[serde(default)]
    pub spec: Option<String>,
    /// Address of a remote A2A agent: agent card or base URL (ADR-0055).
    #[serde(default)]
    pub url: Option<String>,
    /// Headers for `url`, e.g. `Authorization`; `${VAR}` is replaced from the environment.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Description for the LLM: what the sub-agent is for; with `url` and no value, taken
    /// from the agent card.
    #[serde(default)]
    pub description: Option<String>,
    /// Workload identity towards `url` (ADR-0109); not together with an `Authorization` header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthSpec>,
}

/// Names of the built-in skill tools (ADR-0103); offered when the spec has `skills`.
pub const BUILTIN_SKILL_TOOLS: &[&str] = &["load_skill", "read_skill_file"];

/// Knowledge sources: they are split into sections at startup, embedded, and made searchable
/// via the tool `tool` (ADR-0038).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeSpec {
    /// Files or directories (recursive), relative to the spec file.
    #[serde(default)]
    pub sources: Vec<String>,
    /// Web pages (`https://`, plain `http://` only for `localhost`/`127.0.0.1`) that are fetched
    /// at every start and indexed like files; text, Markdown and HTML (ADR-0113).
    #[serde(default)]
    pub urls: Vec<String>,
    /// Embedding model, e.g. `{"provider": "ollama", "id": "nomic-embed-text"}`.
    pub embedding_model: ModelSpec,
    /// Name of the search tool; shares the namespace with the other tools.
    #[serde(default = "default_knowledge_tool")]
    pub tool: String,
    /// Number of sections per search.
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    /// Maximum length of a section in characters.
    #[serde(default = "default_chunk_size")]
    pub chunk_size: usize,
    /// Minimum cosine similarity (0 to 1) of a hit; weaker sections are dropped (ADR-0113).
    #[serde(default)]
    pub min_similarity: Option<f32>,
    /// Adds the best sections for each new user input to the prompt as a system message
    /// (default `false`; ADR-0113).
    #[serde(default)]
    pub inject: bool,
    /// Offers the search tool `tool` to the model (default `true`); `false` with `inject: true`
    /// uses the knowledge only as context (ADR-0113).
    #[serde(default = "default_true")]
    pub tool_enabled: bool,
    /// File relative to the spec in which sections and embeddings persist between runs; only
    /// new or changed sources are then re-embedded (ADR-0057). Applies to the store kind `file`.
    #[serde(default)]
    pub index: Option<String>,
    /// Where sections and embeddings live: `file` (default, with `index`) or `postgres`
    /// (PostgreSQL with pgvector, ADR-0106).
    #[serde(default)]
    pub store: Option<KnowledgeStoreSpec>,
}

/// Kind of the knowledge store (ADR-0106).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeStoreKind {
    /// Vectors in memory, optionally persisted in the `index` file (ADR-0057).
    #[default]
    File,
    /// PostgreSQL with the pgvector extension.
    Postgres,
}

/// The `store` of `knowledge`: `{ "kind": "postgres", "url_env": "PGVECTOR_URL" }`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeStoreSpec {
    /// `file` or `postgres`.
    pub kind: KnowledgeStoreKind,
    /// Name of the environment variable that holds the connection URL (kind `postgres`; the
    /// URL itself never belongs in the spec).
    #[serde(default)]
    pub url_env: Option<String>,
}

/// Long-term memory: facts that the agent stores via one tool and retrieves via another;
/// they remain in `store` across runs and conversations (ADR-0058).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LongTermMemorySpec {
    /// JSON file relative to the spec in which facts and embeddings are stored. Exactly one of
    /// `store` and `vector_store`.
    #[serde(default)]
    pub store: Option<String>,
    /// PostgreSQL with pgvector as the store, with expiry and a minimum similarity (ADR-0106).
    /// Exactly one of `store` and `vector_store`.
    #[serde(default)]
    pub vector_store: Option<MemoryVectorStoreSpec>,
    /// Embedding model, e.g. `{"provider": "ollama", "id": "nomic-embed-text"}`.
    pub embedding_model: ModelSpec,
    /// Name of the tool for storing.
    #[serde(default = "default_remember_tool")]
    pub remember_tool: String,
    /// Name of the tool for retrieving.
    #[serde(default = "default_recall_tool")]
    pub recall_tool: String,
    /// Number of facts per retrieval.
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    /// Before each run, retrieves the facts most relevant to the input and appends them as
    /// a system message (ADR-0069).
    #[serde(default)]
    pub auto_recall: bool,
    /// After each successful run, extracts lasting facts from input and response and stores
    /// them (ADR-0069).
    #[serde(default)]
    pub auto_remember: bool,
    /// Offers the agent the store and recall tools; with `false` the memory works only
    /// automatically (ADR-0069).
    #[serde(default = "default_true")]
    pub tools: bool,
    /// Stores lasting facts from the compressed history (`compress_history` with `facts`,
    /// strategy `chat`) in memory (ADR-0079).
    #[serde(default)]
    pub remember_compressed: bool,
}

/// Kind of the memory's vector store (ADR-0106).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VectorStoreKind {
    /// PostgreSQL with the pgvector extension.
    Postgres,
}

/// The `vector_store` of `long_term_memory`:
/// `{ "kind": "postgres", "url_env": "PGVECTOR_URL", "ttl_days": 90, "min_similarity": 0.4 }`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryVectorStoreSpec {
    /// Only `postgres`.
    pub kind: VectorStoreKind,
    /// Name of the environment variable that holds the connection URL (the URL itself never
    /// belongs in the spec).
    pub url_env: String,
    /// Facts expire this many days after they were stored (at least 1); without a value they
    /// never expire.
    #[serde(default)]
    pub ttl_days: Option<u32>,
    /// Recall returns only facts at least this similar to the query (0 to 1); without a value
    /// the `top_k` best facts are returned however weak.
    #[serde(default)]
    pub min_similarity: Option<f32>,
}

/// Limits per run (ADR-0078); without a value the defaults apply (50 steps per graph run,
/// otherwise unlimited).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LimitsSpec {
    /// At most this many node executions per graph run (≥ 1).
    #[serde(default)]
    pub max_steps: Option<usize>,
    /// At most this many tool calls per run, including parallel branches and subgraphs.
    #[serde(default)]
    pub max_tool_calls: Option<u64>,
    /// At most this many input and output tokens per run (≥ 1).
    #[serde(default)]
    pub max_tokens: Option<u64>,
    /// At most this cost per run in the currency of the price table (> 0, ADR-0071).
    #[serde(default)]
    pub max_cost: Option<f64>,
    /// At most this many tokens per request, estimated in advance; larger requests are not
    /// sent (≥ 1, ADR-0087).
    #[serde(default)]
    pub max_prompt_tokens: Option<u64>,
}

/// Self-created tools (ADR-0075): the model writes tool code that is built into the sandbox
/// and offered in the same run.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelfToolsSpec {
    /// May the agent build outside of subgraphs? Subgraphs decide with their own
    /// `self_tools` (addendum ADR-0075).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Name of the tool with which the agent creates new tools.
    #[serde(default = "default_create_tool")]
    pub tool: String,
    /// At most this many new tools per run.
    #[serde(default = "default_max_tools")]
    pub max_tools: usize,
    /// At most this many bytes of tool code per tool.
    #[serde(default = "default_max_code_bytes")]
    pub max_code_bytes: usize,
    /// A human confirms each new tool before it is built (ADR-0061).
    #[serde(default = "default_true")]
    pub confirm: bool,
    /// Saves built tools in the `tools_dir` so that later runs have them (ADR-0074).
    #[serde(default)]
    pub save: bool,
}

fn default_create_tool() -> String {
    "create_tool".into()
}

fn default_max_tools() -> usize {
    5
}

fn default_max_code_bytes() -> usize {
    20_000
}

fn default_true() -> bool {
    true
}

fn default_remember_tool() -> String {
    "remember".into()
}

fn default_recall_tool() -> String {
    "recall".into()
}

fn default_knowledge_tool() -> String {
    "search_knowledge".into()
}

fn default_top_k() -> usize {
    3
}

fn default_chunk_size() -> usize {
    1000
}

/// A named subgraph with its own signature and optional tool subset (ADR-0034).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubgraphSpec {
    /// Input type: `string` or a name from `types`.
    #[serde(default = "string_type")]
    pub input: String,
    /// Output type: `string` or a name from `types`.
    #[serde(default = "string_type")]
    pub output: String,
    /// Tools available in the subgraph; without a value, all tools of the spec.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    /// May the agent build tools in the subgraph and use self-built ones (also without an entry
    /// in `tools`)? Without a value, the outer setting applies (addendum ADR-0075).
    #[serde(default)]
    pub self_tools: Option<bool>,
    /// Own model in the subgraph; afterwards the outer model applies again (ADR-0043).
    #[serde(default)]
    pub model: Option<ModelSpec>,
    /// Graph of the subgraph; `start` provides the input, `finish` takes the output.
    pub graph: GraphSpec,
}

/// Node kind that includes a subgraph (`params.subgraph` names it).
pub const SUBGRAPH_KIND: &str = "subgraph";

/// Node kind of an LLM planner (ADR-0039): `params.subgraph` runs each step
/// (text → text), optionally `critic` and `max_iterations`; signature text → text.
pub const PLAN_KIND: &str = "plan";

/// Node kind for GOAP over named facts (ADR-0070); like `plan`, not in the catalog.
pub const GOAP_KIND: &str = "goap";

/// Node kind for parallel branches (ADR-0045): `params.subgraphs` run concurrently,
/// `params.merge` (`concat`, `index` with `index`, `llm_select` with `criteria`) merges them.
pub const PARALLEL_KIND: &str = "parallel";

/// Node kind that runs a body once per element of a list (ADR-0101); parameters in
/// [`ForEachParams`]. Like `parallel` it needs the spec, so it is not in the catalog.
pub const FOR_EACH_KIND: &str = "for_each";

/// Node kind that runs a sub-agent from `agent_tools` as a step (ADR-0101): `params.agent`.
pub const AGENT_KIND: &str = "agent";

/// A node kind of the spec that is not in the node catalog, because it needs the spec (its
/// subgraphs, agents or code): name and one sentence on what it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StructuralKind {
    /// Name, as it appears in specs under `kind`.
    pub kind: &'static str,
    /// What a node of this kind does.
    pub description: &'static str,
}

/// The node kinds outside the catalog (ADR-0018): together with the catalog the whole palette of
/// an editor.
pub const STRUCTURAL_NODE_KINDS: &[StructuralKind] = &[
    StructuralKind {
        kind: SUBGRAPH_KIND,
        description: "Runs the subgraph named in `subgraph`.",
    },
    StructuralKind {
        kind: PLAN_KIND,
        description: "An LLM planner; every step runs in the subgraph `subgraph` (text to text).",
    },
    StructuralKind {
        kind: GOAP_KIND,
        description: "Plans over named facts with `actions` and `goals` (goal-oriented action planning).",
    },
    StructuralKind {
        kind: PARALLEL_KIND,
        description: "Runs the subgraphs in `subgraphs` concurrently and merges their results with `merge`.",
    },
    StructuralKind {
        kind: FOR_EACH_KIND,
        description: "Runs the body (an agent or a subgraph) once per element of a list.",
    },
    StructuralKind {
        kind: crate::code::CODE_KIND,
        description: "Runs Rust code on the value (`input`, `output`, `code`).",
    },
];

/// The condition kinds outside the condition catalog: `on_code` (ADR-0102).
pub const STRUCTURAL_CONDITION_KINDS: &[StructuralKind] = &[StructuralKind {
    kind: crate::code::ON_CODE_KIND,
    description: "Decides with Rust code on the value (`input`, `code`).",
}];

/// Default for `max_iterations` of a `plan` node.
pub const DEFAULT_PLAN_ITERATIONS: u64 = 20;

/// Schema of the parameters of a `plan` node.
pub fn plan_params_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "subgraph": { "type": "string" },
            "critic": { "type": "boolean" },
            "max_iterations": { "type": "integer", "minimum": 1 }
        },
        "required": ["subgraph"],
        "additionalProperties": false
    })
}

/// Strategy template (ADR-0041), e.g. `{"kind": "single_run", "params": {"parallel_tools": true}}`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StrategySpec {
    /// Kind of the template: `single_run` (tool loop, `params.parallel_tools`,
    /// `params.moderate`), `react` (think before each action, `params.reasoning_prompt`),
    /// `verified` (have the answer checked and revised with the feedback,
    /// `params.verify_prompt`; defines the type `Verification`) or `chat` (long
    /// conversations, compression to facts, `params.max_messages`/`keep`/`parallel_tools`;
    /// ADR-0076).
    pub kind: String,
    /// Parameters of the template; their schema depends on `kind`.
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

/// Strategy graph of nodes and edges.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphSpec {
    /// Nodes of the graph; `start` and `finish` are implicitly present.
    #[serde(default)]
    pub nodes: Vec<NodeSpec>,
    /// Directed edges between nodes (incl. `start`/`finish`).
    #[serde(default)]
    pub edges: Vec<EdgeSpec>,
}

/// Directed edge between two nodes.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EdgeSpec {
    /// ID of the source node or `start`.
    pub from: String,
    /// ID of the target node or `finish`.
    pub to: String,
    /// Optional condition; without a condition the value is passed on unchanged.
    #[serde(default)]
    pub when: Option<ConditionSpec>,
    /// Optional transformation of the value after the condition matched (ADR-0102); the target
    /// node receives its result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<TransformSpec>,
}

/// Edge condition from the condition catalog (ADR-0021).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConditionSpec {
    /// Kind of the condition, e.g. `on_assistant_message`.
    pub kind: String,
    /// Parameters of the condition; their schema depends on `kind`.
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

/// A node in the graph.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeSpec {
    /// Unique ID that edges refer to.
    pub id: String,
    /// Kind of the node, e.g. `llm_request`; the node catalog defines the valid kinds.
    pub kind: String,
    /// Parameters of the node; their schema depends on `kind`.
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

/// Choice of the LLM.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelSpec {
    /// Name of the provider, e.g. `openai`; valid are the providers registered at runtime.
    pub provider: String,
    /// Model ID at the provider, e.g. `gpt-4o`.
    pub id: String,
    /// Fallback models, asked one after another when a request fails (ADR-0088).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<FallbackModelSpec>,
    /// Sampling temperature, 0 to 2; the provider default if absent (ADR-0097).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Reasoning of the model on or off; only for the provider `ollama`, takes precedence over
    /// `OLLAMA_THINK` (ADR-0097).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub think: Option<bool>,
}

/// A fallback model (ADR-0088); it has no fallback models of its own.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FallbackModelSpec {
    /// Name of the provider, e.g. `openai`.
    pub provider: String,
    /// Model ID at the provider.
    pub id: String,
    /// Sampling temperature, 0 to 2 (ADR-0097).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Reasoning on or off; only for the provider `ollama` (ADR-0097).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub think: Option<bool>,
}

/// Error when loading an [`AgentSpec`].
#[derive(Debug, thiserror::Error)]
pub enum SpecError {
    /// The document is not valid JSON or does not match the spec format.
    /// The JSON does not match the spec format: unknown field, missing required field, wrong type
    /// (ADR-0080).
    #[error("{pointer}: {message}")]
    Format { pointer: String, message: String },
    #[error("invalid spec JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// `strategy` is unknown, has invalid parameters, or stands next to `graph`.
    #[error("/strategy: {0}")]
    Strategy(String),
    /// The `spec_version` of the document is not supported.
    #[error("spec_version {found} is not supported (supported: {SUPPORTED_SPEC_VERSION})")]
    UnsupportedVersion { found: u64 },
}

/// Name of the built-in text type.
pub const STRING_TYPE: &str = "string";

fn string_type() -> String {
    STRING_TYPE.into()
}

/// Spec version that this crate reads.
pub const SUPPORTED_SPEC_VERSION: u64 = 1;

impl AgentSpec {
    /// JSON schema of the spec format (F13.2), e.g. for structured output of an LLM.
    pub fn json_schema() -> serde_json::Value {
        schemars::schema_for!(AgentSpec).to_value()
    }

    /// Names of the tools that come from outside: declared MCP tools, the search tool from
    /// `knowledge`, the memory tools, sub-agents and built-in tools (ADR-0050, ADR-0058).
    pub fn external_tools(&self) -> Vec<String> {
        self.tool_namespace()
            .entries()
            .iter()
            .filter(|entry| entry.origin.is_external())
            .map(|entry| entry.name.clone())
            .collect()
    }

    /// Every place that names a provider, with its JSON pointer: main model, subgraph models,
    /// `set_model`/`llm_moderate` with their own model, `decide` with the `llm` decider's model (`provider/id`), embedding models of `knowledge` and
    /// `long_term_memory` – e.g. for checking against built-in providers (ADR-0080).
    pub fn provider_uses(&self) -> Vec<(String, &str)> {
        let mut uses = model_uses(&self.model, "/model");
        uses.extend(provider_nodes(&self.graph, "/graph"));
        for (name, subgraph) in &self.subgraphs {
            let base = format!("/subgraphs/{}", escape_pointer_token(name));
            if let Some(model) = &subgraph.model {
                uses.extend(model_uses(model, &format!("{base}/model")));
            }
            uses.extend(provider_nodes(&subgraph.graph, &format!("{base}/graph")));
        }
        if let Some(knowledge) = &self.knowledge {
            uses.push((
                "/knowledge/embedding_model/provider".into(),
                knowledge.embedding_model.provider.as_str(),
            ));
        }
        if let Some(memory) = &self.long_term_memory {
            uses.push((
                "/long_term_memory/embedding_model/provider".into(),
                memory.embedding_model.provider.as_str(),
            ));
        }
        uses
    }

    /// All providers the spec uses: main model, subgraph models including fallback models
    /// (ADR-0088), `set_model`- and
    /// `llm_moderate` nodes with their own model, the model of `decide` nodes with the `llm` decider, sorted and without duplicates (ADR-0043, ADR-0066).
    pub fn providers(&self) -> Vec<&str> {
        let graphs = std::iter::once(&self.graph)
            .chain(self.subgraphs.values().map(|subgraph| &subgraph.graph));
        let switched = graphs.flat_map(|graph| {
            graph
                .nodes
                .iter()
                .filter(|node| node.kind == "set_model" || node.kind == "llm_moderate")
                .filter_map(|node| node.params.as_ref())
                .flat_map(|params| {
                    let fallbacks = params
                        .get("fallbacks")
                        .and_then(serde_json::Value::as_array)
                        .into_iter()
                        .flatten();
                    std::iter::once(params.get("provider"))
                        .chain(fallbacks.map(|fallback| fallback.get("provider")))
                        .filter_map(|provider| provider?.as_str())
                })
        });
        // The model decider of a decision node (`provider/id`) needs a client (ADR-0100).
        let deciding = std::iter::once(&self.graph)
            .chain(self.subgraphs.values().map(|subgraph| &subgraph.graph))
            .flat_map(|graph| &graph.nodes)
            .filter(|node| node.kind == "decide")
            .filter_map(|node| node.params.as_ref())
            .filter(|params| {
                params.get("decider").and_then(serde_json::Value::as_str) != Some("system_one")
            })
            .filter_map(|params| params.get("model")?.as_str()?.split_once('/'))
            .map(|(provider, _)| provider);
        let models = std::iter::once(&self.model).chain(
            self.subgraphs
                .values()
                .filter_map(|subgraph| subgraph.model.as_ref()),
        );
        // Fallback models also need a client (ADR-0088).
        let providers: BTreeSet<&str> = models
            .flat_map(|model| {
                std::iter::once(model.provider.as_str()).chain(
                    model
                        .fallbacks
                        .iter()
                        .map(|fallback| fallback.provider.as_str()),
                )
            })
            .chain(switched)
            .chain(deciding)
            .collect();
        providers.into_iter().collect()
    }

    /// The tools that need an approval before every call (ADR-0061, ADR-0119): the tool that
    /// creates tools when `self_tools.confirm` is set, and `run_command` in shell mode with
    /// `approve_each_call` (ADR-0099). The interpreter and generated code gate the same ones.
    pub fn approval_tools(&self) -> Vec<String> {
        let mut tools = Vec::new();
        if let Some(self_tools) = self.self_tools.as_ref().filter(|tools| tools.confirm) {
            tools.push(self_tools.tool.clone());
        }
        if self
            .builtin_tools
            .as_ref()
            .and_then(|builtin| builtin.command.as_ref())
            .is_some_and(CommandSpec::needs_approval)
        {
            tools.push("run_command".to_owned());
        }
        tools
    }

    /// `true` if a decision node asks the `system_one` decider (ADR-0100); the run then needs a
    /// System One service.
    pub fn uses_system_one(&self) -> bool {
        std::iter::once(&self.graph)
            .chain(self.subgraphs.values().map(|subgraph| &subgraph.graph))
            .flat_map(|graph| &graph.nodes)
            .filter(|node| node.kind == "decide")
            .any(|node| {
                node.params
                    .as_ref()
                    .and_then(|params| params.get("decider"))
                    .and_then(serde_json::Value::as_str)
                    == Some("system_one")
            })
    }

    /// The spec as JSON, which `from_json` reads again; a strategy is already
    /// in it as `graph` (ADR-0084).
    pub fn to_json(&self) -> String {
        let spec = AgentSpec {
            strategy: None,
            ..self.clone()
        };
        serde_json::to_string(&spec).expect("an AgentSpec can always be written as JSON")
    }

    /// JSON pointer of `graph`, which must be the main graph or a subgraph of this spec (the
    /// same object, not an equal one): `/graph` or `/subgraphs/<name>/graph`.
    pub fn graph_pointer(&self, graph: &GraphSpec) -> Option<String> {
        if std::ptr::eq(graph, &self.graph) {
            return Some("/graph".into());
        }
        self.subgraphs
            .iter()
            .find(|(_, subgraph)| std::ptr::eq(&subgraph.graph, graph))
            .map(|(name, _)| format!("/subgraphs/{}/graph", escape_pointer_token(name)))
    }

    /// Parses a spec from a JSON string **without** expanding its `strategy`; use
    /// `grauwolf_spec::AgentSpecExt::from_json` for the complete loading.
    ///
    /// The version is read first so that documents of other versions are not misinterpreted as
    /// the current version (ADR-0025).
    pub fn parse(json: &str) -> Result<Self, SpecError> {
        let document: serde_json::Value = serde_json::from_str(json)?;
        if let Some(found) = document
            .get("spec_version")
            .and_then(serde_json::Value::as_u64)
            && found != SUPPORTED_SPEC_VERSION
        {
            return Err(SpecError::UnsupportedVersion { found });
        }
        serde_path_to_error::deserialize(document).map_err(format_error)
    }
}

/// Escapes `token` as a segment of a JSON pointer (RFC 6901).
pub fn escape_pointer_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

/// Format error with JSON pointer and English message for the common cases (ADR-0080).
fn format_error(error: serde_path_to_error::Error<serde_json::Error>) -> SpecError {
    use serde_path_to_error::Segment;
    let mut pointer = String::new();
    for segment in error.path().iter() {
        match segment {
            Segment::Seq { index } => pointer.push_str(&format!("/{index}")),
            Segment::Map { key } => {
                pointer.push_str(&format!("/{}", key.replace('~', "~0").replace('/', "~1")))
            }
            Segment::Enum { variant } => pointer.push_str(&format!("/{variant}")),
            Segment::Unknown => {}
        }
    }
    let message = error.inner().to_string();
    // serde appends line and column; the pointer says more.
    let message = message
        .split(" at line ")
        .next()
        .unwrap_or(&message)
        .to_owned();
    let quoted = |text: &str| text.split('`').nth(1).map(str::to_owned);
    let (pointer, message) = if let Some(rest) = message.strip_prefix("unknown field ") {
        let field = quoted(rest).unwrap_or_default();
        let allowed = rest
            .split_once(", expected ")
            .map(|(_, allowed)| allowed.trim_start_matches("one of ").replace(" or ", ", "))
            .unwrap_or_default();
        (
            with_field(pointer, &field),
            format!("unknown field (allowed: {allowed})"),
        )
    } else if let Some(rest) = message.strip_prefix("missing field ") {
        let field = quoted(rest).unwrap_or_default();
        (
            with_field(pointer, &field),
            "missing required field".to_owned(),
        )
    } else if let Some(rest) = message.strip_prefix("invalid type: ") {
        (pointer, format!("wrong type: {rest}"))
    } else if let Some(rest) = message.strip_prefix("unknown variant ") {
        let rest = rest.replace(", expected ", ", allowed: ");
        (pointer, format!("unknown value {rest}"))
    } else {
        (pointer, message)
    };
    SpecError::Format {
        pointer: if pointer.is_empty() {
            "/".into()
        } else {
            pointer
        },
        message,
    }
}

/// `pointer` with the field `field` at the end, if it is not already there.
fn with_field(pointer: String, field: &str) -> String {
    if field.is_empty() || pointer.ends_with(&format!("/{field}")) {
        pointer
    } else {
        format!("{pointer}/{field}")
    }
}

/// Provider of the `set_model` and `llm_moderate` nodes in `graph`, with pointer under `base`.
/// Provider of a model together with its fallback models, with their pointers under `base`.
fn model_uses<'a>(model: &'a ModelSpec, base: &str) -> Vec<(String, &'a str)> {
    std::iter::once((format!("{base}/provider"), model.provider.as_str()))
        .chain(model.fallbacks.iter().enumerate().map(|(index, fallback)| {
            (
                format!("{base}/fallbacks/{index}/provider"),
                fallback.provider.as_str(),
            )
        }))
        .collect()
}

fn provider_nodes<'a>(graph: &'a GraphSpec, base: &str) -> Vec<(String, &'a str)> {
    graph
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| ["set_model", "llm_moderate", "decide"].contains(&node.kind.as_str()))
        .filter_map(|(index, node)| Some((index, node.params.as_ref()?)))
        .flat_map(|(index, params)| {
            let path = format!("{base}/nodes/{index}/params");
            // The model decider names its model as `provider/id` (ADR-0100).
            let decided = params
                .get("model")
                .and_then(serde_json::Value::as_str)
                .filter(|_| {
                    params.get("decider").and_then(serde_json::Value::as_str) != Some("system_one")
                })
                .and_then(|model| model.split_once('/'))
                .map(|(provider, _)| (format!("{path}/model"), provider));
            let main = params
                .get("provider")
                .and_then(serde_json::Value::as_str)
                .map(|provider| (format!("{path}/provider"), provider));
            let fallbacks = params
                .get("fallbacks")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
                .filter_map(move |(position, fallback)| {
                    let provider = fallback.get("provider")?.as_str()?;
                    Some((format!("{path}/fallbacks/{position}/provider"), provider))
                });
            main.into_iter()
                .chain(decided)
                .chain(fallbacks)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The host (lower case, without user info and port) of an `https://` URL.
pub fn origin_host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?;
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next()?,
        None => authority.split(':').next()?,
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}
