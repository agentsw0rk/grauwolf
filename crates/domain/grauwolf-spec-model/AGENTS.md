# grauwolf-spec-model

**Layer:** `domain` · **Status:** `CODE_KIND`/`ON_CODE_KIND`, `CodeNodeParams`/`OnCodeParams`, `TransformSpec` (the optional `transform` of an `EdgeSpec`) and `CodePiece`/`CodePieceKind` – the code in graphs (ADR-0102, F14.14; `src/code.rs`). the data model of the agent spec DSL, split off from `grauwolf-spec` in ADR-0108 stage 6.

`FOR_EACH_KIND`/`AGENT_KIND` and `ForEachParams::parse` (the parameters of a `for_each` node; ADR-0101); `to_mermaid` links a `for_each` node to its body subgraph.

`ToolNamespace` (`src/namespace.rs`, ADR-0117): every tool of a spec with name, `ToolOrigin` and JSON pointer in declaration order (`AgentSpec::tool_namespace()`); queries for the names taken, the names taken by everything else, and duplicates (later declaration + the earlier one). Callers add what the model cannot know (code pieces) with `push`.

`BUILTIN_GROUPS` (`src/builtin_groups.rs`, ADR-0117): one row per built-in tool group (`BuiltinGroup`: tools, `section`, `needs_root`; the order is the host's build order); `BuiltinGroup::of_tool`, `BuiltinToolsSpec::tools_of/groups/has_section`. Cargo feature names stay out (ADR-0016): the generator and the host map a group to their features themselves.

`Harness` (`src/harness.rs`, ADR-0128): the harness file `<name>.harness.json` – `version`, `name`, `spec` (the root spec, relative to the file), optional `image`, `port` and `env`; `default_image(registry, version)` is `<registry>/<name>:<version>` (ADR-0128 H7) (an object `NAME` → value; for `grauwolf-k8-ops agent deploy --harness`); `Harness::parse_named(name, json)` gives the same errors as text starting with the file's name (`<name>:<pointer>: …`, `<name>: invalid harness JSON: …`), for the CLI and `grauwolf-k8-ops`; `Harness::parse` reports format errors like `AgentSpec::parse` refuses another `version` than `SUPPORTED_HARNESS_VERSION` (`/version`) and a `name` that is no DNS label (`/name`; a–z, 0–9, `-`, starts with a letter, at most 63 characters).

## Purpose
The spec types (`AgentSpec`, `GraphSpec`, `NodeSpec`, `EdgeSpec`, `ConditionSpec`, `StrategySpec`, `ModelSpec`, the tool, section and built-in specs, `ToolPackageSpec`, `SubgraphSpec` …), the plain types (`OciReference`, `GitSource` with `GitRef`/`GitSourceProblem` for git tool packages in `src/git.rs`, `SpecError`), the skill sources `SkillSourceSpec` (`skills`, ADR-0103; `GitSource::from_skill_source`; `BUILTIN_SKILL_TOOLS` are in `external_tools()` while `skills` is set), the constants (`STRING_TYPE`, `START`/`FINISH`, `SUBGRAPH_KIND`, `PLAN_KIND`, `PARALLEL_KIND`, `GOAP_KIND`, the `BUILTIN_*_TOOLS` lists, `SUPPORTED_SPEC_VERSION` …), strict parsing **without** strategy expansion (`AgentSpec::parse`: version check first, format errors with JSON pointers through `serde_path_to_error`), the JSON schema (`AgentSpec::json_schema()`, schemars) and the helpers that only read a spec: `external_tools`, `provider_uses`, `providers`, `uses_system_one`, `to_json`, `to_mermaid` (`src/mermaid.rs`), `escape_pointer_token`.

No strategy expansion (`grauwolf-strategies`), no validation and no `from_json` (`grauwolf-spec`, extension trait `AgentSpecExt`). Everything is re-exported by `grauwolf-spec`, so consumers keep using `grauwolf_spec::…`.

## Features / ADRs
E13, ADR-0022, ADR-0025, ADR-0080, ADR-0084, ADR-0103, ADR-0107, ADR-0108

## Allowed dependencies
- grauwolf crates: grauwolf-graph only (`START`/`FINISH` for the Mermaid diagram) – **no** other spec crate (`grauwolf-spec`, `grauwolf-strategies`; checked by `check-arch`)
- External: serde, serde_json, serde_path_to_error, schemars, thiserror

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- Pure data crate: types, parsing, schema and read-only helpers; no IO.
- `deny_unknown_fields` on all structs; no `flatten`/`untagged` (ADR-0025).
- Format errors come as `SpecError::Format { pointer, message }` with an English message; only invalid JSON stays `SpecError::Json` (ADR-0080).
- The `strategy` of a parsed spec stays unexpanded; `to_json` writes the spec with `strategy: None`.
- Inherent methods of `AgentSpec` that need the strategies or the validation rules cannot live here; they are the extension trait `AgentSpecExt` in `grauwolf-spec`.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-spec-model`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.

Approvals (ADR-0119): `AgentSpec::approval_tools()` names the tools that need an approval before each call (`self_tools.tool` with `confirm`, `run_command` in shell mode with `approve_each_call`); the interpreter and generated code gate the same ones.
