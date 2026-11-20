//! Egress derivation (ADR-0110 addendum B, stage 6): every destination an agent tree may reach,
//! derived from its specs. `grauwolf spec egress`, `app/egress.json` of `spec generate
//! --dockerfile` and `grauwolf-k8-ops agent deploy --egress` all use [`derive_tree`].

mod collect;
mod derive;
pub mod providers;
mod resolve;
pub mod target;

pub use derive::{
    OTLP_ENDPOINT, Options, RKEV_BASE_URL, credentials_file, derive_spec, derive_tree,
};
pub use resolve::{resolve, resolve_all};

use serde_json::{Value, json};

/// When a destination is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// While the agent runs.
    Run,
    /// While the image is built (tool registries, git sources); not needed at run time.
    Build,
}

impl Phase {
    /// `run` or `build`.
    pub fn word(self) -> &'static str {
        match self {
            Phase::Run => "run",
            Phase::Build => "build",
        }
    }
}

/// One destination. `scheme`, `host` and `port` are `None` when they are only known at run time:
/// then `from_env` names the variable that holds the address (the derivation never guesses). With
/// a default address (a provider's) all three are set and `from_env` names the variable that may
/// replace it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub scheme: Option<String>,
    /// Lower case; a pattern (`*.example.com`) as the spec gives it.
    pub host: Option<String>,
    pub port: Option<u16>,
    /// Any port of the host: the spec names a Service of the cluster without a port (a `fetch`
    /// host), so the port is not fixed by the spec. `port` is `None` then.
    pub any_port: bool,
    /// What it is for; several purposes of one destination are joined with `; `.
    pub purpose: String,
    /// Where the spec names it, a JSON pointer (with the spec file for a sub-agent:
    /// `agents/writer.json:/model/provider`); several are joined with `, `.
    pub pointer: String,
    pub phase: Phase,
    pub from_env: Option<String>,
}

impl Destination {
    /// The destination as JSON.
    pub fn to_json(&self) -> Value {
        json!({"scheme": self.scheme, "host": self.host, "port": self.port, "any_port": self.any_port,
            "purpose": self.purpose, "pointer": self.pointer, "phase": self.phase.word(),
            "from_env": self.from_env})
    }

    /// `https://api.openai.com:443` (`https://svc.ns.svc:*` for any port), or `unknown` when the address is only known at run time.
    pub fn target(&self) -> String {
        match (&self.scheme, &self.host, self.port) {
            (scheme, Some(host), port) => format!(
                "{}://{host}{}",
                scheme.as_deref().unwrap_or("tcp"),
                match (port, self.any_port) {
                    (Some(port), _) => format!(":{port}"),
                    (None, true) => ":*".to_owned(),
                    (None, false) => String::new(),
                }
            ),
            _ => "unknown".to_owned(),
        }
    }

    /// One line of the text format.
    pub fn line(&self) -> String {
        let from = self
            .from_env
            .as_ref()
            .map(|name| format!("  (from {name})"))
            .unwrap_or_default();
        format!(
            "{:<5} {}  {}  [{}]{from}",
            self.phase.word(),
            self.target(),
            self.purpose,
            self.pointer
        )
    }
}

/// The destinations as a JSON array (pretty printed, with a final newline).
pub fn to_json(destinations: &[Destination]) -> String {
    let array: Vec<Value> = destinations.iter().map(Destination::to_json).collect();
    let mut text = serde_json::to_string_pretty(&array).expect("JSON values serialize");
    text.push('\n');
    text
}

/// The destinations, one line each.
pub fn to_text(destinations: &[Destination]) -> String {
    destinations
        .iter()
        .map(|destination| destination.line() + "\n")
        .collect()
}
