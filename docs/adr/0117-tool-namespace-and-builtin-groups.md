# ADR-0117: One tool namespace and one table of built-in tool groups

- **Status:** Accepted (User, 2026-11-13: architecture review, candidate 4 "tool namespace and
  built-in group table" – "do it")
- **Date:** 2026-11-13
- **Related:** ADR-0025 (tool namespace), ADR-0046/0098/0099/0104 (built-in tool groups),
  ADR-0103 (skills), ADR-0107 (tool packages, `as`), ADR-0075 (self-built tools), ADR-0102 (code
  pieces), ADR-0108 (layers), ADR-0016 (no build concerns in the domain), ADR-0114 (provisioning).
  Terms: `GLOSSARY.md` (tool namespace, built-in tool group).

## Context
All tools of an agent share one namespace (ADR-0025): own tools, MCP tools (and resource tools),
the knowledge search tool, memory tools, built-in tools, skill tools, sub-agents, package tools
(after renaming), the self-build tool and – since ADR-0102 – code pieces. "Which names are taken" is
computed in five places with different rules (facts from the architecture review, 2026-11-13):
`tool_names()` (own + external + packages + self tools), the tool-package check (without packages),
the built-in check (own + MCP + sub-agents only – it misses knowledge, memory and skills), the
package resolver in `grauwolf-toolpkg`, and `tools_dir` in the CLI (own + external). The list of
built-in groups – their tools, the required section, whether `root` is needed, the Cargo feature – is
kept three times: in the spec checks (three lists), in the host (an exclusion list over nine groups
plus per-group dispatch) and in the generator (group → facade feature). Every new tool kind or group
has to be added in all of these places.

## Decision
1. **Tool namespace module in `grauwolf-spec-model`.** It lists every tool of a spec with its name,
   its origin (own, MCP, MCP resource, knowledge, memory, built-in group, skills, sub-agent, package,
   self-build, code piece) and the JSON pointer of the place that declares it. Queries: all entries,
   the names taken, the names taken by everything except a given origin or pointer, duplicates.
   Every place that asks "is this name taken" uses it: the spec checks (all collision errors come
   from one rule and name the other tool and its pointer), the package resolver, `tools_dir` loading
   and provisioning. Tools whose names are only known after resolving (packages without `tools`)
   stay a resolve-time check, as today.
2. **Built-in group table in `grauwolf-spec-model`.** One row per group: its tools, the required
   spec section (if any), whether `root` is needed. The spec checks, the host's dispatch (which group
   a configured tool belongs to) and the generator read it. **Cargo feature names stay out of the
   domain** (ADR-0016): the generator maps a group to its facade feature in its own small table, and
   the host's "this build has no …" errors stay with the host's `cfg`s – both keyed by the group from
   the domain table, with a test that every group is mapped.
3. **Behaviour change:** collisions the built-in check missed (a built-in tool named like the
   knowledge tool, a memory tool or a skill tool) become validation errors (`DuplicateToolName`, at the
   later of the two declarations). Error texts gain the origin of the other tool.
4. **Tests through the two interfaces:** namespace tests (every origin, duplicates across every pair of
   origins, pointers) and table tests (every built-in tool belongs to exactly one group; sections and
   root rules); the old per-place collision tests are replaced where they only re-tested the same rule.
5. **Order of work**, every commit green: the namespace module with tests → spec checks on it (incl.
   the new collisions) → the built-in table → spec checks, host dispatch and generator on the table
   → package resolver and `tools_dir` on the namespace → docs.

## Consequences
- A new tool source or built-in group is added in one place (plus its feature mapping).
- Collision errors are consistent and name both sides.
- Specs that relied on an unchecked collision now fail validation (deliberate).

## Implementation notes (2026-11-13)
Commits in the order of the decision: namespace module → spec checks on it → built-in table →
spec checks, host dispatch and generator on the table → package resolver and `tools_dir` → docs.

- **Namespace:** `grauwolf-spec-model/src/namespace.rs` (`ToolNamespace`, `ToolEntry`, `ToolOrigin`,
  `ToolCollision`, `AgentSpec::tool_namespace()`), 9 unit tests. Declaration order: own, MCP (per
  server tools, then resource tools), knowledge, memory, built-in, skills, sub-agents, package tools,
  self-build tool, code pieces. `AgentSpec::external_tools()` is now the namespace filtered to the
  external origins (its order changed for mixed specs: built-in and skill tools before sub-agents).
- **Spec checks:** one rule, `Rules::check_tool_collisions` (`validate/tools.rs`), called by every
  check for its own origin at its old place in `validate_with`; `tool_names()` and the `seen` sets
  are gone, also in the subgraph tool lists. `DuplicateToolName` gained `other_origin` and
  `other_pointer`; the text is `tool name '<n>' is used more than once (the other one is the <origin>
  at <pointer>)`. 7 new tests in `grauwolf-spec/tests/tool_namespace.rs`, the old per-place
  collision tests now assert the other tool, too.
- **New collisions:** a knowledge or memory tool, MCP tool or sub-agent named like another
  declaration is reported at the later one for every pair, among them what the built-in check missed
  (knowledge, memory, skills). Built-in tool names are fixed, so the collisions are reported at the
  custom side: a knowledge tool named `think` is reported at the built-in tool, a knowledge tool
  named `load_skill` at `/skills`.
- **Deviations:** (1) Code pieces are not built by `grauwolf-spec-model` (they need the graph
  catalogs): the caller adds them with `ToolNamespace::push`; the spec check does. (2) `ToolOrigin::
  Builtin` carries no group; `BuiltinGroup::of_tool` gives it. (3) A collision is reported once, at
  the later declaration: where two checks used to report both sides (memory tools with equal names,
  a package tool and the self-build tool) only the later one is reported, and an own tool named like
  a skill tool is now reported at `/skills` (it was at the own tool). (4) Names that are invalid ids
  and unknown built-in tools are not checked for collisions (their own error is reported).
- **Table:** `grauwolf-spec-model/src/builtin_groups.rs` (`BUILTIN_GROUPS`, `BuiltinGroup`,
  `BuiltinGroupInfo`, `BuiltinToolsSpec::tools_of/groups/has_section`), 6 unit tests. The row order
  is the host's build order; the `BUILTIN_*_TOOLS` constants stay as the data of the rows. The
  order of `MissingBuiltinSection` errors follows the table (fetch, command, jira, confluence,
  git).
- **Host and generator:** `builtin::build` loops over the groups with an exhaustive `match`; the
  generator maps groups to features with the exhaustive `group_feature` (feature order unchanged);
  tests: every group is built or reported (host), every group is mapped (generator).
- **Resolver and `tools_dir`:** `tools_of` and `tools_dir::taken_names` read the namespace
  (`tools_dir` tools now also must not override package and self-build tools). The CLI no longer
  computes names itself (it moved into provisioning with ADR-0114).
