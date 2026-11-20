# 10 Self-made tools

In [chapter 3](03-tools.md) you wrote tools yourself – as code in the spec, as a tool file or in the
tools directory. Sometimes, though, you don't know in advance which tool an agent will need: a
particular date calculation, a conversion, a text filter. With `self_tools`, the agent may **build
such tools itself during the run** and use them right away.

That is powerful – and needs control. In this chapter you learn

- how to switch on and limit `self_tools`,
- how to review and approve every build beforehand,
- how built tools are kept for later runs,
- how to restrict building to individual subgraphs,
- what is safe about it and where the limits are.

## The first tool builder

[`examples/10-tool-builder.json`](examples/10-tool-builder.json):

```json
{
  "spec_version": 1,
  "name": "tool_builder",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You solve tasks precisely. If you lack a tool, build it with create_tool and then use it.",
  "self_tools": { "max_tools": 3 },
  "strategy": { "kind": "single_run" }
}
```

Even `"self_tools": {}` is enough: the agent then gets an additional tool `create_tool`. Without the
field, no agent gets this tool.

```bash
grauwolf spec run docs/user-guide/examples/10-tool-builder.json \
  "How many vowels does the word Donaudampfschifffahrt have?"
```

What happens now:

1. The model calls `create_tool` with four arguments: `name` (snake_case), `description`, `args`
   (JSON schema of the arguments) and `code` (Rust).
2. Because `confirm` is on by default, grauwolf asks you first (see next section).
3. grauwolf builds the code into a WebAssembly module, just like the tools from chapter 3.
4. The model receives `Tool 'count_vowels' is available from now on.` – from the next request on,
   the new tool is on offer.
5. The model calls the new tool and answers.

Roughly:

```text
The word "Donaudampfschifffahrt" has 6 vowels.
```

### What the model has to write

As in chapter 3, the code is the **body** of the function

```rust
fn tool(args: serde_json::Value) -> Result<String, String>
```

for example:

```rust
let word = args["word"].as_str().ok_or("word is missing")?;
Ok(word.chars().filter(|c| "aeiouAEIOU".contains(*c)).count().to_string())
```

Models often write the whole function including its signature. That is accepted too: if the code
starts with `fn tool(`, grauwolf then calls the function itself. Only `std` and `serde_json` are
allowed; there is no network, no files and no `unsafe`. If `args` is missing,
`{ "type": "object" }` applies.

### Errors go back to the model

If something goes wrong, the run does **not** abort. The error comes back as the tool result, and
the model can fix things:

| Situation | Message to the model (example) |
|---|---|
| name is not snake_case | `name 'CountVowels' is not a snake_case identifier` |
| name is already taken | `tool 'count_vowels' already exists` |
| too many tools in this run | `at most 3 new tools per run` |
| code too long | `code is longer than 20000 bytes` |
| compile error | the compiler's messages with line and column in the tool code |
| build rejected | `The user refused to build the tool 'count_vowels'.` |

## The settings

