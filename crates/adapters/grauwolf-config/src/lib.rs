//! Configuration shared by every program (ADR-0108, ADR-0118): the `Env` port with its adapters
//! (`ProcessEnv`, `MapEnv`), the `RegistryConfig` section that the tool commands read, and
//! `VARIABLES`, the one table of every environment variable grauwolf reads. The host's
//! `config` module re-exports all of it and adds the sections of the agent runtime; the lean
//! `grauwolf-tools` program depends on this crate instead of the host.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Where configuration values come from: the process environment, a map in tests, a closure.
pub trait Env: Send + Sync {
    /// The value of the variable `name`, if set.
    fn var(&self, name: &str) -> Option<String>;
}

impl<F> Env for F
where
    F: Fn(&str) -> Option<String> + Send + Sync,
{
    fn var(&self, name: &str) -> Option<String> {
        self(name)
    }
}

/// The environment of the process (`std::env::var`; a value that is not UTF-8 counts as unset).
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessEnv;

impl Env for ProcessEnv {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

/// A fixed set of variables, for tests.
#[derive(Debug, Clone, Default)]
pub struct MapEnv(BTreeMap<String, String>);

impl MapEnv {
    /// The variables `pairs` (name, value).
    pub fn new<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self(
            pairs
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
        )
    }
}

impl Env for MapEnv {
    fn var(&self, name: &str) -> Option<String> {
        self.0.get(name).cloned()
    }
}

/// A lookup function, the form the sections are built from.
pub type Lookup<'a> = &'a dyn Fn(&str) -> Option<String>;

/// A variable's value if it is set and not empty.
pub fn non_empty(get: Lookup, name: &str) -> Option<String> {
    get(name).filter(|value| !value.is_empty())
}

/// A variable's value if it is not blank (only whitespace counts as blank).
pub fn non_blank(get: Lookup, name: &str) -> Option<String> {
    get(name).filter(|value| !value.trim().is_empty())
}

/// Credentials for tool package registries and git repositories (ADR-0107).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistryConfig {
    /// `GRAUWOLF_REGISTRY_AUTH` (blank counts as unset).
    pub auth: Option<String>,
    /// The Docker configuration file: `$DOCKER_CONFIG/config.json`, else `~/.docker/config.json`
    /// (`DOCKER_CONFIG` and `HOME` count as unset when empty).
    pub docker_config: Option<PathBuf>,
    /// The directories of `PATH`, where credential helpers are found.
    pub helper_dirs: Vec<PathBuf>,
    /// `GIT_TOKEN`, for tool packages from git repositories (blank counts as unset).
    pub git_token: Option<String>,
    /// `GRAUWOLF_TOOL_POLICY`: the policy file for tool packages (ADR-0107); it wins over a
    /// `grauwolf.toml` next to the spec (blank counts as unset).
    pub tool_policy: Option<PathBuf>,
    /// `GRAUWOLF_TOOLS_CATALOG`: the tool catalog (a registry or a full reference) of the
    /// `catalog` commands (ADR-0124; blank counts as unset).
    pub tools_catalog: Option<String>,
}

impl RegistryConfig {
    /// From `env`.
    pub fn from_env(env: &dyn Env) -> Self {
        Self::from_lookup(&|name| env.var(name))
    }

    /// From a lookup function.
    pub fn from_lookup(get: Lookup) -> Self {
        let docker_config = non_empty(get, "DOCKER_CONFIG")
            .map(|dir| PathBuf::from(dir).join("config.json"))
            .or_else(|| {
                non_empty(get, "HOME")
                    .map(|home| PathBuf::from(home).join(".docker").join("config.json"))
            });
        Self {
            auth: non_blank(get, "GRAUWOLF_REGISTRY_AUTH"),
            docker_config,
            helper_dirs: get("PATH")
                .map(|path| std::env::split_paths(&path).collect())
                .unwrap_or_default(),
            git_token: non_blank(get, "GIT_TOKEN"),
            tool_policy: non_blank(get, "GRAUWOLF_TOOL_POLICY").map(PathBuf::from),
            tools_catalog: non_blank(get, "GRAUWOLF_TOOLS_CATALOG"),
        }
    }
}

