# ADR-0074: Tools directory that is loaded at runtime

- **Status:** Accepted (User, 2026-11-09: "yes, do 1 and 2")
- **Date:** 2026-11-09

## Context
With ADR-0073, tools live in their own files but are wired into the spec statically. The user wants to add tools at runtime: put a file there, and at the next run the tool is available, without a project or spec change and without restarting `spec serve`.

## Decision
- Spec field `tools_dir` (relative to the spec). Before each run, `grauwolf::tools_dir::ToolsDir` reads all `*.json` files in it (not recursively) in the format of ADR-0073 (`code` or `code_file` relative to the tool file), builds them with the `ToolBuilder` (cache keyed by the checksum of the definition, so only new and changed tools are built) and instantiates them in the WASM sandbox.
- The tools reach only this run via `RunExtras.tools`. They must not shadow a tool of the spec or one from outside (`external_tools()`).
- Unusable files (no JSON, missing fields, invalid name, compile error, name conflict) are skipped and reported; the run continues. The CLI reports them on stderr.
- CLI `spec run`, `spec chat` (per line) and `spec serve` (per request to `/run` and `/run/stream`, via `ServiceOptions.run_tools`) load the directory; the service's A2A and MCP remain with the spec's tools for now; the generator rejects `tools_dir` with `Unsupported`.

## Consequences
- New tools need only a file; the first build of a tool takes seconds to about a minute, after which it comes from the cache.
- The spec cannot check these tools beforehand, and graphs cannot name them; strategies that offer all tools (`single_run`, `react`) use them. Subgraphs with their own tool list do not see them.
- Tools in the directory run like spec tools in the sandbox, without network and file system.

## Addendum (2026-11-09): Checkpoint runs
`spec run --run-id` and `spec resume` also load the tools directory (ADR-0077); a resumed run sees the tools as they are at the time of resuming. `spec mcp`, and the service's A2A and MCP, still do not use it.
