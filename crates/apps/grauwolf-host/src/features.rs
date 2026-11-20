//! Features with input/output for the feature pipeline (F06.1, ADR-0007).

use std::io::Write;
use std::sync::Mutex;

use grauwolf_agents::events::{AgentEvent, Feature};

/// Writes each event as one JSON line (JSONL), e.g. to stderr or into a file.
pub struct JsonlEventLog<W> {
    out: Mutex<W>,
}

impl<W: Write + Send> JsonlEventLog<W> {
    /// Event log that writes to `out`.
    pub fn new(out: W) -> Self {
        Self {
            out: Mutex::new(out),
        }
    }
}

impl<W: Write + Send> Feature for JsonlEventLog<W> {
    fn on_event(&self, event: &AgentEvent) {
        // A log must not abort the run; write errors are discarded.
        if let (Ok(line), Ok(mut out)) = (serde_json::to_string(event), self.out.lock()) {
            let _ = writeln!(out, "{line}").and_then(|()| out.flush());
        }
    }
}
