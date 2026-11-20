# 11 Code generation

Up to now grauwolf has **interpreted** your specs: `spec run`, `spec chat` and `spec serve` read
the JSON file and execute it. Two commands go one step further:

- `grauwolf spec generate` turns a spec into a standalone **Rust crate** (a Rust package) that you
  can build into your own programs – type-checked by the compiler, with ready-built tools.
- `grauwolf spec create` goes the other way: an LLM writes a valid spec for you from a description
  in words.

Both commands need the Rust toolchain (like the tool build in [chapter 3](03-tools.md)).

## A spec as a Rust crate

The example is an agent with its own tool
([`examples/11-word-counter.json`](examples/11-word-counter.json)):

```json
{
  "spec_version": 1,
  "name": "word_counter",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You count words with the tool count_words and state the result in one sentence.",
  "tools": [
    {
      "name": "count_words",
      "description": "Counts the words of a text.",
      "args": {
        "type": "object",
        "properties": { "text": { "type": "string" } },
        "required": ["text"]
      },
      "code": "let text = args[\"text\"].as_str().ok_or(\"text is missing\")?;\nOk(text.split_whitespace().count().to_string())"
    }
  ],
  "strategy": { "kind": "single_run" }
}
```

```bash
grauwolf spec generate docs/user-guide/examples/11-word-counter.json --out word_counter
```

Output, roughly:

```text
docs/user-guide/examples/11-word-counter.json: crate written to word_counter
```

`generate` checks the spec, first builds all tools (like `spec build`, with the same cache) and
then writes:

```text
word_counter/
├── Cargo.toml              package "word_counter" (the name of the spec) with the grauwolf crates
├── src/lib.rs              the agent as Rust code
└── tools/count_words.wasm  the built tool, embedded into the crate
```

Among other things, the file `src/lib.rs` contains:

| Function | Purpose |
|---|---|
| `graph()` | the flow as a type-checked graph |
| `model()`, `prompt()` | model and system prompt from the spec |
| `tools()` | the tools of the spec, executed in the sandbox |
| `run(executor, input)` | runs the agent in a fresh context |
| `run_observed(…, features)` | like `run`; the features see all events (like `--events` with `spec run`) |
| `run_app(base_dir, input)` | with feature `app`: sets everything up like `spec run` (see below) |
| `subgraph_<name>()` | one function per subgraph |

Named types from `types` ([chapter 5](05-structured-data.md)) become Rust structs, and `run` takes
and returns exactly these types. If the spec needs tools from outside – MCP servers, knowledge,
memory tools, sub-agents or built-in tools – the crate additionally contains `EXTERNAL_TOOLS` (the
required names) and `run_with_tools(executor, input, external)`; your program provides these
tools. How to use the crate from Rust is shown in the
[developer guide](../developer-guide/06-generator.md).

### Everything in one place: feature `app`

If you do not want to wire anything up yourself, enable the crate's Cargo feature `app`. Then you
get `run_app(base_dir, input)`, which sets everything up itself like `grauwolf spec run`: providers
and keys from the environment (`OLLAMA_HOST`, `ANTHROPIC_API_KEY`, …), MCP servers, knowledge,
built-in tools, sub-agents and long-term memory. Paths from the spec (e.g. `root`, `sources`,
`store`) are relative to `base_dir`.

```toml
[dependencies]
word_counter = { path = "word_counter", features = ["app"] }
```

```rust
let answer = word_counter::run_app(Path::new("."), "Count the words in note.txt".into()).await?;
```

For a specific user – their area in long-term memory and their key-value store, like
`spec run --user` – and to follow the events, there is `run_app_with`. Its options are split in two:
`ProvisionOptions` belong to the agent (the environment it reads its settings from, an own decider,
observers, a price table), `RunOptions` to a single run (the user, the features that see the events and
everything else a run of `spec run` can have, see below):

```rust
use grauwolf::config::ProcessEnv;
use grauwolf::provisioning::ProvisionOptions;

let provision = ProvisionOptions::new(Arc::new(ProcessEnv));
let run = grauwolf::app::RunOptions {
    user: Some("anna".into()),
    features: vec![Arc::new(grauwolf::features::JsonlEventLog::new(std::io::stderr()))],
    ..Default::default()
};
let answer = word_counter::run_app_with(Path::new("."), "What do you know about me?".into(), provision, run).await?;
```

`run_app_with` sets the agent up (providers, MCP servers, knowledge, sub-agents) for every call. If
you run the agent many times, set it up once and run it per request:

