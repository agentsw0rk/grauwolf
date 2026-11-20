# grauwolf-tools-command

**Layer:** `adapters` · **Status:** `command_tool(root, CommandConfig)` (ADR-0099, F14.9) provides the tool `run_command` (`COMMAND_TOOLS`). `CommandConfig { allowed, shell, timeout (60 s), max_output_bytes (65536, per stream), env: Vec<(name, value)> }`. Allowlist mode: `{ program, args?[], cwd? }`; the program must be listed (bare name looked up in the `PATH` value of `env`, or an absolute path listed exactly), started directly (no shell, args passed as they are), `cwd` relative to and inside the root (no absolute path, no `..`, symlinks resolved and checked), environment cleared except `env`, stdin null, stdout and stderr captured and cut at `max_output_bytes` with a note `[output truncated: N more bytes cut off after M bytes]`. Shell mode: `{ command }` run with `sh -c` in the root with the same limits. The program runs in its own process group (`process_group(0)`); after the timeout, and after a normal exit (output readers get 500 ms), the whole group is killed with `SIGKILL`. Result text: `exit code: N` / `terminated by signal N` / `timed out after N s and was killed`, then `stdout:` and `stderr:` sections when not empty. Refusals (`program 'x' is not allowed; allowed: …`, `cwd '…' leads outside the root`, not found in PATH) are `ToolError::Execution`, a missing or mistyped argument `ToolError::InvalidArgs`. Unix only: elsewhere `command_tool` returns an error.

## Purpose
The only tool with process-level side effects (E14 F14.9, ADR-0099), in its own crate so that the light `grauwolf-builtin-tools` stays free of process spawning (ADR-0108) and builds without the facade feature `command` can leave it out.

## Features / ADRs
E14 F14.9, ADR-0099; ADR-0108

## Allowed dependencies
- grauwolf crates: grauwolf-prompt, grauwolf-tools
- External: futures-util, serde_json, tokio (io-util, process, rt, time), libc (Unix only, for `kill(-pgid)`); dev: tokio

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- Nothing runs unless the program is listed (allowlist mode) or shell mode was configured explicitly; the spec validation guarantees the three explicit settings of shell mode, the approval per call is the host's job (interpreter approval gate, ADR-0061).
- The environment of the program is exactly `CommandConfig::env`: no API keys or tokens leak unless listed.
- No process outlives its call: the group is killed on timeout and after exit.
- The tool runs outside the WASM sandbox with the process's privileges.

## Tests
- `tests/command.rs` (Unix only) with directories under cargo's `CARGO_TARGET_TMPDIR`; they use `ls`, `cat`, `echo`, `env`, `wc`, `sleep`, `sh`.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-tools-command`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
