//! Central configuration (ADR-0108, stage 4b): the [`Env`] port, the typed [`Config`] built from
//! it, and [`VARIABLES`], the one table of every environment variable grauwolf reads.
//!
//! The process environment is read only at entry points (the CLI, `service::main`, the generated
//! crates): they build a [`Config`] or hand a [`ProcessEnv`] on. Everything below takes a section
//! of the configuration or an environment function, so tests use [`MapEnv`] and never change the
//! process environment. Values are passed on exactly as the variable holds them; each section
//! documents where an empty value counts as unset.
//!
//! Reads that stay outside: the OTLP variables (`OTEL_EXPORTER_OTLP_*`) are read by the
//! OpenTelemetry SDK itself, and the tool build reads `CARGO`/`RUSTC` and the variables it passes
//! on to `cargo` (`grauwolf-toolbuild`); [`VARIABLES`] lists them for completeness.

use std::collections::BTreeMap;
use std::path::PathBuf;

pub use grauwolf_config::{
    Env, Lookup, MapEnv, ProcessEnv, RegistryConfig, VARIABLES, Variable, non_blank, non_empty,
};

/// Environment variables of the OpenAI-compatible providers (ADR-0082): API key and base URL of
/// every provider that [`crate::providers::OPENAI_COMPATIBLE`] lists. Read by name, so a variable
/// outside this list is never looked at.
pub const COMPATIBLE_VARIABLES: [&str; 12] = [
    "OPENROUTER_API_KEY",
    "OPENROUTER_BASE_URL",
    "DEEPSEEK_API_KEY",
    "DEEPSEEK_BASE_URL",
    "MISTRAL_API_KEY",
    "MISTRAL_BASE_URL",
    "GROQ_API_KEY",
    "GROQ_BASE_URL",
    "DASHSCOPE_API_KEY",
    "DASHSCOPE_BASE_URL",
    "XAI_API_KEY",
    "XAI_BASE_URL",
];

/// Everything grauwolf reads from the environment, by section.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Model providers.
    pub providers: ProvidersConfig,
    /// Tool package registries.
    pub registry: RegistryConfig,
    /// Jira and Confluence credentials.
    pub atlassian: AtlassianConfig,
    /// Token and author of the git tools.
    pub git: GitEnvConfig,
    /// The System One service.
    pub system_one: SystemOneConfig,
    /// The generated service (`service::main`).
    pub service: ServiceConfig,
    /// The chat API.
    pub chat_api: ChatApiConfig,
    /// OpenTelemetry.
    pub telemetry: TelemetryConfig,
    /// The price table.
    pub prices: PricesConfig,
    /// The identity service (`grauwolf identity serve`, ADR-0111).
    pub identity: IdentityServiceConfig,
}

impl Config {
    /// All sections from `env`; an invalid port is an error.
    pub fn from_env(env: &dyn Env) -> Result<Self, String> {
        Self::from_lookup(&|name| env.var(name))
    }

    /// Like [`from_env`](Self::from_env) with a lookup function.
    pub fn from_lookup(get: Lookup) -> Result<Self, String> {
        Ok(Self {
            providers: ProvidersConfig::from_lookup(get),
            registry: RegistryConfig::from_lookup(get),
            atlassian: AtlassianConfig::from_lookup(get),
            git: GitEnvConfig::from_lookup(get),
            system_one: SystemOneConfig::from_lookup(get),
            service: ServiceConfig::from_lookup(get)?,
            chat_api: ChatApiConfig::from_lookup(get),
            telemetry: TelemetryConfig::from_lookup(get),
            prices: PricesConfig::from_lookup(get),
            identity: IdentityServiceConfig::from_lookup(get),
        })
    }
}

/// Settings of the model providers. Values are as set (an empty `OLLAMA_HOST` stays empty).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProvidersConfig {
    /// `OLLAMA_HOST`.
    pub ollama_host: Option<String>,
    /// `OLLAMA_BASE_URL`; only used in chat API mode.
    pub ollama_base_url: Option<String>,
    /// `OLLAMA_THINK`: `true`/`1` or `false`/`0`, anything else is ignored.
    pub ollama_think: Option<String>,
    /// `OPENAI_API_KEY`.
    pub openai_api_key: Option<String>,
    /// `OPENAI_BASE_URL`.
    pub openai_base_url: Option<String>,
    /// `ANTHROPIC_API_KEY`.
    pub anthropic_api_key: Option<String>,
    /// `GEMINI_API_KEY`.
    pub gemini_api_key: Option<String>,
    /// `GEMINI_BASE_URL`.
    pub gemini_base_url: Option<String>,
    /// `AWS_ACCESS_KEY_ID` (empty counts as unset), for the bedrock provider.
    pub aws_access_key_id: Option<String>,
    /// `AWS_SECRET_ACCESS_KEY`.
    pub aws_secret_access_key: Option<String>,
    /// `AWS_SESSION_TOKEN`, for temporary credentials.
    pub aws_session_token: Option<String>,
    /// The region: `AWS_REGION`, else `AWS_DEFAULT_REGION`; `None` means the provider default.
    pub aws_region: Option<String>,
    /// `AWS_BEARER_TOKEN_BEDROCK`: a Bedrock API key; it wins over the access keys.
    pub aws_bearer_token_bedrock: Option<String>,
    /// `BEDROCK_BASE_URL`.
    pub bedrock_base_url: Option<String>,
    /// The values of [`COMPATIBLE_VARIABLES`] that are set, by variable name.
    pub compatible: BTreeMap<&'static str, String>,
    /// Chat API mode (`GRAUWOLF_CHAT_API`, or [`with_chat_api_mode`](Self::with_chat_api_mode)):
    /// `OLLAMA_BASE_URL` then stands in for a missing `OLLAMA_HOST` (ADR-0094).
    pub chat_api_mode: bool,
}