```rust
let agent = word_counter::provision(Path::new("."), provision).await?;
let answer = word_counter::run_provisioned(&agent, "What do you know about me?".into(), run).await?;
```

Generated code runs like the interpreter (ADR-0119). The fields of `RunOptions` are:

| Field | Meaning |
|---|---|
| `user` | The user's area in long-term memory and key-value store |
| `features` | Features that see all events of the run |
| `budget` | Limits of this run (`max_tokens`, `max_cost`, `max_tool_calls`, `max_prompt_tokens`); each field set here wins over the spec's `limits`, like `--max-tokens`/`--max-cost` |
| `conversation` | `(store, id)`: continues the conversation and saves it (`ChatHistoryStore`) |
| `attachments` | Images, PDFs or audio for the input of the run |
| `approver` | Decides on tools that need approval (`ToolApprover`); without one such a call is refused with `tool '<name>' needs an approval that nobody can give here` |
| `frame_sink` | Receives the frames of streaming nodes, i.e. the text deltas |
| `checkpoints` | `(store, run id)`: saves the state before every node |

After a crash, `word_counter::resume_provisioned(&agent, store, "run-1", run).await?` continues the run
from its last checkpoint (the input is the one of the checkpoint; `RunOptions::resuming` does the same
for your own entry points). Tools that need approval are `self_tools.tool` with `confirm` and
`run_command` in shell mode with `approve_each_call`: `spec generate` accepts such specs, and the crate
gates the calls like `spec run`.

Costs come from a price table, the same file as `--prices` ([chapter 2](02-models.md)): set
`GRAUWOLF_PRICES` to its path (relative to the base directory) or hand it in with
`ProvisionOptions::with_prices`. Then `agent_completed` carries `cost` and `limits.max_cost` works;
a spec with `limits.max_cost` and no price table fails in `provision`, before the first run.

This is what a generated service does. `AppOptions` no longer exists: its `user` and `features` are
now `RunOptions`, its `decider`, `mcp_tools_observer` and `agent_features` belong to
`ProvisionOptions` (`with_decider`, `with_mcp_tools_observer`, `with_agent_features`).

Without `app`, the crate stays lean and does not depend on the grauwolf facade.

The file is generated – better change the spec and generate again.

### As an HTTP service: `--service`

With `--service`, the crate additionally gets a Cargo feature `service` and the binary `agent`
(`src/main.rs`), which serves the agent like `grauwolf spec serve`: `/run`, `/run/stream`,
`/health`, A2A (agent card and `/a2a`) and MCP (`/mcp`). The binary is only built with the
feature:

```bash
grauwolf spec generate examples/specs/echo.json --out echo --service --check
cargo build --release --features service --manifest-path echo/Cargo.toml
OLLAMA_HOST=http://localhost:11434 GRAUWOLF_ALLOW_ANONYMOUS=true echo/target/release/agent
```

The binary is named `agent` for every spec, so that images do not need to know the spec's name.
Everything is configured through the environment:

| Variable | Meaning | Default |
|---|---|---|
| `SERVER_PORT` | Port of the service (bound to `0.0.0.0`) | `8080` |
| `GRAUWOLF_SERVER_TOKEN` | Shared token (`Authorization: Bearer …`) | – |
| `GRAUWOLF_USERS_FILE` | User tokens as for `spec serve --users` | – |
| `GRAUWOLF_ALLOW_ANONYMOUS` | `true` allows access without any token; otherwise the service refuses to start without a token or users file | – |
| `GRAUWOLF_DATA_DIR` | Directory against which the spec's paths (`knowledge`, memory `store`, `builtin_tools.root`, tool cache) are resolved | working directory |
| `GRAUWOLF_CONVERSATIONS_DIR` | Directory (relative to `GRAUWOLF_DATA_DIR`) in which the conversations (`conversation_id`) are kept, one file each, so they survive a restart; as `spec serve --conversations-dir` | in memory |
| `GRAUWOLF_CONVERSATIONS_DB` | Database URL for the conversations (`sqlite://chats.db`, `postgres://…`), as `spec serve --conversations-db`; the URL is never printed. Needs the facade feature `sql` (included with `--full-facade`); without it the service refuses to start. Setting both conversation variables is a start-up error | in memory |
| `A2A_PORT` | A2A also on this port (it is always on the main port as well) | – |
| `A2A_BASE_PATH` | Path prefix of the A2A routes on that port, e.g. `/agents`; the card is then at `/agents/.well-known/agent-card.json` | none |
| `GRAUWOLF_CHAT_API` | `1` or `true` also serves `POST /api/chat` and `GET /api/signature` ([chapter 9](09-operations.md#the-chat-api-apichat-and-apisignature)); with `AGENT_SIGNATURE_PATH` or `agent-signature.json` and `agent-names.json` in `GRAUWOLF_DATA_DIR`; `--dockerfile` copies those two files next to the spec into `app/` | – |
| `GRAUWOLF_OTEL`, `GRAUWOLF_OTEL_CONTENT` | `1` or `true` exports the spans of every run via OTLP/HTTP, as `spec serve --otel` (target from `OTEL_EXPORTER_OTLP_ENDPOINT` and `OTEL_EXPORTER_OTLP_HEADERS`, service name from `OTEL_SERVICE_NAME`, default `grauwolf`), and continues an incoming `traceparent` ([chapter 9](09-operations.md#opentelemetry)); `GRAUWOLF_OTEL_CONTENT` also records contents. Any other value is a start-up error naming the variable. `--service` crates include the facade feature `otel` (about 19 more crates, which the builder image already compiles) | off |
| `GRAUWOLF_EVENTS` | `1` or `true` writes every event of every run, also of its sub-agents, as one JSON line to stderr, as `spec serve --events` ([chapter 9](09-operations.md#events-as-json-lines)); `agent deploy --events` sets it in a cluster ([chapter 13](13-setting-up-a-cluster.md#events-in-the-pod-log)). Any other value than `0`/`false`/empty is a start-up error naming the variable | off |
| `GRAUWOLF_PUBLIC_URL`, `GRAUWOLF_A2A_PUBLIC_URL` | Public URLs for the agent cards | `http://localhost:<port>` |

Providers and their keys come from the usual variables (`OLLAMA_HOST`, `OPENAI_API_KEY`, …). On
SIGTERM or SIGINT the service stops accepting connections and finishes running requests.

A generated service behaves like `spec serve`: the user (`user_id` or user token → that user's
long-term memory and key-value store; every user has their own), conversations (`conversation_id`, kept
in memory of the process unless `GRAUWOLF_CONVERSATIONS_DIR` or `GRAUWOLF_CONVERSATIONS_DB` is set), attachments, the events and text deltas of a run on `/run/stream` and
approvals (`event: approval` and `POST /approvals/{id}`, [chapter 9](09-operations.md)) all reach the
run. `/run`, A2A and MCP have no back channel, so tools that need approval are refused there. Costs
need `GRAUWOLF_PRICES`. How to run it in a container is described in
[chapter 9](09-operations.md#container-images).

### Identical dependencies: `--full-facade`

Normally the crate depends on the facade only with the features the spec needs (providers, MCP,
built-in tools, ...). With `--full-facade` it depends on the facade with all its features, whatever
the spec needs. The dependency graph and the feature sets are then the same for every spec, so a
build environment can compile all dependencies once and reuse them for every agent (the builder
image, [chapter 9](09-operations.md#container-images)). The price is a longer first build.

### A Dockerfile for the builder image: `--dockerfile`

```bash
grauwolf spec generate spec.json --service --dockerfile --out agent
docker build --network none agent          # or kaniko
```

`--dockerfile` implies `--service` and `--full-facade`, and takes the grauwolf crates from
`/opt/grauwolf/crates`, where the [builder image](09-operations.md#container-images) keeps them
(give `--grauwolf-crates` to use a different directory). It writes next to the crate:

- `Dockerfile`: builds `FROM ghcr.io/agentsw0rk/grauwolf-builder:<version of the CLI>` (change it
  with `--builder-image`). The first build step is a **version guard**: the builder image's
  `GRAUWOLF_VERSION` must equal the version of the CLI that generated the crate, otherwise the build
  fails with `crate generated by grauwolf X, builder is Y - regenerate with grauwolf Y or pass
  --builder-image with --allow-builder-mismatch`. For a deliberate other builder image give
  `--allow-builder-mismatch`: the guard is left out. The build then runs
  `cargo build --release --offline --features service`, so it compiles
  only the agent crate; the runtime stage is distroless (`gcr.io/distroless/cc-debian12:nonroot`),
  or Debian slim with `git` and `ca-certificates` (non-root) if the spec uses git tools. The image
  runs the binary `agent` in `/app` as user 65532 and listens on port 8080 (`SERVER_PORT`).
- `.dockerignore`.
- `app/`: the files the service reads at run time, copied from the spec's directory with their
  relative paths: knowledge sources and index, the memory store, the root of the file tools, and
  the specs of local sub-agents with their own files. It becomes `/app` in the image, the working
  directory and `GRAUWOLF_DATA_DIR`. Paths outside the spec's directory are not copied. Sub-agents
  run **offline**: `spec generate` resolves the whole [agent tree](08-external-tools-and-agents.md#agents-as-tools-agent_tools)
  and `app/` also holds what they need – the built code of their tools and the tool packages they
  use in `app/.grauwolf/tools/` and the `grauwolf.lock` of the tree – and the service loads them
  with `--locked`, from that cache, with no network, no toolchain and no git. The tool policy is
  applied when the image is built.
- `app/egress.json`: every destination of the agent tree, as `grauwolf spec egress --format json`
  prints it (run and build phase, told apart by `phase`; `any_port` marks a `fetch` host inside the cluster without a fixed port; the OTLP endpoint is not in it). A
  deployment derives its network rules from it, see
  [limiting egress](13-setting-up-a-cluster.md#limiting-what-an-agent-may-reach-egress).

Without `--tool-sources`, tool code is compiled to WASM by `spec generate` itself, before the
crate is written; the build stage then needs nothing for it. Run `spec generate` in the builder
image (it contains the CLI, the toolchain and the vendored dependencies), so that this step works
offline as well:

```bash
docker run --rm -v "$PWD:/work" -w /work ghcr.io/agentsw0rk/grauwolf-builder:<version> \
  grauwolf spec generate spec.json --dockerfile --out agent
```

### A project per harness: `grauwolf harness generate`

A **harness** says how an agent tree is packaged as one deployable project
([ADR-0128](../adr/0128-harness.md)). It is a file of its own, `<name>.harness.json`, next to
the specs – the spec says what the agent does, the harness how it is shipped:

```json
{ "version": 1, "name": "support-bot", "spec": "../agents/support.json" }
```

- `version`: the format version, `1`.
- `name`: a DNS label (`a`–`z`, `0`–`9` and `-`, starting with a letter, not ending with `-`,
  at most 63 characters). It names the project folder, and later the image, the Kubernetes
  namespace and service.
- `spec`: the root spec, relative to the harness file. Its local sub-agents belong to the same
  project.
- `image`, `port`, `env` (optional): how the project runs in Kubernetes – read by
  [`grauwolf-k8-ops agent deploy --harness`](13-setting-up-a-cluster.md#from-a-harness-file---harness).
  Without `port` the service listens on 8080. Without `image` the default is
  `<registry>/<name>:<grauwolf version>` when the nearest `grauwolf.toml` above the harness file
  names a registry:

  ```toml
  [harness]
  registry = "ghcr.io/agentsw0rk"
  ```

  Build and push the project under that name (`docker build build/bot -t
  ghcr.io/agentsw0rk/bot:<version>`). A rebuilt image of the same grauwolf version keeps its name, so
  `agent deploy` does not restart the pod; give a new build an `image` with a tag of its own.

```bash
grauwolf harness generate harnesses/support-bot.harness.json   # writes build/support-bot/
docker build --network none build/support-bot
```

`harness generate` writes what `spec generate --dockerfile` writes (crate, `Dockerfile`,
`.dockerignore`, `app/`) into `<out-dir>/<name>/` (`--out-dir`, default `build`) and adds
`GENERATED.md`. The folder is output only: generating again replaces it completely, so change the
harness file or its specs instead of the generated files. Several harnesses give several projects
side by side. Errors in the harness file name the file and the field, for example
`support-bot.harness.json:/name: must be a DNS label …`. The [build options](reference-cli.md#build-options)
are the same as for `spec generate`.

### Tool code as sources: `--tool-sources` and `grauwolf tools build`

With `--tool-sources`, `spec generate` compiles nothing and needs no Rust toolchain. A crate whose
spec has tools gets, besides the library:

- `tools/<name>/Cargo.toml` and `tools/<name>/src/lib.rs`: the guest crate of each tool, exactly
  what `spec build` compiles;
- `tools/tools.json`: the format version and, per tool, its name, the module file
  (`tools/<name>.wasm`) and the SHA-256 of the guest crate's two files;
- the same for the code of the graph ([chapter 4](04-flows.md#code-in-graphs-code-on_code-and-transform)):
  every `code` node, `on_code` condition and edge `transform` is a guest crate of its own, named
  after the node or edge (`code_main_score`, `condition_main_score_finish`,
  `transform_main_score_finish`), with a `"kind"` of `code`, `condition` or `transform` in the
  manifest (tools have none); generated crates run these modules in the sandbox like tools, so
  they behave exactly like the interpreter;
- `build.rs` (standard library only): stops `cargo build` with the message
  `tool modules missing: tools/add.wasm - run \`grauwolf tools build .\` in this directory first`
  while a module is missing.

The generated library still embeds `tools/<name>.wasm`; the files do not exist yet. Build them:

```bash
grauwolf spec generate spec.json --tool-sources --service --out agent
grauwolf tools build agent
cargo build --manifest-path agent/Cargo.toml --features service
```

`tools build <dir>` checks the hash of every guest crate against `tools/tools.json` (a tool whose
sources were edited is an error naming the tool: generate the crate again instead) and builds it
with the isolated tool build ([chapter 10](10-self-made-tools.md); own target directory, only the
allowed crates, offline). It needs the Rust toolchain with the target `wasm32-unknown-unknown`
and `serde_json` in the local cargo sources – the builder image has both. A module whose source hash
matches the one it was built from (stamp `tools/<name>.wasm.src-sha256`) is skipped; `--force`
rebuilds. Options: `--cache`, `--build-wrapper` as for `spec build`.

Errors are reported per tool, with the line and column in the tool code of the spec:

```text
add: line 1, column 14: mismatched types
```

`--format json` prints a report on standard output instead (exit code 1 if a tool failed), which
tools such as AgentWeaver can map to the tool node:

```json
{ "tools": [ { "name": "add", "ok": false, "skipped": false,
               "errors": [ { "message": "mismatched types", "line": 1, "column": 14 } ] } ] }
```

`ok` is `true` for built and skipped tools; `line` and `column` (from 1) are left out where the
compiler gives no position in the tool code.

With `--dockerfile`, the `Dockerfile` copies `tools/` first and runs
`grauwolf tools build . --cache /tmp/tool-cache` in a layer of its own, then copies the rest of the
crate and runs the offline `cargo build`. While the tools do not change, the layer is reused:

```bash
grauwolf spec generate spec.json --tool-sources --dockerfile --out agent
docker build --network none agent          # or kaniko
```

Limitations: `--tool-sources` does not work with `--check` (the check needs the modules) and the
spec's `tool_packages` are refused: OCI packages carry built modules, so generate without
`--tool-sources` or use a git source; git packages are not yet written as sources either.

### Checking with `--check`

With `--check`, grauwolf then runs `cargo check` for the crate – with feature `app`, so that
`run_app` is checked as well (with `--service`: with feature `service`, which includes `app`, so
that `src/main.rs` is checked too):

```bash
grauwolf spec generate docs/user-guide/examples/11-word-counter.json --out word_counter --check
```

Roughly:

```text
docs/user-guide/examples/11-word-counter.json: crate written to word_counter
docs/user-guide/examples/11-word-counter.json: cargo check (with feature app): no errors
```

If the compiler finds errors, grauwolf lists each one with file, line and column in the generated
crate, e.g.
`docs/user-guide/examples/11-word-counter.json: word_counter/src/lib.rs:12:5: error: …`, and exits with exit
code 1. Which `cargo` is called is determined by the environment variable `CARGO` (default:
`cargo`).

### Where do the grauwolf crates come from?

The generated `Cargo.toml` refers to the crates of a grauwolf checkout by path:

```toml
[dependencies]
grauwolf-agents = { path = "/home/you/grauwolf/crates/domain/grauwolf-agents" }
grauwolf-graph = { path = "/home/you/grauwolf/crates/domain/grauwolf-graph" }
# …
```

The default is the checkout your `grauwolf` CLI was built from. You choose a different one with
`--grauwolf-crates` – you pass its `crates/` directory:

```bash
grauwolf spec generate docs/user-guide/examples/11-word-counter.json --out word_counter \
  --grauwolf-crates /opt/grauwolf/crates
```

So the crate works wherever this checkout is located at the same path.

### Building tools: `--cache` and `--build-wrapper`

As with all commands that build tool code, you set the cache directory with `--cache` (default
`.grauwolf/tools`) and a wrapper around `cargo` with `--build-wrapper` (see
[chapter 10](10-self-made-tools.md#security)). A compile error in the tool code aborts and points
to the location in the spec, for example like this:

```text
docs/user-guide/examples/11-word-counter.json:/tools/0/code:1:42: error: …
```

## Automatic memory

If the spec has `long_term_memory` with `auto_recall`, `auto_remember` or `remember_compressed`,
the crate additionally gets `run_with_memory(executor, input, [external,] memory)`. With a memory
(`Some(…)`), the agent recalls matching facts before the run and stores new ones afterwards – just
like `spec run`. `run` and `run_with_tools` run without memory.

## What cannot be generated (yet)

Some capabilities need the interpreter's runtime. For them, `generate` rejects the spec:

| Spec field | Reason |
|---|---|
| `tools_dir` | The tools directory is only read at runtime. |
| `self_tools` | Self-made tools need build and sandbox at runtime. |

The message looks like this:

```text
docs/user-guide/examples/10-tool-builder.json: 'self_tools' is not yet supported by the generator – run the spec with `grauwolf spec run`
```

In addition, named types must be mappable to Rust: every type is an object
(`"type": "object"`), its fields have the types `string`, `integer`, `number`, `boolean` or
`array` (with `items`), and field names consist of lowercase letters, digits and `_`. Otherwise
`generate` reports the location, e.g.:

```text
ticket.json: /types/Ticket/properties/meta: type not supported: supported are "string", "integer", "number", "boolean" and "array"
```

Fields that are not listed under `required` become optional fields.

What the interpreter brings along at runtime, the crate takes as options of a run (`RunOptions`, see
above): budget, conversation, attachments, approvals, checkpoints and costs. OpenTelemetry and the
conversation stores of the CLI you set up in your own program (or use `spec run`/`spec serve`).

## Letting the LLM write specs

`grauwolf spec create` uses the **builder agent**: it receives your requirement, the JSON
Schema of the spec format and all node kinds and conditions, writes a spec and checks it itself.

```bash
grauwolf spec create "An agent that converts temperatures from Celsius to Fahrenheit and uses a tool for it." \
  --model ollama:qwen3.5:9b --out temperature.json
```

Output, roughly:

```text
temperature.json: spec 'temperature_converter' created (1 repair)
```

This is how the builder works:

1. The LLM writes a draft – via structured output, so always syntactically valid JSON.
2. grauwolf loads and validates it. If the spec is valid, its tool code is compiled as well (with
   `--cache` and `--build-wrapper` as with `spec build`).
3. Every problem goes back to the LLM with a JSON Pointer, e.g.
   `/tools/0/code: line 2, column 9: error: …` or `/graph/edges/3/to: unknown node 'answer'`.
   The LLM repairs it, and it goes back to step 2.
4. If everything is fine, grauwolf writes the spec to `--out` (missing directories are created).

| Option | Meaning |
|---|---|
| `--model <provider>:<model>` | LLM for the builder, e.g. `ollama:qwen3.5:9b` or `anthropic:claude-haiku-5-5`. The generated agent runs with exactly this model too – it is written into its spec. |
| `--out <file>` | target file of the spec |
| `--max-repairs <n>` | maximum number of repairs, default `3` |
| `--cache`, `--build-wrapper` | as with the tool build |

The provider's credentials come from the environment as always ([chapter 2](02-models.md)). If
the colon in `--model` is missing, grauwolf reports:

```text
--model expects <provider>:<model>, e.g. ollama:qwen3.5:9b, got: qwen3.5
```

If the repairs are not enough, `create` stops and lists the remaining problems:

```text
no valid spec after 3 repairs:
/graph/nodes/1/kind: unknown node kind 'llm_answer'
```

A generated spec is a starting point, not a finished product: read it, check it with
`spec graph` ([chapter 4](04-flows.md)) and try it with `spec run` before you use it.

## Key points

- `grauwolf spec generate <spec> --out <directory>` creates a Rust crate with `graph()`, `run()`
  and the ready-built tools; `--check` checks it with `cargo check`.
- `--service` adds the HTTP service binary `agent` behind the Cargo feature `service`, configured
  through environment variables (`SERVER_PORT`, `GRAUWOLF_SERVER_TOKEN`, `A2A_PORT`, …).
- `--grauwolf-crates` chooses the grauwolf checkout the crate refers to.
- `tools_dir` and `self_tools` cannot be generated; automatic memory can
  (`run_with_memory`). Named types must be mappable to Rust; their fields are `string`, `integer`,
  `number`, `boolean` or `array`.
- `grauwolf spec create "<requirement>" --model <provider>:<model> --out <file>` lets an LLM write
  and repair a valid spec (`--max-repairs`, default 3).

Continue with the [CLI reference](reference-cli.md) and the [spec reference](reference-spec.md).