| Field | Default | Meaning |
|---|---|---|
| `enabled` | `true` | May the agent build outside of subgraphs? (see [subgraphs](#building-only-in-certain-subgraphs)) |
| `tool` | `"create_tool"` | Name of the build tool; shares the namespace with all other tools. |
| `max_tools` | `5` | At most this many new tools per run. |
| `max_code_bytes` | `20000` | At most this many bytes of code per tool. |
| `confirm` | `true` | A human confirms every new tool before it is built. |
| `save` | `false` | Store built tools in the `tools_dir` so that later runs have them. |

Every build takes time (seconds with a warm cache, minutes the first time) and every repair loop
costs tokens. `max_tools` and a budget (`--max-tokens`, `--max-cost`, see
[chapter 2](02-models.md)) keep this in check.

## Approving every build

With `confirm: true` – the default – `spec run` or `spec chat` shows the name, description and code
on stderr before every build and waits for your answer:

```text
The agent wants to build the tool 'count_vowels': Counts the vowels of a word
---
let word = args["word"].as_str().ok_or("word is missing")?;
Ok(word.chars().filter(|c| "aeiouAEIOU".contains(*c)).count().to_string())
---
Build? [y/N]
```

`y` or `yes` builds the tool (`j` and `ja` work too); anything else rejects it, and the model is told.

Where there is no human who can answer, the build is **rejected**:

- in the service via `/run`, A2A and MCP – via `/run/stream`, on the other hand, an `approval` event
  arrives for the tool `create_tool`, which you decide with `POST /approvals/{id}` as in
  [chapter 9](09-operations.md#approvals-a-human-decides); the arguments in the event contain the
  code;
- with `spec mcp`.

Checkpoint runs (`spec run --run-id`, `spec resume`) ask on the console like ordinary runs.

Only if you really trust the agent – for example in an isolated test environment – do you switch
off the prompt with `"confirm": false`.

## Keeping built tools

Normally a self-made tool lives only until the end of the run. With `save: true`, grauwolf stores
every successfully built tool in the spec's tools directory
([`examples/10-tool-builder-save.json`](examples/10-tool-builder-save.json)):

```json
{
  "spec_version": 1,
  "name": "tool_builder_save",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You solve tasks precisely. Use existing tools; if one is missing, build it with create_tool.",
  "tools_dir": "10-tools",
  "self_tools": { "max_tools": 2, "max_code_bytes": 5000, "save": true },
  "strategy": { "kind": "single_run" }
}
```

`save` needs a `tools_dir`; without one, `spec validate` reports:

```text
10-tool-builder-save.json:/self_tools/save: `save` needs a `tools_dir` to store the tools in
```

After a build, the folder `10-tools/` (relative to the spec, created if needed) contains two
files – in the same format as the tool files from [chapter 3](03-tools.md):

```text
10-tools/count_vowels.json   name, description, args and "code_file": "count_vowels.rs"
10-tools/count_vowels.rs     the code
```

On the next run, grauwolf loads the tools directory as usual (with `spec chat` before every line, in
the service before every request to `/run` and `/run/stream`), and `count_vowels` is there from the
start. Because the name is then taken, the agent does not build it again. Look at saved tools before
you keep them permanently – you can edit or delete them like any tool file.

## Building only in certain subgraphs

Often only a certain part of the flow should be allowed to build. There are two switches for this:

- `self_tools.enabled` controls the main graph. With `false`, it sees neither `create_tool` nor
  self-made tools; the other settings (limits, approval, `save`) still apply.
- Every subgraph ([chapter 6](06-subgraphs-and-planners.md)) can set `"self_tools": true` or
  `false`. If omitted, whatever applies outside applies.

[`examples/10-workshop.json`](examples/10-workshop.json) allows building only in the subgraph
`calculate`; the subsequent node `summarize` sees no tools:

```json
{
  "spec_version": 1,
  "name": "workshop",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You calculate first and then summarize.",
  "self_tools": { "enabled": false, "max_tools": 2 },
  "subgraphs": {
    "calculate": {
      "tools": [],
      "self_tools": true,
      "graph": {
        "nodes": [
          { "id": "call_llm", "kind": "llm_request" },
          { "id": "execute_tool", "kind": "execute_tool" },
          { "id": "send_result", "kind": "llm_send_tool_result" },
          { "id": "execute_tools_in_order", "kind": "execute_multiple_tools", "params": { "sequential": true } },
          { "id": "send_results", "kind": "llm_send_multiple_tool_results" }
        ],
        "edges": [
          { "from": "start", "to": "call_llm" },
          { "from": "call_llm", "to": "execute_tool", "when": { "kind": "on_tool_call" } },
          { "from": "call_llm", "to": "execute_tools_in_order", "when": { "kind": "on_multiple_tool_calls" } },
          { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } },
          { "from": "execute_tool", "to": "send_result" },
          { "from": "send_result", "to": "execute_tool", "when": { "kind": "on_tool_call" } },
          { "from": "send_result", "to": "execute_tools_in_order", "when": { "kind": "on_multiple_tool_calls" } },
          { "from": "send_result", "to": "finish", "when": { "kind": "on_assistant_message" } },
          { "from": "execute_tools_in_order", "to": "send_results" },
          { "from": "send_results", "to": "execute_tool", "when": { "kind": "on_tool_call" } },
          { "from": "send_results", "to": "execute_tools_in_order", "when": { "kind": "on_multiple_tool_calls" } },
          { "from": "send_results", "to": "finish", "when": { "kind": "on_assistant_message" } }
        ]
      }
    }
  },
  "graph": {
    "nodes": [
      { "id": "calculate", "kind": "subgraph", "params": { "subgraph": "calculate" } },
      { "id": "summarize", "kind": "llm_request" }
    ],
    "edges": [
      { "from": "start", "to": "calculate" },
      { "from": "calculate", "to": "summarize" },
      { "from": "summarize", "to": "finish", "when": { "kind": "on_assistant_message" } }
    ]
  }
}
```

### Who sees what?

The same rules apply to `create_tool` and all self-made tools:

| Where | visible? |
|---|---|
| main graph | if `self_tools.enabled` (default `true`) |
| subgraph with `"self_tools": true` | yes – even if its `tools` list does not name them (in the example `"tools": []`) |
| subgraph with `"self_tools": false` | no |
| subgraph without `self_tools` and without a `tools` list | as outside |
| subgraph without `self_tools`, but with a `tools` list | no – the list hides them, even if it names `create_tool` |
| node without tools (e.g. `llm_reason`) | no |

The tools built in a run are kept in one shared place: a tool built in the subgraph `calculate` can
also be used in every other subgraph in which building is allowed.

`self_tools` in a subgraph without `self_tools` at the top of the spec is an error:

```text
spec.json:/subgraphs/calculate/self_tools: `self_tools` in a subgraph needs `self_tools` at the top of the spec
```

## Security

Self-made code runs just as isolated as the tools from chapter 3: in a **WebAssembly sandbox**
without network and without file system. A tool can only compute text from its arguments.

The real risk is the **build**: the code is compiled with `cargo` on your machine. grauwolf secures
this in layers:

1. Before the build, forbidden constructs are rejected: `include!` and relatives, `env!`,
   `option_env!`, `mod`, `extern`, `unsafe` and attributes.
2. `cargo` runs with an empty environment (only toolchain paths) and `--offline`.
3. Every build has a time limit (300 s).
4. Optionally, `cargo` runs inside a further wrapper that you specify with `--build-wrapper` – once
   per part, e.g.:

   ```bash
   grauwolf spec serve docs/user-guide/examples/10-tool-builder.json \
     --build-wrapper bwrap --build-wrapper --unshare-net
   ```

   You append further arguments of the wrapper (with bwrap, for example, the mounted directories)
   the same way, each with its own `--build-wrapper`.

   For services in which a model writes code, such a wrapper (bubblewrap, Docker or similar) is
   **strongly recommended**.

On top of that comes approval with `confirm`: a human sees the code before it is built.

The build needs the Rust toolchain including the target `wasm32-unknown-unknown` on the machine
where the agent runs – as described in chapter 3. Built modules end up in the cache (`--cache`,
default `.grauwolf/tools`); the same code is not built twice.

## Limits

- **Checkpoints:** self-made tools are stored in the checkpoint with their definition.
  `spec resume` rebuilds them before resuming (usually from the build cache); if that fails,
  resuming aborts with a message.
- **Code generation:** `spec generate` rejects specs with `self_tools`, because they would need the
  build and the sandbox at runtime ([chapter 11](11-code-generation.md)).
- **Sub-agents** (`agent_tools`) have no back channel for approvals; if a sub-agent builds with
  `confirm`, the build is rejected.

## Key points

- `self_tools` in the spec gives the agent the tool `create_tool`; it writes Rust code, grauwolf
  builds it into the sandbox, and the new tool is available from the next request on.
- You set limits with `max_tools` and `max_code_bytes`; errors go back to the model as the tool
  result.
- `confirm` (default) shows every build for approval – on the console or via `/run/stream`;
  without a back channel, it is rejected.
- `save` stores built tools in the `tools_dir`.
- `enabled` and `self_tools` per subgraph determine where tools may be built and used.
- The build runs on your machine – use `--build-wrapper` for services.

Continue with [chapter 11: Code generation](11-code-generation.md).
