//! `grauwolf spec chat`: every line from stdin is a run in the same conversation (ADR-0064).

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use grauwolf_agents::memory::{ChatHistoryStore, conversation_key};
use grauwolf_host::memory::FileChatHistoryStore;
use grauwolf_interpreter::RunExtras;
use serde_json::Value;

use super::handles::{console_approver, parse_input};
use super::loader::agent_loader;
use crate::options::{RunOptions, Usage};
use crate::output::{Io, err, errln, outln, report};

/// `grauwolf spec chat <file>`: every line from stdin is a run in the same conversation (ADR-0064).
pub fn run(io: &Io, file: &Path, options: &RunOptions) -> ExitCode {
    if let Err(error) = options.check_effect(Usage::Chat) {
        return report(io, Err(error));
    }
    if options.stream || options.otel {
        return report(
            io,
            Err("--stream and --otel are not yet available in spec chat".to_owned()),
        );
    }
    report(
        io,
        (|| {
            let runtime = crate::errors::runtime()?;
            let (spec, agent, provisioned) =
                runtime.block_on(agent_loader(io, options, None)?.load_provisioned(file, None))?;
            // Saved and resumable with `--conversation`, otherwise only in memory.
            let (history, key): (Arc<dyn ChatHistoryStore>, String) = match &options.conversation {
                Some(id) => (
                    match &options.conversations_db {
                        Some(url) => {
                            runtime.block_on(grauwolf_host::memory::sql_chat_history_store(url))?
                        }
                        None => Arc::new(FileChatHistoryStore::new(options.conversations_dir())),
                    },
                    conversation_key(None, id)?,
                ),
                None => (
                    Arc::new(grauwolf_agents::memory::MemoryChatHistoryStore::default()),
                    "chat".to_owned(),
                ),
            };
            // Memory (ADR-0069) in the shared area; the tools directory is reloaded for every
            // line (ADR-0074).
            let provisioned_run = provisioned.run(None)?;
            // Collected with `/attach <file>`, they go along with the next input (ADR-0081).
            let mut attachments = Vec::new();
            loop {
                err!(io, "> ");
                let mut line = String::new();
                if std::io::stdin()
                    .read_line(&mut line)
                    .map_err(|error| error.to_string())?
                    == 0
                {
                    return Ok(());
                }
                let line = line.trim();
                if line == "/exit" {
                    return Ok(());
                }
                if line.is_empty() {
                    continue;
                }
                if let Some(path) = line.strip_prefix("/attach ") {
                    match grauwolf_host::attachments::from_file(Path::new(path.trim())) {
                        Ok(attachment) => {
                            errln!(io, "Attachment for the next input: {}", path.trim());
                            attachments.push(attachment);
                        }
                        Err(error) => errln!(io, "{error}"),
                    }
                    continue;
                }
                let base = runtime.block_on(provisioned_run.extras());
                let extras = RunExtras {
                    conversation: Some((Arc::clone(&history), key.clone())),
                    approver: console_approver(&spec),
                    attachments: std::mem::take(&mut attachments),
                    ..base
                };
                let output = parse_input(&spec, line.to_owned()).and_then(|input| {
                    runtime
                        .block_on(agent.run_with(input, extras))
                        .map_err(|error| error.to_string())
                });
                match output {
                    Ok(Value::String(text)) => outln!(io, "{text}"),
                    Ok(other) => outln!(io, "{other:#}"),
                    Err(error) => errln!(io, "{}: {error}", file.display()),
                }
            }
        })(),
    )
}
