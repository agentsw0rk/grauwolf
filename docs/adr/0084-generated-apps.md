# ADR-0084: Generated crates provide runtime tools and memory themselves

- **Status:** Accepted (User, 2026-11-09: "do the generated code (spec generate) … MCP, RAG, sub-agents, built-in tools and automatic memory")
- **Date:** 2026-11-09

## Context
Since ADR-0050, generated crates accept external tools (MCP, knowledge, sub-agents, built-in, memory tools) via `run_with_tools`, but the caller has to assemble them with the facade. The generator rejects automatic memory (`auto_recall`, `auto_remember`, `remember_compressed`; ADR-0069, ADR-0079), because the logic lives only in the interpreter. Thus generated code does not behave like `spec run`.

## Decision
- **Shared memory logic:** recalling before the run and storing afterwards move from the interpreter to `grauwolf_agents::memory` (`recall_facts`, `remember_facts`, `text_of`); the interpreter and generated code use the same functions.
- **Generator:** specs with automatic memory get `run_with_memory(executor, input, [external,] memory: Option<Arc<dyn FactMemory>>)`; `run`/`run_with_tools` call it without memory. `remember_compressed` calls `run.remember_compressed()` on the run lifecycle (ADR-0116), which sets the fact memory of the context.
- **Spec in the crate:** `AgentSpec` becomes serializable (`to_json`; the strategy is already expanded to `graph`); the generated crate contains it as `SPEC_JSON`.
- **Cargo feature `app`** of the generated crate (optional; dependency on the facade without default features, only with the features the spec needs): `run_app(base_dir, input)` sets up everything as `spec run` does: providers from the environment (`providers::executor_for`), MCP, search, built-in and memory tools (`external::tools`), sub-agents (`AgentLoader::agent_tools`, tool cache under `<base_dir>/.grauwolf/tools`), memory (`LongTermMemory::for_spec`, shared scope); paths relative to `base_dir`.
- Without the feature `app`, the crate stays as lean as before (no facade).

## Consequences
- Generated code covers automatic memory; with `app`, a generated crate runs without its own wiring code.
- Still only in the interpreter: `tools_dir`, `self_tools`, `limits.max_cost` (they need a build at runtime, or a price table).
- With `app`, the crate depends on the facade; its build is significantly larger than the domain crates.

## Addendum (2026-11-09): Events, users and `--check`
User: "do the generated code, remainders: 13. `spec generate --check` does not check the feature `app` along. 14. `run_app` uses only the shared memory scope … Also, generated code emits no events."
- **Events:** the core of every generated crate is `run_observed(executor, input, [external,] [memory,] features)`. It installs the features in the context and emits the same events in the same order as the interpreter (`AgentStarting`, nodes via `AgentObserver`, LLM, tool and memory events, `AgentCompleted`/`AgentFailed` with usage and costs). `run`, `run_with_tools` and `run_with_memory` call it without features. The equivalence test also compares the event sequence for each case.
- **Users:** `run_app_with(base_dir, input, grauwolf::app::AppOptions { user, features })`; with `user`, memory tools and automatic memory apply to the scope of this user (like `spec run --user`). `run_app(base_dir, input)` = default options.
- **`--check`:** additionally checks the crate with `--features app`.
