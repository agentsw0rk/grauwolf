# ADR-0046: Built-in tools – files under a root, user input/output through a port

- **Status:** Accepted (User, 2026-11-07: "yes, go on", item 7 of the agent features)
- **Date:** 2026-11-07

## Context
Many agents need the same tools (F03.13, F03.14): tell the user something, ask the user, read/write/edit files, list directories, regex search, shell with confirmation. In grauwolf every tool so far has to be written as tool code in the spec; that code runs in the WASM sandbox without a file system.

## Decision
- New adapter crate `grauwolf-builtin-tools` with `DynTool`s:
  - Files (`FileTools::new(root)`): `read_file { path }`, `list_directory { path }`, `write_file { path, content }`, `edit_file { path, old, new }` (`old` must occur exactly once). Paths are relative to the root; absolute paths, `..` and symlinks that lead out of the root are rejected.
  - User: `say_to_user { message }`, `ask_user { question }` through the port `UserIo` (`say`, `ask`); the CLI implements it on the console (stderr/stdin), tests with fixed answers.
- Specs: section `builtin_tools { tools: [...], root }` (`root` relative to the spec, required for file tools). The names share the tool namespace; the interpreter requires them as external tools, the CLI provides them; the generator rejects the section for now.
- Not adopted: `Exit` (graphs end via edges), shell (too risky without an OS sandbox).
- Addendum (User, 2026-11-09: "then do regex search as a built-in tool"): `search_files { pattern, path?, max_results? }` searches text files under the root with the `regex` crate (linear runtime, no catastrophic patterns); symlinks are not followed, hidden directories, binary files and files over 1 MiB are skipped; at most 200 matches.

## Consequences
- Agents can work on files without writing tool code; the spec selects writing tools explicitly.
- The root is the only protection boundary; the tools run with the permissions of the process (like MCP tools, ADR-0036).

## Addendum (2026-11-09): generator and provisioning
The generator no longer rejects `builtin_tools`; the tools are external tools of the generated code, provided via `grauwolf::external::tools` in the facade instead of in the CLI (ADR-0050).