/// One environment variable grauwolf reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Variable {
    /// The name.
    pub name: &'static str,
    /// The section of `Config` it belongs to (or `tool build`, `specs`, `login` for
    /// `grauwolf login`).
    pub section: &'static str,
    /// What it does.
    pub meaning: &'static str,
}

const fn variable(name: &'static str, section: &'static str, meaning: &'static str) -> Variable {
    Variable {
        name,
        section,
        meaning,
    }
}

/// Every environment variable grauwolf reads. The table in `docs/user-guide/reference-cli.md`
/// lists the same names (a test checks it).
pub const VARIABLES: &[Variable] = &[
    variable(
        "OLLAMA_HOST",
        "providers",
        "Address of Ollama; default http://localhost:11434",
    ),
    variable(
        "OLLAMA_BASE_URL",
        "providers",
        "In chat API mode: address of Ollama if OLLAMA_HOST is not set",
    ),
    variable(
        "OLLAMA_THINK",
        "providers",
        "true/1 or false/0: thinking mode of Ollama models",
    ),
    variable("OPENAI_API_KEY", "providers", "Key for the openai provider"),
    variable(
        "OPENAI_BASE_URL",
        "providers",
        "Different base URL for openai",
    ),
    variable(
        "ANTHROPIC_API_KEY",
        "providers",
        "Key for the anthropic provider",
    ),
    variable("GEMINI_API_KEY", "providers", "Key for the gemini provider"),
    variable(
        "GEMINI_BASE_URL",
        "providers",
        "Different base URL for gemini",
    ),
    variable(
        "AWS_ACCESS_KEY_ID",
        "providers",
        "Access key of the bedrock provider (with AWS_SECRET_ACCESS_KEY)",
    ),
    variable(
        "AWS_SECRET_ACCESS_KEY",
        "providers",
        "Secret key of the bedrock provider",
    ),
    variable(
        "AWS_SESSION_TOKEN",
        "providers",
        "Session token for temporary AWS credentials (bedrock)",
    ),
    variable(
        "AWS_REGION",
        "providers",
        "Region of the bedrock provider; default us-west-2",
    ),
    variable(
        "AWS_DEFAULT_REGION",
        "providers",
        "Region of the bedrock provider if AWS_REGION is not set",
    ),
    variable(
        "AWS_BEARER_TOKEN_BEDROCK",
        "providers",
        "Bedrock API key; replaces the AWS access keys",
    ),
    variable(
        "BEDROCK_BASE_URL",
        "providers",
        "Different base URL for bedrock",
    ),
    variable(
        "OPENROUTER_API_KEY",
        "providers",
        "Key for the openrouter provider",
    ),
    variable(
        "OPENROUTER_BASE_URL",
        "providers",
        "Different base URL for openrouter",
    ),
    variable(
        "DEEPSEEK_API_KEY",
        "providers",
        "Key for the deepseek provider",
    ),
    variable(
        "DEEPSEEK_BASE_URL",
        "providers",
        "Different base URL for deepseek",
    ),
    variable(
        "MISTRAL_API_KEY",
        "providers",
        "Key for the mistral provider",
    ),
    variable(
        "MISTRAL_BASE_URL",
        "providers",
        "Different base URL for mistral",
    ),
    variable("GROQ_API_KEY", "providers", "Key for the groq provider"),
    variable("GROQ_BASE_URL", "providers", "Different base URL for groq"),
    variable(
        "DASHSCOPE_API_KEY",
        "providers",
        "Key for the dashscope provider",
    ),
    variable(
        "DASHSCOPE_BASE_URL",
        "providers",
        "Different base URL for dashscope",
    ),
    variable("XAI_API_KEY", "providers", "Key for the xai provider"),
    variable("XAI_BASE_URL", "providers", "Different base URL for xai"),
    variable(
        "RKEV_BASE_URL",
        "system_one",
        "Address of the System One service",
    ),
    variable(
        "RKEV_API_KEY",
        "system_one",
        "Optional key for the System One service",
    ),
    variable(
        "ATLASSIAN_EMAIL",
        "atlassian",
        "E-mail address for the Jira and Confluence tools",
    ),
    variable(
        "ATLASSIAN_API_TOKEN",
        "atlassian",
        "API token for the Jira and Confluence tools",
    ),
    variable(
        "GIT_TOKEN",
        "git",
        "Access token for the git tools with use_token",
    ),
    variable(
        "GIT_AUTHOR_NAME",
        "git",
        "Author name of the commits of git_commit",
    ),
    variable(
        "GIT_AUTHOR_EMAIL",
        "git",
        "Author e-mail of the commits of git_commit",
    ),
    variable(
        "GRAUWOLF_REGISTRY_AUTH",
        "registry",
        "Credentials for tool package registries",
    ),
    variable(
        "GRAUWOLF_TOOL_POLICY",
        "registry",
        "Policy file for tool packages (allowed registries and hosts, signatures)",
    ),
    variable(
        "GRAUWOLF_TOOLS_CATALOG",
        "registry",
        "Tool catalog of grauwolf-tools catalog: a registry (= <registry>/grauwolf-catalog:latest) or a full reference",
    ),
    variable(
        "SOURCE_DATE_EPOCH",
        "registry",
        "Build date of packed tool packages in seconds since 1970, for reproducible builds (default: now)",
    ),
    variable(
        "GRAUWOLF_CREDENTIALS",
        "credentials",
        "File with the [credentials] section (credential sources: file, openbao); wins over a grauwolf.toml next to the spec",
    ),
    variable(
        "SPIFFE_ENDPOINT_SOCKET",
        "spiffe",
        "Socket of the SPIFFE Workload API (unix:///run/spire/sockets/agent.sock); the agent's JWT-SVID for credential sources comes from there",
    ),
    variable(
        "SPIFFE_FETCH_TIMEOUT",
        "spiffe",
        "How long to wait at start-up for the SPIFFE identity while the pod's registration is not synced yet (250ms, 30s, 1m; default 30s; 0s tries once)",
    ),
    variable(
        "DOCKER_CONFIG",
        "registry",
        "Directory of the Docker config.json behind GRAUWOLF_REGISTRY_AUTH",
    ),
    variable(
        "HOME",
        "registry",
        "Where ~/.docker/config.json is looked up",
    ),
    variable(
        "PATH",
        "registry",
        "Where Docker credential helpers are found",
    ),
    variable(
        "GRAUWOLF_SERVER_TOKEN",
        "service",
        "Shared token of the service",
    ),
    variable(
        "SERVER_PORT",
        "service",
        "Port of the service; default 8080",
    ),
    variable("A2A_PORT", "service", "Additional A2A port"),
    variable(
        "A2A_BASE_PATH",
        "service",
        "Path prefix of the A2A routes on that port",
    ),
    variable(
        "GRAUWOLF_SPIFFE",
        "service",
        "1 or true: a second port serves mTLS with the own X.509-SVID (client certificates required) and the HTTP port accepts JWT-SVIDs (ADR-0109); needs SPIFFE_ENDPOINT_SOCKET",
    ),
    variable(
        "GRAUWOLF_SPIFFE_PORT",
        "service",
        "Port of the mTLS listener with GRAUWOLF_SPIFFE; default 8443",
    ),
    variable(
        "GRAUWOLF_SPIFFE_PUBLIC_URL",
        "service",
        "URL under which callers reach the mTLS port; its A2A agent card names it as the endpoint; default https://localhost:<GRAUWOLF_SPIFFE_PORT>",
    ),
    variable(
        "GRAUWOLF_SPIFFE_USERS",
        "service",
        "File { \"<spiffe id>\" | \"<prefix>/*\": \"<user>\" } that maps callers with a SPIFFE ID to users; read again when it changes",
    ),
    variable(
        "GRAUWOLF_SPIFFE_AUDIENCE",
        "service",
        "Audience a JWT-SVID must name; default the service's own SPIFFE ID",
    ),
    variable(
        "GRAUWOLF_SPIFFE_TRUST_DOMAINS",
        "service",
        "Comma-separated trust domains whose callers are accepted besides the own (reserved for federation, ADR-0122; a bundle must be in the Workload API)",
    ),
    variable(
        "GRAUWOLF_LOGIN_TENANT",
        "login",
        "grauwolf login: the tenant (the flag wins, the last login is the fallback)",
    ),
    variable(
        "GRAUWOLF_LOGIN_AUTHORITY",
        "login",
        "grauwolf login: Entra ID or Entra Local's address (default https://login.microsoftonline.com)",
    ),
    variable(
        "GRAUWOLF_LOGIN_CLIENT_ID",
        "login",
        "grauwolf login: the public client (\"grauwolf CLI\")",
    ),
    variable(
        "GRAUWOLF_LOGIN_SCOPE",
        "login",
        "grauwolf login: the API scope, e.g. api://grauwolf-identity/access_as_user",
    ),
    variable(
        "GRAUWOLF_CONFIG_DIR",
        "login",
        "grauwolf login: the directory of the token cache login.json (default $XDG_CONFIG_HOME/grauwolf, ~/.config/grauwolf)",
    ),
    variable(
        "GRAUWOLF_IDENTITY_URL",
        "service",
        "URL of the identity service (ADR-0111): Entra tokens are exchanged there for Txn-Tokens, Txn-Tokens of callers are verified with its keys; needs GRAUWOLF_SPIFFE",
    ),
    variable(
        "GRAUWOLF_IDENTITY_SPIFFE_ID",
        "service",
        "SPIFFE ID of the identity service: the server ID checked over mTLS (default: any ID of the own trust domain) and the audience of the JWT-SVID over http:// (required there)",
    ),
    variable(
        "GRAUWOLF_IDENTITY_TENANT",
        "identity",
        "grauwolf identity serve: the Entra tenant ID (tid) people's tokens must name",
    ),
    variable(
        "GRAUWOLF_IDENTITY_ISSUER",
        "identity",
        "grauwolf identity serve: the issuer (iss) of people's tokens, e.g. https://login.microsoftonline.com/<tenant>/v2.0",
    ),
    variable(
        "GRAUWOLF_IDENTITY_AUDIENCE",
        "identity",
        "grauwolf identity serve: the accepted audiences (aud) of people's tokens, comma-separated: the grauwolf identity API's application ID and its application ID URI",
    ),
    variable(
        "GRAUWOLF_IDENTITY_SCOPE",
        "identity",
        "grauwolf identity serve: the scope people's tokens must carry in scp; default access_as_user",
    ),
    variable(
        "GRAUWOLF_IDENTITY_JWKS_URL",
        "identity",
        "grauwolf identity serve: the JWKS of the issuer; default from its OpenID discovery",
    ),
    variable(
        "GRAUWOLF_IDENTITY_PUBLIC_URL",
        "identity",
        "grauwolf identity serve: its own URL, the iss of the Txn-Tokens",
    ),
    variable(
        "GRAUWOLF_IDENTITY_TRUST_DOMAIN",
        "identity",
        "grauwolf identity serve: the aud of the Txn-Tokens; default the trust domain of its own SPIFFE ID",
    ),
    variable(
        "GRAUWOLF_IDENTITY_POLICY",
        "identity",
        "grauwolf identity serve: the policy file { \"callers\": { \"<spiffe id>\" | \"<prefix>/*\": { \"purposes\"?: [..] } } }; read again when it changes",
    ),
    variable(
        "GRAUWOLF_IDENTITY_TRANSIT_KEY",
        "identity",
        "grauwolf identity serve: the OpenBao Transit key (ecdsa-p256) that signs Txn-Tokens",
    ),
    variable(
        "GRAUWOLF_IDENTITY_TRANSIT_MOUNT",
        "identity",
        "grauwolf identity serve: the mount of the Transit engine; default transit",
    ),
    variable(
        "GRAUWOLF_IDENTITY_OPENBAO_ADDR",
        "identity",
        "grauwolf identity serve: the address of OpenBao",
    ),
    variable(
        "GRAUWOLF_IDENTITY_OPENBAO_ROLE",
        "identity",
        "grauwolf identity serve: the JWT role it logs in to OpenBao with (its JWT-SVID)",
    ),
    variable(
        "GRAUWOLF_IDENTITY_OPENBAO_AUDIENCE",
        "identity",
        "grauwolf identity serve: the audience of the JWT-SVID for the OpenBao login; default openbao",
    ),
    variable(
        "GRAUWOLF_IDENTITY_OPENBAO_MOUNT",
        "identity",
        "grauwolf identity serve: the mount of OpenBao's JWT auth method; default jwt",
    ),
    variable(
        "GRAUWOLF_LOGIN_IDENTITY_URL",
        "login",
        "grauwolf login and grauwolf identity grant: the identity service's public URL",
    ),
    variable(
        "GRAUWOLF_IDENTITY_GRANT_CLIENT_ID",
        "identity",
        "grauwolf identity serve: the public client whose refresh tokens grants hold (grauwolf identity grant create); without it there are no grants",
    ),
    variable(
        "GRAUWOLF_IDENTITY_GRANT_TOKEN_URL",
        "identity",
        "grauwolf identity serve: Entra's token endpoint for redeeming grants; default from the issuer's OpenID discovery",
    ),
    variable(
        "GRAUWOLF_IDENTITY_GRANT_SCOPE",
        "identity",
        "grauwolf identity serve: the scope grants are redeemed with; default <audience URI>/<scope> offline_access",
    ),
    variable(
        "GRAUWOLF_IDENTITY_GRANT_KV_MOUNT",
        "identity",
        "grauwolf identity serve: the KV v2 mount of the grants; default kv",
    ),
    variable(
        "GRAUWOLF_USERS_FILE",
        "service",
        "File with the user tokens",
    ),
    variable(
        "GRAUWOLF_ALLOW_ANONYMOUS",
        "service",
        "true: access without a token",
    ),
    variable(
        "GRAUWOLF_DATA_DIR",
        "service",
        "Base directory of the spec's paths",
    ),
    variable(
        "GRAUWOLF_CONVERSATIONS_DIR",
        "service",
        "Directory of the conversation histories, relative to the base directory (default: in memory)",
    ),
    variable(
        "GRAUWOLF_CONVERSATIONS_DB",
        "service",
        "Database URL of the conversation histories (secret; excludes the directory)",
    ),
    variable(
        "GRAUWOLF_PUBLIC_URL",
        "service",
        "Public URL for the agent card",
    ),
    variable(
        "GRAUWOLF_A2A_PUBLIC_URL",
        "service",
        "Public URL of the additional A2A port",
    ),
    variable(
        "GRAUWOLF_PRICES",
        "prices",
        "Path of the price table of a generated agent (as `--prices`), relative to the data directory",
    ),
    variable(
        "GRAUWOLF_CHAT_API",
        "chat_api",
        "1 or true: switch on /api/chat and /api/signature",
    ),
    variable(
        "AGENT_SIGNATURE_PATH",
        "chat_api",
        "Signature file for /api/signature",
    ),
    variable(
        "GRAUWOLF_OTEL",
        "service",
        "1 or true: a generated service exports its spans via OTLP/HTTP (as spec serve --otel; target from OTEL_EXPORTER_OTLP_*); continues an incoming traceparent (ADR-0123)",
    ),
    variable(
        "GRAUWOLF_OTEL_CONTENT",
        "service",
        "1 or true: with GRAUWOLF_OTEL also record prompts, answers and tool arguments and results in the spans",
    ),
    variable(
        "GRAUWOLF_EVENTS",
        "service",
        "1 or true: a generated service writes every event of its runs (and of its sub-agents) as one JSON line to stderr, as spec serve --events",
    ),
    variable(
        "OTEL_SERVICE_NAME",
        "telemetry",
        "Service name in the spans; default grauwolf",
    ),
    variable(
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "telemetry",
        "Target of the OpenTelemetry export (read by the SDK)",
    ),
    variable(
        "OTEL_EXPORTER_OTLP_HEADERS",
        "telemetry",
        "Headers of the export (read by the SDK)",
    ),
    variable(
        "CARGO",
        "tool build",
        "Program for the tool build and generate --check",
    ),
    variable("RUSTC", "tool build", "Compiler for the tool build"),
    variable(
        "PGVECTOR_URL",
        "specs",
        "Conventional name of the variable that holds the PostgreSQL URL for `knowledge.store` / `long_term_memory.vector_store` (the spec names the variable in `url_env`)",
    ),
    variable(
        "PGVECTOR_TEST_URL",
        "specs",
        "PostgreSQL with pgvector for the opt-in database tests of grauwolf-store-pgvector",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_config_carries_the_git_token_for_tool_packages() {
        let config = RegistryConfig::from_env(&MapEnv::new([("GIT_TOKEN", "t0k")]));
        assert_eq!(config.git_token.as_deref(), Some("t0k"));
        let blank = RegistryConfig::from_env(&MapEnv::new([("GIT_TOKEN", " ")]));
        assert_eq!(blank.git_token, None);
    }

    #[test]
    fn the_registry_config_carries_the_tool_policy_path() {
        let config =
            RegistryConfig::from_env(&MapEnv::new([("GRAUWOLF_TOOL_POLICY", "/p/x.toml")]));
        assert_eq!(config.tool_policy, Some(PathBuf::from("/p/x.toml")));
        let blank = RegistryConfig::from_env(&MapEnv::new([("GRAUWOLF_TOOL_POLICY", " ")]));
        assert_eq!(blank.tool_policy, None);
    }

    #[test]
    fn registry_finds_the_docker_configuration() {
        let docker =
            RegistryConfig::from_env(&MapEnv::new([("DOCKER_CONFIG", "/d"), ("HOME", "/h")]));
        assert_eq!(docker.docker_config, Some(PathBuf::from("/d/config.json")));
        let home = RegistryConfig::from_env(&MapEnv::new([("DOCKER_CONFIG", ""), ("HOME", "/h")]));
        assert_eq!(
            home.docker_config,
            Some(PathBuf::from("/h/.docker/config.json"))
        );
        assert_eq!(
            RegistryConfig::from_env(&MapEnv::default()).docker_config,
            None
        );
    }

    #[test]
    fn the_registry_config_reads_only_variables_of_the_table() {
        let names = std::sync::Mutex::new(Vec::new());
        RegistryConfig::from_env(&|name: &str| {
            names.lock().unwrap().push(name.to_owned());
            None
        });
        let names = names.into_inner().unwrap();
        assert!(!names.is_empty());
        for name in names {
            assert!(
                VARIABLES.iter().any(|variable| variable.name == name),
                "{name} is read but not in VARIABLES"
            );
        }
    }

    #[test]
    fn the_table_has_no_duplicates() {
        let mut names: Vec<_> = VARIABLES.iter().map(|variable| variable.name).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count);
    }

    #[test]
    fn map_env_returns_what_it_was_given() {
        let env = MapEnv::new([("A", "1"), ("B", "")]);
        assert_eq!(env.var("A").as_deref(), Some("1"));
        assert_eq!(env.var("B").as_deref(), Some(""));
        assert_eq!(env.var("C"), None);
    }
}
