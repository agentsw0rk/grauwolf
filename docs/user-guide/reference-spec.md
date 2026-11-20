# Spec reference

All fields of a spec (version 1) with type, default value and a link to the chapter that explains
them, plus all node kinds, conditions and validation errors. `grauwolf spec schema` prints the
complete JSON Schema.

General rules:

- Unknown fields are an error everywhere (`…:/<pointer>: unknown field (allowed: …)`).
- Paths in the spec (`root`, `sources`, `store`, `index`, `tools_dir`, `spec`, `code_file`,
  `tool_files`) are relative to the spec file.
- **IDs** (name of the spec, tool, node, subgraph and MCP server names) are snake_case:
  `[a-z][a-z0-9_]*`. **Type names** are PascalCase: `[A-Z][A-Za-z0-9]*`.
- All tools share **one namespace**: your own tools, MCP tools, the search tool, memory tools,
  sub-agents, built-in tools and the build tool of `self_tools`.

## Top level

| Field | Type | Default | Meaning | Chapter |
|---|---|---|---|---|
| `spec_version` | number | **required** | Version of the format; only `1` is supported | [1](01-getting-started.md) |
| `name` | ID | **required** | Name of the agent (also the crate name with `generate`, the tool name with MCP) | [1](01-getting-started.md) |
| `model` | [Model](#model) | **required** | LLM of the agent | [2](02-models.md) |
| `system_prompt` | text | – | System prompt; also serves as the description for A2A and MCP | [1](01-getting-started.md) |
| `types` | object `name → JSON Schema` | `{}` | Named types for inputs and outputs | [5](05-structured-data.md) |
| `input` | text | `"string"` | Input type: `string` or a name from `types` | [5](05-structured-data.md) |
| `output` | text | `"string"` | Output type: `string` or a name from `types` | [5](05-structured-data.md) |
| `tools` | list of [tools](#tools) | `[]` | Your own tools with code, run in the sandbox | [3](03-tools.md) |
| `tool_files` | list of paths | `[]` | Tool definitions from separate JSON files; appended to `tools` | [3](03-tools.md) |
| `tools_dir` | path | – | Folder whose tool files are loaded before every run | [3](03-tools.md) |
| `tool_packages` | list of [tool packages](#tool_packages) | `[]` | Ready-built tools from OCI registries | [3](03-tools.md#tool-packages-from-a-registry-tool_packages) |
| `skills` | list of [skill sources](#skills) | `[]` | Instruction folders the agent loads on demand (`load_skill`, `read_skill_file`) | [3](03-tools.md#skills-instructions-the-agent-loads-on-demand-skills) |
| `self_tools` | [Self-made tools](#self_tools) | – | The agent may build tools during the run | [10](10-self-made-tools.md) |
| `limits` | [Limits](#limits) | – | Limits per run: steps, tool calls, tokens, costs | [2](02-models.md) |
| `builtin_tools` | [Built-in tools](#builtin_tools) | – | File tools and user input/output | [3](03-tools.md) |
| `mcp_servers` | object `ID → MCP server` | `{}` | MCP servers whose tools the agent uses | [8](08-external-tools-and-agents.md) |
| `agent_tools` | object `ID → sub-agent` | `{}` | Other agents as tools | [8](08-external-tools-and-agents.md) |
| `knowledge` | [Knowledge](#knowledge) | – | Documents, searchable via a search tool (RAG) | [7](07-conversations-and-knowledge.md) |
| `long_term_memory` | [Memory](#long_term_memory) | – | Long-term memory | [7](07-conversations-and-knowledge.md) |
| `subgraphs` | object `ID → subgraph` | `{}` | Named sub-flows | [6](06-subgraphs-and-planners.md) |
| `strategy` | [Template](#strategy) | – | Ready-made flow instead of `graph`; excludes `graph` | [4](04-flows.md) |
| `graph` | [Graph](#graph) | empty | Flow of nodes and edges | [4](04-flows.md) |

## Nested fields

### Model

Applies to `model`, `subgraphs.*.model`, `knowledge.embedding_model` and
`long_term_memory.embedding_model`.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `provider` | text | **required** | Provider: `ollama`, `openai`, `anthropic`, `gemini`, `bedrock` ([chapter 2](02-models.md#amazon-bedrock)) or an OpenAI-compatible one (`openrouter`, `deepseek`, `mistral`, `groq`, `dashscope`, `xai`; [chapter 2](02-models.md#openai-and-compatible-services)) |
| `id` | text | **required** | Model ID at the provider, e.g. `qwen3.5:9b` or `claude-haiku-5-5` |
| `fallbacks` | list of `{ provider, id }` | empty | Fallback models, asked one after another when a request fails; not for embedding models ([chapter 2](02-models.md#fallback-models-fallbacks), ADR-0088) |

| `temperature` | number 0–2 | provider default | Sampling temperature ([chapter 2](02-models.md#temperature-and-thinking-temperature-think), ADR-0097); not for embedding models |
| `think` | bool | unset | Reasoning of Ollama models, takes precedence over `OLLAMA_THINK`; only for `ollama`; not for embedding models |

`fallbacks` entries also accept `temperature` and `think`.

### `tools`

| Field | Type | Default | Meaning |
|---|---|---|---|
| `name` | ID | **required** | Name under which the LLM calls the tool |
| `description` | text | **required** | Description for the LLM |
| `args` | JSON Schema | **required** | Schema of the arguments |
| `code` | text | required (or `code_file`) | Body of `fn tool(args: serde_json::Value) -> Result<String, String>` |
| `code_file` | path | – | Code from a file instead of `code`; exactly one of the two |

A file from `tool_files` or `tools_dir` contains the same fields; its `code_file` is relative to
the tool file.

### `tool_packages`

Each entry names exactly one source.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `oci` | text | – | Reference `registry/repository[:tag][@sha256:<digest>]`, e.g. `ghcr.io/acme/tools/math:1.0`; the registry host is always written out; without tag and digest the tag is `latest` |
| `tools` | list of names | all | The tools of the package to use |
| `as` | object `name → name` | `{}` | Renames imported tools: name in the package → name in the spec |
| `git` | text | – | Git source instead of `oci`: repository URL (`https://`; `http://` only for `localhost` and `127.0.0.1`; no user info, the token is `GIT_TOKEN`) |
| `path` | text | repository root | Git only: folder of the package in the repository (relative, no `..`) |
| `rev`, `tag`, `branch` | text | – | Git only, exactly one: a commit (7 to 40 lowercase hex digits), or a tag or branch that is resolved to a commit and locked |

The tools join the namespace of all other tools. Names that are known without fetching the package
(those listed in `tools`, after renaming) are checked by `spec validate`, which never needs the
network; the tools of an entry without `tools`, unknown tool names and the ABI version are checked
when the package is resolved (`spec run`, `spec build`, `tools lock`). Resolving writes
`grauwolf.lock` next to the spec.

### `skills`

Each entry names exactly one source; its folder holds one sub-directory per skill with a `SKILL.md`
(front matter `name` equal to the folder name, `description`; then the instructions).

| Field | Type | Default | Meaning |
|---|---|---|---|
| `dir` | text | – | Local directory relative to the spec (no `..`, not absolute) |
| `git` | text | – | Git source instead of `dir`: repository URL (`https://`; `http://` only for `localhost` and `127.0.0.1`; no user info, the token is `GIT_TOKEN`) |
| `path` | text | repository root | Git only: folder with the skills in the repository (relative, no `..`) |
| `rev`, `tag`, `branch` | text | – | Git only, exactly one: a commit (7 to 40 lowercase hex digits), or a tag or branch that is resolved to a commit and locked in `grauwolf.lock` (section `skills`) |
| `names` | list of names | all | The skills of the source to use (lowercase letters, digits, `-` and `_`) |

With `skills` the agent gets the tools `load_skill` and `read_skill_file`; the names are reserved
in the shared tool namespace. Skill names must be unique across all sources, every file at most
1 MiB, and no symbolic link may leave its skill folder; `spec validate` checks local directories,
git sources are checked when they are fetched.

### `self_tools`

| Field | Type | Default | Meaning |
|---|---|---|---|
| `enabled` | bool | `true` | Building allowed outside of subgraphs |
| `tool` | ID | `"create_tool"` | Name of the build tool |
| `max_tools` | number | `5` | At most this many new tools per run |
| `max_code_bytes` | number | `20000` | At most this many bytes of code per tool |
| `confirm` | bool | `true` | A human confirms every build |
| `save` | bool | `false` | Store built tools in the `tools_dir`; needs `tools_dir` |

### `limits`

| Field | Type | Default | Meaning |
|---|---|---|---|
| `max_steps` | integer ≥ 1 | 50 | Node executions per graph pass (subgraphs count separately) |
| `max_tool_calls` | integer ≥ 0 | unlimited | Tool calls per run including forks; beyond that a tool result instead of execution |
| `max_prompt_tokens` | integer ≥ 1 | unlimited | Tokens per request, estimated in advance; larger requests are not sent (ADR-0087) |
| `max_tokens` | integer ≥ 1 | unlimited | Input and output tokens per run (like `--max-tokens`) |
| `max_cost` | number > 0 | unlimited | Costs per run (like `--max-cost`); without a price table a load error (generated agents: `GRAUWOLF_PRICES`) |

CLI options take precedence per field. Impossible values → `InvalidLimit` at `/limits/<field>`.

### `builtin_tools`

| Field | Type | Default | Meaning |
|---|---|---|---|
| `tools` | list | **required** | File tools `read_file`, `list_directory`, `write_file`, `edit_file`, `search_files`; user tools `say_to_user`, `ask_user`; `think`; `fetch`; key-value tools `store_value`, `retrieve_value`, `list_keys`; todo tools `add_task`, `complete_task`, `remove_task`, `list_tasks`; `run_command` (needs `command` and `root`) |
| `root` | path | – | Directory under which the file tools and the git directory work; required for file tools and the key-value tools (`kv.json`) |
| `fetch` | `{ allowed_hosts, allow_http?, auth? }` | – | Hosts of the `fetch` tool: exact host/IP or `*.domain`; `allow_http` (default `false`) also allows `http://`; private, loopback and link-local addresses are refused unless the host is listed exactly (ADR-0098); `auth` maps hosts to a workload identity (`{ "<allowed host>": { "spiffe": … } }`, the form of `auth.spiffe` below): such hosts are called over `https://` only, with mTLS or a JWT-SVID, other hosts without (ADR-0110; `AuthHostNotAllowed`) |
| `command` | `{ allowed, shell?, i_understand_shell?, approve_each_call?, timeout_secs?, max_output_bytes?, env?, workload_api? }` | – | The `run_command` tool (ADR-0099): `allowed` programs (bare names or absolute paths; exactly `["*"]` in shell mode); `shell` (default `false`) runs `{ command }` with `sh -c` and needs `allowed: ["*"]` and `i_understand_shell: true`; `approve_each_call` (default `true`, shell mode only) asks before every call; `timeout_secs` 1–3600 (default 60); `max_output_bytes` 1–10485760 per stream (default 65536); `env` the variables the program gets (default `["PATH"]`, everything else is cleared); `workload_api` (default `false`) adds `SPIFFE_ENDPOINT_SOCKET` (and `SPIFFE_FETCH_TIMEOUT`) for the allowlisted programs, which then act as the agent; an error in shell mode (`WorkloadApiWithShell`, ADR-0110) |
| `jira` | `{ base_url }` | – | Atlassian Cloud site of the `jira_*` tools; `base_url` is `https://` (a path such as `/wiki` is ignored) |
| `confluence` | `{ base_url }` | – | Atlassian Cloud site of the `confluence_*` tools |
| `git` | `{ repo_url, directory?, push_policy?, use_token?, auth? }` | – | The one repository of the `git_*` tools: `repo_url` `https://` without user info, `directory` relative to `root` (or the spec directory), default `repo`; `push_policy` `new_branches` (default) or `any`; `use_token` (default `false`) authenticates with `GIT_TOKEN`; `auth` (`{ "<host of repo_url or a pattern>": { "spiffe": … } }`) uses mTLS or a JWT-SVID instead of the token for clone, fetch and push, excludes `use_token` (ADR-0110; `AuthHostNotRepository`, `GitTokenWithAuth`) |

The tools `jira_search_issues`, `jira_get_issue`, `jira_create_issue`, `jira_add_comment`, `jira_transition_issue`, `confluence_search_pages`, `confluence_get_page`, `confluence_create_page`, `confluence_update_page`, `git_checkout`, `git_list_branches`, `git_create_branch`, `git_commit` and `git_push` (ADR-0104) each need the section of their group. A git tool that is not listed is not offered: listing only `git_checkout` and `git_list_branches` makes the repository read-only.

### `mcp_servers.<id>`

Exactly one of `command` (local, stdio) and `url` (remote, Streamable HTTP or, with `transport: "sse"`, the older HTTP+SSE transport).

| Field | Type | Default | Meaning |
|---|---|---|---|
| `command` | text | – | Program that starts the server, e.g. `npx` |
| `args` | list of texts | `[]` | Arguments of the program |
| `url` | text | – | Endpoint of a remote server |
| `transport` | `"streamable_http"` or `"sse"` | `"streamable_http"` | Transport of the remote server; `sse` is the HTTP+SSE transport of protocol 2024-11-05 (ADR-0112); an error without `url` |
| `headers` | object `name → text` | `{}` | Headers for `url`; `${VAR}` is replaced from the environment |
| `tools` | list of tool names | **required** | Tools of the server that the agent uses; only these are offered |
| `resources` | bool | `false` | Offer resources as tools `<server>_list_resources` and `<server>_read_resource` |
| `auth` | `{ spiffe: … }` | – | Workload identity towards `url` instead of a secret in `headers`; see `auth.spiffe` below; an error for a `command` server and together with an `Authorization` header (ADR-0109) |
| `workload_api` | bool | `false` | Passes `SPIFFE_ENDPOINT_SOCKET` (and `SPIFFE_FETCH_TIMEOUT` if set) to the child process; only with `command`, otherwise `WorkloadApiWithoutStdio`. The child gets the agent's identity (ADR-0110) |

### `agent_tools.<id>`

Exactly one of `spec` (local) and `url` (remote, A2A). The key is the tool name.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `spec` | path | – | Spec file of the sub-agent |
| `url` | text | – | Agent card or base URL of an A2A agent |
| `headers` | object `name → text` | `{}` | Headers for `url`; `${VAR}` from the environment |
| `description` | text | with `url`: from the agent card | Description for the LLM; required with `spec` |
| `auth` | `{ spiffe: … }` | – | Workload identity towards `url` instead of a secret in `headers`; see `auth.spiffe` below; an error for a local (`spec`) agent and together with an `Authorization` header (ADR-0109) |

### `auth.spiffe` (in `agent_tools.<id>` and `mcp_servers.<id>`)

Authenticates this agent towards a remote agent (A2A) or MCP server with its SPIFFE identity (ADR-0109, user guide chapter 9). Only with `url`; not together with an `Authorization` header.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `mode` | `"mtls"` or `"jwt"` | `"mtls"` | `mtls`: the client presents its X.509-SVID and checks the server's SPIFFE ID against the bundle; `jwt`: a fresh JWT-SVID for `audience` as `Authorization: Bearer`, for targets behind a TLS-terminating ingress |
| `id` | SPIFFE ID | – | The server must have exactly this ID, e.g. `spiffe://agentswork.de/ns/reviewer/sa/reviewer` |
| `ids` | list of SPIFFE IDs | – | The server must have one of these IDs |
| `trust_domain` | text | – | The server may have any ID of this trust domain |
| `audience` | text | – | Audience of the JWT-SVID; required with `mode: "jwt"`, an error with `mtls` |

With `mtls`, exactly one of `id`, `ids` and `trust_domain` is required; with `jwt`, one of them is optional (then the TLS connection is verified against the SPIFFE bundle instead of the web PKI). The target URL must be `https://`. Without `SPIFFE_ENDPOINT_SOCKET`, loading the spec fails and names the tool or server.

### `knowledge`

| Field | Type | Default | Meaning |
|---|---|---|---|
| `sources` | list of paths | `[]` | Files or directories (recursive); at least one of `sources` and `urls` |
| `urls` | list of URLs | `[]` | Web pages fetched at every start with conditional requests (ADR-0113): `https://` (`http://` only for `localhost`/`127.0.0.1`), no credentials, no fragment, no duplicates; text, Markdown, HTML |
| `min_similarity` | number | – | Minimum similarity 0 to 1 of a hit |
| `inject` | boolean | `false` | Adds the best `top_k` sections to every new user input as a system message |
| `tool_enabled` | boolean | `true` | Offers the search tool; `false` needs `inject: true` |
| `embedding_model` | model | **required** | Embedding model, e.g. `ollama`/`nomic-embed-text` |
| `tool` | ID | `"search_knowledge"` | Name of the search tool |
| `top_k` | number | `3` | Sections per search |
| `chunk_size` | number | `1000` | Maximum length of a section in characters |
| `index` | path | – | File in which sections and embeddings persist between runs (store `file` only) |
| `store` | object | `{ "kind": "file" }` | `{ "kind": "file" }` or `{ "kind": "postgres", "url_env": "PGVECTOR_URL" }`: PostgreSQL with pgvector (ADR-0106); `url_env` names the environment variable with the connection URL, the URL never belongs in the spec |

### `long_term_memory`

| Field | Type | Default | Meaning |
|---|---|---|---|
| `store` | path | – | JSON file with facts and embeddings; exactly one of `store` and `vector_store` |
| `vector_store` | object | – | PostgreSQL with pgvector (ADR-0106): `{ "kind": "postgres", "url_env": "PGVECTOR_URL", "ttl_days"?: n, "min_similarity"?: 0..1 }`; `url_env` names the environment variable with the connection URL; `ttl_days` (at least 1) lets facts expire, `min_similarity` drops weak matches from recall; without them facts never expire and `top_k` facts are always returned |
| `embedding_model` | model | **required** | Embedding model |
| `remember_tool` | ID | `"remember"` | Name of the tool for remembering |
| `recall_tool` | ID | `"recall"` | Name of the tool for recalling |
| `top_k` | number | `3` | Facts per recall |
| `auto_recall` | bool | `false` | Before every run, recall matching facts and attach them as a system message |
| `auto_remember` | bool | `false` | After every successful run, store facts |
| `remember_compressed` | bool | `false` | Store lasting facts from history compression into facts (`chat`, `compress_history` with `facts`) (ADR-0079) |
| `tools` | bool | `true` | Offer the two tools; with `false` it needs `auto_recall`, `auto_remember` or `remember_compressed` |

### `subgraphs.<id>`

| Field | Type | Default | Meaning |
|---|---|---|---|
| `input` | text | `"string"` | Input type |
| `output` | text | `"string"` | Output type |
| `tools` | list of tool names | all tools | Tools available in the subgraph |
| `self_tools` | bool | as outside | Allow (`true`) or forbid (`false`) building and using self-made tools; needs `self_tools` at the top |
| `model` | model | outer model | Own model in the subgraph |
| `graph` | graph | **required** | Flow; `start` delivers the input, `finish` takes the output |

### `strategy`

| Field | Type | Default | Meaning |
|---|---|---|---|
| `kind` | text | **required** | `single_run`, `react`, `verified` or `chat` |
| `params` | object | – | Parameters depending on `kind` |

| `kind` | Parameters | Flow |
|---|---|---|
| `single_run` | `parallel_tools` (bool), `moderate` (bool) | Tool loop `call_llm` → `execute_tool` → `send_result`; an answer with several tool calls takes `execute_tools_in_order` → `send_results` and runs them one after another (all strategies except `parallel_tools`); with `parallel_tools` the calls of one answer run concurrently (`execute_tools`); with `moderate` a moderation first (creates type `Moderation`) |
| `react` | `reasoning_prompt` (text) | think before every action (`think`, `reflect`; `reflect_all` after several calls in one answer) |
| `verified` | `verify_prompt` (text) | have the answer checked and revise it with the feedback (creates type `Verification`) |
| `chat` | `max_messages` (≥ 1, default 30), `keep` (≥ 0, default 10, less than `max_messages`), `parallel_tools` (bool, default `true`) | before the answer `compress` (`compress_history` with `facts`) when the history is longer than `max_messages`; then a tool loop like `single_run` |

The created type names are reserved; your own type with the same name is an error.

### `graph`

| Field | Type | Default | Meaning |
|---|---|---|---|
| `nodes` | list of nodes | `[]` | `start` and `finish` exist implicitly |
| `edges` | list of edges | `[]` | Directed edges; the first matching one wins |

**Node:** `id` (ID, required, not `start`/`finish`), `kind` (required, see
[Node kinds](#node-kinds)), `params` (depending on the kind).

**Edge:** `from` and `to` (required; node ID, `start` or `finish`), `when` (optional: `kind` and
`params` of a [condition](#conditions)), `transform` (optional, see below). Without `when` the
value is passed on unchanged.

**`transform`** (ADR-0102): code that maps the value of the edge before the target node receives it.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `input` | type name | what the edge delivers | Type of the value; if given, it must be exactly what the edge delivers (output of the source node, after the condition) |
| `output` | type name | required | Type of the result; the target node receives it and it must be the target's input type |
| `code` | text | required (or `code_file`) | Body of `fn transform(value: In) -> Out` |
| `code_file` | path | – | Code from a file relative to the spec instead of `code`; exactly one of the two |

It runs after the condition matched and before the target runs; the transformation is a pure
function in the sandbox like tool code. Types: `string`, a type from `types` or `array<T>` of
these. See [chapter 4](04-flows.md#code-in-graphs-code-on_code-and-transform).

## Data types on edges

| Type | Meaning |
|---|---|
| `string` | Text |
| `message` | Answer of an LLM (text and/or tool calls) |
| `tool_call` / `tool_calls` | one or all tool calls of an answer |
| `tool_result` / `tool_results` | result of one or several tool calls |
| `<Name>` | named type from `types`; compared by name |
| `array<T>` | list of values of the type `T` (`string` or a type from `types`); only in the parameters of `agent`-flow nodes (`for_each`) and in subgraph signatures, never as a type of the agent itself (ADR-0101) |

## Node kinds

"Input → output" is the signature against which the edges are checked. Parameters without a
marker are optional; unknown parameters are an error.

| Kind | Parameters | Input → output | Meaning |
|---|---|---|---|
| `llm_request` | – | `string` → `message` | ask the LLM, tools offered |
| `llm_request_streaming` | – | `string` → `message` | like `llm_request`, text is output live |
| `llm_request_only_tools` | – | `string` → `message` | a tool call is required |
| `llm_request_force_tool` | `tool` (required) | `string` → `message` | exactly this tool must be called |
| `llm_request_best_of` | `choices` (required, ≥ 1), `criteria` | `string` → `message` | several answers, the best goes into the history |
| `llm_request_structured` | `type` (required), `retries` (≥ 0) | `string` → `type` | answer as a named type |
| `llm_continue` | – | `string` → `message` | ask again without appending the input |
| `llm_reason` | `value` (required: `string`, `tool_result` or `tool_results`), `prompt` | `value` → `string` | think without tools |
| `llm_verify` | `type` (required), `prompt` | `string` → `type` | check the answer; verdict with `correct`, `feedback`, `answer` |
| `llm_moderate` | `type` (required), `provider` + `id` (only together), `temperature`, `think`, `fallbacks` | `string` → `type` | moderate text; verdict with `harmful`, `categories`, `text`, `message` |
| `execute_tool` | – | `tool_call` → `tool_result` | execute a tool |
| `llm_send_tool_result` | – | `tool_result` → `message` | send the result to the LLM |
| `execute_multiple_tools` | `sequential` (bool, default `false`) | `tool_calls` → `tool_results` | execute several tools concurrently; with `sequential: true` one after another in call order |
| `llm_send_multiple_tool_results` | – | `tool_results` → `message` | send several results |
| `get_field` | `type`, `field` (both required) | `type` → `string` | one field of a named type as text |
| `compress_history` | `value`, `strategy` (required: `summarize`, `keep_last` or `facts`), `keep` (≥ 0, required with `keep_last` and `facts`) | `value` → `value` | compress the history (`facts`: older messages as a list of facts), pass the value through |
| `agent` | `agent` (required: name of an entry of `agent_tools`), `input`, `output` (type names, default `string`; not lists; they must be the types of the sub-agent's spec) | `input` → `output` | run the sub-agent as a step: own history, usage/cost/tool-call counters, limits and approvals shared with the caller (ADR-0101) |
| `decide` | `value`, `kind` (`choice`, `yes_no` or `score`), `question` (all required); `options` (required for `choice`: 1–255 `{ name, description? }`, unique names), `levels` (required for `score`: 1–255 unique descriptions, lowest first), `threshold` (`yes_no`, 0–1, default 0.5), `yes`, `no` (`yes_no`: what they mean), `decider` (`system_one` or `llm`, default `llm`), `model` (a System One checkpoint, or `provider/id` for `llm`), `temperature` (0–2), `think` (bool; only `ollama`) – both only with the `llm` decider, for the decision request only | `value` → `value` | ask a decider a question about the value; the value passes through, the verdict goes to the storage under `grauwolf.decision` (ADR-0100) |
| `set_model` | `provider`, `id`, `value` (all required), `temperature`, `think`, `fallbacks` | `value` → `value` | switch the model for the rest of the run; `fallbacks` as with `model` (ADR-0088) |
| `store` | `key`, `value` (required) | `value` → `value` | store a value in the agent's storage |
| `load` | `key`, `value`, `type` (required) | `value` → `type` | fetch a value from the storage; the input is ignored |

These kinds are not in the catalog but include subgraphs ([chapter 6](06-subgraphs-and-planners.md)):

| Kind | Parameters | Input → output | Meaning |
|---|---|---|---|
| `subgraph` | `subgraph` (required) | like the subgraph | run a subgraph |
| `plan` | `subgraph` (required, `string` → `string`), `critic` (bool), `max_iterations` (≥ 1, default 20) | `string` → `string` | LLM planner; every step runs in the subgraph |
| `parallel` | `subgraphs` (required, ≥ 2 with the same signature), `merge` (required: `concat` – only with output `string` –, `index` with `index`, `llm_select` with optional `criteria`) | like the branches | branches concurrently, then merge |
| `for_each` | `body` (required: `{ "agent": "<name>" }` or `{ "subgraph": "<name>" }`), `input` (`array<T>` or `string`, default `array<string>`), `output` (`array<U>` or `string`, default `array<string>`), `max_parallel` (1–16, default 1), `on_error` (`fail` or `skip`, default `fail`) | `input` → `output` | run the body once per element of a JSON list on its own fork, `max_parallel` at a time; results in input order; with `skip` failed elements are left out (event `for_each_item_failed`); progress per element under `grauwolf.for_each.<id>`; text `input` is parsed as a JSON array (a code fence is ignored), text `output` is the list as JSON text (ADR-0101) |
| `code` | `input`, `output` (required: `string`, a type from `types` or `array<T>` of these), `code` (required, or `code_file`: a path relative to the spec; exactly one of the two) | `input` → `output` | run the body of `fn run(input: In) -> Result<Out, String>` in the sandbox (tool ABI v1, fuel and memory limits) on the value; `Err(message)` fails the node; a pure function: no history, storage, tools, model, clock or randomness (ADR-0102) |
| `goap` | `state` (required), `values`, `actions` (required, ≥ 1), `goals` (required, ≥ 1), `input_fact`, `output_fact`, `max_iterations` (≥ 1, default 20), `max_search_nodes` (≥ 1, default 10000), `max_plan_depth` (≥ 1, default 50) | `string` → `string` | planning over named facts |

For `goap`: `state` is `{ "fact": true|false|integer|number|"text" }`; the type follows the initial
value (`0` integer, `0.0` number); a text fact with a list in `values` (`{ "fact": ["text", …] }`) is an
enumeration, without a list a free text. Every action has `name` (unique) and exactly one executor:
`subgraph` (input `string`) or `agent` (an entry of `agent_tools`). Optional: `pre` (conditions: a
value, or one operator `eq`/`ne`/`gt`/`lt`/`ge`/`le`; ordering only for integers and numbers),
`effects`, `on_success` (default `effects`) and `on_failure` (values, or `{ "add": n }` for integers
and numbers), `cost` (≥ 0, default 1), `description`, `input` (template with `{{fact}}` and
`{{task}}`), `output_fact` (free-text fact for the answer) and `success_contains` (text). Every goal has
`name`, `facts` (conditions) and optionally `priority` (integer ≥ 0); the goal with the smallest
`cost / (priority + 1)` is pursued. `input_fact` and `output_fact` name free-text facts for the input
and the result of the node.

## Conditions

| Kind | Parameters | Input → passed on | Matches when … |
|---|---|---|---|
| `on_assistant_message` | – | `message` → `string` | the answer is text |
| `on_tool_call` | – | `message` → `tool_call` | the answer calls a tool |
| `on_multiple_tool_calls` | – | `message` → `tool_calls` | the answer calls tools (all calls) |
| `on_field` | `field`, `equals` (both required) | `<Type>` → `<Type>` | the field of a named type equals `equals` |
| `on_decision` | `answers` (required: non-empty list of unique answers) | any → unchanged | the answer of the latest decision is one of `answers`; only after `decide` (ADR-0100) |
| `on_history_longer` | `messages` (required, ≥ 0) | any → unchanged | the history has more messages |
| `on_history_tokens_above` | `tokens` (required, ≥ 0) | any → unchanged | the history has more tokens (estimated, ADR-0087) |
| `on_code` | `input` (required: the output type of the source), `code` (required, or `code_file`) | `input` → unchanged | the body of `fn check(value: &In) -> bool` returns `true`; a panic, trap or exhausted fuel is an error of the run, not `false` (ADR-0102) |

## Load errors

| Message | Meaning |
|---|---|
| `invalid spec JSON: …` | not valid JSON (with line and column) |
| `<pointer>: unknown field (allowed: …)` | typo in a field name |
| `<pointer>: missing required field` | a required field is missing |
| `<pointer>: wrong type: …, expected …` | e.g. a number instead of text |
| `<pointer>: unknown value …, allowed: …` | a value outside a fixed list of choices |
| `spec_version <n> is not supported (supported: 1)` | a different version |
| `/strategy: …` | unknown template, invalid parameters, next to `graph`, or a reserved type name |
| `` /tools/<i>: give exactly one of `code` and `code_file` `` | both given (without either: `/tools/<i>/code: missing required field`) |
| `/tool_files/<i>: …`, `/tools/<i>/code_file: …` | file missing, not readable or not JSON |
| `` /graph/nodes/<i>/params: give exactly one of `code` and `code_file` ``, `…/params/code_file: …` | both given, or the file is missing; the same at `…/edges/<i>/when/params` and `…/edges/<i>/transform`, and in `/subgraphs/<name>/graph` (ADR-0102) |
| `<spec>: <registry> …`, `<spec>: invalid tool package: …`, `<lockfile> is out of date: …` | errors of tool packages when a spec is loaded: registry unreachable or refusing, digest mismatch, not a grauwolf package, unknown ABI version, a tool name that does not exist or clashes, a lockfile that disagrees (see [chapter 3](03-tools.md#tool-packages-from-a-registry-tool_packages)) |

## Validation errors

`spec validate` reports all errors at once, each with a JSON Pointer:
`<file>:<pointer>: <message>`.

| Error kind | Message | Meaning and typical location |
|---|---|---|
| `UnknownNode` | `unknown node '<id>'` | an edge points to a node that does not exist (`…/edges/<i>/from` or `/to`) |
| `ReservedNodeId` | `'<id>' is reserved …` | a node is called `start` or `finish` |
| `InvalidId` | `'<id>' is not a valid ID …` | name of the spec, a tool, node, subgraph, server or similar is not snake_case |
| `DuplicateNodeId` | `node ID '<id>' is used more than once` | two nodes with the same ID in one graph |
| `DuplicateFallback` | `fallback model '<provider>:<id>' is already listed` | a fallback model equals the model or an earlier fallback (`…/fallbacks/<i>`) |
| `TemperatureOutOfRange` | `temperature must be a number from 0 to 2` | `…/temperature` of a model, fallback, `set_model`/`llm_moderate` or `decide` node |
| `ThinkNotSupported` | `'think' is only supported by the provider 'ollama', not '<provider>'` | `…/think` (of a `decide` node: the provider of its `model`, else of the main model) |
| `SamplingWithSystemOne` | `the 'system_one' decider is a decision model: it has no temperature or think` | `…/params/temperature` or `…/params/think` of a `decide` node with `decider: system_one` |
| `EmbeddingSampling` | `embedding models have no temperature or think` | `temperature`/`think` on an embedding model |
| `EmbeddingFallbacks` | `embedding models have no fallback models: a different model would produce different vectors` | `fallbacks` on `knowledge.embedding_model` or `long_term_memory.embedding_model` |
| `DuplicateToolName` | `tool name '<name>' is used more than once (the other one is the <origin> at <pointer>)` | a name that an earlier declaration of the shared tool namespace already has – reported at the later declaration, the text names the earlier one (origin: own tool, MCP tool, MCP resource tool, knowledge search tool, memory tool, built-in tool, skill tool, sub-agent, package tool after renaming, self-build tool, code piece). The order of declaration is: `tools`, `mcp_servers` (tools, then resource tools), `knowledge`, `long_term_memory`, `builtin_tools`, `skills` (`load_skill`, `read_skill_file`, at `/skills`), `agent_tools`, `tool_packages` (`/tool_packages/<i>/tools/<j>` or `/as/<name>`), `self_tools`, then the code pieces of the graph (`code_main_<node>`, …, at the piece's `…/code`) |
| `SkillSource` | `` give exactly one of `dir` and `git` `` | a `skills` entry with neither or both (`/skills/<i>`); other problems of an entry are `InvalidGitSource` (`/skills/<i>/dir`, `/git`, `/path`, `/rev`, …) or `InvalidId` (`/skills/<i>/names/<j>`) |
| `ToolPackageSource` | `` give exactly one of `oci` and `git` `` | a `tool_packages` entry with neither or both (`/tool_packages/<i>`) |
| `UnsupportedToolSource` | `git sources for tool packages are not supported yet` | `/tool_packages/<i>/git` |
| `InvalidOciReference` | `invalid OCI reference: …` | `/tool_packages/<i>/oci` is not `registry/repository[:tag][@sha256:…]` (the message never repeats the reference) |
| `UnknownNodeKind` | `unknown node kind '<kind>'` | `kind` is not in the catalog |
| `InvalidParams` | `invalid parameters for '<kind>': <reason>` | parameters of a node or a condition do not fit (e.g. `/extra: unknown parameter`) |
| `UnknownConditionKind` | `unknown condition '<kind>'` | `when.kind` unknown |
| `InvalidTypeName` | `'<name>' is not a valid type name …` | type name in `types` is not PascalCase |
| `UnknownType` | `unknown type '<name>' …` | `input`, `output` or a node parameter names an undeclared type |
| `FinishUnreachable` | `'finish' is not reachable from 'start'` | the graph can never deliver a result |
| `UnreachableNode` | `node '<id>' is not reachable from 'start'` | the node would never run |
| `DeadEnd` | `no path leads from node '<id>' to 'finish' …` | the run would get stuck there; an outgoing edge is missing |
| `UnknownSubgraph` | `unknown subgraph '<name>'` | a `subgraph`/`plan`/`parallel` node or a `for_each` body names a missing subgraph (for the body at `…/params/body/subgraph`) |
| `UnknownAgent` | `unknown agent '<name>': it is not in agent_tools` | an `agent` node or a `for_each` body names an agent that `agent_tools` lacks (at `…/params/agent` or `…/params/body/agent`) |
| `UnknownTool` | `unknown tool '<name>'` | `tools` of a subgraph names an unknown tool, or `builtin_tools.tools` an unknown built-in one, or `as` renames a tool that `tools` does not select (`/tool_packages/<i>/as/<name>`) |
| `SubgraphCycle` | `subgraph '<name>' includes itself` | direct or indirect self-inclusion |
| `McpTransport` | `` MCP server needs exactly one of `command` (local) and `url` (remote) `` | neither or both given |
| `AgentToolSource` | `` sub-agent needs exactly one of `spec` (local) and `url` (remote, A2A) `` | neither or both given |
| `AuthWithoutUrl` | `` `auth` only applies to a remote server or agent with `url` `` | `…/auth` of a `command` MCP server or a local `spec` sub-agent |
| `AuthWithAuthorizationHeader` | `` `auth` and an `Authorization` header exclude each other `` | `…/auth` next to an `Authorization` entry (any case) in `headers` |
| `SpiffeMatch` | `` `auth.spiffe` needs exactly one of `id`, `ids` and `trust_domain` `` | `…/auth/spiffe` in mode `mtls` without or with more than one; in mode `jwt` more than one |
| `SpiffeAudience` | `` `auth.spiffe` with mode `jwt` needs an `audience` `` | `…/auth/spiffe` |
| `SpiffeAudienceWithMtls` | `` `audience` only applies to mode `jwt` `` | `…/auth/spiffe/audience` |
| `AuthHostNotAllowed` | ``'<host>' is not in `allowed_hosts`: `auth` can only name an allowed host`` | a key of `builtin_tools.fetch.auth` that is not an entry of `allowed_hosts` (at `…/fetch/auth/<host>`) |
| `AuthHostNotRepository` | ``'<host>' is not the host of `repo_url`: `auth` can only name the repository's host`` | a key of `builtin_tools.git.auth` that does not match the host of `repo_url` |
| `GitTokenWithAuth` | `` `use_token` and `auth` exclude each other: the identity replaces `GIT_TOKEN` `` | `builtin_tools.git` with both |
| `WorkloadApiWithoutStdio` | `` `workload_api` only applies to a local MCP server with `command` (stdio) `` | `mcp_servers.<id>.workload_api` on a server with `url` |
| `WorkloadApiWithShell` | `` `workload_api` is refused in shell mode: … `` | `builtin_tools.command.workload_api` with `shell: true` |
| `InvalidSpiffe` | `invalid SPIFFE value: …` | `…/auth/spiffe/id`, `…/ids/<i>`, `…/trust_domain` that is not a SPIFFE ID (`spiffe://<trust domain>/<path>`, path segments without `.`/`..`/empty) or a trust domain (`a-z 0-9 . - _`) |
| `MissingAgentDescription` | `` local sub-agent needs a `description` `` | `agent_tools.<id>` with `spec` but without `description` |
| `UnusedMemory` | `` memory without tools needs `auto_recall`, `auto_remember` or `remember_compressed` `` | `long_term_memory.tools: false` without automatic memory |
| `InvalidLimit` | `invalid limit: …` | `limits.max_steps`, `max_tokens` or `max_prompt_tokens` 0, or `max_cost` ≤ 0 |
| `UnsupportedSchema` | `unsupported schema: …` | type schema outside the subset (object with fields of `string`, `integer`, `number`, `boolean`, `array` with `items`) |
| `SaveWithoutToolsDir` | `` `save` needs a `tools_dir` to store the tools in `` | `self_tools.save` without `tools_dir` |
| `SelfToolsNotConfigured` | `` `self_tools` in a subgraph needs `self_tools` at the top of the spec `` | a subgraph controls `self_tools`, but the spec does not allow it |
| `MissingBuiltinRoot` | `` built-in file and key-value tools need `root` (directory relative to the spec) `` | file tools or key-value tools without `builtin_tools.root` |
| `MissingBuiltinSection` | `` the tools of '<group>' need a `<group>` section in `builtin_tools` `` | a `jira_*`, `confluence_*`, `git_*`, `fetch` or `run_command` tool without `builtin_tools.jira`, `.confluence`, `.git`, `.fetch` or `.command` (at that path) |
| `EmptyAllowedCommands` | `allowed is empty: the command tool could not start any program` | `builtin_tools.command.allowed` is `[]` |
| `WildcardWithoutShell` | `'*' is only allowed together with "shell": true` | `*` in `allowed` (`…/allowed/<i>`) without shell mode |
| `InvalidAllowedProgram` | `'<program>' is not a program: …` | an entry that is empty, has spaces or is a relative path with a `/` |
| `ShellNeedsWildcard` | `shell mode needs exactly "allowed": ["*"]` | `shell: true` with another `allowed` |
| `ShellNotAcknowledged` | `shell mode needs "i_understand_shell": true` | `shell: true` without the acknowledgement |
| `CommandLimitOutOfRange` | `<field> is out of range` | `timeout_secs` or `max_output_bytes` is 0 or too large |
| `InvalidUrl` | `invalid URL: …` | `base_url` or `repo_url` is not `https://`, has no host or contains user info (the message never repeats the URL) |
| `EmptyAllowedHosts` | `allowed_hosts is empty: the fetch tool could not call any host` | `builtin_tools.fetch.allowed_hosts` is `[]` |
| `InvalidAllowedHost` | `'<host>' is not a host: …` | an entry with scheme, path, port or a stray `*` (`…/allowed_hosts/<i>`) |
| `DirectoryOutsideRoot` | ``the directory must be relative and inside the root, without `..` `` | `builtin_tools.git.directory` is absolute or contains `..` |
| `NoKnowledgeSources` | `knowledge needs at least one source` | `knowledge.sources` and `knowledge.urls` are both empty |
| `UnusedKnowledge` | `knowledge without the tool needs `inject: true`` | `knowledge.tool_enabled: false` without `inject` |
| `InvalidUrl` | `invalid URL: <reason>` | a `knowledge.urls` entry that is not `https://` (or `http://` for localhost), has credentials or a fragment, or is listed twice (also used by `fetch` and git sources) |
| `InvalidLimit` | `invalid limit: <reason>` | also `knowledge.min_similarity` outside 0 to 1 |
| `InvalidStore` | `invalid store: <reason>` | `knowledge.store` of kind `postgres` without `url_env` or with something that is not a variable name (the reason never repeats it), `url_env` on kind `file`, `knowledge.index` together with `postgres`; `long_term_memory` with none or both of `store` and `vector_store`, a `vector_store.url_env` that is not a variable name, `ttl_days` of 0, `min_similarity` outside 0 to 1 |
| `MissingCode` | `the code is missing or empty` | a `code` node, an `on_code` condition or a `transform` without code (`…/params/code`, `…/transform/code`) |
| `TransformInputMismatch` | `the transformation declares the input <a>, the edge delivers <b>` | `transform.input` differs from what the edge delivers (`…/edges/<i>/transform/input`) |
| `TypeMismatch` | `type error: target expects <a>, the edge delivers <b>` | an edge connects incompatible types |
| `ConditionInputMismatch` | `condition '<kind>' expects <a>, the source delivers <b>` | the condition does not fit the value of the edge (at `…/when`) |
| `SubAgentTypeMismatch` | `the agent '<name>' has the <input\|output> type '<a>', but the node declares '<b>'` | found by `spec validate` and when loading, not by the single-spec check: an `agent` node or `for_each` agent body declares other types than the sub-agent's own spec (at `…/params/input` or `…/params/output`) |
| `DecisionEdgeSource` | `` `on_decision` may only leave a `decide` node `` | an `on_decision` edge starts at another node (at `…/edges/<i>/when`) |
| `DecisionEdgeCondition` | `` edges leaving the `decide` node '<node>' must use `on_decision` `` | an edge of a `decide` node has no condition or another one (at `…/edges/<i>`) |
| `UnknownDecisionAnswer` | `the decision '<node>' has no answer '<answer>'` | `answers` names something that is no option, `yes`/`no` or level (at `…/answers/<j>`) |
| `DecisionAnswerUncovered` | `no edge handles the answer '<answer>' of the decision '<node>'` | an answer without an edge (at the node) |
| `DecisionAnswerDuplicated` | `the answer '<answer>' of the decision '<node>' is handled by more than one edge` | two edges for one answer (at the second edge) |

## Validation warnings

Warnings point at specs that are valid but probably not what you want. They never make
`spec validate` fail (exit code `0`); the commands that load a spec print them to stderr as
`warning: <file>:<pointer>: <message>`.

| Warning | Meaning and location |
|---|---|
| `shell mode gives the model a shell with the permissions of this process; …` | `builtin_tools.command.shell` is `true` (at `/builtin_tools/command/shell`) |
| ``answers with several tool calls have no edge; add an `on_multiple_tool_calls` edge …`` | a node whose answer can carry tool calls (`llm_request`, `llm_send_tool_result`, `llm_continue`, `llm_request_streaming`, …) has an `on_tool_call` edge, which needs exactly one call, but no `on_multiple_tool_calls` edge; if the model sends two calls in one answer the run stops with `no matching edge`. At the node (`…/nodes/<i>`), in subgraphs too. The [strategies](04-flows.md) show the pattern. |
