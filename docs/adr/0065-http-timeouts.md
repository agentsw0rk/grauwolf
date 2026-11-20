# ADR-0065: Timeouts for HTTP requests to LLM providers

- **Status:** Accepted (User, 2026-11-08: "yes, implement the HTTP timeout"; extended to MCP and A2A: "yes, bring timeouts to MCP and A2A too")
- **Date:** 2026-11-08

## Context
`grauwolf-llm-http` builds its client with `reqwest::Client::new()`, without any timeout. If a connection hangs (overloaded server, lost packet, proxy), the run waits indefinitely; in the HTTP service the request stays open. An overall timeout does not fit: streams run arbitrarily long, and models can think for minutes before the first output (measured: 133 s with qwen3.5:9b; Anthropic names several minutes per request for Fable).

## Decision
- Connection establishment takes at most 30 s (`connect_timeout`).
- Read timeout `DEFAULT_READ_TIMEOUT` = 10 minutes (like the default of the Anthropic SDKs): it applies to each read operation, i.e. to the time without received data, both before the response headers and between two parts of a stream. A stream that delivers data may run arbitrarily long.
- `HttpClient::with_read_timeout(duration)` changes it per client.
- The same values apply to MCP over HTTP (`McpClient::connect_http_with_read_timeout`) and A2A (`A2aClient::connect_with_read_timeout`); `connect_http` and `connect` use the default. With A2A, `SendMessage` only answers after the entire run of the remote agent, so raise the value for long tasks.
- A timeout is a transient error (`LlmError::Transient`, as already classified before) and is retried by the executor according to the retry policy.

## Consequences
- No run waits indefinitely on an LLM connection anymore.
- A request that computes for more than 10 minutes without any output fails and is retried; for such cases, streaming must be used or the timeout raised.
- MCP over stdio (child process) also waits at most `DEFAULT_READ_TIMEOUT` for each line from the server (`McpClient::connect_with_read_timeout`, error `McpError::Timeout`); there is no connection establishment there.
- MCP and A2A deliberately do not retry automatically after a timeout (User, 2026-11-08: errors as tool result to the model): a tool call may have side effects, and after a read timeout it is unclear whether the server has already executed it. So that the model can decide sensibly, the message names the case: "timeout: … the call was sent and may have been executed" respectively "no connection … the call was not executed."
