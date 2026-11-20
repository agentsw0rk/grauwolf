# ADR-0026: Runtime abstractions in the domain

- **Status:** Accepted (review iteration 1, 2026-11-06)
- **Date:** 2026-11-06

## Context
Timeouts, retry waits and true parallelism need a runtime; `tokio::sync::RwLock`, `JoinSet` and session closures (`AsyncFnOnce` without an expressible `Send` bound) do not fit the IO-free domain.

## Decision
- Ports `Clock` (now, sleep) and `Spawn` (spawn with `JoinHandle`) in `grauwolf-agents`; tokio adapters in the facade. **Refinement (2026-11-06):** `Clock` lives in `grauwolf-llm`, because the `PromptExecutor` already has to wait for retries and `grauwolf-llm` must not depend on `grauwolf-agents`; `grauwolf-agents` uses the same port.
- Concurrent tool calls within the same task via `futures::future::join_all`/`FuturesUnordered`; true parallelism (parallel nodes, sub-agents) via `Spawn`.
- LLM sessions as guard API: `let mut s = ctx.llm().write().await;` (`async-lock`), no closure API.
- Storage values are `Value` (ADR-0020), so `fork()` for parallel branches is cheap; the chosen branch replaces the context on merge.
- Cancellation via drop; the retry policy only supplies `next_delay`, and waiting happens via `Clock`.

## Consequences
The domain stays runtime-free and testable (deterministic test clock).
