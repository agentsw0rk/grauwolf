# ADR-0114: Provisioning – one module that prepares everything an agent run needs

- **Status:** Accepted (User, 2026-11-13: architecture review, candidate "provisioning module in the
  host"; decisions Q1–Q17 of the review session)
- **Date:** 2026-11-13
- **Related:** ADR-0108 (layers, host as composition root, central configuration), ADR-0050/0084
  (external tools and apps in generated code), ADR-0059/0069 (memory per user), ADR-0098 (key-value
  store), ADR-0093 (generated service), ADR-0113 (knowledge as context), ADR-0111 (delegation).
  Terms: `GLOSSARY.md` (provisioning, provisioned agent, provisioned run).

## Context
Preparing an agent run – model access, decider, tools from outside the graph (MCP, knowledge,
built-in groups, skills, memory tools, sub-agents, package tools) and long-term memory – is spread
over shallow modules in `grauwolf-host` whose interfaces are ladders of near-identical functions
(`external::tools` → `tools_for` → `tools_for_observed` → `tools_with_env_for`; five `builtin::tools*`
variants; six `LongTermMemory` constructors; three `knowledge::search_tool*`). Every caller composes
them itself: the loader (`load_in`), the CLI (`run`, `chat`, `handles` open long-term memory again on
their own), and the code the generator emits (five separate host calls). Consequences found in the
review:
- adding a store or tool kind touches the host, the CLI and the generator template (pgvector: 14
  files);
- the process environment is read in several places, so a loader built with another `Env` and the
  separately opened memory can see different configurations; the command tool always reads the
  process environment;
- the generated service calls `run_app_with` per request and so rebuilds executor, decider, MCP
  servers, knowledge index, built-in tools and sub-agents on every request, opening memory twice;
- in `spec serve` the key-value store is fixed to "no user", while memory is per user; sub-agents
  always run without a user.

## Decision
1. **New module `grauwolf_host::provisioning`.** It starts from a **loaded, resolved spec** (code files
   read, git skills localised, tool packages resolved – producing that stays with the loader; a later
   "prepared agent tree" may own it). It does not read spec files itself.
2. **Two stages.**
   - **Provisioned agent** – built once per loaded agent and shared by all its runs: executor, decider,
     started MCP servers, knowledge with its context source (ADR-0113), built-in tool configuration,
     skills, package tools, sub-agents.
   - **Provisioned run** – a cheap view per run and user: the user's memory tools and memory facts,
     the user's key-value store, the tools of `tools_dir` (reloaded per run, so tools built in an
     earlier run are available), and the user passed on to the runs of sub-agents.
3. **Sub-agents through a port.** Provisioning receives sub-agents from a supplier it is given; the
   loader satisfies it (it reads sub-agent specs and builds their tools); generated code passes the
   loader.
4. **One environment, given from outside.** Everything inside provisioning reads configuration only
   from the `Env` / `Config` sections it is given – including the values the command tool passes on
   (`command.env`). `ProcessEnv` appears only at the outermost entry points: the CLI's `main` and the
   generated service's / app's entry point.
5. **Options split.** `ProvisionOptions` per agent (environment, user I/O, decider, MCP tools observer,
   features for sub-agents, tool cache); per run only the user and the run's features remain. The
   generated `run_app_with` changes accordingly.
6. **All callers use it.** The loader, `spec run|chat|serve|mcp`, the server and generated code. The
   **generated service builds the provisioned agent once at start** and a provisioned run per request.
7. **Behaviour changes** (deliberate): in `spec serve` and generated services every user gets their
   own key-value area (as with memory); sub-agents run with the user of the calling run.
8. **The old function ladders are removed**, not kept as wrappers (grauwolf is before 1.0; generated
   crates are regenerated with the same version, ADR-0108). Public stays: `provisioning`, `providers`
   (executor, client, embedder – needed for own providers), `AgentLoader`/`load_spec`,
   `mcp::agent_server` (serving, not provisioning), `packages` (the `tools …` commands). `external`,
   the `builtin::tools*` constructors, `LongTermMemory::open*`/`for_spec*`, `knowledge::search_tool*`,
   `systemone::decider_for`, `skills::tools` and `mcp::start_servers*` become internal building blocks.
   The developer guide's promise that "the older functions remain as thin wrappers" is withdrawn.
9. **Tests through the interface.** The main verification is in-process tests of `provisioning` with
   `MapEnv`, a fake LLM and a fake embedder: memory per user, knowledge, built-in groups, sub-agents,
   missing features and error texts. Tests that only exercised the old variants are replaced, not
   kept in addition; the slow end-to-end tests (generated app, CLI binary) stay as a thin safety net.
10. **Order of work**, every commit green: the module with its tests next to the old code → the
    loader uses it → the CLI → server and generated code (build once, per-request run) → remove the
    old ladders, move tests, update the docs.

## Consequences
- A new tool kind or store is added in one module; loader, CLI, server and generated code follow
  without changes.
