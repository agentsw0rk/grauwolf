//! Semantic validation of an [`AgentSpec`].
//!
//! All errors are collected, each with a JSON pointer to the affected place,
//! so that people and LLMs can correct them specifically.

use grauwolf_graph::{Catalogs, DataType};

use crate::{AgentSpec, GraphSpec, ToolOrigin};

pub use grauwolf_graph::{FINISH, START};

mod builtin_tools;
mod code;
mod decisions;
mod edges;
mod goap;
mod graph;
mod models;
mod reachability;
mod schema;
mod sections;
mod skills;
mod sub_agents;
mod subgraphs;
mod tool_packages;
mod tools;
mod warnings;

use decisions::check_decisions;
use reachability::check_reachability;

/// A semantic error in a spec.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {kind}")]
pub struct ValidationError {
    /// JSON pointer (RFC 6901) to the faulty place.
    pub path: String,
    /// Kind of the error.
    pub kind: ValidationErrorKind,
}

/// A finding that does not make the spec invalid, but probably is not what the author wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecWarning {
    /// JSON pointer (RFC 6901) to the place the warning is about.
    pub pointer: String,
    /// Human-readable text.
    pub message: String,
}

impl std::fmt::Display for SpecWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.pointer, self.message)
    }
}

/// Kinds of semantic errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationErrorKind {
    /// An edge refers to a node that does not exist.
    #[error("unknown node '{id}'")]
    UnknownNode { id: String },
    /// A node uses a reserved ID (`start` or `finish`).
    #[error("'{id}' is reserved and must not be used as a node ID")]
    ReservedNodeId { id: String },
    /// An ID is not a snake_case identifier (`[a-z][a-z0-9_]*`).
    #[error(
        "'{id}' is not a valid ID: allowed are lowercase letters, digits and '_', starting with a letter"
    )]
    InvalidId { id: String },
    /// A node ID is already taken.
    #[error("node ID '{id}' is used more than once")]
    DuplicateNodeId { id: String },
    /// A fallback model equals the model or an earlier fallback model (ADR-0088).
    #[error("fallback model '{model}' is already listed")]
    DuplicateFallback { model: String },
    /// Embedding models have no fallback models (ADR-0088).
    #[error(
        "embedding models have no fallback models: a different model would produce different vectors"
    )]
    EmbeddingFallbacks,
    /// `temperature` is not a number from 0 to 2 (ADR-0097).
    #[error("temperature must be a number from 0 to 2")]
    TemperatureOutOfRange,
    /// `think` is only supported by the provider `ollama` (ADR-0097).
    #[error("'think' is only supported by the provider 'ollama', not '{provider}'")]
    ThinkNotSupported { provider: String },
    /// `temperature` or `think` on a `decide` node with the `system_one` decider, which is a
    /// decision model and not a language model (ADR-0100).
    #[error("the 'system_one' decider is a decision model: it has no temperature or think")]
    SamplingWithSystemOne,
    /// Embedding models have no sampling settings (ADR-0097).
    #[error("embedding models have no temperature or think")]
    EmbeddingSampling,
    /// `builtin_tools.fetch.allowed_hosts` is empty (ADR-0098).
    #[error("allowed_hosts is empty: the fetch tool could not call any host")]
    EmptyAllowedHosts,
    /// An entry of `allowed_hosts` is not a host name, an IP address or `*.domain` (ADR-0098).
    #[error(
        "'{host}' is not a host: write a host name, an IP address or *.domain, without scheme, path or port"
    )]
    InvalidAllowedHost { host: String },
    /// `builtin_tools.command.allowed` is empty (ADR-0099).
    #[error("allowed is empty: the command tool could not start any program")]
    EmptyAllowedCommands,
    /// `*` in `allowed` without shell mode (ADR-0099).
    #[error("'*' is only allowed together with \"shell\": true")]
    WildcardWithoutShell,
    /// An entry of `allowed` is neither a bare program name nor an absolute path (ADR-0099).
    #[error(
        "'{program}' is not a program: write a bare name or an absolute path, without arguments"
    )]
    InvalidAllowedProgram { program: String },
    /// Shell mode with an `allowed` other than `["*"]` (ADR-0099).
    #[error("shell mode needs exactly \"allowed\": [\"*\"]")]
    ShellNeedsWildcard,
    /// Shell mode without `i_understand_shell: true` (ADR-0099).
    #[error("shell mode needs \"i_understand_shell\": true")]
    ShellNotAcknowledged,
    /// `timeout_secs` or `max_output_bytes` outside the sensible range (ADR-0099).
    #[error("{field} is out of range")]
    CommandLimitOutOfRange { field: String },
    /// A tool name is already taken by an earlier declaration (ADR-0117): the error is at the
    /// later one, `other_origin` and `other_pointer` name the earlier one.
    #[error(
        "tool name '{name}' is used more than once (the other one is the {other_origin} at {other_pointer})"
    )]
    DuplicateToolName {
        name: String,
        other_origin: ToolOrigin,
        other_pointer: String,
    },
    /// A code piece has no code (ADR-0102).
    #[error("the code is missing or empty")]
    MissingCode,
    /// The input type that a transformation declares differs from what the edge delivers
    /// (ADR-0102).
    #[error("the transformation declares the input {expected}, the edge delivers {found}")]
    TransformInputMismatch { expected: DataType, found: DataType },
    /// A node kind is not known in the node catalog.
    #[error("unknown node kind '{kind}'")]
    UnknownNodeKind { kind: String },
    /// The parameters of a node do not match its kind.
    #[error("invalid parameters for '{kind}': {reason}")]
    InvalidParams { kind: String, reason: String },
    /// A condition kind is not known in the condition catalog.
    #[error("unknown condition '{kind}'")]
    UnknownConditionKind { kind: String },
    /// A type name is not a PascalCase identifier (`[A-Z][A-Za-z0-9]*`).
    #[error(
        "'{name}' is not a valid type name: allowed are letters and digits, starting with an uppercase letter"
    )]
    InvalidTypeName { name: String },
    /// A type schema lies outside the subset that the interpreter and generator both
    /// support (ADR-0031, ADR-0080).
    #[error("unsupported schema: {reason}")]
    UnsupportedSchema { reason: String },
    /// A type reference names neither `string` nor a type from `types`.
    #[error("unknown type '{name}': allowed are 'string' and the names from 'types'")]
    UnknownType { name: String },
    /// No path leads from `start` to `finish`; the graph could never produce a result.
    #[error("'finish' is not reachable from 'start'")]
    FinishUnreachable,
    /// A node is not reachable from `start` and would never run.
    #[error("node '{id}' is not reachable from 'start'")]
    UnreachableNode { id: String },
    /// No path leads from a node to `finish`; a run would get stuck there.
    #[error("no path leads from node '{id}' to 'finish' – an outgoing edge is missing")]
    DeadEnd { id: String },
    /// A node includes a subgraph that is not in `subgraphs`.
    #[error("unknown subgraph '{name}'")]
    UnknownSubgraph { name: String },
    /// An `agent` node or a `for_each` body names an agent that is not in `agent_tools`
    /// (ADR-0101).
    #[error("unknown agent '{name}': it is not in agent_tools")]
    UnknownAgent { name: String },
    /// The type that a node declares for a sub-agent differs from the type in the sub-agent's
    /// own spec (ADR-0101).
    #[error(
        "the agent '{agent}' has the {field} type '{actual}', but the node declares '{declared}'"
    )]
    SubAgentTypeMismatch {
        agent: String,
        field: String,
        declared: String,
        actual: String,
    },
    /// A subgraph names a tool that is not in `tools`.
    #[error("unknown tool '{name}'")]
    UnknownTool { name: String },
    /// A subgraph includes itself, directly or through other subgraphs.
    #[error("subgraph '{name}' includes itself")]
    SubgraphCycle { name: String },
    /// An MCP server does not name exactly one of `command` and `url`.
    #[error("MCP server needs exactly one of `command` (local) and `url` (remote)")]
    McpTransport,
    /// `transport` of an MCP server without `url` (it only selects the HTTP transport).
    #[error("`transport` only applies to a remote MCP server with `url`")]
    McpTransportWithoutUrl,
    /// `auth` on a server or agent without `url` (stdio MCP server, local sub-agent).
    #[error("`auth` only applies to a remote server or agent with `url`")]
    AuthWithoutUrl,
    /// `auth` together with an `Authorization` header.
    #[error("`auth` and an `Authorization` header exclude each other")]
    AuthWithAuthorizationHeader,
    /// `auth.spiffe` in mode `mtls` without `id`, `ids` or `trust_domain`, or with more than one.
    #[error("`auth.spiffe` needs exactly one of `id`, `ids` and `trust_domain`")]
    SpiffeMatch,
    /// `auth.spiffe` with `mode: "jwt"` without `audience`.
    #[error("`auth.spiffe` with mode `jwt` needs an `audience`")]
    SpiffeAudience,
    /// `audience` with mode `mtls`.
    #[error("`audience` only applies to mode `jwt`")]
    SpiffeAudienceWithMtls,
    /// A SPIFFE ID or trust domain is not valid.
    #[error("invalid SPIFFE value: {reason}")]
    InvalidSpiffe { reason: String },
    /// `workload_api` on an MCP server without `command` (ADR-0110).
    #[error("`workload_api` only applies to a local MCP server with `command` (stdio)")]
    WorkloadApiWithoutStdio,
    /// `workload_api` on `run_command` in shell mode (ADR-0110).
    #[error(
        "`workload_api` is refused in shell mode: a model-controlled shell must not act with the agent's identity; list the programs in `allowed` instead"
    )]
    WorkloadApiWithShell,
    /// A key of `builtin_tools.fetch.auth` matches no entry of `allowed_hosts` (ADR-0110).
    #[error("'{host}' is not in `allowed_hosts`: `auth` can only name an allowed host")]
    AuthHostNotAllowed { host: String },
    /// A key of `builtin_tools.git.auth` does not match the host of `repo_url` (ADR-0110).
    #[error("'{host}' is not the host of `repo_url`: `auth` can only name the repository's host")]
    AuthHostNotRepository { host: String },
    /// `use_token` together with `auth` of the git tools (ADR-0110).
    #[error("`use_token` and `auth` exclude each other: the identity replaces `GIT_TOKEN`")]
    GitTokenWithAuth,
    /// A sub-agent does not name exactly one of `spec` and `url`.
    #[error("sub-agent needs exactly one of `spec` (local) and `url` (remote, A2A)")]
    AgentToolSource,
    /// A local sub-agent without `description`.
    #[error("local sub-agent needs a `description`")]
    MissingAgentDescription,
    /// Memory without tools and without automatic recall or storage (ADR-0069).
    #[error("memory without tools needs `auto_recall`, `auto_remember` or `remember_compressed`")]
    UnusedMemory,
    /// A limit in `limits` is impossible (ADR-0078).
    #[error("invalid limit: {reason}")]
    InvalidLimit { reason: String },
    /// `self_tools.save` without a `tools_dir` where the tools could land (ADR-0075).
    #[error("`save` needs a `tools_dir` to store the tools in")]
    SaveWithoutToolsDir,
    /// A subgraph sets `self_tools`, but the spec does not allow self-built tools.
    #[error("`self_tools` in a subgraph needs `self_tools` at the top of the spec")]
    SelfToolsNotConfigured,
    /// Built-in file tools without `builtin_tools.root`.
    #[error("built-in file and key-value tools need `root` (directory relative to the spec)")]
    MissingBuiltinRoot,
    /// A tool of a built-in group without the section of that group (ADR-0104).
    #[error("the tools of '{group}' need a `{group}` section in `builtin_tools`")]
    MissingBuiltinSection { group: String },
    /// A URL of `builtin_tools` is not usable (ADR-0104); the reason never repeats the URL.
    #[error("invalid URL: {reason}")]
    InvalidUrl { reason: String },
    /// The git directory leaves the root (ADR-0104).
    #[error("the directory must be relative and inside the root, without `..`")]
    DirectoryOutsideRoot,
    /// A tool package names none or both of `oci` and `git` (ADR-0107).
    #[error("give exactly one of `oci` and `git`")]
    ToolPackageSource,
    /// A `skills` entry names none or both of `dir` and `git` (ADR-0103).
    #[error("give exactly one of `dir` and `git`")]
    SkillSource,
    /// The git source of a tool package is unusable (ADR-0107); the reason never repeats the URL.
    #[error("invalid git source: {reason}")]
    InvalidGitSource { reason: String },
    /// The OCI reference of a tool package is malformed (ADR-0107).
    #[error("invalid OCI reference: {reason}")]
    InvalidOciReference { reason: String },
    /// `knowledge` names no sources.
    #[error("knowledge needs at least one source")]
    NoKnowledgeSources,
    /// `knowledge` without the search tool and without injection: it would never be used
    /// (ADR-0113).
    #[error("knowledge without the tool needs `inject: true`")]
    UnusedKnowledge,
    /// The `store` of `knowledge` or `long_term_memory` is inconsistent (ADR-0106): the reason
    /// says what; it never repeats a URL.
    #[error("invalid store: {reason}")]
    InvalidStore { reason: String },
    /// `on_decision` leaves a node that is not a `decide` node (ADR-0100).
    #[error("`on_decision` may only leave a `decide` node")]
    DecisionEdgeSource,
    /// An edge leaving a `decide` node does not use `on_decision` (ADR-0100).
    #[error("edges leaving the `decide` node '{node}' must use `on_decision`")]
    DecisionEdgeCondition { node: String },
    /// `on_decision` names an answer that the decision does not have (ADR-0100).
    #[error("the decision '{node}' has no answer '{answer}'")]
    UnknownDecisionAnswer { node: String, answer: String },
    /// No edge handles an answer of a decision (ADR-0100).
    #[error("no edge handles the answer '{answer}' of the decision '{node}'")]
    DecisionAnswerUncovered { node: String, answer: String },
    /// More than one edge handles an answer of a decision (ADR-0100).
    #[error("the answer '{answer}' of the decision '{node}' is handled by more than one edge")]
    DecisionAnswerDuplicated { node: String, answer: String },
    /// The value of an edge does not have the type that the target expects.
    #[error("type error: target expects {expected}, the edge delivers {found}")]
    TypeMismatch { expected: DataType, found: DataType },
    /// A condition does not accept the value of the edge.
    #[error("condition '{kind}' expects {expected}, the source delivers {found}")]
    ConditionInputMismatch {
        kind: String,
        expected: DataType,
        found: DataType,
    },
}

