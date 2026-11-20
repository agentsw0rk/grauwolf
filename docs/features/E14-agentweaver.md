# E14 – AgentWeaver integration

grauwolf as an export backend of AgentWeaver, a visual editor for agent graphs
([ADR-0092](../adr/0092-agentweaver-export-backend.md)). Superseded by grauwolf studio
([ADR-0126](../adr/0126-grauwolf-studio.md), F14.27): the AgentWeaver frontend on a Rust backend that
edits grauwolf specs directly; the export path below stays as the history of the integration.

**Idea:** a harness in AgentWeaver with the backend `grauwolf` exports a grauwolf spec, generates a
Rust crate from it at export time and builds a small container image that serves AgentWeaver's
endpoints. AgentWeaver's runtime tab, cron jobs and signature comparison work unchanged.

```
AgentWeaver editor graph (harness, backend = grauwolf)
        │  AgentWeaver: GrauwolfBackend
        ▼
  spec.json (+ sub-agent specs, tool sources, knowledge files, agent-signature.json)
        │  grauwolf spec generate --dockerfile [--tool-sources]    (at export time, in the builder image)
        ▼
  Rust crate (Cargo.toml, src/lib.rs, src/main.rs, tools/*/src/lib.rs)
        │  Dockerfile on grauwolf-builder (generated): grauwolf tools build → cargo build --release --offline
        ▼
  image (distroless) ── k8s Deployment/Service ── POST /api/chat, GET /api/signature, /run, /health
```

## Features in grauwolf