impl ProvidersConfig {
    /// From `env`.
    pub fn from_env(env: &dyn Env) -> Self {
        Self::from_lookup(&|name| env.var(name))
    }

    /// From a lookup function.
    pub fn from_lookup(get: Lookup) -> Self {
        Self {
            ollama_host: get("OLLAMA_HOST"),
            ollama_base_url: get("OLLAMA_BASE_URL"),
            ollama_think: get("OLLAMA_THINK"),
            openai_api_key: get("OPENAI_API_KEY"),
            openai_base_url: get("OPENAI_BASE_URL"),
            anthropic_api_key: get("ANTHROPIC_API_KEY"),
            gemini_api_key: get("GEMINI_API_KEY"),
            gemini_base_url: get("GEMINI_BASE_URL"),
            aws_access_key_id: non_empty(get, "AWS_ACCESS_KEY_ID"),
            aws_secret_access_key: non_empty(get, "AWS_SECRET_ACCESS_KEY"),
            aws_session_token: non_empty(get, "AWS_SESSION_TOKEN"),
            aws_region: non_empty(get, "AWS_REGION")
                .or_else(|| non_empty(get, "AWS_DEFAULT_REGION")),
            aws_bearer_token_bedrock: non_empty(get, "AWS_BEARER_TOKEN_BEDROCK"),
            bedrock_base_url: non_empty(get, "BEDROCK_BASE_URL"),
            compatible: COMPATIBLE_VARIABLES
                .iter()
                .filter_map(|name| get(name).map(|value| (*name, value)))
                .collect(),
            chat_api_mode: ChatApiConfig::from_lookup(get).enabled,
        }
    }

    /// Switches chat API mode on or off (the CLI's `serve --chat-api`).
    pub fn with_chat_api_mode(mut self, on: bool) -> Self {
        self.chat_api_mode = on;
        self
    }

    /// The value of the OpenAI-compatible provider variable `variable`
    /// (one of [`COMPATIBLE_VARIABLES`]).
    pub fn compatible(&self, variable: &str) -> Option<String> {
        self.compatible.get(variable).cloned()
    }
}

/// Jira and Confluence credentials (blank counts as unset).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AtlassianConfig {
    /// `ATLASSIAN_EMAIL`.
    pub email: Option<String>,
    /// `ATLASSIAN_API_TOKEN`.
    pub api_token: Option<String>,
}

impl AtlassianConfig {
    /// From `env`.
    pub fn from_env(env: &dyn Env) -> Self {
        Self::from_lookup(&|name| env.var(name))
    }

    /// From a lookup function.
    pub fn from_lookup(get: Lookup) -> Self {
        Self {
            email: non_blank(get, "ATLASSIAN_EMAIL"),
            api_token: non_blank(get, "ATLASSIAN_API_TOKEN"),
        }
    }
}

/// Token and author of the git tools (blank counts as unset).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitEnvConfig {
    /// `GIT_TOKEN`.
    pub token: Option<String>,
    /// `GIT_AUTHOR_NAME`.
    pub author_name: Option<String>,
    /// `GIT_AUTHOR_EMAIL`.
    pub author_email: Option<String>,
}

impl GitEnvConfig {
    /// From `env`.
    pub fn from_env(env: &dyn Env) -> Self {
        Self::from_lookup(&|name| env.var(name))
    }

    /// From a lookup function.
    pub fn from_lookup(get: Lookup) -> Self {
        Self {
            token: non_blank(get, "GIT_TOKEN"),
            author_name: non_blank(get, "GIT_AUTHOR_NAME"),
            author_email: non_blank(get, "GIT_AUTHOR_EMAIL"),
        }
    }
}

/// The System One service (ADR-0100).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemOneConfig {
    /// `RKEV_BASE_URL` (trimmed; blank counts as unset).
    pub base_url: Option<String>,
    /// `RKEV_API_KEY` (empty counts as unset).
    pub api_key: Option<String>,
}

impl SystemOneConfig {
    /// From `env`.
    pub fn from_env(env: &dyn Env) -> Self {
        Self::from_lookup(&|name| env.var(name))
    }

    /// From a lookup function.
    pub fn from_lookup(get: Lookup) -> Self {
        Self {
            base_url: non_blank(get, "RKEV_BASE_URL").map(|url| url.trim().to_owned()),
            api_key: non_empty(get, "RKEV_API_KEY"),
        }
    }
}

