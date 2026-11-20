# ADR-0094: Chat API: `/api/chat` and `/api/signature`

- **Status:** Accepted (User, 2026-11-10: "grauwolf offers both"; 2026-11-11: "do the endpoints /api/chat and /api/signature"; naming without a product name, User 2026-11-11; ADR-0092)
- **Date:** 2026-11-10

## Context
The chat API is what AgentWeaver (a visual agent editor and one consumer) uses to talk to deployed agents, through two fixed endpoints: cron jobs and users call
`POST /api/chat`; the signature poller reads `GET /api/signature` every 5 minutes via the
Kubernetes service proxy (first service port, 5 s timeout) and compares it with the editor's
signature. The signature file `agent-signature.json` is written by AgentWeaver at export time; the
agent adds *observations*: the model versions it actually used and the MCP tools it saw. Neither the
cron job (`curl -X POST … -d '{"message": …}'`) nor the poller sends credentials; AgentWeaver's own
exports have no authentication.

AgentWeaver's wire format (`SignatureSupportGenerator`, `ObservationStore.parse`):
`{"signature": <JSON or null>, "observations": {"startedAt": <ISO-8601 instant>,
"models": {"<agent name>": [{"modelId": <string or null>, "firstSeen": <instant>, "lastSeen":
<instant>}]}, "mcpTools": {"<server name>": [{"tools": [{"name", "description", "schemaHash"}],
"firstSeen", "lastSeen"}]}}}`. A new entry is added when the observed value changes; repeated
sightings only move `firstSeen`/`lastSeen`. An unreadable `signature` is shown as "reports no
signature"; `observations` is required.

