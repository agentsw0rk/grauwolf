# E07 – Memory & persistence


## Chat memory (short-term memory across runs)

| ID | Feature | Rust approach | Prio |
|----|---------|-------------|------|
| F07.1 | ChatMemory feature: load/store history per conversation ID | Feature that loads before the strategy starts and stores after it finishes | M2 |
| F07.2 | ChatHistoryProvider trait + in-memory | `trait ChatHistoryProvider` | M2 |
| F07.3 | SQL provider (Postgres, MySQL, H2) incl. schema migration | `sqlx` (Postgres, MySQL, **SQLite instead of H2**), `sqlx::migrate!` | M2 |
| F07.4 | AWS AgentCore short-term memory provider | `aws-sdk-bedrockagentcore` | M3 |
| F07.5 | Chat backend pattern (multi-user chat with memory) | Example with axum (E11) | M3 |

## Long-term memory

| ID | Feature | Rust approach | Prio |
|----|---------|-------------|------|
| F07.6 | Ingestion: extraction of memories from messages | `trait DocumentExtractor` | M3 |
| F07.7 | Retrieval + prompt augmentation (system/user prompt) | `trait PromptAugmenter`, impl System/User | M3 |
| F07.8 | Record storage (in-memory, vectors via RAG) | via E08 vector store | M3 |
| F07.9 | Failure policy (ignore memory errors vs. abort) | Enum | M3 |
| F07.10 | AWS AgentCore long-term memory | AWS SDK | M3 |
| F07.11 | Legacy memory model (facts, concepts, subjects, scopes) | integrate into F05.14/F05.15 | M2 |

Implementation (ADR-0058): the agent decides via the tools `remember`/`recall` what it stores and retrieves; F07.8 as a file-based `VectorStore` (`long_term_memory.store`). Added (ADR-0069): F07.6 as `auto_remember` (fact request after each successful run), F07.7 as `auto_recall` (system message before the input), F07.9 as a fixed policy "ignore errors, report as event".

## Persistence / checkpoints (snapshot)

| ID | Feature | Rust approach | Prio |
|----|---------|-------------|------|
| F07.12 | Create checkpoints (node path, input, message history, storage) | `ExecutionState` (ADR-0020): path stack, node input, prompt, model, params, tool names, storage, iterations | M2 |
| F07.13 | Restore from checkpoint / resume after a crash | Engine can enter at a node path; input deserialized per registered deserializer at the node | M2 |
| F07.14 | Continuous persistence (checkpoint after each node) | Config flag | M2 |
| F07.15 | Storage providers: None, in-memory, file, SQL (Postgres/MySQL/H2), cleanup config | `trait CheckpointStore`; impls in-memory, file (JSON), `sqlx` | M2 |
| F07.16 | Rollback of tool side effects | Registry: tool → counter-tool; execute backwards on restore | M3 |
| F07.17 | Manually setting execution points | `ctx.set_execution_point(node, input)` | M3 |
| F07.18 | Persist AIAgentStorage | Storage is always serializable (`StorageKey<T: Serialize + DeserializeOwned>`); handles in `Extensions` (ADR-0020) | M2 |

## Decision (2026-11-06)
**All** node input and output types must be `Serialize + DeserializeOwned + Send + 'static`. The graph builder enforces this via a trait bound (`trait NodeData`, blanket impl). The engine processes values internally as `serde_json::Value` (ADR-0017). The full checkpoint scope (session, storage, subgraph path) is the `ExecutionState` from ADR-0020.
