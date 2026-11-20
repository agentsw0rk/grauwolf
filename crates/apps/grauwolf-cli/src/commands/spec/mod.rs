//! `grauwolf spec …`: one module per subcommand.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;

use crate::options::{BuildOptions, RunOptions};
use crate::output::Io;

pub mod build;
pub mod chat;
pub mod create;
pub mod egress;
pub mod generate;
pub mod graph;
pub(crate) mod handles;
pub(crate) mod loader;
pub mod mcp;
pub mod resume;
pub mod run;
pub mod schema;
pub mod serve;
pub mod validate;

/// `spec serve` options for callers with a SPIFFE identity (ADR-0109); the environment
/// variables `GRAUWOLF_SPIFFE`, `GRAUWOLF_SPIFFE_PORT` and `GRAUWOLF_SPIFFE_USERS` do the same.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct SpiffeFlags {
    /// Serves a second port over mTLS with the SVID of the Workload API
    /// (`SPIFFE_ENDPOINT_SOCKET`), requiring client certificates, and accepts JWT-SVIDs in
    /// `Authorization: Bearer` on the HTTP port.
    #[arg(long)]
    pub spiffe: bool,
    /// Port of the mTLS listener; default 8443.
    #[arg(long)]
    pub spiffe_port: Option<u16>,
    /// File `{ "<spiffe id>" | "<prefix>/*": "<user>" }` that maps callers to users; read
    /// again when it changes.
    #[arg(long)]
    pub spiffe_users: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum SpecCommand {
    /// Checks a spec and reports all errors with a JSON pointer.
    Validate {
        /// Path to the spec.
        file: PathBuf,
    },
    /// Prints the flow of the spec as a Mermaid diagram (F13.14).
    Graph {
        /// Path to the spec.
        file: PathBuf,
    },
    /// Prints every destination the agent (and its local sub-agents) may reach, derived from the
    /// specs: model hosts, MCP servers, remote agents, `fetch` hosts, Jira, git, … with `phase`
    /// (`run` or `build`) and `from_env` for addresses that a variable may change (ADR-0110).
    Egress {
        /// Path to the spec.
        file: PathBuf,
        /// `text`: one line per destination; `json`: an array.
        #[arg(long, value_enum, default_value = "text")]
        format: egress::EgressFormat,
        /// Also lists the OTLP endpoint (the agent runs with `--otel`).
        #[arg(long)]
        otel: bool,
    },
    /// Prints the JSON schema of the spec format (F13.2).
    Schema,
    /// Builds the tool code of the spec into WASM artifacts (ADR-0024).
    Build {
        /// Path to the spec.
        file: PathBuf,
        #[command(flatten)]
        build: BuildOptions,
    },
    /// Runs the spec in the interpreter against the provider from `model.provider`.
    ///
    /// Credentials come from the environment: `OLLAMA_HOST`, `OPENAI_API_KEY`
    /// (+ `OPENAI_BASE_URL`), `ANTHROPIC_API_KEY`, `GEMINI_API_KEY` (+ `GEMINI_BASE_URL`), `AWS_ACCESS_KEY_ID` +
    /// `AWS_SECRET_ACCESS_KEY` (or `AWS_BEARER_TOKEN_BEDROCK`; + `AWS_REGION`) and for
    /// OpenAI-compatible providers `<NAME>_API_KEY` (+ `<NAME>_BASE_URL`).
    Run {
        /// Path to the spec.
        file: PathBuf,
        /// Input: text for `input: string`, otherwise JSON.
        input: String,
        #[command(flatten)]
        options: RunOptions,
        /// Saves a checkpoint before every node under this run ID (for `spec resume`).
        #[arg(long)]
        run_id: Option<String>,
        /// User of the run: selects their area in the long-term memory (ADR-0059).
        #[arg(long)]
        user: Option<String>,
        /// File attached to the input (repeatable): png, jpg, gif, webp, pdf, wav, mp3
        /// (ADR-0081).
        #[arg(long = "attach", value_name = "FILE")]
        attachments: Vec<PathBuf>,
    },
    /// Chats with the agent: every line from stdin is a run in the same conversation, the
    /// answer goes to stdout; ends at EOF or `/exit` (ADR-0064).
    Chat {
        /// Path to the spec.
        file: PathBuf,
        #[command(flatten)]
        options: RunOptions,
    },
    /// Serves the agent as an HTTP service: `POST /run`, `POST /run/stream` (SSE),
    /// `GET /health` (ADR-0053). With `GRAUWOLF_SERVER_TOKEN` it requires
    /// `Authorization: Bearer <token>`; without a token only on local addresses.
    Serve {
        /// Path to the spec.
        file: PathBuf,
        /// Address of the service; without authentication, hence local only by default.
        #[arg(long, default_value = "127.0.0.1:8080")]
        addr: String,
        /// Public base URL for the A2A agent card, e.g. behind a reverse proxy;
        /// defaults to the bound address.
        #[arg(long)]
        public_url: Option<String>,
        /// Users file `{ "<user>": "sha256:<hex>" }` (entries from `grauwolf token`):
        /// requests with a user token run as this user (ADR-0068).
        #[arg(long)]
        users: Option<PathBuf>,
        /// Also serves the chat API: `POST /api/chat` (`{"message": …}` → `{"response": …}`;
        /// typed input and output as the body itself) and `GET /api/signature` (the file
        /// `agent-signature.json` next to the spec or at `AGENT_SIGNATURE_PATH`, plus the models
        /// and MCP tools the agent has used; open without a token). `agent-names.json` next to
        /// the spec maps spec names to agent names. `OLLAMA_BASE_URL` stands in for
        /// `OLLAMA_HOST` (ADR-0094).
        #[arg(long)]
        chat_api: bool,
        #[command(flatten)]
        spiffe: SpiffeFlags,
        #[command(flatten)]
        options: RunOptions,
    },
    /// Serves the agent as an MCP server over stdin/stdout, e.g. for Claude Desktop
    /// (ADR-0056); messages go to stderr.
    Mcp {
        /// Path to the spec.
        file: PathBuf,
        #[command(flatten)]
        options: RunOptions,
    },
    /// Creates a valid spec from a requirement with an LLM (builder agent, F13.12).
    ///
    /// Errors from validation and from compiling the tool code go back to the LLM with their
    /// path until the spec is valid or no more repairs are allowed.
    Create {
        /// Requirement as text.
        requirement: String,
        /// LLM as `<provider>:<model>`, e.g. `ollama:qwen3.5:9b` or `openai:gpt-4o`.
        #[arg(long)]
        model: String,
        /// Target file of the spec.
        #[arg(long)]
        out: PathBuf,
        /// Maximum number of repairs.
        #[arg(long, default_value_t = grauwolf_builder_agent::DEFAULT_MAX_REPAIRS)]
        max_repairs: u32,
        #[command(flatten)]
        build: BuildOptions,
    },
    /// Resumes an interrupted run at the last checkpoint (ADR-0020, ADR-0034, ADR-0077).
    Resume {
        /// Path to the spec.
        file: PathBuf,
        /// Run ID under which `spec run --run-id` saved the checkpoints.
        #[arg(long)]
        run_id: String,
        /// User of the run as at the start with `--user`: their memory instead of the
        /// shared area (ADR-0077 addendum).
        #[arg(long)]
        user: Option<String>,
        #[command(flatten)]
        options: RunOptions,
    },
    /// Generates a standalone Rust crate from the spec (including built tools).
    Generate {
        /// Path to the spec.
        file: PathBuf,
        /// Target directory of the crate.
        #[arg(long)]
        out: PathBuf,
        #[command(flatten)]
        build: BuildOptions,
        /// `crates/` directory of a grauwolf checkout; default: the checkout of this CLI.
        #[arg(long)]
        grauwolf_crates: Option<PathBuf>,
        /// Checks the generated crate afterwards with `cargo check`.
        #[arg(long)]
        check: bool,
        /// Also writes `src/main.rs`: the binary `agent`, behind the Cargo feature `service`,
        /// serves the agent over HTTP (ADR-0093); `--check` then checks with that feature.
        #[arg(long)]
        service: bool,
        /// Depends on the facade with all its features, whatever the spec needs, so that the
        /// dependency graph is the same for every spec (pre-built dependencies, ADR-0096).
        #[arg(long)]
        full_facade: bool,
        /// Also writes a `Dockerfile` and `.dockerignore` that build the service offline in the
        /// builder image, and copies the files the spec reads at run time to `app/`; implies
        /// `--service`, `--full-facade` and `--grauwolf-crates /opt/grauwolf/crates` (ADR-0096).
        #[arg(long)]
        dockerfile: bool,
        /// Builder image for `--dockerfile` [default:
        /// ghcr.io/agentsw0rk/grauwolf-builder:<version of this CLI>].
        #[arg(long, requires = "dockerfile")]
        builder_image: Option<String>,
        /// Do not make the build fail when the builder image is of another version than this CLI
        /// (for a deliberate `--builder-image`).
        #[arg(long, requires = "dockerfile")]
        allow_builder_mismatch: bool,
        /// Writes the tool code as sources (`tools/<name>/`, manifest `tools/tools.json`) instead
        /// of building WASM modules; needs no Rust toolchain. `grauwolf tools build <dir>` builds
        /// the modules before `cargo build`; the `--dockerfile` does it in the image build
        /// (ADR-0095). Not with `--check`.
        #[arg(long)]
        tool_sources: bool,
    },
}