/// Checks the spec against the built-in catalogs and returns all errors found.
pub(crate) fn validate(spec: &AgentSpec) -> Result<(), Vec<ValidationError>> {
    validate_with(spec, &Catalogs::builtin())
}

/// Checks the spec against `catalogs` and returns all errors found.
pub(crate) fn validate_with(
    spec: &AgentSpec,
    catalogs: &Catalogs,
) -> Result<(), Vec<ValidationError>> {
    let rules = Rules(spec);
    let mut errors = Vec::new();
    rules.check_name(&mut errors);
    rules.check_fallbacks(&mut errors);
    rules.check_sampling(&mut errors);
    rules.check_tool_names(&mut errors);
    rules.check_tool_packages(&mut errors);
    rules.check_skills(&mut errors);
    rules.check_type_names(&mut errors);
    rules.check_type_schemas(&mut errors);
    rules.check_signature_types(&mut errors);
    rules.check_mcp_servers(&mut errors);
    rules.check_agent_tools(&mut errors);
    rules.check_builtin_tools(&mut errors);
    rules.check_knowledge(&mut errors);
    rules.check_long_term_memory(&mut errors);
    rules.check_self_tools(&mut errors);
    rules.check_limits(&mut errors);
    rules.check_subgraph_definitions(&mut errors);
    rules.check_subgraph_cycles(&mut errors);
    for scope in rules.graph_scopes() {
        rules.check_graph(&scope, catalogs, &mut errors);
    }
    rules.check_code_piece_names(&mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub(crate) use code::code_pieces;
pub(crate) use sub_agents::check_sub_agent_types;

/// Warnings of the spec against the built-in catalogs; they never fail the validation.
pub(crate) fn warnings(spec: &AgentSpec) -> Vec<SpecWarning> {
    warnings_with(spec, &Catalogs::builtin())
}

/// Warnings of the spec against `catalogs`, in graph order.
pub(crate) fn warnings_with(spec: &AgentSpec, catalogs: &Catalogs) -> Vec<SpecWarning> {
    let rules = Rules(spec);
    let mut warnings = Vec::new();
    for scope in rules.graph_scopes() {
        rules.warn_missing_multi_call_edge(&scope, catalogs, &mut warnings);
    }
    rules.warn_shell_mode(&mut warnings);
    warnings
}

/// The rules of the validation. `AgentSpec` belongs to `grauwolf-spec-model`, so the rules are
/// methods of this wrapper, which dereferences to the spec.
struct Rules<'a>(&'a AgentSpec);

impl std::ops::Deref for Rules<'_> {
    type Target = AgentSpec;

    fn deref(&self) -> &AgentSpec {
        self.0
    }
}

/// A graph with its signature and its JSON pointer: the main graph or a subgraph.
struct GraphScope<'a> {
    graph: &'a GraphSpec,
    input: &'a str,
    output: &'a str,
    path: String,
}

/// `true` if `id` matches the pattern `[a-z][a-z0-9_]*`.
fn is_snake_case_identifier(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

pub(crate) use grauwolf_spec_model::escape_pointer_token;

#[cfg(test)]
mod tests {
    use super::{escape_pointer_token, is_snake_case_identifier};

    #[test]
    fn snake_case_identifiers_start_with_a_lowercase_letter() {
        assert!(is_snake_case_identifier("call_llm2"));
        for bad in ["", "Call", "_x", "2x", "a-b"] {
            assert!(!is_snake_case_identifier(bad), "{bad}");
        }
    }

    #[test]
    fn pointer_tokens_escape_tilde_and_slash() {
        assert_eq!(escape_pointer_token("a/b~c"), "a~1b~0c");
    }
}
