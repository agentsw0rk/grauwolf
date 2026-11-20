# E05 – Graph strategies


| ID | Feature | Rust approach | Prio |
|----|---------|-------------|------|
| F05.1 | Strategy graph `strategy<I, O>("name") { ... }` with `nodeStart`/`nodeFinish` | `GraphBuilder::<I, O>::new("name")`, `g.start()` / `g.finish()` return `NodeHandle<I,I>` / `NodeHandle<O,O>` | M1 |
| F05.2 | Custom nodes `node<I, O> { input -> ... }` | `g.node::<I, O>("name", |ctx, input| async move { ... })` → `NodeHandle<I, O>` | M1 |
| F05.3 | Edges `edge(a forwardTo b)` – type-checked | `g.edge(a.forward_to(&b))` – compiles only if `a::Out == b::In` (or after `transformed`) | M1 |
| F05.4 | Conditions `onCondition`, `onToolCall(s)`, `onTextMessage`, `onAssistantMessage`, `onToolNotCalled`, `onIsInstance` | Builder methods; `on_is_instance` → `.on_variant(Message::as_tool_call)` (option mapper instead of a type check) | M1 |
| F05.5 | Edge transformation `transformed { }` | `.transformed(|x| ...)` changes the output type of the edge | M1 |
| F05.6 | Subgraphs with their own tool subset and their own model | `g.subgraph::<I,O>("name", |sg| {...}).tools(..).model(..)` | M1 |
| F05.7 | Graph validation (reachability, missing edges to finish) | `build()` → `Result<Strategy, GraphError>` | M1 |
| F05.8 | Data exchange between nodes via storage | `StorageKey<T>` (E04) | M1 |
| F05.9 | Predefined nodes: `nodeDoNothing`, `nodeAppendPrompt`, `nodeLLMRequest`, `nodeLLMRequestOnlyCallingTools`, `nodeLLMForceOneTool`, `nodeLLMRequestStructured`, `nodeLLMRequestStreaming`, `nodeLLMRequestMultiple`, `nodeExecuteTool`, `nodeExecuteMultipleTools`, `nodeLLMSendToolResult`, `nodeLLMSendMultipleToolResults`, `nodeLLMCompressHistory` | Functions `nodes::llm_request()`, `nodes::execute_tool()`, … that create `NodeHandle`s | M1 |
| F05.10 | Node output transformation (`node.transform { }`) | Adapter node | M2 |
| F05.11 | Parallel node execution with merge (`selectBy`, `selectByMax`, `selectByIndex`, `fold`) | `g.parallel([a, b, c]).merge(Merge::select_by_max(..))`; the context is forked per branch and executed via the `Spawn` port; the selected branch replaces the context on merge (ADR-0026) | M2 |
| F05.12 | Predefined strategies: single run, tool-based, chat agent, ReAct, streaming data | `strategies::single_run()`, `::chat()`, `::react(n)` | M1 (single_run), M2 (rest) |
| F05.13 | Predefined subgraphs: task execution, task execution with verification | `subgraphs::with_task(..)`, `::with_verification(..)` | M2 |
| F05.14 | History compression strategies: WholeHistory, FromLastNMessages, Chunked, FactRetrieval, custom | `trait HistoryCompression`; predefined impls | M2 |
| F05.15 | Memory retention during compression (facts/concepts) | Part of F05.14 + E07 | M2 |
| F05.16 | Graph visualization (Mermaid/Dot export) | `strategy.to_mermaid()` – cheap, useful | M2 |
| F05.17 | Content moderation node/edge | `nodes::moderate()` | M3 |

## Graph DSL in Rust – decision: option A (typed builder), 2026-11-06

Node input and output types must be serde-capable (ADR-0010). Conditions in JSON specs come from the condition catalog (ADR-0021); for edge semantics (the first matching edge wins) see ADR-0017.

Typed builder (option A):
```rust
let strategy = Graph::<String, String>::build("calc", |g| {
    let call_llm    = g.add(nodes::llm_request());          // NodeHandle<String, Message>
    let exec_tool   = g.add(nodes::execute_tool());         // NodeHandle<ToolCall, ToolResult>
    let send_result = g.add(nodes::llm_send_tool_result()); // NodeHandle<ToolResult, Message>

    g.edge(g.start().to(&call_llm));
    g.edge(call_llm.to(&g.finish()).on_assistant_message());   // Message -> String
    g.edge(call_llm.to(&exec_tool).on_tool_call());            // Message -> ToolCall
    g.edge(exec_tool.to(&send_result));
    g.edge(send_result.to(&exec_tool).on_tool_call());
    g.edge(send_result.to(&g.finish()).on_assistant_message());
})?;
```
`on_tool_call()` is a "filter_map" `Message -> Option<ToolCall>` – condition and type conversion in one. Type errors in edges surface at compile time; the engine processes values internally as `serde_json::Value` (ADR-0017).

Alternatives:
- **Option B – proc-macro DSL** (`graph! { start -> call_llm; call_llm -> finish if assistant_message; ... }`): more compact, but worse IDE support and error messages.
- **Option C – enum-based state machine** (the user writes `enum State` + `match`): maximally idiomatic, but loses the reuse of building blocks, per-node events and per-node checkpoints.
