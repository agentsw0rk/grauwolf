# ADR-0033: Streaming port and frames

- **Status:** Accepted (User, 2026-11-06); refines ADR-0023
- **Date:** 2026-11-06

## Context
ADR-0023 establishes that streams run as a side channel and that only the aggregated result flows along edges. Still open are the shape of the port, of the frames, and the interplay with adapters that do not (yet) stream.

## Decision
- `StreamFrame` in `grauwolf-llm`: `TextDelta { text }`, `ToolCall(ToolCall)` (complete), `Reasoning(Reasoning)` (complete), and `End { finish_reason }`. Adapters assemble partial fragments of tool arguments and reasoning blocks themselves; only text deltas go out piece by piece, because only they are shown live.
- `LlmClient::execute_streaming(prompt, model, tools) -> BoxStream<'a, Result<StreamFrame, LlmError>>` with a default implementation: call `execute` and emit the response as frames. Every client thus streams, just in one piece; adapters override it with real streaming.
- `aggregate` rebuilds from frames exactly the `Message` that `execute` would have returned (text deltas are joined, parts stay in order).
- Transport in `grauwolf-llm-http`: line-by-line responses (`post_lines`) and Server-Sent Events on top of them (`post_sse`); Ollama uses NDJSON, OpenAI and Anthropic use SSE.
- Retries (ADR-0026) apply only as long as no frame has been passed on yet.
- In the graph: node `llm_request_streaming` (`string → message`) writes frames to the frame sink of the `AgentContext` and passes on the aggregated response.

## Consequences
- Streaming works immediately with every client; real live output depends on the adapter.
- Cancellation happens by dropping the stream.