/// Default port of the generated service.
pub const DEFAULT_SERVER_PORT: u16 = 8080;

/// Settings of the generated service (ADR-0093); empty values count as unset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceConfig {
    /// `SERVER_PORT` (default [`DEFAULT_SERVER_PORT`]).
    pub port: u16,
    /// `GRAUWOLF_SERVER_TOKEN`.
    pub token: Option<String>,
    /// `GRAUWOLF_USERS_FILE`.
    pub users_file: Option<String>,
    /// `GRAUWOLF_ALLOW_ANONYMOUS` (only `true` allows).
    pub allow_anonymous: bool,
    /// `GRAUWOLF_DATA_DIR` (default the working directory).
    pub data_dir: PathBuf,
    /// `A2A_PORT`.
    pub a2a_port: Option<u16>,
    /// `A2A_BASE_PATH`, normalized to `/prefix` (none for empty or `/`).
    pub a2a_base_path: String,
    /// `GRAUWOLF_A2A_PUBLIC_URL`.
    pub a2a_public_url: Option<String>,
    /// `GRAUWOLF_PUBLIC_URL`.
    pub public_url: Option<String>,
    /// `GRAUWOLF_CONVERSATIONS_DIR`: directory of the conversation histories, relative to
    /// `data_dir`; without it and without a database they stay in memory.
    pub conversations_dir: Option<PathBuf>,
    /// `GRAUWOLF_CONVERSATIONS_DB`: URL of the database of the conversation histories
    /// (`sqlite://…`, `postgres://…`); a secret, never printed. Excludes the directory.
    pub conversations_db: Option<String>,
    /// `GRAUWOLF_OTEL` (`1`/`true` on, `0`/`false` off): export the spans of the service
    /// (ADR-0123).
    pub otel: bool,
    /// `GRAUWOLF_OTEL_CONTENT` (same values): also record contents in the spans.
    pub otel_content: bool,
    /// `GRAUWOLF_EVENTS` (same values): write every event as a JSON line to stderr.
    pub events: bool,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self::from_lookup(&|_| None).expect("no variables, no invalid port")
    }
}

impl ServiceConfig {
    /// From `env`; an invalid port is an error.
    pub fn from_env(env: &dyn Env) -> Result<Self, String> {
        Self::from_lookup(&|name| env.var(name))
    }

    /// From a lookup function.
    pub fn from_lookup(get: Lookup) -> Result<Self, String> {
        // `1`/`true` on, `0`/`false` or empty off, anything else names the variable.
        let switch = |name: &str| -> Result<bool, String> {
            match non_empty(get, name).as_deref() {
                None | Some("0" | "false") => Ok(false),
                Some("1" | "true") => Ok(true),
                Some(value) => Err(format!("{name}: {value:?} is not 1, true, 0 or false")),
            }
        };
        let port = |name: &str| -> Result<Option<u16>, String> {
            non_empty(get, name)
                .map(|value| {
                    value
                        .parse()
                        .map_err(|_| format!("{name}: {value:?} is not a port number"))
                })
                .transpose()
        };
        let conversations_dir = non_empty(get, "GRAUWOLF_CONVERSATIONS_DIR").map(PathBuf::from);
        let conversations_db = non_empty(get, "GRAUWOLF_CONVERSATIONS_DB");
        if conversations_dir.is_some() && conversations_db.is_some() {
            return Err(
                "GRAUWOLF_CONVERSATIONS_DIR and GRAUWOLF_CONVERSATIONS_DB are mutually \
                        exclusive: set only one of them"
                    .to_owned(),
            );
        }
        Ok(Self {
            port: port("SERVER_PORT")?.unwrap_or(DEFAULT_SERVER_PORT),
            token: non_empty(get, "GRAUWOLF_SERVER_TOKEN"),
            users_file: non_empty(get, "GRAUWOLF_USERS_FILE"),
            allow_anonymous: non_empty(get, "GRAUWOLF_ALLOW_ANONYMOUS")
                .is_some_and(|value| value == "true"),
            data_dir: non_empty(get, "GRAUWOLF_DATA_DIR")
                .map_or_else(|| PathBuf::from("."), PathBuf::from),
            a2a_port: port("A2A_PORT")?,
            a2a_base_path: non_empty(get, "A2A_BASE_PATH")
                .map(|path| format!("/{}", path.trim_matches('/')))
                .filter(|path| path != "/")
                .unwrap_or_default(),
            a2a_public_url: non_empty(get, "GRAUWOLF_A2A_PUBLIC_URL"),
            public_url: non_empty(get, "GRAUWOLF_PUBLIC_URL"),
            conversations_dir,
            conversations_db,
            otel: switch("GRAUWOLF_OTEL")?,
            otel_content: switch("GRAUWOLF_OTEL_CONTENT")?,
            events: switch("GRAUWOLF_EVENTS")?,
        })
    }
}

