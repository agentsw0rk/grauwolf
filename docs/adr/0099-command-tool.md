# ADR-0099: Opt-in command tool

- **Status:** Accepted (User, 2026-11-12: shell mode only explicitly; available everywhere with approval in the CLI; approval per call only in shell mode; AgentWeaver hardens the pods automatically; ADR-0092; revisits "shell" in ADR-0046)
- **Date:** 2026-11-10 (decided 2026-11-12)

## Context
ADR-0046 left out a shell tool: on a developer machine a model-controlled shell runs with the
user's permissions and without an OS sandbox. AgentWeaver offers a shell tool
(`executeCommand(command)` in a working directory) and deploys agents as pods. A pod is an isolation
boundary of its own (own file system, user, network policy), so the risk is different there – but a
command tool can still read secrets from the environment, reach cluster-internal services and
destroy the pod's data.

## Decision
- **New built-in tool `run_command`**, only when the spec configures it explicitly (section
  `builtin_tools.command`, required for the tool; `root` required):
  ```json
  "builtin_tools": {
    "tools": ["run_command"],
    "root": "work",
    "command": {
      "allowed": ["git", "ls", "cat", "python3"],
      "timeout_secs": 60,
      "max_output_bytes": 65536,
      "env": ["PATH", "HOME", "LANG"]
    }
  }
  ```
- **Default: allowlist mode.** Arguments `{ program, args[], cwd? }`; the program is started directly
  (no `sh -c`, no pipes, globbing or variables), only programs from `allowed` (bare name looked up in
  the `PATH` of the cleared environment, or an absolute path), `cwd` relative to and inside `root`,
  environment cleared except the listed variables (no API keys, no tokens), stdin closed,
  stdout/stderr captured and truncated to `max_output_bytes` (with a note), the whole process group
  killed after `timeout_secs` (default 60). Result text: exit code (or signal / timeout) and output.
  `allowed` must not be empty and must not contain `*` in this mode.
- **Shell mode, only explicitly:** `"shell": true` together with `"allowed": ["*"]` and
  `"i_understand_shell": true`; any other combination is a validation error. Arguments
  `{ command }`, run with `sh -c` in `root` with the same cleared environment, timeout and output
  limit. `spec validate` (and every command that loads the spec) prints a warning for shell mode.
  This is the mapping target for AgentWeaver's shell tool.
- **Approval:** in allowlist mode no approval by default – the list is the approval. In shell mode
  every call needs an approval (ADR-0061): the CLI (`spec run`, `spec chat`) asks on the console
  before each call; a service (`spec serve`, generated services) asks through `/approvals`; a run
  without an approver refuses the call with a clear text. Unattended services may switch this off
  explicitly with `"approve_each_call": false` (default `true` in shell mode, ignored in allowlist
  mode). Interceptors and `--confirm-tools` work as for every tool.
- **Available everywhere:** CLI/interpreter on developer machines, `spec serve` and generated
  services; the protection comes from the explicit configuration, the cleared environment, the
  allowlist and – in shell mode – the per-call approval.
- **Container hardening:** grauwolf documents the recommended pod settings for agents with this tool:
  non-root user, read-only root file system with a writable `emptyDir` at the root,
  `automountServiceAccountToken: false`, no privilege escalation, dropped capabilities, a
  NetworkPolicy that allows only the model endpoint and listed hosts. **AgentWeaver sets them
  automatically** in the manifests of every harness whose spec uses `run_command`.
- **Mapping from AgentWeaver:** `shell` with `workingDir` → `run_command` in shell mode with
  `root = workingDir`, `"allowed": ["*"]`, `"i_understand_shell": true`; the editor shows the risk
  and offers the allowlist mode (ask for the allowed programs) as the safer alternative.
- Unix only (process groups, `sh`); on other platforms the tool refuses to start with a clear error.

## Consequences
- Agents that need command-line programs become possible without custom tool code; the default is
  restrictive and explicit, the open shell needs three deliberate settings and a human per call.
- The command tool is the only built-in tool with process-level side effects; its tests need a
  POSIX system (skipped elsewhere).
- AgentWeaver gets a second hardening rule set for grauwolf harnesses with commands.

## Implementation notes (2026-11-12)
- **Crate:** `grauwolf-tools-command` (adapter), not a module of `grauwolf-builtin-tools`: it is the
  only tool that spawns processes and needs `libc` for the group kill, and ADR-0108 keeps the light
  tools free of such concerns; the facade and host feature is `command` (in `default`; a spec with
  `run_command` in a build without it fails before the run; generated crates enable it when the spec
  uses the tool). The path rules for `cwd` are the file tools' rules, written out again in the crate
  (adapters do not depend on each other).
- **Environment:** `env` defaults to `["PATH"]`; values come from the process environment at start;
  the `PATH` among them is where bare names are looked up (an absolute entry of `allowed` must be
  given exactly). Limits: `timeout_secs` 1–3600, `max_output_bytes` 1–10 MiB, applied per stream
  (stdout and stderr each).
- **Approval:** no new mechanism. When `builtin_tools.command` is in shell mode with
  `approve_each_call`, the interpreter adds `run_command` to the approval-required tools of every
  agent it loads (like `self_tools.confirm`): ADR-0061's `ApprovalGate` rejects the call without an
  approver, `spec run|chat|resume` use the console approver (`The agent wants to run the shell
  command …`), `spec serve` the `/approvals` stream (tested in `grauwolf-server`'s `tests/approvals.rs`: the command runs only once approved, not when refused). `ConfirmTools` and `--confirm-tools` still work
  on top. Sub-agents and non-streaming paths have no approver and therefore refuse the shell.
- **Generated crates** (ADR-0119): the refusal is gone. Generated code installs the same
  `ApprovalGate` for `AgentSpec::approval_tools()`; without an approver in `RunOptions` the call is
  refused with the interpreter's text, and a generated service asks through `/approvals` like
  `spec serve`.
- **Warning:** `warnings()` reports `/builtin_tools/command/shell` for shell mode.
- **AgentWeaver hardening (not part of this repository):** for every harness whose spec uses
  `run_command` AgentWeaver must set a non-root user, a read-only root file system with a writable
  `emptyDir` at the root, `automountServiceAccountToken: false`, `allowPrivilegeEscalation: false`,
  dropped capabilities and a NetworkPolicy that allows only the model endpoint and the listed hosts.
- **Live check (2026-11-12):** `ls`/`wc` allowlist on Ollama `qwen3.5:9b`, "How many lines does
  README.md have?": the first call had a malformed `args` (a string) and came back as
  `invalid arguments … 'args' must be a list of texts`; the model corrected it to
  `{ program: wc, args: ["README.md"] }` and answered "README.md has 37 lines." (result
  `exit code: 0`, `37 74 287 README.md`).
