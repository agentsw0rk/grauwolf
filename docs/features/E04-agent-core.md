# E04 – Agent core


| ID | Feature | Rust approach | Prio |
|----|---------|-------------|------|
| F04.1 | Basic agent (system prompt + model + tools, `run(input)`) | `Agent::builder().executor(e).model(m).system_prompt(..).tools(reg).build()?.run(input).await` | M1 |
| F04.2 | Generic agent `Agent<I, O>` with strategy | `Agent<I, O>`; `I`, `O` generic | M1 |
| F04.3 | Agent configuration (prompt, model, maxAgentIterations, missingToolsConversionStrategy) | `struct AgentConfig` | M1 |
| F04.4 | Functional strategy (arbitrary async logic) | `Strategy::functional(|ctx, input| async move { ... })` – closure receives `&mut AgentContext` | M1 |
| F04.5 | Agent context (LLM access, tools, storage, config, run ID, pipeline) | `struct AgentContext` | M1 |
| F04.6 | LLM sessions: read session / write session | Guard API `ctx.llm().read().await` / `.write().await` via `async-lock` (ADR-0026) | M1 |
| F04.7 | Session requests: requestLLM, requestLLMOnlyCallingTools, requestLLMForceOneTool, requestLLMMultipleChoices, requestLLMStructured, requestLLMStreaming | Methods on `WriteSession` | M1 |
| F04.8 | Manual history management (appendPrompt, rewritePrompt, switching model/tools) | Methods on `WriteSession` | M1 |
| F04.9 | Switching the LLM during the run, keeping the history | `session.set_model(..)` | M1 |
| F04.10 | Agent storage (typed key-value data between nodes) | `StorageKey<T: Serialize + DeserializeOwned>` (name + `PhantomData<T>`), values as `Value`; handles in `Extensions` (ADR-0020) | M1 |
| F04.11 | Environment (tool execution, reporting errors, termination) | `trait AgentEnvironment`; default calls `ToolRegistry` | M1 |
| F04.12 | Max iterations guard | Counter in the executor, `AgentError::MaxIterations` | M1 |
| F04.13 | Agent state & lifecycle (NotStarted, Running, Finished, Failed), `close()` | State enum; `close()` async, no async `Drop` | M1 |
| F04.14 | AIAgentService (factory for many runs, same config, parallel runs) | `AgentService<I, O>`: `Arc`-shared config, `create_agent()` per run | M2 |
| F04.15 | Run sessions with IDs (session/run ID for memory, persistence, tracing) | `RunId`, `SessionId` (UUID) | M1 |
| F04.16 | Execution info / path (current node path for events & checkpoints) | `ExecutionPath` Vec | M2 |
| F04.17 | Error/exception model | `thiserror` enum `AgentError` | M1 |
| F04.18 | Missing tools conversion (tool calls to text when a tool is missing / the model changes) | Enum in config | M2 |
| F04.19 | Java builder API / blocking API | **dropped**; possibly a `run_blocking()` helper with its own runtime | – |

## Rust specifics
- `AIAgentStorage` uses reified generics; in Rust a typed key, values as `Value` (persistable, ADR-0020).
- Agents can be "run once"; in Rust we can enforce that via typestate (`run(self)` consumes) or make them reusable – open.
