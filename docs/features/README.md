# grauwolf – Feature catalog

grauwolf is a standalone Rust framework for AI agents. The API follows Rust idioms
(builder + generics, proc macros, `async`/`tokio`, axum).

## Epics

| Epic | Topic | File |
|------|-------|-------|
| E01 | Prompt & message model | [E01-prompt.md](E01-prompt.md) |
| E02 | LLM clients, executor, streaming | [E02-llm-clients.md](E02-llm-clients.md) |
| E03 | Tools | [E03-tools.md](E03-tools.md) |
| E04 | Agent core | [E04-agent-core.md](E04-agent-core.md) |
| E05 | Graph strategies | [E05-graph-strategies.md](E05-graph-strategies.md) |
| E06 | Feature system & observability | [E06-features-observability.md](E06-features-observability.md) |
| E07 | Memory & persistence | [E07-memory-persistence.md](E07-memory-persistence.md) |
| E08 | Embeddings & RAG | [E08-embeddings-rag.md](E08-embeddings-rag.md) |
| E09 | Protocols: MCP, A2A, ACP | [E09-protocols.md](E09-protocols.md) |
| E10 | Planner (LLM, GOAP) | [E10-planner.md](E10-planner.md) |
| E11 | Integrations & runtime | [E11-integrations.md](E11-integrations.md) |
| E12 | Testing | [E12-testing.md](E12-testing.md) |
| E13 | **Generative agents: JSON DSL → GraphAgent (codegen + interpreter)** | [E13-generative-agents.md](E13-generative-agents.md) |
| E14 | AgentWeaver integration: grauwolf as an export backend, then grauwolf studio (ADR-0126) | [E14-agentweaver.md](E14-agentweaver.md) |

## Priorities

- **M1 (MVP)** – One agent with OpenAI/Anthropic/Ollama, tools, functional + graph strategy, streaming, EventHandler, mock executor; AgentSpec (JSON DSL) with interpreter **and** codegen, tool sandbox (WASM).
- **M2** – Remaining providers, persistence/checkpoints, chat memory, history compression, MCP, tracing/OpenTelemetry, structured output complete.
- **M3** – A2A, ACP, RAG/long-term memory, planner/GOAP, axum integration, Bedrock AgentCore, CLI agents.
- **Out of scope** (✅ confirmed) – bindings for other languages, swappable HTTP clients, on-device runtimes.

## Crate structure

See [../architecture.md](../architecture.md) (layers domain/adapters/support/apps).

## Design decisions

Decisions D1–D15 are recorded as ADRs: [../adr/README.md](../adr/README.md) (D1→ADR-0001 … D15→ADR-0015, D5 in revised form).

## Implementation status (2026-11-10)

| Epic | Implemented | Open |
|------|-------------|------|
| E01 | Messages with parts (text, tool calls, reasoning), prompt builder, tool choice, structured responses, moderation port (OpenAI native, otherwise classification) | Markdown/XML prompt builder |
| E02 | Ollama, OpenAI (+ compatible), Anthropic with prompt cache, Gemini, Amazon Bedrock; streaming; executor with routing, retries, fallback models and response cache; multiple responses | further providers |
| E03 | Typed tools (`#[tool]`, `#[toolset]`, also via the facade), registry, tool packages from OCI registries and git repositories (pull, lockfile), tool code in WASM (also from files, tools directory at runtime, built by the agent itself; ADR-0073–0075), built-in file/user tools incl. regex search, agent as tool, parallel execution | Rollback; dropped: shell tool (ADR-0046); tool packages from OCI registries: pull and push, `grauwolf.lock`, Docker credential store, `tools pull/inspect/lock/pack/push` (F03.18, F03.21; ADR-0107 stages 1–2); git sources (F03.19, stage 3) and the tool policy with signatures (F03.22, stage 4) |
| E04 | Context with history, model, tools, storage, model switching, fork for branches | Agent service for many parallel runs |
| E05 | Graph engine (typed and on `Value`), subgraphs, catalogs, strategy templates (single_run, react, verified, chat), parallel branches, history compression (also to facts, F05.15, ADR-0076), limits per run (ADR-0078), Mermaid export (`spec graph`), chat as dialogue loop (`spec chat`, ADR-0064), content moderation (`llm_moderate`, `single_run` with `moderate`, ADR-0066) | – |
| E06 | Events, features, JSONL log, OpenTelemetry (with token usage, cost and budget per run, ADR-0067/0071/0072), interceptors, asynchronous approval of tool calls (also via `spec serve`) | Remote debugger |
| E07 | Chat memory (files, SQL), checkpoints (files, SQL) with resuming also in `plan`/`parallel` and with runtime handles (ADR-0077), long-term memory via `remember`/`recall` (file), automatic recall/storage (`auto_recall`/`auto_remember`, also without tools, only new facts; ADR-0069) | MySQL |
| E08 | Embeddings (Ollama, OpenAI), splitting into chunks, in-memory vector store with a persistent index (only changed content is re-embedded), search tool | Vector database for very large collections |
| E09 | MCP client (stdio, Streamable HTTP; tools, resources, prompts), A2A server (JSON-RPC, streaming), A2A client (remote agents in `agent_tools`), MCP server (stdio, HTTP; tools, resources, prompts), A2A streaming in the client | ACP (Agent Client Protocol for editors) |
| E10 | LLM planner with critic (also as node `plan`), GOAP as Rust API and in the DSL (node `goap`, ADR-0070; numbers, operators, priorities and sub-agent executors: F14.15) | – |
| E11 | CLI, HTTP service (axum, `spec serve` with `/run`, `/run/stream`, bearer token, user tokens via `--users`/`grauwolf token`, also for A2A) | Bedrock AgentCore, user management at runtime |
| E12 | Mock LLM, conformance suite for providers, equivalence tests interpreter ⇔ codegen | – |
| E13 | JSON DSL with validation, interpreter, codegen, builder agent with repair | –; dropped: source map for compile errors in the generated code (ADR-0053) |
| E14 | Decision node with an external decider (`decide`, `on_decision`, the model or a System One service such as RKEV; F14.12, ADR-0100) | AgentWeaver backend: (generated service ✅ F14.1, F14.10, F14.11, ADR-0093), `/api/chat`, `/api/signature` (F14.2–F14.4 ✅, ADR-0094), tool sources (F14.5 ✅, ADR-0095), images (CLI, offline builder image, `--dockerfile`: F14.6 ✅), model parameters, more built-in tools, command tool (F14.9 ✅, ADR-0099); Jira, Confluence and git tools (F14.17, ADR-0104); skills (F14.16 ✅, ADR-0103); flows (F14.13 ✅, ADR-0101), Bedrock (F14.18 ✅, ADR-0105), then code in graph agents, pgvector |
