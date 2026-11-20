# Architecture Decision Records

| No. | Title | Status |
|-----|-------|--------|
| [0001](0001-async-runtime.md) | Async runtime tokio | Accepted |
| [0002](0002-typed-graph-builder.md) | Typed graph builder instead of a macro DSL | Accepted |
| [0003](0003-tool-definition.md) | Tool definition via trait and `#[tool]` macro | Accepted |
| [0004](0004-serde-schemars.md) | Serialization with serde, schemas with schemars | Accepted |
| [0005](0005-provider-adapters.md) | LLM providers as separate adapter crates with their own DTOs | Accepted |
| [0006](0006-error-handling.md) | Error handling with thiserror and Result | Accepted |
| [0007](0007-observability.md) | Domain events plus tracing/OpenTelemetry | Accepted |
| [0008](0008-http-reqwest.md) | HTTP exclusively via reqwest | Accepted |
| [0009](0009-web-axum.md) | Web integration with axum | Accepted |
| [0010](0010-serde-node-data.md) | All node data is serde-capable; persistence via sqlx | Accepted |
| [0011](0011-edition-msrv.md) | Edition 2024, MSRV 1.85 | Accepted |
| [0012](0012-project-name.md) | Project name grauwolf | Accepted |
| [0013](0013-codegen-fork.md) | Fork of carllerche/codegen as grauwolf-codegen | Accepted |
| [0014](0014-wasm-tool-sandbox.md) | Tool code from specs runs in a WASM sandbox | Accepted |
| [0015](0015-vertical-slices.md) | Vertical slices, spec-first | Accepted |
| [0016](0016-layered-architecture.md) | Layers domain/adapters/support/apps (ports & adapters) | Accepted |
| [0017](0017-value-engine.md) | One graph engine on serde_json::Value for all inputs | Accepted |
| [0018](0018-node-catalog.md) | Node catalog as the only source of node kinds | Accepted |
| [0019](0019-ai-guardrails.md) | Guardrails for AI-assisted development | Accepted |
| [0020](0020-execution-state.md) | ExecutionState as a complete, serializable run state | Accepted |
| [0021](0021-condition-catalog.md) | Condition catalog and expression language for edges | Accepted |
| [0022](0022-spec-type-system.md) | Spec type system: nominal types | Accepted |
| [0023](0023-streaming-side-channel.md) | Streaming is a side channel | Accepted |
| [0024](0024-tool-build.md) | Tool build separated from execution | Accepted |
| [0025](0025-strict-spec.md) | Strict spec deserialization, namespaces, IDs, versioning | Accepted |
| [0026](0026-runtime-abstractions.md) | Runtime abstractions in the domain | Accepted |
| [0027](0027-interceptors-features.md) | Interceptors as fold, feature catalog, enforcement of tool sets | Accepted |
| [0028](0028-custom-nodes-wasm.md) | Custom nodes in specs only as WASM transformations | Accepted |
| [0029](0029-tool-abi-v1.md) | Tool ABI v1 – core WASM module without imports | Accepted |
| [0030](0030-isolated-tool-build.md) | Isolated tool build in layers | Accepted |
| [0031](0031-structured-output.md) | Structured LLM responses (`llm_request_structured`) | Accepted |
| [0032](0032-assistant-message-parts.md) | Assistant messages made of parts (text, tool calls, reasoning) | Accepted |
| [0033](0033-streaming-port.md) | Streaming port and frames | Accepted |
| [0034](0034-subgraphs-checkpoints.md) | Subgraphs in spec and engine, checkpoints at node boundaries | Accepted |
| [0035](0035-builder-agent.md) | Builder agent creates and repairs specs | Accepted |
| [0036](0036-mcp-client.md) | Own MCP client, MCP tools as grauwolf tools | Accepted |
| [0037](0037-history-compression-chat-memory.md) | History compression as a node, chat memory via a store port | Accepted |
| [0038](0038-rag.md) | RAG – embedder port, vector store, search as a tool | Accepted |
| [0039](0039-planner.md) | Planner – plan loop, LLM planner with subgraph steps, node kind `plan` | Accepted |
| [0040](0040-agent-as-tool.md) | Agent as tool – specs call other specs as tools | Accepted |
| [0041](0041-strategy-templates.md) | Strategy templates in the DSL – `strategy` instead of `graph` | Accepted |
| [0042](0042-request-modes.md) | Request kinds – tool choice in the prompt, "best of n" via the port | Accepted |
| [0043](0043-model-switching.md) | Model switching within a run and own model per subgraph | Accepted |
| [0044](0044-agent-storage.md) | Agent storage – named values in the context, nodes `store` and `load` | Accepted |
| [0045](0045-parallel-branches.md) | Parallel branches – node kind `parallel` with merge strategy | Accepted |
| [0046](0046-builtin-tools.md) | Built-in tools – files under one root, user input/output via a port | Accepted |
| [0047](0047-interceptors.md) | Intervening interceptors for LLM and tool calls | Accepted |
| [0048](0048-sql-chat-store.md) | SQL store for conversation histories | Accepted |
| [0049](0049-opentelemetry.md) | OpenTelemetry – spans from agent events following the GenAI conventions | Accepted |
| [0050](0050-external-tools-in-generated-code.md) | External tools in generated code | Accepted |
| [0051](0051-mcp-streamable-http.md) | MCP over Streamable HTTP | Accepted |
| [0052](0052-progress-inside-nodes.md) | Securing progress inside long nodes (`plan`, `parallel`) | Accepted |
| [0053](0053-http-server.md) | Agents as HTTP service (`spec serve`) | Accepted |
| [0054](0054-a2a-server.md) | A2A server – making grauwolf agents reachable for other agents | Accepted |
| [0055](0055-a2a-client.md) | A2A client – remote agents as sub-agents | Accepted |
| [0056](0056-mcp-server.md) | MCP server – offering grauwolf agents as MCP tools | Accepted |
| [0057](0057-persistent-knowledge-index.md) | Persistent knowledge index – re-embed only what changed | Accepted |
| [0058](0058-long-term-memory.md) | Long-term memory via the tools `remember` and `recall` | Accepted |
| [0059](0059-memory-per-user.md) | Long-term memory separated per user | Accepted |
| [0060](0060-conversations-per-user.md) | Bind conversations to users | Accepted |
| [0061](0061-tool-approval.md) | Asynchronous approval of tool calls | Accepted |
| [0062](0062-anthropic-prompt-cache.md) | Prompt cache for Anthropic | Accepted |
| [0063](0063-tool-choice-without-forcing.md) | Tool choice for models without forced tool choice | Accepted |
| [0064](0064-chat-loop.md) | Chat as a dialog loop instead of a strategy | Accepted |
| [0065](0065-http-timeouts.md) | Timeouts for HTTP requests to LLM providers | Accepted |
| [0066](0066-content-moderation.md) | Content moderation | Accepted |
| [0067](0067-token-usage.md) | Token usage per LLM call | Accepted |
| [0068](0068-user-tokens.md) | User tokens in the HTTP service | Accepted |
| [0069](0069-automatic-memory.md) | Automatic recall and storage in long-term memory | Accepted |
| [0070](0070-goap-in-the-dsl.md) | GOAP in the DSL (addenda: enumerations; numbers, operators, priorities and executors) | Accepted |
| [0071](0071-costs.md) | Costs per LLM call and run | Accepted |
| [0072](0072-run-budget.md) | Budget per run | Accepted |
| [0073](0073-tool-files.md) | Tool code and tool definitions in separate files | Accepted |
| [0074](0074-tools-dir.md) | Tools directory that is loaded at runtime | Accepted |
| [0075](0075-self-made-tools.md) | The agent creates its tools itself | Accepted |
| [0076](0076-chat-strategy-and-fact-compression.md) | `chat` strategy and history compression with facts | Accepted |
| [0077](0077-checkpointed-runs-with-run-handles.md) | Checkpoint runs with runtime handles and self-built tools | Accepted |
| [0078](0078-run-limits-in-specs.md) | Limits per run in the spec | Accepted |
| [0079](0079-remember-compressed-facts.md) | Facts from history compression into long-term memory | Accepted |
| [0080](0080-stricter-spec-checks.md) | Stricter checking of specs and errors with JSON pointers | Accepted |
| [0081](0081-multimodal-attachments.md) | Multimodal attachments on user messages | Accepted |
| [0082](0082-openai-compatible-providers.md) | OpenAI-compatible providers with their own names | Accepted |
| [0083](0083-gemini-client.md) | Gemini client | Accepted |
| [0084](0084-generated-apps.md) | Generated crates provide runtime tools and memory themselves | Accepted |
| [0085](0085-english-readme.md) | README in English, German version alongside | Accepted |
| [0086](0086-prompt-cache.md) | Response cache for LLM requests | Accepted |
| [0087](0087-tokenizer.md) | Counting tokens before the request | Accepted |
| [0088](0088-model-fallbacks.md) | Fallback models on errors | Accepted |
| [0089](0089-protocols-complete.md) | A2A and MCP like `/run` – runtime tools, resources, prompts, streaming | Accepted |
| [0090](0090-english-documentation-and-messages.md) | Handbooks and messages in English | Accepted |
| [0091](0091-english-everywhere.md) | English everywhere except the German handbooks | Accepted |
| [0092](0092-agentweaver-export-backend.md) | grauwolf as an export backend of AgentWeaver | Superseded by 0126 |
| [0093](0093-generated-service.md) | Generated crates as HTTP services | Accepted |
| [0094](0094-chat-api.md) | Chat API: `/api/chat` and `/api/signature` | Accepted |
| [0095](0095-tool-sources-in-generated-crates.md) | Tool sources in generated crates, compiled during the image build | Accepted |
| [0096](0096-builder-and-cli-images.md) | Builder and CLI images | Accepted |
| [0097](0097-model-temperature-and-think.md) | `temperature` and `think` in the model of a spec | Accepted |
| [0098](0098-more-builtin-tools.md) | Built-in tools `think`, `fetch`, key-value store and `todo` | Accepted |
| [0099](0099-command-tool.md) | Opt-in command tool | Accepted |
| [0100](0100-decision-node.md) | Decision node with an external decider | Accepted |
| [0101](0101-multi-agent-flows.md) | Multi-agent flows – agents as steps, `for_each` and decisions | Accepted |
| [0102](0102-code-in-graph-agents.md) | Code in graph agents – conditions, custom nodes and edge transformations in Rust | Accepted |
| [0103](0103-skills.md) | Skills – instruction folders an agent loads on demand | Accepted |
| [0104](0104-jira-confluence-git-tools.md) | Built-in tools for Jira, Confluence and Git | Accepted |
| [0105](0105-bedrock-provider.md) | Amazon Bedrock as a provider | Accepted |
| [0106](0106-pgvector-knowledge-store.md) | PostgreSQL with pgvector for knowledge and long-term memory | Accepted |
| [0107](0107-tool-registry.md) | Tool packages from OCI registries and git repositories | Accepted |
| [0108](0108-restructuring-for-separation-of-concerns.md) | Restructuring for separation of concerns and testability | Accepted |
| [0109](0109-workload-identity-with-spiffe.md) | Workload identity with SPIFFE/SPIRE | Accepted |
| [0110](0110-workload-identity-for-tools-and-credentials.md) | Workload identity for tools, credential sources and credential gateways | Accepted (delegation: ADR-0111) |
| [0111](0111-delegation-with-transaction-tokens.md) | Delegation through transaction tokens and the identity service | Accepted |
| [0112](0112-mcp-over-http-sse.md) | MCP over the HTTP+SSE transport | Accepted |
| [0113](0113-rag-urls-threshold-context.md) | RAG: URL sources, similarity threshold and knowledge as context | Accepted |
| [0114](0114-provisioning.md) | Provisioning – one module that prepares everything an agent run needs | Accepted |
| [0115](0115-agent-tree.md) | The prepared agent tree – one walk over a spec and its sub-agents | Accepted |
| [0116](0116-run-lifecycle.md) | The run lifecycle – one sequence for every agent run | Accepted |
| [0117](0117-tool-namespace-and-builtin-groups.md) | One tool namespace and one table of built-in tool groups | Accepted |
| [0118](0118-grauwolf-tools-binary.md) | `grauwolf-tools` – a lean program for building and distributing tools | Accepted |
| [0119](0119-generated-code-parity.md) | Generated code runs like the interpreter | Accepted |
| [0120](0120-grauwolf-k8-ops.md) | `grauwolf-k8-ops` – setting up SPIRE and OpenBao for agents on Kubernetes | Accepted |
| [0121](0121-grauwolf-k8-ops-agent-deploy.md) | `grauwolf-k8-ops agent deploy` – running an agent in its namespace | Accepted |
| [0122](0122-spiffe-federation.md) | SPIFFE federation – agents of other trust domains | Proposed |
| [0123](0123-traces-across-agents-and-in-generated-services.md) | One trace across agents, and tracing in generated services | Accepted |
| [0124](0124-tool-catalog.md) | A tool catalog in the registry, richer package metadata, and a path for tool repositories | Accepted |
| [0125](0125-grant-consent-through-the-identity-service.md) | Grant consent through the identity service as a confidential client | Proposed |
| [0126](0126-grauwolf-studio.md) | grauwolf studio – the AgentWeaver frontend on a Rust backend | Accepted |
| [0127](0127-mcp-authorization-tool-rules-and-boundary-exchange.md) | MCP authorization, tool access rules and a standard token exchange at the boundary | Accepted |
| [0128](0128-harness.md) | Harnesses – the boundary of a generated, deployable project | Proposed |
| [0129](0129-building-harness-images-in-the-cluster.md) | Building harness images in the cluster – kubeconfig, Kaniko from git, push to a registry | Proposed |
| [0130](0130-project-repository.md) | The project repository – generated harness projects pushed to GitHub on every change | Proposed |

New ADRs: next free number, same format (Status, Context, Decision, Consequences). Superseded ADRs are not deleted but set to `Superseded by ADR-XXXX`.
