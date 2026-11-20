# grauwolf-agent-tree

**Layer:** `adapters` · **Status:** the prepared agent tree of ADR-0115, moved out of the host so that `grauwolf-host` and `grauwolf-tools-app` share one read phase (ADR-0118). `AgentTree::read(file, &ReadOptions)` / `read_spec(spec, base_dir, &ReadOptions)` is the read phase over a spec and its local sub-agents (local, no network): every spec read once with `tool_files` and `code_file` inlined and validated, warnings in `tree.warnings()`, a sub-spec under several parents is one node (`Child::Local { node }` is the same index), cycles are errors naming the chain, sub-specs outside the root's directory (`..` out of it or absolute) are errors, `Child::Remote` (A2A) is a leaf, per node `companions()` (`grauwolf_generator::companion_paths`, only below the spec's directory) and the sub-agents' signatures are checked; `ReadErrors` collects all errors as `<file>:<pointer>: …`. **`ReadOptions` plugs in what only the program knows:** `providers: Option<Vec<String>>` (the model providers it supports; `None` skips the check – the host passes its built-ins, `grauwolf-tools` does not run agents and passes none) and `skill_dirs` (check the local `skills` directories like loading them does; on by default, both programs do). The resolve phase `tree.resolve(&ResolveOptions { builder, locked, config, build_code, resolve_root })` localises git skills, resolves tool packages (`grauwolf_toolpkg::tree_lock::lock_tree`) and builds code tools once per node into `node.resolution()`; `copy_into(app, builder)` copies a resolved tree into an image's `app/`. Also the spec reading the tree is made of: `read_spec(file)` (reads without validating), `warning_lines`, `resolve_code_file`, `build_tools(file, spec, builder)`, and `skills::{check_local, replace_with_dir}`. The host's `agent_tree`, `loader` and `skills` modules re-export all of it. Feature `packages` enables `grauwolf-toolpkg`; without it a spec with `tool_packages` or git `skills` is refused when it is resolved.

`resolve_file_references(document, base)` resolves `tool_files` and every `code_file` of a spec document in memory (as the reader does for a file in `base`); grauwolf studio validates edited specs with it (ADR-0126). `file_references(document, base)` lists every file a spec document names, existing or not, with its JSON pointer and `ReferenceKind`: the code files and tool files by the same walk as the resolution (`walk_code_files` with a `CodeFiles` visitor: `Resolver` inlines, `Lister` records and puts empty code in place so the spec can still load; the `code_file` of a tool file is `/tool_files/<i>/code_file`, relative to the tool file), then – if the spec loads – `tools_dir`, local sub-agent specs (`/agent_tools/<name>/spec`) and `grauwolf_generator::companions` (URLs left out; `CompanionKind` is re-exported); grauwolf studio shows them and offers to create missing ones.

## Purpose
One walk over an agent tree for every program that reads one (`spec validate`, `build`, `run`, `generate` of the host; `lock` of `grauwolf-tools`), below the apps so that the lean `grauwolf-tools` program needs no host (ADR-0115, ADR-0118).

## Features / ADRs
ADR-0115, ADR-0118, ADR-0073 (code files), ADR-0103 (skills)

`harness_registry(harness_file)` (ADR-0128 H7, `src/harness.rs`): `[harness] registry` of the nearest `grauwolf.toml` in the harness file's folder or above – the registry of a harness's default image `<registry>/<name>:<version>`; `None` without a file or entry, an error naming the file for invalid TOML or a non-text value. Test `tests/harness.rs`.

## Allowed dependencies
- grauwolf crates: grauwolf-spec, grauwolf-generator (`companion_paths`), grauwolf-builtin-tools (`SkillLibrary`, the skill directory check), grauwolf-toolbuild (building code), grauwolf-toolpkg (optional, feature `packages`), grauwolf-config (`RegistryConfig`), grauwolf-tools; dev: grauwolf-test (`TestDir` for temporary directories in tests)
- External: serde_json, toml (`[harness] registry` of `grauwolf.toml`, ADR-0128 H7)

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md). No dependency on an app crate; no model providers, no wasmtime: `grauwolf-tools-app` links this crate, and the 75 % guard of ADR-0118 (`xtask/tests/workspace_architecture.rs`) must keep passing.

## Invariants
- One rule set for every node and every program: a check that differs per program is an option of `ReadOptions`, never a second reader.
- The read phase never touches the network or the lockfile; the resolve phase is the only one that does.
- Errors have the form `<file>:<pointer>: <message>`, the same in every program.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- Tests `tests/read.rs` (read phase over temporary spec trees), `tests/references.rs` (`file_references`), `tests/resolve.rs` (lockfile, policy, diamonds; one slow), `tests/copy.rs` (copy into an image's `app/`; one slow) and `tests/git_skills.rs` (git skills resolved and locked) – the last three need feature `packages` (`required-features`) and use the test registry (`grauwolf-toolpkg/test-registry`) and the test git server (`grauwolf-git/test-git-server`) as dev-dependencies. What the host does with a copied tree (loading the sub-agents from it) is tested in `crates/apps/grauwolf-host/tests/agent_tree.rs`.
- `cargo test -p grauwolf-agent-tree --features packages`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
