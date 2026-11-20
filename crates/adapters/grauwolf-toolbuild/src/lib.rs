//! Builds tool code from specs into WASM modules following tool ABI v1 (ADR-0024, ADR-0029).
//!
//! Each tool becomes a small guest crate (`cdylib` for `wasm32-unknown-unknown`) that
//! embeds the code from the spec in `fn tool(args: Value) -> Result<String, String>` and
//! provides the ABI exports `gw_alloc`/`gw_call`. cargo builds it in a shared target
//! directory under the cache directory.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use grauwolf_spec::{CodePiece, CodePieceKind, ToolSpec};
use grauwolf_tools::{ToolArtifact, ToolSourceCrate, ToolSourceKind};

mod code;

/// Error while building a tool.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// File system or process error.
    #[error("I/O error during tool build: {0}")]
    Io(#[from] std::io::Error),
    /// `rustc` or `cargo` (or the wrapper) cannot be started.
    #[error(
        "Rust toolchain not found: cannot start `{program}` – install rustup, \
         extend PATH or set CARGO/RUSTC"
    )]
    ToolchainMissing { program: String },
    /// The tool code contains a construct that could access the host while compiling or
    /// bypass the sandbox guarantees (F13.15).
    #[error("tool '{tool}' uses the forbidden construct `{construct}`")]
    ForbiddenCode { tool: String, construct: String },
    /// The tool code is not a valid Rust token stream.
    #[error("tool '{tool}' contains invalid Rust code: {reason}")]
    InvalidCode { tool: String, reason: String },
    /// `prepare()` has not run yet; without network access the dependencies are missing.
    #[error("tool dependencies are not prepared – run `ToolBuilder::prepare()` first")]
    NotPrepared,
    /// Resolving or loading the dependencies failed.
    #[error("preparing the tool dependencies failed:\n{output}")]
    Prepare { output: String },
    /// The build exceeded the time limit and was aborted.
    #[error("build of tool '{tool}' aborted after {limit:?} (time limit)")]
    Timeout { tool: String, limit: Duration },
    /// The tool code does not compile. `diagnostics` refer to lines of the tool code in the
    /// spec (F13.9); `output` is the complete compiler output.
    #[error("tool '{tool}' does not compile:\n{}", render_diagnostics(.diagnostics, .output))]
    Compile {
        tool: String,
        diagnostics: Vec<CodeDiagnostic>,
        output: String,
    },
}

/// A compiler message about the tool code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeDiagnostic {
    /// `error`, `warning`, …
    pub level: String,
    /// Compiler message.
    pub message: String,
    /// Line in the spec's tool code (from 1); `None` if the location is outside the tool code.
    pub line: Option<usize>,
    /// Column in this line (from 1).
    pub column: Option<usize>,
}

/// Line of the generated `src/lib.rs` where the tool code begins (see `guest_source`).
const CODE_FIRST_LINE: usize = 6;

/// Compiler errors from cargo's JSON output (`--message-format=json`); lines are mapped
/// back to the tool code.
fn parse_diagnostics(messages: &str) -> Vec<CodeDiagnostic> {
    messages
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|message| message["reason"] == "compiler-message")
        .map(|message| message["message"].clone())
        .filter(|message| message["level"] == "error")
        .map(|message| {
            let primary = message["spans"]
                .as_array()
                .and_then(|spans| spans.iter().find(|span| span["is_primary"] == true));
            let position = primary.and_then(|span| {
                let line = span["line_start"].as_u64()? as usize;
                let column = span["column_start"].as_u64()? as usize;
                (span["file_name"] == "src/lib.rs" && line >= CODE_FIRST_LINE)
                    .then(|| (line - CODE_FIRST_LINE + 1, column))
            });
            CodeDiagnostic {
                level: message["level"].as_str().unwrap_or("error").to_owned(),
                message: message["message"].as_str().unwrap_or_default().to_owned(),
                line: position.map(|(line, _)| line),
                column: position.map(|(_, column)| column),
            }
        })
        .collect()
}

