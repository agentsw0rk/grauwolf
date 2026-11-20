//! The `run_command` tool (ADR-0099): programs from an allowlist, or a shell, with a cleared
//! environment, a time limit and truncated output. Unix only.

use std::sync::Arc;
use std::time::Duration;

use grauwolf_tools::DynTool;

/// Names of the command tool.
pub const COMMAND_TOOLS: &[&str] = &["run_command"];

/// Seconds after which a command is killed unless configured otherwise.
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// Bytes kept per output stream unless configured otherwise.
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 65536;

/// Settings of the command tool.
#[derive(Debug, Clone)]
pub struct CommandConfig {
    /// Allowed programs: bare names or absolute paths; ignored in shell mode.
    pub allowed: Vec<String>,
    /// Shell mode: `{ command }` runs with `sh -c`.
    pub shell: bool,
    /// The process group is killed after this time.
    pub timeout: Duration,
    /// Bytes kept of stdout and of stderr each; the rest is cut off with a note.
    pub max_output_bytes: usize,
    /// The only environment variables the program gets (`PATH` is also where bare program
    /// names are looked up).
    pub env: Vec<(String, String)>,
}

impl CommandConfig {
    /// Allowlist mode with the default limits and an empty environment.
    pub fn new(allowed: Vec<String>) -> Self {
        Self {
            allowed,
            shell: false,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            env: Vec::new(),
        }
    }
}

/// The tool `run_command` working in `root` (which must exist).
#[cfg(unix)]
pub fn command_tool(
    root: impl AsRef<std::path::Path>,
    config: CommandConfig,
) -> Result<Arc<dyn DynTool>, String> {
    let root = root.as_ref();
    let root = root
        .canonicalize()
        .map_err(|error| format!("root of the command tool {}: {error}", root.display()))?;
    Ok(Arc::new(unix::CommandTool { root, config }))
}

/// The tool `run_command`; refused on platforms without process groups and `sh`.
#[cfg(not(unix))]
pub fn command_tool(
    _root: impl AsRef<std::path::Path>,
    _config: CommandConfig,
) -> Result<Arc<dyn DynTool>, String> {
    Err("the command tool needs a Unix system (process groups, sh)".to_owned())
}

#[cfg(unix)]
mod unix {
    use std::os::unix::process::ExitStatusExt;
    use std::path::{Component, Path, PathBuf};
    use std::process::Stdio;
    use std::time::Duration;

    use futures_util::future::BoxFuture;
    use grauwolf_prompt::ToolDescriptor;
    use grauwolf_tools::{DynTool, ToolError};
    use serde_json::{Value, json};
    use tokio::io::{AsyncRead, AsyncReadExt};
    use tokio::process::Command;
    use tokio::task::JoinHandle;

    use super::CommandConfig;

    /// How long output readers get after the process is gone before the group is cleaned up.
    const DRAIN_GRACE: Duration = Duration::from_millis(500);

    pub(super) struct CommandTool {
        pub(super) root: PathBuf,
        pub(super) config: CommandConfig,
    }

    impl CommandTool {
        fn invalid(&self, reason: impl Into<String>) -> ToolError {
            ToolError::InvalidArgs {
                tool: "run_command".to_owned(),
                reason: reason.into(),
            }
        }

        fn refuse(&self, message: impl Into<String>) -> ToolError {
            ToolError::Execution(message.into())
        }

        /// `cwd` relative to the root: no `..`, no absolute path, no symlink out of the root.
        fn resolve_cwd(&self, cwd: Option<&str>) -> Result<PathBuf, ToolError> {
            let Some(cwd) = cwd else {
                return Ok(self.root.clone());
            };
            let relative = Path::new(cwd);
            if relative.is_absolute() {
                return Err(self.refuse(format!("cwd '{cwd}' must be relative to the root")));
            }
            if relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
            {
                return Err(self.refuse(format!("cwd '{cwd}' leads outside the root")));
            }
            let resolved = self
                .root
                .join(relative)
                .canonicalize()
                .map_err(|error| self.refuse(format!("cwd '{cwd}': {error}")))?;
            if !resolved.starts_with(&self.root) {
                return Err(self.refuse(format!("cwd '{cwd}' leads outside the root")));
            }
            Ok(resolved)
        }

