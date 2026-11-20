//! The clap definition of the top level: `grauwolf spec …`, `grauwolf tools …`, `grauwolf token`.

use clap::{Parser, Subcommand};

use crate::commands::harness::HarnessCommand;
use crate::commands::spec::SpecCommand;
use crate::commands::tools::ToolsCommand;

#[derive(Parser)]
#[command(name = "grauwolf", version, about = "grauwolf – agents from specs")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

// Parsed only once, so the size difference between the variants does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub enum Command {
    /// Work with agent specs (JSON).
    Spec {
        #[command(subcommand)]
        command: SpecCommand,
    },
    /// Harnesses: generated, deployable projects around an agent tree (ADR-0128).
    Harness {
        #[command(subcommand)]
        command: HarnessCommand,
    },
    /// Tool packages from OCI registries (ADR-0107).
    Tools {
        #[command(subcommand)]
        command: ToolsCommand,
    },
    /// Prints the version of this program (`grauwolf --version` as well). With `--json`:
    /// `{"version": "…", "builder_image": "…"}` – the builder image that `spec generate
    /// --dockerfile` uses by default, for tools that check CLI and builder (ADR-0096).
    Version {
        /// Print JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Credential sources (ADR-0110).
    Credentials {
        #[command(subcommand)]
        command: CredentialsCommand,
    },
    /// Delegation (ADR-0111): the identity service of a trust domain.
    Identity {
        #[command(subcommand)]
        command: IdentityCommand,
    },
    /// Signs a person in with Entra (or Entra Local) by the device code flow (ADR-0111) and
    /// caches the tokens in `<config dir>/login.json` (mode 0600; config dir: `--config-dir`,
    /// `GRAUWOLF_CONFIG_DIR`, `$XDG_CONFIG_HOME/grauwolf`, `~/.config/grauwolf`). The settings come
    /// from the flags, else `GRAUWOLF_LOGIN_*`, else the last login. `--print-token` prints the
    /// access token for `Authorization: Bearer` (refreshed when needed); `--status` shows who is
    /// signed in.
    Login(LoginArgs),
    /// Removes the cached tokens of `grauwolf login`.
    Logout(LogoutArgs),
    /// Calls an agent's `/run` once and prints its output (ADR-0111 stage 3; the command of a
    /// schedule's CronJob). With `SPIFFE_ENDPOINT_SOCKET` it authenticates with the workload
    /// identity – `https://` with mTLS (the server must be `--spiffe-id`), `http://` with a
    /// JWT-SVID for the audience `--spiffe-id` –, else with `--token`. Exit codes: 0 answered,
    /// 1 failed, 3 refused (401/403).
    Call(CallArgs),
    /// Creates a user token for `spec serve --users` (ADR-0068): first line the token
    /// (visible only now), second the line for the users file.
    Token {
        /// User who owns the token.
        user: String,
    },
}

/// `grauwolf identity …`.
#[derive(Debug, Subcommand)]
pub enum IdentityCommand {
    /// Serves the identity service (ADR-0111): `POST /api/identity/token` exchanges a person's
    /// Entra access token (entry) or a Txn-Token (renewal) for a Txn-Token, for callers with a
    /// SPIFFE identity allowed by `GRAUWOLF_IDENTITY_POLICY`; `GET /api/identity/jwks.json` and
    /// `GET /api/identity/config` are public. Configured by the `GRAUWOLF_IDENTITY_*` variables;
    /// needs `SPIFFE_ENDPOINT_SOCKET` and OpenBao Transit.
    Serve {
        /// HTTP port (JWT-SVID callers, keys, configuration); default `SERVER_PORT` or 8080.
        #[arg(long)]
        port: Option<u16>,
        /// mTLS port; default `GRAUWOLF_SPIFFE_PORT` or 8443.
        #[arg(long)]
        mtls_port: Option<u16>,
    },
    /// A person's grants for unattended runs (ADR-0111 stage 3), signed in with `grauwolf login`.
    Grant {
        #[command(subcommand)]
        command: GrantCommand,
    },
}

/// Where the grants are: the identity service and the login.
#[derive(Debug, clap::Args)]
pub struct GrantTarget {
    /// The identity service's public URL (`GRAUWOLF_LOGIN_IDENTITY_URL`, else the one of
    /// `grauwolf login --identity-url`).
    #[arg(long, value_name = "URL")]
    pub identity_url: Option<String>,
    /// Where the login's token cache lives.
    #[arg(long, value_name = "DIR")]
    pub config_dir: Option<std::path::PathBuf>,
}

/// `grauwolf identity grant …`.
#[derive(Debug, Subcommand)]
pub enum GrantCommand {
    /// Creates a grant: a separate device code sign-in with `offline_access` (the login cache is
    /// not touched); its refresh token goes to the identity service once and is not kept.
    Create(GrantCreateArgs),
    /// Lists your grants (never a token).
    List {
        #[command(flatten)]
        target: GrantTarget,
        /// Print JSON.
        #[arg(long)]
        json: bool,
    },
    /// Revokes one of your grants: its metadata and refresh token are deleted at once.
    Revoke {
        #[command(flatten)]
        target: GrantTarget,
        /// The grant ID (`grt_…`).
        id: String,
    },
}

