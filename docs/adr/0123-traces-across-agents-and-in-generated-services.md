# ADR-0123: One trace across agents, and tracing in generated services

- **Status:** Accepted (User, 2026-11-15: "let us do both: link the traces with traceparent over A2A
  and MCP, and tracing for generated services")
- **Date:** 2026-11-15
- **Related:**
  - ADR-0049 (OpenTelemetry, `OtelFeature`, `--otel`);
  - ADR-0109 (`grauwolf.caller`);
  - ADR-0121 (`agent deploy --otel`, live check in Tempo: caller and callee were separate traces, and generated services have no tracing);
  - ADR-0055 / ADR-0089 (A2A and MCP clients and servers);
  - ADR-0093 / ADR-0119 (generated service, parity with the interpreter).

## Context
The live check of `agent deploy --otel` showed that the trace of the called agent (`invoke_agent reviewer`, with `grauwolf.caller`) and the trace of the calling agent (`invoke_agent coordinator`) are two separate traces. The A2A and MCP clients send no trace context, and the service does not continue one. Generated services (`spec generate --service`) have no tracing at all, so `agent deploy --otel` refuses images without `--spec`.

## Decision
1. **W3C Trace Context over A2A and MCP.**
   - When a run is traced, the A2A client and the MCP HTTP clients (Streamable HTTP and SSE) send `traceparent` (and `tracestate` when present) with every request of a call. The value is the context of the span that makes the call: the tool or `agent` node span inside the caller's `invoke_agent`.
   - The run's tracing feature provides that context to the call. How it travels from the tracing feature to the client (a port in the run context, a task-local, extras of the call) is an implementation choice, recorded in the notes. Domain crates must not depend on OpenTelemetry.
   - Without tracing, nothing is sent.
2. **The service continues the trace.**
   - `/run`, `/run/stream`, `/api/chat`, the A2A `rpc` and the MCP tool read an incoming `traceparent`.
   - The called agent's `invoke_agent` span becomes a child of it, in the same trace.
   - A missing or malformed header starts a new trace, as today.
   - The header is accepted from every caller that passed authentication: it only links spans and grants nothing. It is never logged as a credential.
   - `grauwolf.caller` stays on the span.
3. **Tracing in generated services.**
   - A generated service turns tracing on by environment, because it has no flags: `GRAUWOLF_OTEL=1`, plus `GRAUWOLF_OTEL_CONTENT=1` for contents. The SDK reads `OTEL_EXPORTER_OTLP_*` and `OTEL_SERVICE_NAME`, as with `spec serve --otel`.
   - It records the same spans as `spec serve --otel`: the agent, sub-agents, nodes, model calls, tools and `grauwolf.caller`.
   - It continues incoming `traceparent` (decision 2) and flushes on shutdown.
   - Generated crates need the facade feature `otel` for this; `--service` includes what is needed. The variables go into `VARIABLES` and the reference.
4. **`grauwolf-k8-ops agent deploy --otel` without `--spec`** sets `GRAUWOLF_OTEL=1` (and `GRAUWOLF_OTEL_CONTENT=1` with `--otel-content`) instead of refusing.
5. **Parity and tests.**
   - Fast tests with an in-memory span exporter check parenting: a caller with a span sends a `traceparent`, and the service's `invoke_agent` span gets the caller's trace ID and parent span ID. They cover A2A and MCP.
   - One slow equivalence case runs a generated service with `GRAUWOLF_OTEL=1`, and it continues a `traceparent`.
   - A live check in Tempo looks for one trace from `coordinator` to `reviewer`.

## Consequences
- A call chain of agents is one trace in Tempo; `grauwolf.caller` still says who called.
- Generated services can be observed like `spec serve`, and `agent deploy --otel` works for every image.
- Any caller that passes authentication can attach its spans to a trace; it cannot do more with the header.

## Implementation notes

### Part 1: one trace across agents (decisions 1 and 2, the fast tests of decision 5)
- **How the context travels out.** `Feature` (grauwolf-agents) got `trace_headers(call: Option<&str>) -> Vec<(String, String)>`, empty by default. Domain crates only carry the plain header pairs; `OtelFeature` is the only implementation and the only place that uses OpenTelemetry's W3C propagator (`opentelemetry_sdk::propagation::TraceContextPropagator`). The alternatives were a port object in the run context (needs a second install step next to the feature) and extras in every tool call (changes `DynTool`); a feature method reuses the installed feature, so tracing on means the headers flow.
  - `ToolExecution` asks the bus for the headers of the tool call (its ID; parallel calls each get their own `execute_tool` span as the parent), and `AgentContext::run_agent_step` asks for those of the innermost open step (the `node` span of an `agent` or `for_each` node). Both wrap the call in `grauwolf_tools::with_trace_headers(headers, future)`.
  - `grauwolf-tools` holds that scope: a `poll_fn` that sets a thread-local while it polls, so it needs no runtime (the domain has none), holds across `.await` and is not seen by spawned tasks. Empty headers keep the enclosing scope. The A2A client (`SendMessage`, `SendStreamingMessage`) and the MCP clients (the `POST` requests of Streamable HTTP and HTTP+SSE) add `current_trace_headers()` to each request. The handshake, agent card and tool listing happen outside a run and carry none.
- **How the context comes in.** The server routes (`/run`, `/run/stream`, `/api/chat`, the A2A `rpc`, the MCP tool) read `traceparent` and `tracestate` into `RunExtras.trace_parent: Option<TraceParent>` (raw header values; `TraceParent { traceparent, tracestate? }` in `grauwolf_agents::events`). `AgentRun::with_trace_parent` reports it in `AgentEvent::AgentStarting.trace_parent`, next to `caller`, and `OtelFeature` starts `invoke_agent` as a child of it; a header that does not parse (or an all-zero ID) starts a new trace. The route reads it only after authentication and never logs it.
- **Parity for generated services.** The same field travels as `RunOptions.trace_parent` (grauwolf-host), which `handles()` passes as `RunHandles.trace_parent`; `GeneratedAgent` maps `RunExtras.trace_parent` to it, like `caller`. Part 3 only has to switch tracing on and install the feature (`OtelTracing::feature`) in the generated service's run options.
- **Tests.** `grauwolf-otel/tests/spans.rs` (headers of tool call, node and agent spans; continued, malformed and absent parent), `grauwolf-tools` (scope), `grauwolf-agents` (`ToolExecution` gives the call's headers; `RunHandles` reports the parent), `grauwolf-server/tests/trace_context.rs` (a traced run sends a valid `traceparent` to a wiremock A2A agent, from an `agent` node, and to a wiremock MCP server; every route passes the incoming header on; a coordinator and a reviewer behind the SPIFFE gate, both traced into one in-memory exporter, give one trace ID and `grauwolf.caller`).
- **Sub-agents in the same process** (`agent_tools` with a `spec`) have their own `OtelFeature`, but they run inside the call of their caller. `AgentRun` takes the headers of that scope as the parent when no explicit `trace_parent` was given (`ambient_trace_parent`), so their `invoke_agent` is a child of the caller's `node` or `execute_tool` span without any further code.

### Part 2: tracing in generated services and `agent deploy --otel` for images (decisions 3 and 4, the slow test of decision 5)
- **Configuration.** `grauwolf-host`'s `ServiceConfig` (`from_lookup`, the one fallible section) reads `GRAUWOLF_OTEL` and `GRAUWOLF_OTEL_CONTENT` into `otel` and `otel_content`: `1`/`true` on, `0`/`false`/empty off, anything else is an error that names the variable. Both are in `VARIABLES` (grauwolf-config) and in reference-cli; `grauwolf-server`'s `ServiceConfig` carries them.
- **Service.** `grauwolf-server` got the Cargo feature `otel` (in its `default`; the facade's `otel` forwards to it). `service::ServiceTracing::start(&config, &env)` runs in the sync `main`, before the runtime, because the exporter sets up its own threads (as `spec serve --otel` does); the service name comes from `OTEL_SERVICE_NAME` through the credential-aware environment, the target from the SDK's `OTEL_EXPORTER_OTLP_*`. Without the feature, `GRAUWOLF_OTEL=1` is the start error `GRAUWOLF_OTEL: this build has no OpenTelemetry (feature `otel`)`. `GeneratedAgent::traced_by` adds the main agent's `OtelFeature` to every run's features (before the chat API's observer), and `provision_options_traced` gives every sub-agent its own through `with_agent_features`, next to the observer when the chat API is on. `main` calls `shutdown` after `serve` returned (SIGTERM or Ctrl+C); that is outside the async runtime, so it needs no `spawn_blocking`. The `traceparent` wiring of part 1 needed no change.
- **Generator.** The generated `service` feature is `["app", "grauwolf/service", "grauwolf/otel"]` (snapshots `echo_service_cargo_toml` and `echo_full_facade_cargo_toml`). **Cost:** `cargo tree` for the facade with `service,ollama` lists 227 unique packages without and 246 with `otel`: 19 more crates (opentelemetry, opentelemetry_sdk, opentelemetry-otlp, opentelemetry-proto, opentelemetry-http, prost, rand 0.9 and its helpers, and the like; `reqwest` was already there). The builder image (ADR-0096) compiles the facade with all default features, `otel` included, so a `--dockerfile` image build does not compile any of them again. A plain `--service` crate without a builder image compiles them once.
- **k8-ops.** `agent deploy --otel` without `--spec` sets `GRAUWOLF_OTEL=1` (and `GRAUWOLF_OTEL_CONTENT=1` with `--otel-content`) beside `OTEL_EXPORTER_OTLP_ENDPOINT` and `OTEL_SERVICE_NAME`; the four names are reserved under `--otel`; `tracing_of` (`status`, `doctor`) also recognises the switches. An image built with an older grauwolf ignores them.
- **Tests.** `grauwolf-host` (`tracing_of_the_service_is_a_switch_that_names_a_bad_value`), `grauwolf-server/tests/service.rs` (the switch, the start with and without the feature; `xtask`'s feature matrix now has `grauwolf-server +service`, so the build without `otel` is compiled and tested), `grauwolf-k8-ops-app/tests/deploy.rs`, and the slow case `generated_services_export_spans_and_continue_the_callers_trace` in `crates/domain/grauwolf-generator/tests/equivalence.rs`. It builds the echo service, starts it with `GRAUWOLF_OTEL=1` against a wiremock that accepts `POST /v1/traces`, sends `/run` with a `traceparent`, checks that nothing has arrived yet (the batch exporter waits), sends SIGTERM, expects exit code 0 and then looks into the request bodies. The protobuf is not decoded: ids are `bytes` and names `string` fields, which protobuf writes unchanged, so the test searches for the raw 16-byte trace ID and 8-byte parent span ID of the `traceparent`, the span name `invoke_agent` and the service name.
- **Live check should look at:** `agent deploy … --image <image from --dockerfile> --otel` (no `--spec`) for both agents; in Tempo one trace from `invoke_agent coordinator` to `invoke_agent reviewer` with `grauwolf.caller`; the pod's environment has `GRAUWOLF_OTEL=1`; `kubectl delete pod` (SIGTERM) loses no spans of the last request.

## Live check (2026-11-15, Tempo of the owner's LGTM stack)
- **Stage 1, CLI image (`584a16b`).** `coordinator` and `reviewer` ran with `agent deploy --spec … --otel`, and `reviewer` allowed `coordinator`. One call of `coordinator` gave **one** trace. `invoke_agent coordinator` is its root. Under it hangs `node ask` (the `agent` node), and under that `invoke_agent reviewer` with `grauwolf.caller=spiffe://agentswork.de/ns/coordinator/sa/coordinator`, above `node call_llm` and `chat qwen3.5:9b`.
- **Stage 2, generated service.** `reviewer` was replaced by an image from `docker/agent.Dockerfile` (`SPEC=docs/user-guide/examples/13-reviewer.json`), deployed with `--otel` and no `--spec`.
  - **First finding:** that image did not start: `this build has no credential sources (Cargo feature credentials of grauwolf-host)`. `agent deploy` gives every agent a `[credentials]` configuration (its server token comes from OpenBao), but `--service` crates were built without the facade feature `credentials`, so no generated service could run under `grauwolf-k8-ops`. **Fixed** in `580eab5`: `--service` crates enable `grauwolf/credentials`.
  - **Result:** with an image built from `580eab5`, the generated `reviewer` started (`GRAUWOLF_OTEL=1`), and the call gave one trace again, with its `invoke_agent reviewer` as a child of the coordinator's `node ask` and `grauwolf.caller` set.
- **Clean-up and check.** Everything was removed afterwards: agents, build namespace, images. The full check passed on `580eab5`; a first attempt was killed by the out-of-memory killer while linking and passed with fewer parallel jobs.