## Decision
- **Opt-in:** `ServiceConfig` field / env `GRAUWOLF_CHAT_API=1` (or `true`; generated services) and
  `spec serve --chat-api` (interpreter), `ServiceConfig::chat_api`. Deployments that use the chat
  API enable it (e.g. AgentWeaver's export); others do not see the routes.
- **`POST /api/chat`:** a thin layer over the same run as `/run`.
  - Spec input `string`: request `{"message": "…"}`; output `string`: response `{"response": "…"}`.
  - Typed input or output (object types): the request body *is* the input object, the response
    *is* the output object (AgentWeaver's typed input/output classes).
  - Synchronous, no conversation id; errors `400` (unreadable body, wrong shape) and `500` (run
    failed) with `{"error": "…"}`. Budgets and limits apply as for `/run`; tool approvals cannot be
    answered through this route (no approver: approval-needing calls are refused, as for runs
    without an approver).
  - Authentication as for `/run` (token or users file); such a deployment runs the service with
    `GRAUWOLF_ALLOW_ANONYMOUS=true` inside the cluster, because its cron jobs send no token.
- **`GET /api/signature`:** `{"signature": …, "observations": {...}}` in the format above.
  - Never requires a token (it contains no secrets; the poller sends none).
  - `signature`: the JSON of `agent-signature.json`, read once at start from
    `AGENT_SIGNATURE_PATH` or the service's base directory (`/app` in images, the spec directory for
    `spec serve`); `null` if absent or unreadable.
  - `observations.startedAt`: process start (RFC 3339, UTC).
  - `observations.models`: per agent name, entries `{modelId, firstSeen, lastSeen}`.
    `modelId` is the model the provider reported: a new optional field `Completion.model`
    (OpenAI, Anthropic, Gemini and OpenAI-compatible providers fill it from their response; it also
    goes into `llm_call_completed`). For Ollama it is `<model>@<first 12 hex of the digest>` from
    `GET <ollama host>/api/tags` (exact name match, or `<name>:latest` for a name without tag),
    cached per model for 10 minutes, looked up outside the request path; when unknown: the reported
    model, else `null`. In chat API mode `OLLAMA_BASE_URL` is accepted as a fallback for
    `OLLAMA_HOST` (AgentWeaver's deployments set the former).
  - `observations.mcpTools`: per MCP server name, entries `{tools, firstSeen, lastSeen}` with
    `tools` = `{name, description, schemaHash}` sorted by name, recorded whenever the agent lists
    the server's tools. `schemaHash` = SHA-256 (lowercase hex) over
    `required(<p>,…);optional(<p>,…)`, each parameter `name:type:description` in schema order, types
    `string`, `integer`, `number`, `boolean`, `enum[a,b]`, `list[<type>]`, `anyOf[<p>,…]`,
    `object[<p>,…;required=a,b;additional=<bool>:<type>]` (AgentWeaver's rule with JSON-schema type
    names; the hash only has to be stable for one agent, because notices compare observations of
    the same running agent).
  - Observations live in memory and are collected by a feature on the existing events
    (`llm_call_completed` per agent, a new event or hook when MCP tools are listed); the agent code
    is not touched. Sub-agents are recorded under their own names.
- **Agent names:** `agent-names.json` next to the main spec (`{"agents": {"<spec name>": "<agent
  name>"}}`) maps spec names to the agent names of the signature; without it, spec names are used. Like
  the signature, it is read from the base directory; `spec generate --dockerfile` copies both into
  `app/` when they lie next to the spec.

## Consequences
- AgentWeaver's runtime tab, cron jobs and signature comparison work for grauwolf harnesses
  without changes on the AgentWeaver side; its export sets `GRAUWOLF_CHAT_API=1`,
  `GRAUWOLF_ALLOW_ANONYMOUS=true` and writes `agent-signature.json` and `agent-names.json`.
- The signature format belongs to AgentWeaver; a change there needs a change here. Tests use a
  fixture shaped like AgentWeaver's exports and its parser's expectations.
- `Completion.model` is useful beyond AgentWeaver (events, OpenTelemetry `gen_ai.response.model`).

## Implementation notes
- **Where it lives:** the collector is `grauwolf::observations::Observations` (in `grauwolf-server` since ADR-0108 stage 4c; the
  architecture test forbids `sha2` in domain crates); `grauwolf::chat_api::ChatApi` reads the
  files and connects it. The server only knows `ChatApiOptions { signature, observations }`.
- **Keyed by value:** like AgentWeaver's own observer, an entry per distinct value; a value seen
  again only moves `firstSeen`/`lastSeen` (also when other values came in between).
- **Ollama digests** are looked up when `GET /api/signature` is answered (2 s timeout, cached
  per model for 10 minutes), not when the model answers: sightings of Ollama models are kept
  until the next report and get the digest known then.
- **MCP tools** are recorded where `mcp::start_servers` lists them (once per start of an agent;
  once per run in generated services), through the explicit `McpToolsObserver` that
  `Observations::mcp_tools_observer()` builds and the loader, `external::tools_for_observed` and
  `AppOptions.mcp_tools_observer` pass on (no process-wide hook since ADR-0108 stage 4c).
- **Streams report the model (2026-11-12):** streamed runs (`/run/stream`, `llm_request_streaming`
  nodes) record the model the provider reports like any other run. `grauwolf_llm::StreamFrame`
  has a variant `Model(String)`, emitted at most once before `End`: OpenAI from the chunk's
  `model`, Anthropic from `message_start.message.model`, Gemini from `modelVersion`, Ollama from
  the `model` of the last line; the default streaming of `LlmClient` replays
  `Completion.model`. `aggregate` ignores it, and the frame sinks of the server, A2A and CLI only
  look at `TextDelta`, so no client-visible output changes. The streaming gateway keeps the model
  of the candidate that answered (fallbacks, ADR-0088) and does not count the frame as output
  for the "no frame yet" rule.
- **Sub-agents in generated services (2026-11-12):** `AppOptions.agent_features` (a factory by
  spec, the loader's `AgentFeatures`) is handed to the `AgentLoader` in the generated
  `run_app_with`, which installs it in every sub-agent it loads; `GeneratedAgent::observed_by`
  sets it to `feature_for(spec name)`, so sub-agents are recorded under their own mapped names
  like in `spec serve --chat-api`. The main agent still gets its feature through `features`.
- **Live check (2026-11-12):** `grauwolf spec serve --chat-api` with a coordinator and a
  translator sub-agent (both `llm_request_streaming`, Ollama `qwen3.5:9b`, `OLLAMA_THINK=false`),
  one `POST /run/stream`: `GET /api/signature` lists `qwen3.5:9b@d63f830f28d5` under both
  `coordinator` and `translator`. Other providers are covered by recorded-response tests, and
  sub-agents in generated services by the generator's equivalence test
  (`generated_apps_install_the_agent_features_in_their_sub_agents`).
- **Deviations:** (3) Cached answers (`prompt_cache`) have no reported
  model (the cache stores the answer only), also when streamed.
- **Names:** no product name in switches and files: `GRAUWOLF_CHAT_API`, `spec serve --chat-api`,
  `ServiceConfig::chat_api`, `agent-names.json`.