/// `grauwolf identity grant create`.
#[derive(Debug, clap::Args)]
pub struct GrantCreateArgs {
    /// The entry agent's SPIFFE ID.
    #[arg(long, value_name = "SPIFFE_ID")]
    pub agent: String,
    /// The SPIFFE ID of the workload that may start runs (e.g. a schedule).
    #[arg(long, value_name = "SPIFFE_ID")]
    pub trigger: String,
    /// A purpose of the runs (repeatable); the policy must allow it for the agent.
    #[arg(long = "purpose", value_name = "PURPOSE")]
    pub purposes: Vec<String>,
    /// How long the grant is valid (at most 90d).
    #[arg(long, default_value = "30d", value_name = "DURATION")]
    pub valid: String,
    /// How long one run may last (its chain's maximum lifetime).
    #[arg(long, default_value = "4h", value_name = "DURATION")]
    pub max_run: String,
    #[command(flatten)]
    pub target: GrantTarget,
}

/// `grauwolf call`.
#[derive(Debug, clap::Args)]
pub struct CallArgs {
    /// The agent's address or its `/run` URL (`/run` is appended to a bare address).
    pub url: String,
    /// The agent's expected SPIFFE ID (mTLS server check; the JWT-SVID audience over http://).
    #[arg(long, value_name = "SPIFFE_ID")]
    pub spiffe_id: Option<String>,
    /// A person's grant (`grt_…`), sent as `Grauwolf-Grant`.
    #[arg(long, value_name = "GRANT")]
    pub grant: Option<String>,
    /// The input; without it stdin is read.
    #[arg(long)]
    pub input: Option<String>,
    /// The input is JSON (else it is sent as text).
    #[arg(long)]
    pub json: bool,
    /// A bearer token instead of the workload identity (shared token, user token, a person's
    /// access token).
    #[arg(long, value_name = "TOKEN")]
    pub token: Option<String>,
    /// Seconds to wait for the answer.
    #[arg(long, default_value_t = 600)]
    pub timeout: u64,
}

/// `grauwolf credentials …`.
#[derive(Debug, Subcommand)]
pub enum CredentialsCommand {
    /// Resolves variables through the credential sources and prints one line per variable,
    /// `resolved <VAR> from <file|openbao|environment> (N bytes)` – never the value – or the
    /// error that names the variable (on stderr; exit code 1 if any fails). Reads the
    /// `[credentials]` section of `--config`, else of `GRAUWOLF_CREDENTIALS`, else of
    /// `./grauwolf.toml`.
    Check {
        /// The variables, e.g. `ATLASSIAN_API_TOKEN`.
        #[arg(required_unless_present = "all", value_name = "VARIABLE")]
        variables: Vec<String>,
        /// Check every variable mapped in the `[credentials]` section.
        #[arg(long, conflicts_with = "variables")]
        all: bool,
        /// The file with the `[credentials]` section.
        #[arg(long)]
        config: Option<std::path::PathBuf>,
    },
}

/// `grauwolf login …`.
#[derive(Debug, clap::Args)]
pub struct LoginArgs {
    /// The tenant ID (`GRAUWOLF_LOGIN_TENANT`).
    #[arg(long, value_name = "ID")]
    pub tenant: Option<String>,
    /// The authority (`GRAUWOLF_LOGIN_AUTHORITY`; default `https://login.microsoftonline.com`;
    /// Entra Local: its address). The endpoints are `<authority>/<tenant>/oauth2/v2.0/devicecode`
    /// and `…/token`.
    #[arg(long, value_name = "URL")]
    pub authority: Option<String>,
    /// The application ID of the public client "grauwolf CLI" (`GRAUWOLF_LOGIN_CLIENT_ID`).
    #[arg(long, value_name = "ID")]
    pub client_id: Option<String>,
    /// The API scope, e.g. `api://grauwolf-identity/access_as_user` (`GRAUWOLF_LOGIN_SCOPE`);
    /// `offline_access` is added for the refresh token.
    #[arg(long, value_name = "SCOPE")]
    pub scope: Option<String>,
    /// The identity service's public URL, kept for `grauwolf identity grant`
    /// (`GRAUWOLF_LOGIN_IDENTITY_URL`, else the last login's).
    #[arg(long, value_name = "URL")]
    pub identity_url: Option<String>,
    /// Where the token cache lives.
    #[arg(long, value_name = "DIR")]
    pub config_dir: Option<std::path::PathBuf>,
    /// Show who is signed in and until when; exit code 1 when nobody is (or the login expired).
    #[arg(long, conflicts_with_all = ["print_token", "tenant", "authority", "client_id", "scope", "identity_url"])]
    pub status: bool,
    /// Print only the access token on stdout (refreshed first when it has less than a minute left),
    /// e.g. `curl -H "Authorization: Bearer $(grauwolf login --print-token)" …`.
    #[arg(long, conflicts_with_all = ["tenant", "authority", "client_id", "scope", "identity_url"])]
    pub print_token: bool,
}

/// `grauwolf logout`.
#[derive(Debug, clap::Args)]
pub struct LogoutArgs {
    /// Where the token cache lives.
    #[arg(long, value_name = "DIR")]
    pub config_dir: Option<std::path::PathBuf>,
}