/// Inbound SPIFFE authentication of the service (ADR-0109).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpiffeServiceConfig {
    /// `GRAUWOLF_SPIFFE` is `1` or `true`.
    pub enabled: bool,
    /// `GRAUWOLF_SPIFFE_PORT` (default [`DEFAULT_SPIFFE_PORT`]).
    pub port: u16,
    /// `GRAUWOLF_SPIFFE_USERS`.
    pub users_file: Option<String>,
    /// `GRAUWOLF_SPIFFE_AUDIENCE` (default: the service's own SPIFFE ID).
    pub audience: Option<String>,
    /// `GRAUWOLF_SPIFFE_TRUST_DOMAINS`.
    pub trust_domains: Vec<String>,
    /// `GRAUWOLF_SPIFFE_PUBLIC_URL`: the URL of the mTLS port that its agent card names (default
    /// `https://localhost:<port>`).
    pub public_url: Option<String>,
    /// `GRAUWOLF_IDENTITY_URL`: the identity service for delegation (ADR-0111).
    pub identity_url: Option<String>,
    /// `GRAUWOLF_IDENTITY_SPIFFE_ID`: the identity service's SPIFFE ID.
    pub identity_spiffe_id: Option<String>,
}

impl SpiffeServiceConfig {
    /// Delegation needs the agent's workload identity (ADR-0111): `GRAUWOLF_IDENTITY_URL`
    /// without `GRAUWOLF_SPIFFE` is a start error.
    pub fn check_identity(&self) -> Result<(), String> {
        if self.identity_url.is_some() && !self.enabled {
            return Err("GRAUWOLF_IDENTITY_URL needs GRAUWOLF_SPIFFE".to_owned());
        }
        Ok(())
    }
}

/// Port of the mTLS listener if `GRAUWOLF_SPIFFE_PORT` is not set.
pub const DEFAULT_SPIFFE_PORT: u16 = 8443;

impl SpiffeServiceConfig {
    /// From `env`.
    pub fn from_env(env: &dyn Env) -> Result<Self, String> {
        Self::from_lookup(&|name| env.var(name))
    }