- Generated services start their MCP servers and index their knowledge once instead of per request.
- Callers and tests learn one interface; the environment seen by a run is exactly the one given.
- Breaking change for Rust users of the removed functions and for `run_app_with`/`AppOptions`;
  documented in the developer guide and user guide chapter 11.

## Implementation notes (2026-11-13)
- **Module:** `grauwolf_host::provisioning` (`ProvisionOptions`, `ProvisionedAgent`, `ProvisionedRun`,
  port `SubAgentSupplier` with `SubAgent`). The loader implements the port (`AgentLoader` and a
  private `LoaderSupplier` that carries the cycle chain and collects the sub-agent signatures) and
  offers `load_provisioned`; sub-agents of a calling run with a user run with that user's memory
  tools, key-value store and facts, without a user exactly as before. The generated crate has
  `provision`, `run_provisioned`, `run_app` and `run_app_with(base_dir, input, ProvisionOptions,
  RunOptions)`; `grauwolf::service::main(spec_json, provision, run)` provisions once at start.
  `app::AppOptions` became `app::RunOptions { user, features }`; decider, MCP observer and the
  features for sub-agents moved to `ProvisionOptions`.
- **Removed:** `external` (whole module), `builtin::tools*`, `long_term_memory::tools`,
  `LongTermMemory::for_spec*`, `systemone::decider_for`, `mcp::start_servers`,
  `AgentLoader::agent_tools`, `AppOptions`; `knowledge` and `long_term_memory` are crate-private,
  `skills::tools`, `systemone::decider_with` and `mcp::start_servers_with` crate-internal. The
  facade no longer re-exports `external`, `knowledge`, `long_term_memory`; it re-exports
  `provisioning`.
- **Tests:** new `tests/provisioning.rs` (9 tests: memory per user, automatic memory, key-value per
  user, knowledge indexed once, `command.env` only from the given `MapEnv`, missing credential,
  sub-agents with the user, missing supplier, `tools_for`), `tests/loader.rs` (sub-agent key-value
  of the calling user through a mocked Ollama), server `tests/provisioned.rs` (key-value per user
  in the service). Tests of the removed variants were moved, not duplicated: `knowledge`,
  `long_term_memory`, `pgvector_stores` and `systemone` are unit tests under `src/tests/` (they
  need crate-private items); `external`, `skills`, `git_skills`, `atlassian_git`, `todo`,
  `mcp_sse` and the server tests now go through provisioning (`tests/support/mod.rs`). Snapshots
  of 11 generated libraries and the service `main` were regenerated. Slow equivalence tests: all
  10 of `grauwolf-generator/tests/equivalence.rs` ran with `-- --ignored` after the generator change
  and passed.
- **Deviations / notes:** (1) the brief's fake embedder lives in the test files (a word counter),
  not in `grauwolf-test`, which has no dependency on `grauwolf-rag`. (2) `spec run|chat|resume --user`
  also gets the user's key-value store (ADR-0098 already said "per user with `--user`"; it was
  never wired). (3) `AgentLoader::new` still defaults to `ProcessEnv` (embedding applications);
  the CLI passes it explicitly. (4) Tool-directory loading is configured only for the loaded
  top-level agent, not for its sub-agents (as before). (5) A user ID that cannot name a user's
  area (empty or more than 128 bytes) is refused at the service entry with `400` (`/run`,
  `/run/stream`, from the body or the token), and a users file with such a name does not load
  (`grauwolf_agents::memory::check_user_id`, 2026-11-13) – before that fix such a request ran
  without the user's tools. (6) Moving the memory tools out of the agent's static list changes the order of the tools
  presented to the model (memory, key-value and sub-agents come after the others).
- **Live check** (Ollama `qwen3.5:9b`, `nomic-embed-text`, `OLLAMA_THINK=false`; spec with knowledge
  from `docs/user-guide/examples/07-knowledge`, long-term memory and key-value tools): with
  `spec serve` and with a generated service (`spec generate --service`, debug build) the users
  `anna` and `bob` each had their own memory facts (`memory.json` with `user`) and their own
  `state/users/<hex>/kv.json`; the answers to "what is my favourite colour" matched the user. The
  service was ready after about 2 s (knowledge indexed at start); after moving the knowledge
  sources away, a house-rules question was still answered from the in-memory index in 4–6 s, so
  nothing is re-indexed per request. In one pass of the generated service a value in anna's key-value
  file was overwritten with text saying blue; three repeats with the tool calls inspected on
  `/run/stream` always wrote into the caller's file (the model sometimes stores odd values itself).
  Follow-up (2026-11-13): `grauwolf-server` `tests/provisioned.rs`
  `concurrent_runs_of_two_users_keep_their_key_value_stores_apart` sends concurrent `/run` requests
  of `anna` and `bob` to one provisioned agent with a deterministic mock model that stores exactly
  the value named in each request; every value lands in the asker's store and neither user sees the
  other's keys (8 runs per user in the suite; also run 5 times with 40 per user, 80 concurrent
  requests). So the service does not mix up the users' tools; the observed value came from the
  model.