        /// The program to start: it must be listed; a bare name is looked up in `PATH` of the
        /// cleared environment.
        fn resolve_program(&self, program: &str) -> Result<PathBuf, ToolError> {
            if !self.config.allowed.iter().any(|allowed| allowed == program) {
                return Err(self.refuse(format!(
                    "program '{program}' is not allowed; allowed: {}",
                    self.config.allowed.join(", ")
                )));
            }
            if program.starts_with('/') {
                return Ok(PathBuf::from(program));
            }
            let path = self
                .config
                .env
                .iter()
                .find(|(name, _)| name == "PATH")
                .map_or("", |(_, value)| value.as_str());
            path.split(':')
                .filter(|dir| !dir.is_empty())
                .map(|dir| Path::new(dir).join(program))
                .find(|candidate| is_executable(candidate))
                .ok_or_else(|| {
                    self.refuse(format!(
                        "program '{program}' not found in PATH of the command environment"
                    ))
                })
        }

        async fn execute(&self, mut command: Command, cwd: &Path) -> Result<String, ToolError> {
            command
                .env_clear()
                .envs(self.config.env.iter().map(|(name, value)| (name, value)))
                .current_dir(cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .process_group(0);
            let mut child = command
                .spawn()
                .map_err(|error| self.refuse(format!("could not start the command: {error}")))?;
            let group = child.id().and_then(|pid| i32::try_from(pid).ok());
            let max = self.config.max_output_bytes;
            let stdout = tokio::spawn(read_limited(child.stdout.take(), max));
            let stderr = tokio::spawn(read_limited(child.stderr.take(), max));
            let limit = self.config.timeout;
            let outcome = tokio::time::timeout(limit, child.wait()).await;
            let status = match outcome {
                Ok(status) => Some(status.map_err(|error| self.refuse(error.to_string()))?),
                Err(_) => None,
            };
            // Always clean up the group: after a timeout it is the point, after a normal exit
            // it stops background leftovers that keep the pipes open.
            if status.is_none() {
                kill_group(group);
                let _ = child.wait().await;
            }
            let (stdout, stderr) = match status {
                Some(_) => {
                    let out = drain(stdout).await;
                    let err = drain(stderr).await;
                    kill_group(group);
                    (out, err)
                }
                None => (drain(stdout).await, drain(stderr).await),
            };
            let head = match status {
                None => format!("timed out after {} s and was killed", limit.as_secs()),
                Some(status) => match (status.code(), status.signal()) {
                    (Some(code), _) => format!("exit code: {code}"),
                    (None, Some(signal)) => format!("terminated by signal {signal}"),
                    _ => "ended without an exit code".to_owned(),
                },
            };
            Ok(render(&head, &stdout, &stderr, max))
        }
    }

    fn is_executable(path: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }

    /// Kills the process group of the command (and everything it started).
    fn kill_group(group: Option<i32>) {
        if let Some(pid) = group {
            // SAFETY: plain syscall; a negative pid addresses the group, which we created.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }

    /// What a stream produced: the kept bytes and how many were cut off.
    type Captured = (Vec<u8>, u64);

    /// Reads to the end, keeping at most `max` bytes (the rest is counted and dropped so the
    /// process never blocks on a full pipe).
    async fn read_limited(stream: Option<impl AsyncRead + Unpin>, max: usize) -> Captured {
        let Some(mut stream) = stream else {
            return (Vec::new(), 0);
        };
        let (mut kept, mut cut) = (Vec::new(), 0u64);
        let mut buffer = [0u8; 8192];
        loop {
            match stream.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let room = max.saturating_sub(kept.len()).min(read);
                    kept.extend_from_slice(&buffer[..room]);
                    cut += (read - room) as u64;
                }
            }
        }
        (kept, cut)
    }