    /// From a lookup function; an invalid port is an error.
    pub fn from_lookup(get: Lookup) -> Result<Self, String> {
        let port = match non_empty(get, "GRAUWOLF_SPIFFE_PORT") {
            Some(value) => value
                .parse()
                .map_err(|_| format!("GRAUWOLF_SPIFFE_PORT: {value:?} is not a port number"))?,
            None => DEFAULT_SPIFFE_PORT,
        };
        Ok(Self {
            enabled: matches!(
                non_empty(get, "GRAUWOLF_SPIFFE").as_deref(),
                Some("1" | "true")
            ),
            port,
            users_file: non_empty(get, "GRAUWOLF_SPIFFE_USERS"),
            audience: non_empty(get, "GRAUWOLF_SPIFFE_AUDIENCE"),
            public_url: non_empty(get, "GRAUWOLF_SPIFFE_PUBLIC_URL"),
            identity_url: non_empty(get, "GRAUWOLF_IDENTITY_URL"),
            identity_spiffe_id: non_empty(get, "GRAUWOLF_IDENTITY_SPIFFE_ID"),
            trust_domains: non_empty(get, "GRAUWOLF_SPIFFE_TRUST_DOMAINS")
                .map(|list| {
                    list.split(',')
                        .map(|domain| domain.trim().to_owned())
                        .filter(|domain| !domain.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

/// The identity service (`grauwolf identity serve`, ADR-0111). Every field is optional here;
/// [`IdentityServiceConfig::required`] names what the command cannot start without.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdentityServiceConfig {
    /// `GRAUWOLF_IDENTITY_TENANT`.
    pub tenant: Option<String>,
    /// `GRAUWOLF_IDENTITY_ISSUER`: `iss` of people's tokens.
    pub issuer: Option<String>,
    /// `GRAUWOLF_IDENTITY_AUDIENCE`: the accepted `aud` values of people's tokens, a
    /// comma-separated list (application ID and application ID URI). Empty when unset.
    pub audiences: Vec<String>,
    /// `GRAUWOLF_IDENTITY_SCOPE` (default [`DEFAULT_IDENTITY_SCOPE`]).
    pub scope: String,
    /// `GRAUWOLF_IDENTITY_JWKS_URL` (default: OpenID discovery of the issuer).
    pub jwks_url: Option<String>,
    /// `GRAUWOLF_IDENTITY_PUBLIC_URL`: `iss` of the Txn-Tokens.
    pub public_url: Option<String>,
    /// `GRAUWOLF_IDENTITY_TRUST_DOMAIN` (default: the trust domain of the own SPIFFE ID).
    pub trust_domain: Option<String>,
    /// `GRAUWOLF_IDENTITY_POLICY`: the policy file.
    pub policy: Option<String>,
    /// `GRAUWOLF_IDENTITY_TRANSIT_KEY`.
    pub transit_key: Option<String>,
    /// `GRAUWOLF_IDENTITY_TRANSIT_MOUNT` (default `transit`).
    pub transit_mount: String,
    /// `GRAUWOLF_IDENTITY_OPENBAO_ADDR`.
    pub openbao_address: Option<String>,
    /// `GRAUWOLF_IDENTITY_OPENBAO_ROLE`.
    pub openbao_role: Option<String>,
    /// `GRAUWOLF_IDENTITY_OPENBAO_AUDIENCE` (default `openbao`).
    pub openbao_audience: String,
    /// `GRAUWOLF_IDENTITY_OPENBAO_MOUNT` (default `jwt`).
    pub openbao_mount: String,
    /// `GRAUWOLF_IDENTITY_GRANT_CLIENT_ID`: the public client (`grauwolf identity grant create`)
    /// whose refresh tokens grants hold (stage 3). Without it the service has no grants.
    pub grant_client_id: Option<String>,
    /// `GRAUWOLF_IDENTITY_GRANT_TOKEN_URL` (default: `token_endpoint` from the OpenID discovery of
    /// the issuer).
    pub grant_token_url: Option<String>,
    /// `GRAUWOLF_IDENTITY_GRANT_SCOPE` (default [`IdentityServiceConfig::grant_scope`]).
    pub grant_scope: Option<String>,
    /// `GRAUWOLF_IDENTITY_GRANT_KV_MOUNT` (default `kv`): the KV v2 mount of the grants.
    pub grant_kv_mount: String,
}

/// The scope people's tokens carry if `GRAUWOLF_IDENTITY_SCOPE` is not set.
pub const DEFAULT_IDENTITY_SCOPE: &str = "access_as_user";

impl IdentityServiceConfig {
    /// From `env`.
    pub fn from_env(env: &dyn Env) -> Self {
        Self::from_lookup(&|name| env.var(name))
    }

    /// From a lookup function.
    pub fn from_lookup(get: Lookup) -> Self {
        let or =
            |name: &str, default: &str| non_empty(get, name).unwrap_or_else(|| default.to_owned());
        Self {
            tenant: non_empty(get, "GRAUWOLF_IDENTITY_TENANT"),
            issuer: non_empty(get, "GRAUWOLF_IDENTITY_ISSUER"),
            audiences: non_empty(get, "GRAUWOLF_IDENTITY_AUDIENCE")
                .map(|list| {
                    list.split(',')
                        .map(str::trim)
                        .filter(|audience| !audience.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            scope: or("GRAUWOLF_IDENTITY_SCOPE", DEFAULT_IDENTITY_SCOPE),
            jwks_url: non_empty(get, "GRAUWOLF_IDENTITY_JWKS_URL"),
            public_url: non_empty(get, "GRAUWOLF_IDENTITY_PUBLIC_URL"),
            trust_domain: non_empty(get, "GRAUWOLF_IDENTITY_TRUST_DOMAIN"),
            policy: non_empty(get, "GRAUWOLF_IDENTITY_POLICY"),
            transit_key: non_empty(get, "GRAUWOLF_IDENTITY_TRANSIT_KEY"),
            transit_mount: or("GRAUWOLF_IDENTITY_TRANSIT_MOUNT", "transit"),
            openbao_address: non_empty(get, "GRAUWOLF_IDENTITY_OPENBAO_ADDR"),
            openbao_role: non_empty(get, "GRAUWOLF_IDENTITY_OPENBAO_ROLE"),
            openbao_audience: or("GRAUWOLF_IDENTITY_OPENBAO_AUDIENCE", "openbao"),
            openbao_mount: or("GRAUWOLF_IDENTITY_OPENBAO_MOUNT", "jwt"),
            grant_client_id: non_empty(get, "GRAUWOLF_IDENTITY_GRANT_CLIENT_ID"),
            grant_token_url: non_empty(get, "GRAUWOLF_IDENTITY_GRANT_TOKEN_URL"),
            grant_scope: non_empty(get, "GRAUWOLF_IDENTITY_GRANT_SCOPE"),
            grant_kv_mount: or("GRAUWOLF_IDENTITY_GRANT_KV_MOUNT", "kv"),
        }
    }

    /// The scope a grant's refresh token is redeemed with: `GRAUWOLF_IDENTITY_GRANT_SCOPE`, else
    /// the API scope as `grauwolf login` asks for it – the first audience that is a URI
    /// (`api://…`), else the first one, then `/<scope>` – and `offline_access`.
    pub fn grant_scope(&self) -> Option<String> {
        if let Some(scope) = &self.grant_scope {
            return Some(scope.clone());
        }
        let audience = self
            .audiences
            .iter()
            .find(|audience| audience.contains("://"))
            .or(self.audiences.first())?;
        Some(format!(
            "{}/{} offline_access",
            audience.trim_end_matches('/'),
            self.scope
        ))
    }

    /// The accepted audiences, or the error that names `GRAUWOLF_IDENTITY_AUDIENCE`.
    pub fn required_audiences(&self) -> Result<Vec<String>, String> {
        if self.audiences.is_empty() {
            return Err("grauwolf identity serve needs GRAUWOLF_IDENTITY_AUDIENCE".to_owned());
        }
        Ok(self.audiences.clone())
    }

    /// The value of a required setting, or the error that names its variable.
    pub fn required<'a>(value: &'a Option<String>, variable: &str) -> Result<&'a str, String> {
        value
            .as_deref()
            .ok_or_else(|| format!("grauwolf identity serve needs {variable}"))
    }
}

/// The chat API (ADR-0094).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatApiConfig {
    /// `GRAUWOLF_CHAT_API` is `1` or `true`.
    pub enabled: bool,
    /// `AGENT_SIGNATURE_PATH` (empty counts as unset).
    pub signature_path: Option<String>,
}

impl ChatApiConfig {
    /// From `env`.
    pub fn from_env(env: &dyn Env) -> Self {
        Self::from_lookup(&|name| env.var(name))
    }

    /// From a lookup function.
    pub fn from_lookup(get: Lookup) -> Self {
        Self {
            enabled: matches!(
                non_empty(get, "GRAUWOLF_CHAT_API").as_deref(),
                Some("1" | "true")
            ),
            signature_path: non_empty(get, "AGENT_SIGNATURE_PATH"),
        }
    }
}

/// Service name of the spans when `OTEL_SERVICE_NAME` is not set.
pub const DEFAULT_SERVICE_NAME: &str = "grauwolf";

/// OpenTelemetry settings. The endpoint and headers (`OTEL_EXPORTER_OTLP_*`) are read by the
/// OpenTelemetry SDK from the process environment, not through this section.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TelemetryConfig {
    /// `OTEL_SERVICE_NAME`.
    pub service_name: Option<String>,
}

impl TelemetryConfig {
    /// From `env`.
    pub fn from_env(env: &dyn Env) -> Self {
        Self::from_lookup(&|name| env.var(name))
    }

    /// From a lookup function.
    pub fn from_lookup(get: Lookup) -> Self {
        Self {
            service_name: get("OTEL_SERVICE_NAME"),
        }
    }

    /// The service name of the spans, default [`DEFAULT_SERVICE_NAME`].
    pub fn service_name(&self) -> String {
        self.service_name
            .clone()
            .unwrap_or_else(|| DEFAULT_SERVICE_NAME.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records every name asked for.
    struct Recording(Mutex<Vec<String>>);

    impl Env for Recording {
        fn var(&self, name: &str) -> Option<String> {
            self.0.lock().unwrap().push(name.to_owned());
            None
        }
    }

    #[test]
    fn config_reads_only_variables_of_the_table() {
        let recording = Recording(Mutex::new(Vec::new()));
        Config::from_env(&recording).unwrap();
        let names = recording.0.lock().unwrap().clone();
        assert!(!names.is_empty());
        for name in names {
            assert!(
                VARIABLES.iter().any(|variable| variable.name == name),
                "{name} is read but not in VARIABLES"
            );
        }
    }

    #[test]
    fn every_variable_appears_in_the_cli_reference() {
        let reference = include_str!("../../../../docs/user-guide/reference-cli.md");
        for variable in VARIABLES {
            assert!(
                reference.contains(&format!("`{}`", variable.name)),
                "{} is missing in docs/user-guide/reference-cli.md",
                variable.name
            );
        }
    }

    #[test]
    fn the_bedrock_settings_come_from_the_aws_variables() {
        let config = ProvidersConfig::from_env(&MapEnv::new([
            ("AWS_ACCESS_KEY_ID", "AKID"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
            ("AWS_SESSION_TOKEN", ""),
            ("AWS_DEFAULT_REGION", "eu-central-1"),
            ("AWS_BEARER_TOKEN_BEDROCK", "key"),
            ("BEDROCK_BASE_URL", "http://proxy"),
        ]));
        assert_eq!(config.aws_access_key_id.as_deref(), Some("AKID"));
        assert_eq!(config.aws_secret_access_key.as_deref(), Some("secret"));
        assert_eq!(config.aws_session_token, None, "empty counts as unset");
        assert_eq!(config.aws_region.as_deref(), Some("eu-central-1"));
        assert_eq!(config.aws_bearer_token_bedrock.as_deref(), Some("key"));
        assert_eq!(config.bedrock_base_url.as_deref(), Some("http://proxy"));
        let both = ProvidersConfig::from_env(&MapEnv::new([
            ("AWS_REGION", "us-east-1"),
            ("AWS_DEFAULT_REGION", "eu-central-1"),
        ]));
        assert_eq!(
            both.aws_region.as_deref(),
            Some("us-east-1"),
            "AWS_REGION wins"
        );
    }

    #[test]
    fn defaults_without_any_variable() {
        let config = Config::from_env(&MapEnv::default()).unwrap();
        assert_eq!(config.service.port, 8080);
        assert_eq!(config.service.data_dir, PathBuf::from("."));
        assert!(!config.chat_api.enabled);
        assert_eq!(config.telemetry.service_name(), "grauwolf");
        assert_eq!(config.system_one, SystemOneConfig::default());
    }

    #[test]
    fn service_section_parses_ports_and_prefix() {
        let env = MapEnv::new([
            ("SERVER_PORT", "9000"),
            ("A2A_PORT", "9001"),
            ("A2A_BASE_PATH", "/agents/"),
            ("GRAUWOLF_ALLOW_ANONYMOUS", "true"),
            ("GRAUWOLF_SERVER_TOKEN", ""),
        ]);
        let service = ServiceConfig::from_env(&env).unwrap();
        assert_eq!((service.port, service.a2a_port), (9000, Some(9001)));
        assert_eq!(service.a2a_base_path, "/agents");
        assert!(service.allow_anonymous);
        assert_eq!(service.token, None);
        let error = ServiceConfig::from_env(&MapEnv::new([("SERVER_PORT", "x")])).unwrap_err();
        assert_eq!(error, "SERVER_PORT: \"x\" is not a port number");
    }

    #[test]
    fn the_event_log_of_the_service_is_a_switch_that_names_a_bad_value() {
        assert!(!ServiceConfig::from_env(&MapEnv::default()).unwrap().events);
        for on in ["1", "true"] {
            let env = MapEnv::new([("GRAUWOLF_EVENTS", on)]);
            assert!(ServiceConfig::from_env(&env).unwrap().events, "{on}");
        }
        for off in ["0", "false", ""] {
            let env = MapEnv::new([("GRAUWOLF_EVENTS", off)]);
            assert!(!ServiceConfig::from_env(&env).unwrap().events, "{off}");
        }
        let error =
            ServiceConfig::from_env(&MapEnv::new([("GRAUWOLF_EVENTS", "yes")])).unwrap_err();
        assert!(error.contains("GRAUWOLF_EVENTS"), "{error}");
    }

    #[test]
    fn tracing_of_the_service_is_a_switch_that_names_a_bad_value() {
        let off = ServiceConfig::from_env(&MapEnv::new([("GRAUWOLF_OTEL", "")])).unwrap();
        assert_eq!((off.otel, off.otel_content), (false, false));
        let on = ServiceConfig::from_env(&MapEnv::new([
            ("GRAUWOLF_OTEL", "true"),
            ("GRAUWOLF_OTEL_CONTENT", "1"),
        ]))
        .unwrap();
        assert_eq!((on.otel, on.otel_content), (true, true));
        let explicit = ServiceConfig::from_env(&MapEnv::new([("GRAUWOLF_OTEL", "false")])).unwrap();
        assert!(!explicit.otel);
        let error = ServiceConfig::from_env(&MapEnv::new([("GRAUWOLF_OTEL", "yes")])).unwrap_err();
        assert!(error.contains("GRAUWOLF_OTEL"), "{error}");
        let error =
            ServiceConfig::from_env(&MapEnv::new([("GRAUWOLF_OTEL_CONTENT", "on")])).unwrap_err();
        assert!(error.contains("GRAUWOLF_OTEL_CONTENT"), "{error}");
    }

    #[test]
    fn conversations_live_in_a_directory_or_a_database_but_not_both() {
        let none =
            ServiceConfig::from_env(&MapEnv::new([("GRAUWOLF_CONVERSATIONS_DIR", "")])).unwrap();
        assert_eq!(
            (none.conversations_dir, none.conversations_db),
            (None, None)
        );

        let dir = ServiceConfig::from_env(&MapEnv::new([("GRAUWOLF_CONVERSATIONS_DIR", "chats")]))
            .unwrap();
        assert_eq!(
            dir.conversations_dir.as_deref(),
            Some(std::path::Path::new("chats"))
        );

        let db = ServiceConfig::from_env(&MapEnv::new([(
            "GRAUWOLF_CONVERSATIONS_DB",
            "sqlite://chats.db",
        )]))
        .unwrap();
        assert_eq!(db.conversations_db.as_deref(), Some("sqlite://chats.db"));

        let error = ServiceConfig::from_env(&MapEnv::new([
            ("GRAUWOLF_CONVERSATIONS_DIR", "chats"),
            ("GRAUWOLF_CONVERSATIONS_DB", "sqlite://chats.db"),
        ]))
        .unwrap_err();
        assert!(
            error.contains("GRAUWOLF_CONVERSATIONS_DIR")
                && error.contains("GRAUWOLF_CONVERSATIONS_DB")
                && !error.contains("sqlite://"),
            "{error}"
        );
    }

    #[test]
    fn chat_api_mode_follows_the_variable() {
        let on = ProvidersConfig::from_env(&MapEnv::new([("GRAUWOLF_CHAT_API", "1")]));
        assert!(on.chat_api_mode);
        assert!(!ProvidersConfig::default().chat_api_mode);
        assert!(
            ProvidersConfig::default()
                .with_chat_api_mode(true)
                .chat_api_mode
        );
    }

    #[test]
    fn credentials_count_blank_values_as_unset() {
        let env = MapEnv::new([
            ("ATLASSIAN_EMAIL", "  "),
            ("GIT_TOKEN", "t"),
            ("RKEV_BASE_URL", " http://x "),
        ]);
        assert_eq!(AtlassianConfig::from_env(&env).email, None);
        assert_eq!(GitEnvConfig::from_env(&env).token.as_deref(), Some("t"));
        assert_eq!(
            SystemOneConfig::from_env(&env).base_url.as_deref(),
            Some("http://x")
        );
    }
    #[test]
    fn the_identity_audience_is_a_comma_separated_list() {
        let config = IdentityServiceConfig::from_env(&MapEnv::new([(
            "GRAUWOLF_IDENTITY_AUDIENCE",
            "2222-guid, api://grauwolf-identity,",
        )]));
        assert_eq!(config.audiences, ["2222-guid", "api://grauwolf-identity"]);
        let one = IdentityServiceConfig::from_env(&MapEnv::new([(
            "GRAUWOLF_IDENTITY_AUDIENCE",
            "api://x",
        )]));
        assert_eq!(one.audiences, ["api://x"]);
        let unset = IdentityServiceConfig::from_env(&MapEnv::new([("A", "b")]));
        assert!(
            unset
                .required_audiences()
                .unwrap_err()
                .contains("GRAUWOLF_IDENTITY_AUDIENCE")
        );
    }
    #[test]
    fn the_identity_service_settings_have_defaults_and_name_missing_variables() {
        let config = IdentityServiceConfig::from_env(&MapEnv::new([
            ("GRAUWOLF_IDENTITY_TENANT", "t1"),
            ("GRAUWOLF_IDENTITY_SCOPE", ""),
        ]));
        assert_eq!(config.tenant.as_deref(), Some("t1"));
        assert_eq!(config.scope, DEFAULT_IDENTITY_SCOPE);
        assert_eq!(config.transit_mount, "transit");
        assert_eq!(config.openbao_mount, "jwt");
        assert_eq!(config.openbao_audience, "openbao");
        let missing = IdentityServiceConfig::required(&config.issuer, "GRAUWOLF_IDENTITY_ISSUER")
            .unwrap_err();
        assert!(missing.contains("GRAUWOLF_IDENTITY_ISSUER"));
    }

    #[test]
    fn grants_are_configured_by_their_client_id_with_a_scope_from_the_audience() {
        let config = IdentityServiceConfig::from_env(&MapEnv::new([
            (
                "GRAUWOLF_IDENTITY_AUDIENCE",
                "2222-guid,api://grauwolf-identity",
            ),
            ("GRAUWOLF_IDENTITY_GRANT_CLIENT_ID", "cli-guid"),
        ]));
        assert_eq!(config.grant_client_id.as_deref(), Some("cli-guid"));
        assert_eq!(config.grant_token_url, None);
        assert_eq!(config.grant_kv_mount, "kv");
        assert_eq!(
            config.grant_scope().as_deref(),
            Some("api://grauwolf-identity/access_as_user offline_access")
        );
        let explicit = IdentityServiceConfig::from_env(&MapEnv::new([
            ("GRAUWOLF_IDENTITY_GRANT_SCOPE", "x/y offline_access"),
            ("GRAUWOLF_IDENTITY_GRANT_TOKEN_URL", "https://e/t/token"),
        ]));
        assert_eq!(
            explicit.grant_scope().as_deref(),
            Some("x/y offline_access")
        );
        assert_eq!(
            explicit.grant_token_url.as_deref(),
            Some("https://e/t/token")
        );
        assert_eq!(IdentityServiceConfig::default().grant_scope(), None);
    }

    #[test]
    fn an_identity_url_without_spiffe_is_a_start_error() {
        let without = SpiffeServiceConfig::from_env(&MapEnv::new([(
            "GRAUWOLF_IDENTITY_URL",
            "https://identity:8443",
        )]))
        .unwrap();
        assert_eq!(
            without.check_identity().unwrap_err(),
            "GRAUWOLF_IDENTITY_URL needs GRAUWOLF_SPIFFE"
        );
        let with = SpiffeServiceConfig::from_env(&MapEnv::new([
            ("GRAUWOLF_IDENTITY_URL", "https://identity:8443"),
            ("GRAUWOLF_SPIFFE", "1"),
        ]))
        .unwrap();
        assert!(with.check_identity().is_ok());
    }

    #[test]
    fn the_identity_url_of_an_agent_belongs_to_the_spiffe_settings() {
        let config = SpiffeServiceConfig::from_env(&MapEnv::new([
            (
                "GRAUWOLF_IDENTITY_URL",
                "https://identity.identity.svc:8443",
            ),
            (
                "GRAUWOLF_IDENTITY_SPIFFE_ID",
                "spiffe://d.de/ns/identity/sa/identity",
            ),
        ]))
        .unwrap();
        assert_eq!(
            config.identity_url.as_deref(),
            Some("https://identity.identity.svc:8443")
        );
        assert_eq!(
            config.identity_spiffe_id.as_deref(),
            Some("spiffe://d.de/ns/identity/sa/identity")
        );
    }
}

/// The price table of a generated agent (ADR-0071, ADR-0119).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PricesConfig {
    /// `GRAUWOLF_PRICES`: path of the price table, as `--prices` of the CLI (empty counts as
    /// unset).
    pub file: Option<String>,
}

impl PricesConfig {
    /// From `env`.
    pub fn from_env(env: &dyn Env) -> Self {
        Self::from_lookup(&|name| env.var(name))
    }

    /// From a lookup function.
    pub fn from_lookup(get: Lookup) -> Self {
        Self {
            file: non_empty(get, "GRAUWOLF_PRICES"),
        }
    }
}
