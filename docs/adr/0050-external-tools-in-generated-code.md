# ADR-0050: External tools in generated code

- **Status:** Accepted (decision by Claude, User 2026-11-08: "decide on your own which things are important")
- **Date:** 2026-11-08

## Context
MCP tools (ADR-0036), the search tool from `knowledge` (ADR-0038), sub-agents from `agent_tools` (ADR-0040) and built-in tools (ADR-0046) are provided by the runtime; the interpreter receives them via `with_tools`. The generator has so far rejected such specs (`GeneratorError::Unsupported`). As a result, the core promise of the DSL – interpreter and generated code behave the same – holds only for part of the specs.

## Decision
- Generated crates get `pub const EXTERNAL_TOOLS: &[&str]` (all names that must come from outside) and `pub async fn run_with_tools(executor, input, external: Vec<Arc<dyn DynTool>>)`; a missing expected tool is an error naming it. `run(executor, input)` calls `run_with_tools` without external tools.
- The generator no longer rejects `mcp_servers`, `knowledge`, `agent_tools` and `builtin_tools`.
- Provisioning moves from the CLI into the facade: `grauwolf::external::tools(spec, base_dir, io)` starts MCP servers, indexes knowledge and builds built-in tools; the CLI and generated applications use the same function. Sub-agents remain the CLI's job for now (they need tool builds and recursive loading).
- The equivalence test covers external tools with a spec using the built-in `read_file`.

## Consequences
- Every valid spec can be generated; what is external is stated explicitly in `EXTERNAL_TOOLS`.
- Generated applications with external tools depend on the facade for provisioning.

## Addendum (2026-11-09): sub-agents in the facade
Sub-agents are no longer the CLI's job: `grauwolf::loader::AgentLoader` builds tool code, sets up providers, binds external tools and sub-agents recursively and is shared by the CLI and applications; generated applications get the sub-agents via `AgentLoader::agent_tools`.
