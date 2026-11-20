//! `grauwolf spec run` and `spec resume`: one run of the agent, fresh or from a checkpoint.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use grauwolf_agents::FrameSink;
use grauwolf_agents::checkpoints::CheckpointStore;
use grauwolf_agents::memory::{ChatHistoryStore, conversation_key};
use grauwolf_host::checkpoints::FileCheckpointStore;
use grauwolf_host::memory::FileChatHistoryStore;
use grauwolf_host::otel::OtelTracing;
use grauwolf_interpreter::RunExtras;
use grauwolf_llm::StreamFrame;
use serde_json::Value;

use super::handles::{console_approver, parse_input};
use super::loader::agent_loader;
use crate::options::{RunOptions, Usage};
use crate::output::{Io, out, outln, report};

/// How a run starts: fresh (optionally with checkpoints) or at the last checkpoint.
pub enum Start {
    Fresh {
        input: String,
        run_id: Option<String>,
        user: Option<String>,
        attachments: Vec<PathBuf>,
    },
    Resume {
        run_id: String,
        user: Option<String>,
    },
}

/// `grauwolf spec run <file> <input> [--run-id <id>]` and
/// `grauwolf spec resume <file> --run-id <id>`.
pub fn run(io: &Io, file: &Path, start: Start, options: &RunOptions) -> ExitCode {
    let usage = match &start {
        Start::Fresh { run_id, .. } => Usage::Run {
            checkpointed: run_id.is_some(),
        },
        Start::Resume { .. } => Usage::Resume,
    };
    if let Err(error) = options.check_effect(usage) {
        return report(io, Err(error));
    }
    let tracing = match options
        .otel
        .then(|| {
            OtelTracing::init_with(
                None,
                &grauwolf_host::config::TelemetryConfig::from_env(
                    &grauwolf_host::config::ProcessEnv,
                ),
            )
        })
        .transpose()
    {
        Ok(tracing) => tracing,
        Err(error) => return report(io, Err(error)),
    };
    let result = run_agent(io, file, start, options, tracing.as_ref());
    // Send open spans even after errors.
    let flushed = tracing.map_or(Ok(()), OtelTracing::shutdown);
    report(io, result.and(flushed))
}

/// Runs the spec `file`; with `tracing` every agent reports its spans.
fn run_agent(
    io: &Io,
    file: &Path,
    start: Start,
    options: &RunOptions,
    tracing: Option<&OtelTracing>,
) -> Result<(), String> {
    (|| {
        let fail = |error: &dyn std::fmt::Display| format!("{}: {error}", file.display());
        let runtime = tokio::runtime::Runtime::new().map_err(|error| fail(&error))?;
        // Live output: write text deltas immediately; then the text is not repeated at the end.
        let streamed = Arc::new(AtomicBool::new(false));
        let sink: Option<FrameSink> = options.stream.then(|| {
            let io = io.clone();
            let streamed = Arc::clone(&streamed);
            Arc::new(move |frame: &StreamFrame| {
                if let StreamFrame::TextDelta { text } = frame {
                    out!(io, "{text}");
                    streamed.store(true, Ordering::Relaxed);
                }
            }) as FrameSink
        });
        let (spec, agent, provisioned) =
            runtime.block_on(agent_loader(io, options, tracing)?.load_provisioned(file, sink))?;
        let store: Arc<dyn CheckpointStore> = match &options.runs_db {
            Some(url) => runtime.block_on(grauwolf_host::checkpoints::sql_checkpoint_store(url))?,
            None => Arc::new(FileCheckpointStore::new(options.runs_dir())),
        };
        let output = match start {
            Start::Fresh {
                input,
                run_id,
                user,
                attachments,
            } => {
                let input = parse_input(&spec, input)?;
                let attachments = attachments
                    .iter()
                    .map(|file| grauwolf_host::attachments::from_file(file))
                    .collect::<Result<Vec<_>, _>>()?;
                // The user's memory tools replace those of the shared area (ADR-0059);
                // without memory, `--user` only affects the conversation (ADR-0060).
                if user.is_some() && !provisioned.has_memory() && options.conversation.is_none() {
                    return Err("--user needs a long_term_memory in the spec \
                                or --conversation"
                        .to_owned());
                }
                let base = runtime.block_on(provisioned.run(user.as_deref())?.extras());
                match (run_id, &options.conversation) {
                    (Some(_), Some(_)) => {
                        return Err("--run-id and --conversation cannot be combined".to_owned());
                    }
                    (Some(run_id), None) => {
                        // Checkpoint runs with the same handles as ordinary runs (ADR-0077).
                        let extras = RunExtras {
                            approver: console_approver(&spec),
                            attachments: attachments.clone(),
                            ..base
                        };
                        runtime.block_on(agent.run_checkpointed_with(input, store, &run_id, extras))
                    }
                    (None, Some(conversation)) => {
                        let history: Arc<dyn ChatHistoryStore> = match &options.conversations_db {
                            Some(url) => runtime
                                .block_on(grauwolf_host::memory::sql_chat_history_store(url))?,
                            None => {
                                Arc::new(FileChatHistoryStore::new(options.conversations_dir()))
                            }
                        };
                        // The conversation belongs to the user (ADR-0060).
                        let key = conversation_key(user.as_deref(), conversation)?;
                        let extras = RunExtras {
                            conversation: Some((history, key)),
                            approver: console_approver(&spec),
                            attachments: attachments.clone(),
                            ..base
                        };
                        runtime.block_on(agent.run_with(input, extras))
                    }
                    (None, None) => {
                        let extras = RunExtras {
                            approver: console_approver(&spec),
                            attachments: attachments.clone(),
                            ..base
                        };
                        runtime.block_on(agent.run_with(input, extras))
                    }
                }
            }
            Start::Resume { run_id, user } => {
                // Tools directory, memory (the user's or the shared area) and approval
                // as at the start (ADR-0077).
                if user.is_some() && !provisioned.has_memory() {
                    return Err("--user needs a long_term_memory in the spec".to_owned());
                }
                let base = runtime.block_on(provisioned.run(user.as_deref())?.extras());
                let extras = RunExtras {
                    approver: console_approver(&spec),
                    ..base
                };
                runtime.block_on(agent.resume_with(store, &run_id, extras))
            }
        }
        .map_err(|error| fail(&error))?;
        match output {
            Value::String(_) if streamed.load(Ordering::Relaxed) => outln!(io),
            Value::String(text) => outln!(io, "{text}"),
            other => outln!(io, "{other:#}"),
        }
        Ok(())
    })()
}