/// Runs one `spec` subcommand.
pub fn run(io: &Io, command: SpecCommand) -> ExitCode {
    match command {
        SpecCommand::Validate { file } => validate::run(io, &file),
        SpecCommand::Graph { file } => graph::run(io, &file),
        SpecCommand::Egress { file, format, otel } => egress::run(io, &file, format, otel),
        SpecCommand::Schema => schema::run(io),
        SpecCommand::Build {
            file,
            build: options,
        } => build::run(io, &file, &options),
        SpecCommand::Run {
            file,
            input,
            options,
            run_id,
            user,
            attachments,
        } => run::run(
            io,
            &file,
            run::Start::Fresh {
                input,
                run_id,
                user,
                attachments,
            },
            &options,
        ),
        SpecCommand::Create {
            requirement,
            model,
            out,
            max_repairs,
            build: options,
        } => create::run(io, &requirement, &model, &out, max_repairs, &options),
        SpecCommand::Resume {
            file,
            run_id,
            user,
            options,
        } => resume::run(io, &file, run_id, user, &options),
        SpecCommand::Mcp { file, options } => mcp::run(io, &file, &options),
        SpecCommand::Chat { file, options } => chat::run(io, &file, &options),
        SpecCommand::Serve {
            file,
            addr,
            public_url,
            users,
            chat_api,
            spiffe,
            options,
        } => serve::run(
            io,
            &file,
            &addr,
            public_url.as_deref(),
            users.as_deref(),
            chat_api,
            &spiffe,
            &options,
        ),
        SpecCommand::Generate {
            file,
            out,
            build: options,
            grauwolf_crates,
            check,
            service,
            full_facade,
            dockerfile,
            builder_image,
            allow_builder_mismatch,
            tool_sources,
        } => generate::run(
            io,
            &file,
            &out,
            &options,
            generate::GenerateArgs {
                grauwolf_crates,
                check,
                service,
                full_facade,
                dockerfile,
                builder_image,
                allow_builder_mismatch,
                tool_sources,
            },
        ),
    }
}
