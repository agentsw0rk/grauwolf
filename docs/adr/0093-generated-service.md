# ADR-0093: Generated crates as HTTP services

- **Status:** Accepted (User, 2026-11-11: "do step 1 with the runtime image for generated services … that needs F14.1"; ADR-0092)
- **Date:** 2026-11-10

## Context
A generated crate is a library: `run_app(base_dir, input)` runs the agent once (ADR-0084). The HTTP
service (`grauwolf spec serve`, ADR-0053, ADR-0089) only serves interpreted agents
(`grauwolf_server::router(Arc<InterpretedAgent>, …)`). AgentWeaver deploys a container that must
answer HTTP requests, so the generated crate needs a binary that serves the generated agent with
the same routes as `spec serve`.

## Decision
- **Trait instead of the interpreter type:** `grauwolf-server` serves any agent behind a trait
  `ServedAgent` (name, description, input/output schema; `run(input, extras)`; `run_streaming`
  with a frame sink; `resume` for run handles). `InterpretedAgent` implements it; existing
  `router*` functions keep their signatures as thin wrappers.
- **Facade:** `grauwolf::service` wraps a generated agent (its `run_observed` entry point plus
  `SPEC_JSON`) as `ServedAgent` and offers `serve(agent, ServiceConfig)`; `ServiceConfig::from_env()`
  reads `SERVER_PORT` (default 8080), `GRAUWOLF_TOKEN`/users file, data directory, A2A settings.
- **Generator:** new Cargo feature `service` of the generated crate (implies `app`) and a
  `src/main.rs` (`[[bin]]`, built only with the feature) that calls `grauwolf::service::serve`.
  `spec generate --service` writes it; `--check` also checks `--features service`.
- **Routes** are the same as `spec serve`: `/run`, `/run/stream`, `/health`, approvals,
  conversations, MCP (`/mcp`) and A2A. A2A can listen on a second port and base path
  (`A2A_PORT`, `A2A_BASE_PATH`), because AgentWeaver's harnesses expect it on port 9000 at `/a2a`.
- **Shutdown:** SIGTERM finishes running requests up to a grace period (Kubernetes rollout).

## Consequences
- One code path serves interpreted and generated agents; tests run the route suite against both.
- Generated services depend on the facade with server features; the binary is larger than the
  library crate, but has no runtime dependencies besides the C library.
- AgentWeaver-specific routes are a separate opt-in layer (ADR-0094).

## Implementation notes
- **Trait:** `ServedAgent::run(input, extras, user)` returns `Result<Value, String>`; `user` is passed
  besides `RunExtras` because a generated agent selects its memory by user itself. New routers
  `router_for`, `a2a::router_for`, `mcp_router_for` take `Arc<dyn ServedAgent>`; the old
  constructors are wrappers. `InterpretedAgent::input_schema()` was added for the MCP tool.
- **Environment:** the token is `GRAUWOLF_SERVER_TOKEN` (as `spec serve`), users come from
  `GRAUWOLF_USERS_FILE`. The service refuses to start without a token or users file on a non-local
  address (it binds `0.0.0.0`) unless `GRAUWOLF_ALLOW_ANONYMOUS=true`, so that a container is not
  open by accident. `GRAUWOLF_DATA_DIR` (default the working directory) is the base directory of
  the spec's paths (knowledge, memory store, built-in tools root, tool cache).
- **Limits of generated services:** `run_app_with` knows only the user and the features, so
  conversations (`conversation_id`), per-request tools, text deltas and approvals are not
  supported; A2A streaming sends the whole answer at once. Adding them needs more parameters on the
  generated entry points and is left for later.
- **A2A:** always on the main port; with `A2A_PORT` also on a second port. `A2A_BASE_PATH` is a
  path prefix of the routes on that port (card at `<prefix>/.well-known/agent-card.json`, RPC at
  `<prefix>/a2a`); the public URLs for the cards are `GRAUWOLF_PUBLIC_URL` and
  `GRAUWOLF_A2A_PUBLIC_URL`. Whether AgentWeaver expects the RPC endpoint exactly at `/a2a` on port
  9000 is still open (an empty prefix gives `/a2a`).
- **Binary name:** `agent` for every spec (`[[bin]]` with `required-features = ["service"]`), so
  that images need not know the spec's name. The `main` is one call of
  `grauwolf::service::main`; the facade feature `service` is on by default (it implies `mcp`).
- **Shutdown:** `with_graceful_shutdown` on SIGTERM/SIGINT; there is no grace period limit yet
  (open SSE streams delay the exit).
- `spec generate --check` with `--service` checks with the feature `service`, which includes `app`.