| ID | Feature | Approach | Prio |
|----|---------|----------|------|
| F14.1 | **Generated service**: generated crate with `src/main.rs` serving the agent like `spec serve` | Trait `ServedAgent` in `grauwolf-server`, facade `grauwolf::service`, Cargo feature `service`, `spec generate --service` ([ADR-0093](../adr/0093-generated-service.md)) – ✅ implemented | M1 |
| F14.2 | **`POST /api/chat`** (`{message}`→`{response}`, typed objects) | Opt-in layer over `/run` (`--chat-api`, `GRAUWOLF_CHAT_API`; [ADR-0094](../adr/0094-chat-api.md)) – ✅ implemented | M1 |
| F14.3 | **`GET /api/signature`** with embedded `agent-signature.json` and observed models | Feature on `llm_call_completed`; `Completion.model` (also streamed runs, `StreamFrame::Model`); Ollama digest via `/api/tags` (ADR-0094) – ✅ implemented | M1 |
| F14.4 | Observed MCP tools in the signature | Schema hash by AgentWeaver's rule (ADR-0094) – ✅ implemented | M2 |
| F14.5 | **Tool sources in generated crates** and `grauwolf tools build` | `spec generate --tool-sources` ([ADR-0095](../adr/0095-tool-sources-in-generated-crates.md)) ✅ (`tools/<name>/` guest crates, `tools/tools.json`, `build.rs`, `tools build` with per-tool errors and `--format json`, Dockerfile layer; OCI packages refused, git packages not yet written as sources) | M1 |
| F14.6 | **Builder and CLI images** with offline builds | `docker/*.Dockerfile`, GitHub workflow to ghcr.io ([ADR-0096](../adr/0096-builder-and-cli-images.md)) – CLI image, agent runtime images, offline builder image (vendored and pre-built dependencies), `spec generate --dockerfile`/`--full-facade`/`--builder-image` and workflow ✅ (the images are CI-verified only); tool sources in the builder (F14.5) ✅ (CI-verified only); still open: the version check of the exporter against the builder | M1 |
| F14.7 | `model.temperature`, `model.think` | `LlModel` fields, all adapters ([ADR-0097](../adr/0097-model-temperature-and-think.md)) – ✅ implemented | M1 |
| F14.8 | Built-in tools `think`, `fetch`, key-value store, `todo` | `grauwolf-builtin-tools` ([ADR-0098](../adr/0098-more-builtin-tools.md)) – ✅ implemented | M2 |
| F14.9 | Opt-in command tool `run_command` (allowlist, optional shell mode) | `grauwolf-tools-command`, approval per call in shell mode ([ADR-0099](../adr/0099-command-tool.md)) – ✅ implemented | M2 |
| F14.10 | A2A on a separate port and base path | `A2A_PORT`, `A2A_BASE_PATH` (ADR-0093) – ✅ implemented (path prefix on the additional port) | M2 |
| F14.11 | Graceful shutdown on SIGTERM | ADR-0093 – ✅ implemented | M2 |
| F14.12 | Decision node with an external decider (System One API) or the model | Port `Decider`, node `decide`, adapter `grauwolf-systemone`, `temperature`/`think` for the `llm` decider ([ADR-0100](../adr/0100-decision-node.md)) – ✅ implemented | M3 |
| F14.13 | Multi-agent flows (`next`, `forEach`, `collect`, decisions as branches) | Nodes `agent` (a sub-agent from `agent_tools` as a step) and `for_each` (a body per element of a list, `max_parallel`, `on_error`, progress per element), `array<T>` types ([ADR-0101](../adr/0101-multi-agent-flows.md)) – ✅ implemented | M3 |
| F14.14 | Conditions, custom nodes and edge transformations as code in graph agents | Rust compiled to WASM like tools and run in the sandbox as pure functions: node `code`, condition `on_code`, edge `transform`; `code_file`; built by `spec build`, at run start and by `spec generate` (embedded; `--tool-sources` with kinds `code`/`condition`/`transform`); interpreter and generated crates run the same modules ([ADR-0102](../adr/0102-code-in-graph-agents.md)) – ✅ implemented | M3 |
| F14.15 | GOAP like AgentWeaver's planner: numbers, operators, priorities, executors, success checks | Addendum to [ADR-0070](../adr/0070-goap-in-the-dsl.md): integer, number and free-text facts, conditions `eq`/`ne`/`gt`/`lt`/`ge`/`le`, effects `add`, `on_success`/`on_failure`, `input` templates, `output_fact`, `success_contains`, `agent` executors, goal `priority`, `input_fact`/`output_fact`, bounded search – ✅ implemented | M3 |
| F14.16 | Skills (instruction folders, local or from git, copied into the image) | Spec section `skills` (list of sources `dir` / `git`), tools `load_skill`, `read_skill_file`, event `skill_loaded`, lockfile section `skills` ([ADR-0103](../adr/0103-skills.md)) – ✅ implemented | M3 |
| F14.17 | Jira, Confluence, Git | Built-in tools: adapter `grauwolf-atlassian` (REST), git via libgit2 (`git2`, no `git` program; [ADR-0104](../adr/0104-jira-confluence-git-tools.md)) – ✅ implemented | M3 |
| F14.18 | Bedrock provider | Adapter `grauwolf-bedrock`, Converse API, SigV4 ([ADR-0105](../adr/0105-bedrock-provider.md)) – ✅ implemented (verified against recorded responses; live check pending) | M3 |
| F14.19 | PostgreSQL/pgvector knowledge store | Adapter `grauwolf-store-pgvector`, incremental indexing, also for long-term memory with TTL and minimum similarity ([ADR-0106](../adr/0106-pgvector-knowledge-store.md)) – ✅ implemented (verified without a database; live check against PostgreSQL pending) | M3 |
| F14.20 | MCP over the old SSE transport | MCP client transport for the HTTP+SSE transport of protocol 2024-11-05: spec `mcp_servers.<name>.transport: "sse"` (default `streamable_http`, no automatic fallback), endpoint on the origin of `url` only, reconnect once after a lost stream ([ADR-0112](../adr/0112-mcp-over-http-sse.md)) – ✅ implemented | M3 |
| F14.22 | Workload identity for tools: `fetch`/git `auth.spiffe`, Workload API opt-in for stdio MCP and `run_command`, credential sources (`file`, OpenBao), tool gateway, built-in scion tools | [ADR-0110](../adr/0110-workload-identity-for-tools-and-credentials.md) | M3 |
| F14.23 | Delegation: Txn-Tokens from `grauwolf identity serve` (`/api/identity`), Entra ID entry, exchange at the boundary (Entra OBO, user-bound Atlassian tokens), grants with offline refresh tokens, `requires_delegation` | [ADR-0111](../adr/0111-delegation-with-transaction-tokens.md) | M3 |
| F14.21 | Workload identity with SPIFFE/SPIRE: mTLS and JWT-SVIDs for A2A and MCP, SPIFFE IDs mapped to users in the service | Adapter `grauwolf-spiffe`, spec `auth.spiffe`, stages outbound → inbound → AgentWeaver manifests ([ADR-0109](../adr/0109-workload-identity-with-spiffe.md)) | M2 |
| F14.25 | Agents calling agents in a cluster set up by `grauwolf-k8-ops`: `agent allow/deny/callers` (ConfigMap `grauwolf-callers`, NetworkPolicy `grauwolf-mtls-callers`), `agent deploy` with the mTLS port, the check of a spec's calls in `deploy` and `doctor`, `agent deploy --otel` (traces with `grauwolf.caller` in Tempo) | [ADR-0109](../adr/0109-workload-identity-with-spiffe.md) Q7-Q9, [ADR-0121](../adr/0121-grauwolf-k8-ops-agent-deploy.md) – ✅ implemented, live check open | M3 |
| F14.24 | SPIFFE federation: accept agents of other trust domains (`GRAUWOLF_SPIFFE_TRUST_DOMAINS`, federated bundles from the Workload API), `grauwolf-k8-ops federation add/list/remove` | [ADR-0122](../adr/0122-spiffe-federation.md) – proposed, later | M3 |
| F14.26 | Tool catalog: index artifact in the registry (`grauwolf-tools catalog list/search/show/add/remove/sync`), OCI annotations with description, build date, source, licence, versions, signature, sandbox limits and capabilities; `grauwolf-tools new` and a CI template for tool repositories | [ADR-0124](../adr/0124-tool-catalog.md) – ✅ implemented (tested against the in-memory registry; live check against a registry and publishing the `grauwolf-tools` image open) | M3 |
| F14.27 | **grauwolf studio, part 1**: the AgentWeaver frontend on a Rust backend that edits grauwolf specs of a local workspace directly – palette from the node catalog, validation at the node, Mermaid diagram, layout file with auto-layout, OpenAPI with a generated TypeScript client; part 1b: overview of the agent and its resources, the flow derived from a strategy with conversion to a graph (`POST /specs/flow`), working connections and edges | App crate `grauwolf-studio`, frontend `studio/web/` ([ADR-0126](../adr/0126-grauwolf-studio.md), `studio/README.md` (Enterprise, not in the public repository)) – ✅ implemented (part 2a: F14.29; MCP editor: F14.30; part 2b: GitHub workspaces) | M3 |
| F14.28 | MCP authorization (Protected Resource Metadata, Entra tokens at the MCP endpoint), tool access rules in the spec (`tool_access`, incl. `requires_delegation`), and the boundary exchange of the identity service (RFC 8693 with actor token, RFC 7523/OBO, stored connections) | [ADR-0127](../adr/0127-mcp-authorization-tool-rules-and-boundary-exchange.md) | M3 |
| F14.29 | **grauwolf studio, part 2a**: specs run in the studio process (runs API with SSE events, approvals, stop, chat turns, a budget per turn, at most two runs), the running node highlighted on the canvas, API keys stored locally and never shown again, the models of a provider and the tools of an MCP server in the inspector | `runs.rs`, `keys.rs`, `discovery.rs` of `grauwolf-studio`, the run panel of `studio/web` ([ADR-0126](../adr/0126-grauwolf-studio.md) decision 13) – ✅ implemented | M3 |
| F14.30 | **grauwolf studio as an MCP editor with live sync**: a shared working copy in the backend (documents with invertible JSON-pointer ops, base versions, rebasing of concurrent batches, save, revert, disk-change detection, an event stream), the frontend on it, `/mcp` (Streamable HTTP behind a per-start token) with 18 tools to read, edit, save and run specs, the connect dialog, the MCP status and highlighted remote changes | `ops.rs`, `documents.rs`, `mcp.rs`, `mcp_tools.rs` of `grauwolf-studio`, `store/documentStore.ts` of `studio/web` ([ADR-0126](../adr/0126-grauwolf-studio.md) decision 17) – ✅ implemented | M3 |
| F14.31 | **Undo and redo in grauwolf studio**: Ctrl/⌘+Z and Ctrl+Shift+Z/Ctrl+Y for the user's own changes, one step per action (a drag, a connection, typing until a pause), toolbar buttons and palette commands, steps rebased over the changes of MCP clients, and an **Undo** per MCP change in EVENTS | `lib/undo.ts`, `store/documentStore.ts` of `studio/web` ([ADR-0126](../adr/0126-grauwolf-studio.md) decision 18) – ✅ implemented | M3 |
| F14.32 | **Layout with ELK in grauwolf studio**: the flow and the overview laid out by the Eclipse Layout Kernel (`elkjs`) – layers left to right with crossing minimisation, orthogonal edges with rounded corners routed around the nodes, labels that cover no node or other label, subgraphs as groups (a planner's action subgraphs in a row below it); missing positions placed by ELK next to their neighbours, edges around moved nodes by a fallback router, **Tidy up** (⇧⌥F, button, palette) as one undo step, fits at zoom 0.6 or more | `lib/elk-layout.ts`, `lib/auto-layout.ts`, `lib/edge-routes.ts` of `studio/web` ([ADR-0126](../adr/0126-grauwolf-studio.md) decision 19) – ✅ implemented | M3 |
| F14.33 | **Harnesses**: a harness file `<name>.harness.json` per agent tree (name as DNS label, root spec), `grauwolf harness generate` writes its project into `build/<name>/`, `grauwolf-k8-ops agent deploy --harness` takes name, image, port and environment from it, the studio frames a harness on the overview of its root spec, edits it in the spec's inspector and lists all in the Harnesses side bar view, which also generates their projects; next: a workspace canvas, A2A between harnesses | `Harness` in `grauwolf-spec-model`, `commands/harness.rs` of `grauwolf-cli`, `src/harness.rs` of `grauwolf-k8-ops-app` ([ADR-0128](../adr/0128-harness.md)) – 🔄 steps 1 (format and CLI), 2 (`agent deploy --harness`) 3 (studio API) 4 (System tab, replaced by H9 stage 1) and H9 stage 1 (harness in the overview and a Harnesses side bar view) implemented | M3 |

## Features in AgentWeaver (tracked there, listed for the overview)

| ID | Feature | Prio |
|----|---------|------|
| A1 | `GrauwolfBackend`: agent model → spec(s), `grauwolf spec generate`, Dockerfile, k8s, signature, `agent-names.json` name mapping | M1 |
| A2 | Validation per backend (allowed nodes/fields), spec errors → editor nodes | M1 |
| A3 | Backend choice and defaults in the harness panel; frontend types | M1 |
| A4 | Tool editor in Rust for grauwolf harnesses | M1 |
| A5 | Runtime: deployable backend, required secrets from the spec, readiness probe `/health`, container hardening for `run_command` | M1 |
| A6 | Golden tests for the grauwolf backend | M1 |
| A7 | Build button (`cargo check` in the builder image, rustc JSON → nodes) | M2 |
| A8 | Preview through a grauwolf process (events → SSE) | M2 |
| A9 | Export review prompt for Rust/grauwolf | M3 |

## Mapping of editor nodes

| AgentWeaver | grauwolf spec | Status |
|---|---|---|
| agent: name, systemPrompt, modelProvider/modelId, maxIterations | `name`, `system_prompt`, `model`, strategy `react`, `limits.max_steps` | available |
| agent: temperature, ollamaThink | `model.temperature`, `model.think` | available (F14.7) |
| providers openai, anthropic, google, ollama / bedrock | openai, anthropic, gemini, ollama, `bedrock` | available (F14.18; `AWS_BEDROCK_*` names map onto the `AWS_*` variables in the deployment) |
| sub-agent (also delegation as a tool) | `agent_tools` with its own spec | available |
| tool (functions with typed parameters, body) | one spec tool per function, `args` schema from the parameters, Rust `code` | available (Rust: A4) |
| builtin filesystem (readOnly, rootDir) | `builtin_tools` read/list/search, write/edit only without readOnly, `root` | available |
| builtin user_prompt (exitTool) | `ask_user`/`say_to_user`, strategy `chat` | available |
| builtin think, fetch, memory, todo | built-in tools | available (F14.8) |
| builtin shell | `run_command` (shell mode) | F14.9 ✅ |
| builtin jira, confluence, git | built-in tools `jira_*`, `confluence_*`, `git_*` | available |
| mcp-server stdio / http / sse | `mcp_servers` stdio / http / `transport: "sse"` | available (sse: F14.20 ✅) |
| rag (embedder, files, threshold, maxDocuments, as tool or context) | `knowledge` (store `file` or `postgres`) | available (`store: postgres` with pgvector, F14.19 ✅; `urlSources` → `urls`, `similarityThreshold` → `min_similarity`, `maxDocuments` → `top_k`, `injectAsContext` → `inject`, `provideAsTool` → `tool_enabled`: [ADR-0113](../adr/0113-rag-urls-threshold-context.md)) |
| a2a-agent (client) / a2aConfig (server) | remote `agent_tools` / A2A of the service | available / available (F14.10) |
| planner-agent llm | node `plan` with critic | available |
| planner-agent goap | node `goap`: `initialState` → `state` (texts become free-text facts), `operator` `equals`/`notEquals`/`greaterThan`/`lessThan` → `eq`/`ne`/`gt`/`lt`, `op: increment` → `{ "add": value }`, `executor` → `agent`, `inputTemplate` → `input`, action `outputKey` → `output_fact`, `successContains` → `success_contains`, `onSuccess`/`onFailure` → `on_success`/`on_failure`, goal `priority` → `priority`, planner `inputKey`/`outputKey` → `input_fact`/`output_fact`, `maxIterations` → `max_iterations` | available (F14.15) |
| input/output (fields) | `types`, typed input/output | available |
| graph-agent: llm-request, llm-streaming, llm-structured, execute-tool, execute-multiple-tools, send-tool-result(s), compress-history, subgraph, do-nothing, start/finish | catalog nodes and edge conditions `on_assistant_message`, `on_tool_call`, `on_multiple_tool_calls` | available |
| graph-agent: `node-custom` (Kotlin body, input and output type) | node `code` (Rust; Kotlin is never translated automatically) | available (F14.14) |
| graph-agent: `node-condition` and edge condition (Kotlin) | `on_field` where recognizable (`field == literal`, `!field`), else `on_code` (Rust) | available (F14.14) |
| graph-agent: edge transformation (Kotlin) | edge `transform` (Rust) | available (F14.14) |
| decision (System One; choice, noul, score) | node `decide` (`choice`, `yes_no`, `score`) + `on_decision` edges | available (F14.12) |
| flows next/forEach/collect | nodes `agent`, `for_each`, `decide` in the top-level graph | available (F14.13) |
| skill | `skills` section (local `dir` or `git`), `load_skill`, `read_skill_file`, `skill_loaded` | available (F14.16) |
| cron | CronJob calls `/api/chat` | available (F14.2) |
| harness otelConfig | OpenTelemetry export (env) | available |
