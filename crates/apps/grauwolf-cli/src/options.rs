//! Option structs shared by several commands: build/cache options (defined with the tool commands in
//! `grauwolf-tools-app`) and run options.

use std::path::PathBuf;

pub use grauwolf_tools_app::{BuildOptions, CacheOptions};

/// Options shared by `spec run` and `spec resume`.
#[derive(clap::Args)]
pub struct RunOptions {
    #[command(flatten)]
    pub build: BuildOptions,
    /// Prints text from streaming nodes (`llm_request_streaming`) immediately.
    #[arg(long)]
    pub stream: bool,
    /// Writes all events of the run as JSON lines to stderr.
    #[arg(long)]
    pub events: bool,
    /// Price table `{ "currency", "models": { "<provider>:<id>": { "input", "output", … } } }`
    /// (per million tokens): events and spans name the cost (ADR-0071).
    #[arg(long)]
    pub prices: Option<PathBuf>,
    /// Ends a run as soon as it has used this many input and output tokens
    /// (ADR-0072).
    #[arg(long)]
    pub max_tokens: Option<u64>,
    /// Ends a run as soon as it reaches this cost; needs `--prices` (ADR-0072).
    #[arg(long)]
    pub max_cost: Option<f64>,
    /// Continues the conversation with this ID and stores the history (chat memory).
    #[arg(long)]
    pub conversation: Option<String>,
    /// Directory of the conversation histories [default: .grauwolf/conversations].
    #[arg(long)]
    pub conversations_dir: Option<PathBuf>,
    /// Database of the conversation histories instead of the directory, e.g. `sqlite://chats.db` or
    /// `postgres://user@host/db` (ADR-0048).
    #[arg(long)]
    pub conversations_db: Option<String>,
    /// Answer cache: identical LLM requests are served from files in this directory instead of
    /// the provider (ADR-0086).
    #[arg(long, value_name = "DIR")]
    pub llm_cache: Option<PathBuf>,
    /// Directory of the checkpoints [default: .grauwolf/runs].
    #[arg(long)]
    pub runs_dir: Option<PathBuf>,
    /// Database of the checkpoints instead of the directory, e.g. `sqlite://runs.db` (ADR-0048).
    #[arg(long)]
    pub runs_db: Option<String>,
    /// Exports the run as OpenTelemetry spans over OTLP/HTTP; endpoint and headers from
    /// `OTEL_EXPORTER_OTLP_ENDPOINT`/`OTEL_EXPORTER_OTLP_HEADERS` (ADR-0049).
    #[arg(long)]
    pub otel: bool,
    /// Also records content in the spans (answers, tool arguments and results).
    #[arg(long, requires = "otel")]
    pub otel_content: bool,
    /// Asks on the console before every call of these tools (comma-separated, ADR-0047).
    #[arg(long, value_delimiter = ',')]
    pub confirm_tools: Vec<String>,
}

/// Default directory for conversation histories, relative to the working directory.
pub const DEFAULT_CONVERSATIONS: &str = ".grauwolf/conversations";

/// Default directory for checkpoints, relative to the working directory.
pub const DEFAULT_RUNS: &str = ".grauwolf/runs";

/// What a command uses the run options for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Usage {
    /// `spec run`, with or without `--run-id`.
    Run {
        checkpointed: bool,
    },
    Resume,
    Chat,
    Serve,
    Mcp,
}

impl Usage {
    pub fn command(self) -> &'static str {
        match self {
            Usage::Run { .. } => "spec run",
            Usage::Resume => "spec resume",
            Usage::Chat => "spec chat",
            Usage::Serve => "spec serve",
            Usage::Mcp => "spec mcp",
        }
    }
}

impl RunOptions {
    pub fn conversations_dir(&self) -> PathBuf {
        self.conversations_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CONVERSATIONS))
    }

    pub fn runs_dir(&self) -> PathBuf {
        self.runs_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_RUNS))
    }

    /// Error if options are set that would have no effect for `usage` – better to report
    /// them than to ignore them silently.
    pub fn check_effect(&self, usage: Usage) -> Result<(), String> {
        let mut ignored: Vec<String> = Vec::new();
        let mut flag = |set: bool, name: &str, hint: Option<&str>| {
            if set {
                ignored.push(match hint {
                    Some(hint) => format!("{name} ({hint})"),
                    None => name.to_owned(),
                });
            }
        };
        let per_request = matches!(usage, Usage::Serve | Usage::Mcp);
        flag(per_request && self.stream, "--stream", None);
        let conversation = matches!(usage, Usage::Resume | Usage::Serve | Usage::Mcp);
        flag(
            conversation && self.conversation.is_some(),
            "--conversation",
            None,
        );
        // Conversations: `run`/`chat` only with `--conversation`, `serve` always (per request).
        let (stored, hint) = match usage {
            Usage::Run { .. } | Usage::Chat => (
                self.conversation.is_some(),
                Some("only with --conversation"),
            ),
            Usage::Serve => (true, None),
            Usage::Resume | Usage::Mcp => (false, None),
        };
        flag(
            !stored && self.conversations_dir.is_some(),
            "--conversations-dir",
            hint,
        );
        flag(
            !stored && self.conversations_db.is_some(),
            "--conversations-db",
            hint,
        );
        // Checkpoints: `run` only with `--run-id`, `resume` always.
        let (checkpointed, hint) = match usage {
            Usage::Run { checkpointed } => (checkpointed, Some("only with --run-id")),
            Usage::Resume => (true, None),
            Usage::Chat | Usage::Serve | Usage::Mcp => (false, None),
        };
        flag(!checkpointed && self.runs_dir.is_some(), "--runs-dir", hint);
        flag(!checkpointed && self.runs_db.is_some(), "--runs-db", hint);
        if ignored.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "no effect with {}: {}",
                usage.command(),
                ignored.join(", ")
            ))
        }
    }
}
