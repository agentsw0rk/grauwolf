# ADR-0098: Built-in tools `think`, `fetch`, key-value store and `todo`

- **Status:** Accepted (User, 2026-11-11: "close the smaller grauwolf gaps: temperature/think (ADR-0097) and the tools think, fetch, key-value and todo (ADR-0098)"; ADR-0092)
- **Date:** 2026-11-10

## Context
grauwolf's built-in tools cover files and user input/output (ADR-0046). AgentWeaver offers further
built-in tools that agents commonly use: think, fetch, memory (key-value), todo. Without them, many
AgentWeaver graphs cannot be exported to grauwolf.

## Decision
New names in `builtin_tools.tools` (same section, same namespace, external tools for interpreter
and generated code, ADR-0050):
- **`think { thought }`:** returns `ok`; gives the model room to reason between tool calls. No
  side effects.
- **`fetch { url, method?, headers?, body? }`:** HTTP request, response as text (status line,
  selected headers, body up to 1 MiB, HTML reduced to text). Requires `builtin_tools.fetch:
  { allowed_hosts: [...] }` (exact host or `*.domain`), HTTPS only unless `allow_http: true`;
  redirects only to allowed hosts; private, loopback and link-local addresses are refused unless
  listed explicitly (no SSRF into the cluster); timeout 30 s.
- **Key-value store** – `store_value { key, value }`, `retrieve_value { key }`, `list_keys {}`:
  one JSON file `kv.json` under the root of `builtin_tools` (per user with `--user`, as ADR-0059);
  keys up to 256 bytes, values up to 64 KiB, writes atomic. Distinct from long-term memory
  (`remember`/`recall`, semantic facts, ADR-0058).
- **`todo`** – `add_task { title }`, `complete_task { id }`, `remove_task { id }`,
  `list_tasks {}`: a task list per run in agent storage (ADR-0044), so it is part of checkpoints and
  survives a resume.
- AgentWeaver's names map in the export: `store`/`retrieve`/`listKeys` → the three key-value tools,
  `addTask`/`completeTask`/`listTasks`/`removeTask` → the todo tools, `fetch` → `fetch`.

## Consequences
- AgentWeaver graphs with these tools export without code; `fetch` is safe by default because the
  spec must name hosts.
- The key-value file and the todo list are visible in events like other tool calls; no new events.

## Implementation notes (2026-11-11)
- Spec: `builtin_tools.tools` knows `think`, `fetch`, `store_value`/`retrieve_value`/`list_keys` and
  `add_task`/`complete_task`/`remove_task`/`list_tasks`. `fetch` needs the section `fetch`
  (`MissingBuiltinSection`; `EmptyAllowedHosts`, `InvalidAllowedHost` for bad entries); the key-value
  tools need `root` (`MissingBuiltinRoot`).
- `fetch`: the connection is pinned to the checked address (`reqwest` `resolve`), redirects are
  followed by hand (at most 5, each hop checked again), the result is `HTTP <code> <reason>`,
  `content-type`, `content-length`, a blank line and the body (AgentWeaver returns only
  `HTTP <code>` and the body, without a host list). Refusals are tool errors, so the model sees
  `Error: tool failed: refused: …`. A host listed exactly may be an internal address; wildcards never.
- Key-value: the file is `<root>/kv.json`, with a user `<root>/users/<hex>/kv.json` (the scheme of
  chat histories, ADR-0060) instead of a scope field inside one file. Messages follow AgentWeaver's
  memory tool (`Stored key: k`, `Key not found: k`, `No keys stored`).
- `todo`: built-in tools cannot reach the agent storage through `DynTool::call_json`, so `DynTool`
  got two defaulted methods, `state_key()` and `call_with_state(args, &mut Value)`; the agent
  context runs such a call on its storage entry (`grauwolf.todo`) and keeps the change only if the
  call succeeds. The storage moved behind a `Mutex` for this; it is part of every checkpoint, so the
  list survives a resume. Task fields are `id`, `title`, `completed`; messages follow AgentWeaver's.
- Live check (Ollama `qwen3.5:9b`): one run used `fetch` of https://example.com,
  `store_value`/`retrieve_value` and the todo tools together and answered correctly with `think: true`;
  with `think: false` the model announced the steps and made no tool call at all.