    /// Waits for a reader after a normal exit; leftovers holding the pipe open are cut off.
    async fn drain(handle: JoinHandle<Captured>) -> Captured {
        let abort = handle.abort_handle();
        match tokio::time::timeout(DRAIN_GRACE, handle).await {
            Ok(Ok(captured)) => captured,
            _ => {
                abort.abort();
                (Vec::new(), 0)
            }
        }
    }

    fn render(head: &str, stdout: &Captured, stderr: &Captured, max: usize) -> String {
        let mut text = head.to_owned();
        for (label, (bytes, cut)) in [("stdout", stdout), ("stderr", stderr)] {
            if bytes.is_empty() && *cut == 0 {
                continue;
            }
            text.push_str(&format!("\n{label}:\n"));
            text.push_str(&String::from_utf8_lossy(bytes));
            if *cut > 0 {
                text.push_str(&format!(
                    "\n[output truncated: {cut} more bytes cut off after {max} bytes]"
                ));
            }
        }
        text
    }

    impl DynTool for CommandTool {
        fn name(&self) -> &str {
            "run_command"
        }

        fn descriptor(&self) -> ToolDescriptor {
            let (description, parameters) = if self.config.shell {
                (
                    format!(
                        "Runs a shell command line with `sh -c` in the working directory \
                         (pipes, globbing and variables work). The environment is cleared; \
                         after {} s the command is killed. Returns the exit code and the output.",
                        self.config.timeout.as_secs()
                    ),
                    json!({
                        "type": "object",
                        "properties": { "command": { "type": "string", "description": "Shell command line" } },
                        "required": ["command"]
                    }),
                )
            } else {
                (
                    format!(
                        "Runs one program directly (no shell: no pipes, globbing or \
                         variables) in the working directory. Allowed programs: {}. The \
                         environment is cleared; after {} s the program is killed. Returns the \
                         exit code and the output.",
                        self.config.allowed.join(", "),
                        self.config.timeout.as_secs()
                    ),
                    json!({
                        "type": "object",
                        "properties": {
                            "program": { "type": "string", "description": "Name of an allowed program" },
                            "args": { "type": "array", "items": { "type": "string" }, "description": "Arguments, passed as they are" },
                            "cwd": { "type": "string", "description": "Directory relative to the working directory; default `.`" }
                        },
                        "required": ["program"]
                    }),
                )
            };
            ToolDescriptor {
                name: "run_command".to_owned(),
                description,
                parameters,
            }
        }

        fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
            Box::pin(async move {
                if self.config.shell {
                    let line = args["command"]
                        .as_str()
                        .ok_or_else(|| self.invalid("missing field 'command' (text)"))?;
                    let mut command = Command::new("sh");
                    command.arg("-c").arg(line);
                    return self.execute(command, &self.root).await;
                }
                let program = args["program"]
                    .as_str()
                    .ok_or_else(|| self.invalid("missing field 'program' (text)"))?;
                let arguments = match &args["args"] {
                    Value::Null => Vec::new(),
                    Value::Array(items) => items
                        .iter()
                        .map(|item| {
                            item.as_str()
                                .map(str::to_owned)
                                .ok_or_else(|| self.invalid("'args' must be a list of texts"))
                        })
                        .collect::<Result<_, _>>()?,
                    _ => return Err(self.invalid("'args' must be a list of texts")),
                };
                let cwd = match &args["cwd"] {
                    Value::Null => None,
                    Value::String(text) => Some(text.as_str()),
                    _ => return Err(self.invalid("'cwd' must be text")),
                };
                let path = self.resolve_program(program)?;
                let cwd = self.resolve_cwd(cwd)?;
                let mut command = Command::new(path);
                command.args(arguments);
                self.execute(command, &cwd).await
            })
        }
    }
}