/// Readable form for error messages: diagnostics with position, otherwise the compiler output.
fn render_diagnostics(diagnostics: &[CodeDiagnostic], output: &str) -> String {
    if diagnostics.is_empty() {
        return output.to_owned();
    }
    diagnostics
        .iter()
        .map(|diagnostic| match (diagnostic.line, diagnostic.column) {
            (Some(line), Some(column)) => format!(
                "line {line}, column {column}: {}: {}",
                diagnostic.level, diagnostic.message
            ),
            _ => format!("{}: {}", diagnostic.level, diagnostic.message),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Target triple of the tool modules.
const TARGET: &str = "wasm32-unknown-unknown";

/// Builds tools in a cache directory.
#[derive(Debug, Clone)]
pub struct ToolBuilder {
    cache_dir: PathBuf,
    timeout: Duration,
    wrapper: Vec<String>,
}

/// Default time limit for a tool build.
pub const DEFAULT_BUILD_TIMEOUT: Duration = Duration::from_secs(300);

impl ToolBuilder {
    /// Builder that stores guest crates and build outputs under `cache_dir`.
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            cache_dir: cache_dir.into(),
            timeout: DEFAULT_BUILD_TIMEOUT,
            wrapper: Vec::new(),
        }
    }

    /// The cache directory; tool packages (ADR-0107) are cached below it, too.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Starts cargo under `wrapper`, a command prefix for OS-level isolation, e.g.
    /// `bwrap …` or `docker run --network none …` (ADR-0030).
    /// The wrapper gets cargo and its arguments appended.
    pub fn with_wrapper(mut self, wrapper: Vec<String>) -> Self {
        self.wrapper = wrapper;
        self
    }

    /// `program`, if configured under the wrapper.
    fn command(&self, program: &str) -> Command {
        match self.wrapper.split_first() {
            None => Command::new(program),
            Some((wrapper, args)) => {
                let mut command = Command::new(wrapper);
                command.args(args).arg(program);
                command
            }
        }
    }

    /// Aborts builds that run longer than `timeout` (ADR-0030).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Artifact for `tool` from the cache, if it was already built.
    pub fn cached(&self, tool: &ToolSpec) -> Result<Option<ToolArtifact>, BuildError> {
        self.cached_guest(&tool_guest(tool))
    }

    /// Artifact for the code piece from the cache, if it was already built (ADR-0102).
    pub fn cached_code(&self, piece: &CodePiece) -> Result<Option<ToolArtifact>, BuildError> {
        self.cached_guest(&piece_guest(piece)?)
    }

    fn cached_guest(&self, guest: &Guest) -> Result<Option<ToolArtifact>, BuildError> {
        let hash = self.cache_key(guest)?;
        match std::fs::read(self.artifact_path(&hash)) {
            Ok(bytes) => Ok(Some(ToolArtifact {
                name: guest.name.clone(),
                bytes,
                hash,
            })),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Builds `tool` into a WASM module; already built artifacts come from the cache.
    pub fn build(&self, tool: &ToolSpec) -> Result<ToolArtifact, BuildError> {
        self.build_guest(&tool_guest(tool))
    }

    /// Builds the code of a graph (a `code` node, an `on_code` condition or an edge
    /// transformation, ADR-0102) into a WASM module exactly like a tool: the same code check,
    /// isolated build, cache and tool ABI v1.
    pub fn build_code(&self, piece: &CodePiece) -> Result<ToolArtifact, BuildError> {
        self.build_guest(&piece_guest(piece)?)
    }

    fn build_guest(&self, guest: &Guest) -> Result<ToolArtifact, BuildError> {
        check_source(&guest.name, &guest.code)?;
        if let Some(artifact) = self.cached_guest(guest)? {
            return Ok(artifact);
        }
        let hash = self.cache_key(guest)?;
        let crate_name = format!("gw_tool_{}_{hash}", guest.name);
        // Own working directory (crate + target) per build, deleted afterwards: builds share no
        // state, not even two concurrent builds of the same tool, and cargo needs no locking
        // (flock is not always exclusive on mounted file systems).
        let crate_dir = self.unique_work_dir("builds", &crate_name);
        std::fs::create_dir_all(crate_dir.join("src"))?;
        std::fs::write(crate_dir.join("Cargo.toml"), guest_manifest(&crate_name))?;
        std::fs::write(crate_dir.join("src/lib.rs"), &guest.source)?;
        let bytes = self.compile(&crate_dir, &crate_name, &guest.name)?;
        let artifact_path = self.artifact_path(&hash);
        std::fs::create_dir_all(artifact_path.parent().unwrap_or(&self.cache_dir))?;
        // Via a unique temporary file and `rename`: readers never see half-written artifacts.
        let partial = self
            .unique_work_dir("artifacts", &hash)
            .with_extension("partial");
        std::fs::write(&partial, &bytes)?;
        std::fs::rename(&partial, artifact_path)?;
        Ok(ToolArtifact {
            name: guest.name.clone(),
            bytes,
            hash,
        })
    }

    /// Builds the guest crate in `crate_dir` (which holds its own `Cargo.toml` and `src/lib.rs`)
    /// and returns the module; `crate_dir` is deleted afterwards. Needs `prepare()`.
    fn compile(
        &self,
        crate_dir: &Path,
        crate_name: &str,
        tool: &str,
    ) -> Result<Vec<u8>, BuildError> {
        let lock = self.lock_path();
        if !lock.exists() {
            let _ = std::fs::remove_dir_all(crate_dir);
            return Err(BuildError::NotPrepared);
        }
        std::fs::copy(&lock, crate_dir.join("Cargo.lock"))?;
        let target_dir = crate_dir.join("target");
        let log = crate_dir.join("build.log");
        let messages = crate_dir.join("messages.json");
        let child = self
            .command(&cargo())
            .args([
                "build",
                "--release",
                "--quiet",
                "--message-format=json",
                "--offline",
                "--target",
                TARGET,
                "--manifest-path",
            ])
            .arg(crate_dir.join("Cargo.toml"))
            .env_clear()
            .envs(toolchain_env())
            .env("CARGO_TARGET_DIR", &target_dir)
            .stdout(std::fs::File::create(&messages)?)
            .stderr(std::fs::File::create(&log)?)
            .process_group(0)
            .spawn()
            .map_err(missing(self.wrapper.first().cloned().unwrap_or_else(cargo)));
        let mut child = match child {
            Ok(child) => child,
            Err(error) => {
                let _ = std::fs::remove_dir_all(crate_dir);
                return Err(error);
            }
        };
        let waited = wait_with_timeout(&mut child, self.timeout);
        let wasm = target_dir
            .join(TARGET)
            .join("release")
            .join(format!("{}.wasm", crate_name.replace('-', "_")));
        let bytes = match waited {
            Err(error) => Err(error.into()),
            Ok(None) => Err(BuildError::Timeout {
                tool: tool.to_owned(),
                limit: self.timeout,
            }),
            Ok(Some(status)) if !status.success() => Err(BuildError::Compile {
                tool: tool.to_owned(),
                diagnostics: parse_diagnostics(
                    &std::fs::read_to_string(&messages).unwrap_or_default(),
                ),
                output: std::fs::read_to_string(&log).unwrap_or_default(),
            }),
            Ok(Some(_)) => std::fs::read(wasm).map_err(BuildError::from),
        };
        let _ = std::fs::remove_dir_all(crate_dir);
        bytes
    }

    /// Builds a guest crate written by [`guest_crate`] (ADR-0095), e.g. read from the `tools/`
    /// directory of a generated crate. Not cached: the caller decides when to build. Needs
    /// `prepare()`; the code was checked when the sources were written (`guest_crate`), and the
    /// caller has verified the hash.
    pub fn build_sources(&self, tool: &ToolSourceCrate) -> Result<ToolArtifact, BuildError> {
        let crate_name = guest_crate_name(&tool.name);
        let crate_dir = self.unique_work_dir("builds", &crate_name);
        std::fs::create_dir_all(crate_dir.join("src"))?;
        std::fs::write(crate_dir.join("Cargo.toml"), &tool.manifest)?;
        std::fs::write(crate_dir.join("src/lib.rs"), &tool.source)?;
        let bytes = self.compile(&crate_dir, &crate_name, &tool.name)?;
        Ok(ToolArtifact {
            name: tool.name.clone(),
            bytes,
            hash: tool.hash.clone(),
        })
    }

    /// Resolves and downloads the dependencies of the guest crates (requires network). Runs
    /// without tool code, before foreign code is involved; afterwards tools build offline
    /// (ADR-0030). Idempotent: if already prepared, nothing happens.
    pub fn prepare(&self) -> Result<(), BuildError> {
        if self.lock_path().exists() {
            return Ok(());
        }
        // Own working directory per call: parallel `prepare()` calls do not interfere.
        let dir = self.unique_work_dir("prepare", "gw_tool_prepare");
        std::fs::create_dir_all(dir.join("src"))?;
        std::fs::write(dir.join("Cargo.toml"), guest_manifest("gw_tool_prepare"))?;
        std::fs::write(dir.join("src/lib.rs"), "")?;
        for args in [
            &["generate-lockfile"][..],
            &["fetch", "--target", TARGET][..],
        ] {
            let output = Command::new(cargo())
                .args(args)
                .arg("--manifest-path")
                .arg(dir.join("Cargo.toml"))
                .env_clear()
                .envs(toolchain_env())
                .output()
                .map_err(missing(cargo()))?;
            if !output.status.success() {
                return Err(BuildError::Prepare {
                    output: String::from_utf8_lossy(&output.stderr).into_owned(),
                });
            }
        }
        // `rename` is atomic: other builds see either no lock or a complete one.
        std::fs::rename(dir.join("Cargo.lock"), self.lock_path())?;
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// Path `<cache>/<area>/<name>-<pid>-<thread>-<nanos>`, unique per call.
    fn unique_work_dir(&self, area: &str, name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let thread = format!("{:?}", std::thread::current().id());
        let thread: String = thread.chars().filter(char::is_ascii_digit).collect();
        self.cache_dir
            .join(area)
            .join(format!("{name}-{}-{thread}-{nanos}", std::process::id()))
    }

    /// Shared `Cargo.lock` of all guest crates; exists after `prepare()`.
    fn lock_path(&self) -> PathBuf {
        self.cache_dir.join("Cargo.lock")
    }

    fn artifact_path(&self, hash: &str) -> PathBuf {
        self.cache_dir
            .join("artifacts")
            .join(format!("{hash}.wasm"))
    }
}

/// Version of the guest template; changes with `guest_source`, `code::piece_source` and the ABI glue.
const GUEST_TEMPLATE_VERSION: &str = "abi-v1.2";

/// What is built: a tool or a code piece as guest source.
struct Guest {
    /// Name of the tool or code piece.
    name: String,
    kind: ToolSourceKind,
    /// The user's code, which is checked for forbidden constructs.
    code: String,
    /// Content of `src/lib.rs`.
    source: String,
    /// What else enters the cache key besides the source.
    extra: String,
}

fn tool_guest(tool: &ToolSpec) -> Guest {
    Guest {
        name: tool.name.clone(),
        kind: ToolSourceKind::Tool,
        code: tool.code.clone(),
        source: guest_source(tool),
        extra: tool.args.to_string(),
    }
}

fn piece_guest(piece: &CodePiece) -> Result<Guest, BuildError> {
    Ok(Guest {
        name: piece.name.clone(),
        kind: match piece.kind {
            CodePieceKind::Node => ToolSourceKind::Code,
            CodePieceKind::Condition => ToolSourceKind::Condition,
            CodePieceKind::Transform => ToolSourceKind::Transform,
        },
        code: piece.code.clone(),
        source: code::piece_source(piece)?,
        extra: String::new(),
    })
}

/// File in the cache directory that stands in for `rustc --version` where there is no
/// toolchain (the image of a generated service, ADR-0115): [`ToolBuilder::export_toolchain`]
/// writes it, so that the artifacts built on the build machine are found by their cache key.
pub const TOOLCHAIN_FILE: &str = "toolchain";

impl ToolBuilder {
    /// Writes the version of the toolchain that builds the tools into the cache directory
    /// `cache_dir` (as the file [`TOOLCHAIN_FILE`]), for a copy of the cache on a machine without
    /// a toolchain: it finds the artifacts of this one by their cache key.
    pub fn export_toolchain(&self, cache_dir: &Path) -> Result<(), BuildError> {
        let version = self.toolchain_version()?;
        std::fs::create_dir_all(cache_dir)?;
        std::fs::write(cache_dir.join(TOOLCHAIN_FILE), version)?;
        Ok(())
    }

    /// The version of the toolchain that builds the tools (`rustc 1.99.1 (…)`), if it can be
    /// found; recorded in tool packages (ADR-0124).
    pub fn rustc_version(&self) -> Option<String> {
        let version = self.toolchain_version().ok()?;
        let version = String::from_utf8_lossy(&version).trim().to_owned();
        (!version.is_empty()).then_some(version)
    }

    /// `rustc --version`; without `rustc` the content of [`TOOLCHAIN_FILE`] in the cache
    /// directory, if there is one.
    fn toolchain_version(&self) -> Result<Vec<u8>, BuildError> {
        let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
        self.toolchain_version_of(rustc)
    }

    fn toolchain_version_of(&self, rustc: String) -> Result<Vec<u8>, BuildError> {
        match Command::new(&rustc).arg("--version").output() {
            Ok(output) => Ok(output.stdout),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::read(self.cache_dir.join(TOOLCHAIN_FILE))
                    .map_err(|_| BuildError::ToolchainMissing { program: rustc })
            }
            Err(error) => Err(BuildError::Io(error)),
        }
    }

    /// Cache key: hash of guest source, template version and toolchain version.
    fn cache_key(&self, guest: &Guest) -> Result<String, BuildError> {
        use std::hash::{Hash, Hasher};

        let toolchain = self.toolchain_version()?;
        let mut hasher = std::hash::DefaultHasher::new();
        (
            GUEST_TEMPLATE_VERSION,
            &guest.name,
            &guest.source,
            &guest.extra,
            toolchain,
        )
            .hash(&mut hasher);
        Ok(format!("{:016x}", hasher.finish()))
    }
}

/// Name of the guest crate of the tool `name` when it is written as source (ADR-0095).
fn guest_crate_name(name: &str) -> String {
    format!("gw_tool_{name}")
}

/// The guest crate of `tool` as text, for `spec generate --tool-sources` (ADR-0095): exactly
/// what [`ToolBuilder::build`] writes and compiles, with the hash that `grauwolf tools build`
/// checks. The code is checked for forbidden constructs here (F13.15); no toolchain is needed.
pub fn guest_crate(tool: &ToolSpec) -> Result<ToolSourceCrate, BuildError> {
    source_crate(&tool_guest(tool))
}

/// The guest crate of a code piece (ADR-0102) as text, like [`guest_crate`]: the sources that
/// `spec generate --tool-sources` writes under `tools/<name>/`.
pub fn code_guest_crate(piece: &CodePiece) -> Result<ToolSourceCrate, BuildError> {
    source_crate(&piece_guest(piece)?)
}

fn source_crate(guest: &Guest) -> Result<ToolSourceCrate, BuildError> {
    check_source(&guest.name, &guest.code)?;
    let manifest = guest_manifest(&guest_crate_name(&guest.name));
    let source = guest.source.clone();
    let hash = source_hash(&manifest, &source);
    Ok(ToolSourceCrate {
        name: guest.name.clone(),
        kind: guest.kind,
        manifest,
        source,
        hash,
    })
}

/// SHA-256 of the two files of a guest crate, `sha256:<hex>`; every file enters with its path
/// and length, so content cannot move from one file to the other unnoticed.
pub fn source_hash(manifest: &str, source: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    for (path, content) in [("Cargo.toml", manifest), ("src/lib.rs", source)] {
        hasher.update(format!("{path}\n{}\n", content.len()));
        hasher.update(content.as_bytes());
    }
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{digest}")
}

/// `Cargo.toml` of a guest crate; the dependencies are the same for all tools (v1: only
/// `serde_json`), so a shared `Cargo.lock` pins them.
fn guest_manifest(crate_name: &str) -> String {
    format!(
        r#"[package]
name = "{crate_name}"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
serde_json = "1"

[profile.release]
opt-level = "s"
panic = "abort"

[workspace]
"#
    )
}

/// Source of the guest crate: tool code plus ABI glue (ADR-0029).
fn guest_source(tool: &ToolSpec) -> String {
    format!(
        "//! Generated from the spec tool `{name}` – do not edit by hand.\n\
         \n\
         use serde_json::Value;\n\
         \n\
         fn tool(args: Value) -> Result<String, String> {{\n\
         {code}\n\
         }}\n\
         {ABI_GLUE}",
        name = tool.name,
        code = tool.code
    )
}

/// The ABI exports of tool ABI v1 around `fn tool(args: Value) -> Result<String, String>`.
const ABI_GLUE: &str = r#"
/// Reserves `len` bytes for the input.
#[unsafe(no_mangle)]
pub extern "C" fn gw_alloc(len: i32) -> i32 {
    let mut buffer = Vec::<u8>::with_capacity(len as usize);
    let pointer = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    pointer as i32
}

/// Runs the tool; returns `(ptr << 32) | len` of a JSON `{"ok": …}` or `{"err": …}`.
#[unsafe(no_mangle)]
pub extern "C" fn gw_call(pointer: i32, len: i32) -> i64 {
    // SAFETY: `pointer`/`len` come from `gw_alloc` and were filled in by the host.
    let input = unsafe { Vec::from_raw_parts(pointer as *mut u8, len as usize, len as usize) };
    let result = match serde_json::from_slice::<Value>(&input) {
        Ok(args) => tool(args),
        Err(error) => Err(format!("invalid arguments: {error}")),
    };
    let output = match result {
        Ok(text) => serde_json::json!({ "ok": text }),
        Err(error) => serde_json::json!({ "err": error }),
    };
    let bytes = output.to_string().into_bytes().into_boxed_slice();
    let len = bytes.len() as i64;
    let pointer = Box::into_raw(bytes) as *mut u8 as i64;
    (pointer << 32) | len
}
"#;

/// cargo binary: the calling cargo, if present.
fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

/// A program that cannot be started becomes [`BuildError::ToolchainMissing`]; other errors
/// remain I/O errors.
fn missing(program: String) -> impl FnOnce(std::io::Error) -> BuildError {
    move |error| match error.kind() {
        std::io::ErrorKind::NotFound => BuildError::ToolchainMissing { program },
        _ => BuildError::Io(error),
    }
}

/// Macros that read host files or environment variables while compiling.
const FORBIDDEN_MACROS: &[&str] = &[
    "include",
    "include_str",
    "include_bytes",
    "env",
    "option_env",
];

/// Keywords that load files (`mod`) or weaken sandbox guarantees.
const FORBIDDEN_KEYWORDS: &[&str] = &["mod", "extern", "unsafe"];

/// Rejects tool code with forbidden constructs before it is compiled (F13.15).
fn check_source(name: &str, code: &str) -> Result<(), BuildError> {
    let tokens: proc_macro2::TokenStream =
        code.parse().map_err(|error: proc_macro2::LexError| {
            // Position relative to the tool code, so that humans and LLMs can find the spot.
            let start = error.span().start();
            BuildError::InvalidCode {
                tool: name.to_owned(),
                reason: format!("line {}, column {}: {error}", start.line, start.column + 1),
            }
        })?;
    match find_forbidden(tokens) {
        Some(construct) => Err(BuildError::ForbiddenCode {
            tool: name.to_owned(),
            construct,
        }),
        None => Ok(()),
    }
}

/// First forbidden construct in the token stream, including inside nested groups.
fn find_forbidden(tokens: proc_macro2::TokenStream) -> Option<String> {
    use proc_macro2::TokenTree;

    let tokens: Vec<TokenTree> = tokens.into_iter().collect();
    for (index, token) in tokens.iter().enumerate() {
        match token {
            TokenTree::Punct(punct) if punct.as_char() == '#' => return Some("#[…]".into()),
            TokenTree::Ident(ident) => {
                let name = ident.to_string();
                let is_macro = matches!(
                    tokens.get(index + 1),
                    Some(TokenTree::Punct(next)) if next.as_char() == '!'
                );
                if is_macro && FORBIDDEN_MACROS.contains(&name.as_str()) {
                    return Some(format!("{name}!"));
                }
                if FORBIDDEN_KEYWORDS.contains(&name.as_str()) {
                    return Some(name);
                }
            }
            TokenTree::Group(group) => {
                if let Some(found) = find_forbidden(group.stream()) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

/// Waits for `child`; after `timeout` the process is killed and `None` is returned.
fn wait_with_timeout(child: &mut Child, timeout: Duration) -> std::io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            kill_process_group(child)?;
            child.wait()?;
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Kills cargo along with all child processes (rustc, linker): cargo runs in its own
/// process group, whose ID equals the process ID.
fn kill_process_group(child: &mut Child) -> std::io::Result<()> {
    let status = Command::new("kill")
        .args(["-KILL", "--", &format!("-{}", child.id())])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        child.kill()
    }
}

/// Variables the build inherits from the environment: only what the toolchain needs to
/// start. Everything else (secrets, `RUSTFLAGS`, proxy settings …) stays out (ADR-0030).
const TOOLCHAIN_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
];

fn toolchain_env() -> impl Iterator<Item = (&'static str, std::ffi::OsString)> {
    TOOLCHAIN_ENV
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (*name, value)))
}

#[cfg(test)]
mod toolchain_file_tests {
    use super::*;

    fn dir(name: &str) -> grauwolf_test::TestDir {
        grauwolf_test::TestDir::new(&format!("toolchain-{name}"))
    }

    #[test]
    fn without_rustc_the_exported_toolchain_file_stands_in_for_its_version() {
        let cache = dir("copy");
        let source = dir("source");
        let exporting = ToolBuilder::new(&source);
        exporting.export_toolchain(&cache).unwrap();
        let exported = std::fs::read(cache.join(TOOLCHAIN_FILE)).unwrap();
        assert!(exported.starts_with(b"rustc"), "{exported:?}");

        let copy = ToolBuilder::new(&cache);
        let version = copy
            .toolchain_version_of("/no/such/rustc".to_owned())
            .unwrap();

        assert_eq!(version, exported);
    }

    #[test]
    fn without_rustc_and_without_the_file_the_toolchain_is_missing() {
        let empty = dir("empty");
        let builder = ToolBuilder::new(&empty);

        let error = builder
            .toolchain_version_of("/no/such/rustc".to_owned())
            .unwrap_err();

        assert!(
            matches!(error, BuildError::ToolchainMissing { .. }),
            "{error:?}"
        );
    }
}
