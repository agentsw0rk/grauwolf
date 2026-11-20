# grauwolf-builtin-tools

**Layer:** `adapters` · **Status:** `think_tool()` (`think { thought }` → `ok`; ADR-0098); `KvTools::new(path).tools(names)` (`store_value`, `retrieve_value`, `list_keys` over one JSON object file; keys 1–256 bytes, values ≤ 64 KiB; atomic write via temp file + rename; one process-wide lock per file; messages `Stored key: k`, `Key not found: k`, `No keys stored`); `todo_tools(names)` (`add_task { title }`, `complete_task { id }`, `remove_task { id }`, `list_tasks {}`; the list is a tool with a state: `DynTool::state_key()` = `TODO_STATE_KEY` (`grauwolf.todo`) and `call_with_state`, so the agent context keeps it in the agent storage; ids stable and never reused; messages like AgentWeaver's todo tool) (ADR-0098). `FileTools::new(root)` (the root is canonicalized) provides `DynTool`s via `tools(names)`: `read_file { path }`, `list_directory { path }` (sorted, directories with `/`, root = `.`), `write_file { path, content }` (creates directories), `edit_file { path, old, new }` (`old` must occur exactly once, otherwise an error with the count), `search_files { pattern, path?, max_results? }` (regex via `regex`, recursive from `path` (default `.`), hits `path:line: text` sorted, default 50, at most 200, a note when truncated; hidden entries, symlinks, non-UTF-8 files, and files > 1 MiB are skipped; invalid pattern → `InvalidArgs`; addendum ADR-0046). Paths are relative to the root; absolute paths, `..`, and symlinks that leave the root (including via parent directories of targets that do not exist yet, `canonicalize_existing`) → `ToolError::Execution`. `UserTools::new(io).tools(names)`: `say_to_user { message }` (→ "shown"), `ask_user { question }` via the port `UserIo` (`say`, `ask`); `ConsoleUserIo` writes to stderr and reads answers line by line from stdin.


Skills (ADR-0103, F14.16): `SkillLibrary::load(&[SkillRoot { dir, names }])` reads and checks the skill folders (`SKILL.md` with front matter `name` = folder name and `description`, unique names across the roots, every file ≤ `MAX_SKILL_FILE_BYTES` = 1 MiB, no symlink leaving the skill folder; errors name the file); `tools()` gives `load_skill { name }` (instructions without front matter plus the list of the other files) and `read_skill_file { name, path }` (text only, path checked against escapes at call time, binary and large files refused, unknown skill → error listing the known ones), read-only, scripts are never run; `prompt_block()` is the system prompt block, which `load_skill` returns as its `DynTool::prompt_addition`.

## Purpose
Built-in tools (E03 F03.13, F03.14): files under a root, key-value store, to-do list, think, user input/output. The git tools live in `grauwolf-tools-git`, the `fetch` tool in `grauwolf-tools-web` (ADR-0108, stage 3a).

## Features / ADRs
E03 F03.13–F03.14, ADR-0046; E14 F14.8, ADR-0098; ADR-0108

## Allowed dependencies
- grauwolf crates: grauwolf-prompt, grauwolf-tools
- External: futures-util, serde, regex, serde_json, tokio (fs, io-std, io-util, rt, sync, time); dev: tokio. Deliberately no `git2` and no `reqwest` (ADR-0108: they live in `grauwolf-tools-git` and `grauwolf-tools-web`, so the light tools build without libgit2 and an HTTP stack).

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- No file access outside the root; every path goes through `resolve`.
- Tools run outside the WASM sandbox, with the process's privileges.

## Tests
- Against directories under `target/builtin-tools-tests`.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-builtin-tools`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
