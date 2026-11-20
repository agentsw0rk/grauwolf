# ADR-0075: The agent creates its tools itself

- **Status:** Accepted (User, 2026-11-09: "yes, do it that way")
- **Date:** 2026-11-09

## Context
ADR-0073/0074 let people add tools as files. The next step: the model writes tool code itself during a run, e.g. a helper tool for a data conversion, and uses it immediately in the same run. So far, the tools of a run are fixed at the start.

## Decision
- **Opt-in in the spec:** `self_tools: { tool?, max_tools?, max_code_bytes?, confirm?, save? }`. Without this field, no agent gets the tool. Defaults: `tool` = `create_tool` (counts toward the tool namespace), `max_tools` = 5 per run, `max_code_bytes` = 20000, `confirm` = true, `save` = false.
- **Tool shelf per run:** `grauwolf_tools::ToolShelf` (shared, mutable) is attached to the `ToolRegistry` (`with_shelf`). `descriptors()` and `call()` see the tools on it; forks and clones share it. Each run gets a fresh shelf; from the next LLM request on, a new tool is offered.
- **Port `ToolFactory`** (grauwolf-tools): turns `NewTool { name, description, args, code }` into a `DynTool` or returns an error text. The facade implements it with `ToolBuilder` (build in `spawn_blocking`, cache) and sandbox; compile errors come back with line/column in the tool code. With `save: true`, it stores the tool as `<name>.json` + `<name>.rs` in the `tools_dir` (ADR-0074); otherwise `save` is a spec error.
- **Tool `create_tool { name, description, args, code }`** (interpreter): checks snake_case names, conflicts with existing tools, `max_tools` and `max_code_bytes`, calls the factory and puts the tool on the shelf. Every error (including compile errors) goes back to the model as a tool result, so that it can correct itself.
- **Approval:** with `confirm` (default), the tool requires approval (ADR-0061). The CLI asks on stderr for `spec run`/`spec chat` and shows the code; the service rejects via `/run/stream` and `/run`.
- An interpreter without a factory rejects a spec with `self_tools` when loading it; the generator rejects `self_tools` with `Unsupported`.

## Consequences
- Runtime safety as for spec tools: sandbox without network and file system. The actual risk is the build with `cargo` on the host; the deny list (F13.15) and the time limit (ADR-0030) apply, and for services `--build-wrapper` is recommended.
- A build takes seconds (warm cache) to minutes in the middle of a run; repair loops cost tokens, limited by `max_tools` and the budget (ADR-0072).
- Self-built tools are not in the checkpoint; after `resume` they are missing (with `save`, they return via the `tools_dir` in later runs). Subgraphs with their own tool list do not see them.

## Addendum: Switch and approval per subgraph (User, 2026-11-09: "per subgraph, and also an explicit switch")
- `self_tools.enabled` (default `true`): may the agent build outside subgraphs? With `false`, the settings (limits, approval, `save`) remain in place, without the main graph seeing `create_tool`.
- Subgraphs have `self_tools: true | false` (optional). `true` allows building and using self-built tools within the subgraph, even if its `tools` list does not name them; `false` forbids it. Without a specification, the outer setting applies. Without `self_tools` at the top of the spec, the field is an error (`SelfToolsNotConfigured`).
- Implementation in the `AgentContext`: tool scopes carry, besides the list of names, an optional build permission. For `create_tool` and the tools of the shelf, the innermost build permission applies (otherwise `enabled`); and a list of names further inside than that permission hides them (e.g., the empty list of `llm_reason`).
- Self-built tools follow the same visibility as `create_tool`; the shelf remains shared per run, so a tool built in a subgraph is also usable at other places where building is allowed.

## Addendum (2026-11-09): Self-built tools in the checkpoint
"Self-built tools are not in the checkpoint; after `resume` they are missing" is superseded by ADR-0077: `ContextSnapshot.self_tools` stores their definitions, and `resume_with` rebuilds them via the factory. For visibility in subgraphs, see the addendum above ("Switch and approval per subgraph").
