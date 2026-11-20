# ADR-0102: Code in graph agents – conditions, custom nodes and edge transformations in Rust

- **Status:** Accepted (owner asked for the implementation, 2026-11-13; ADR-0092, F14.14)
- **Date:** 2026-11-10

## Context
AgentWeaver graph agents may contain code: `node-custom` (a body with input and output type),
`node-condition` and edge conditions (a boolean expression on a value) and edge transformations (a
function from one value to another). For its JVM backends this code is Kotlin. grauwolf graphs use
catalog nodes and declarative conditions (`on_field`, `on_tool_call`, …); custom code exists only in
tools, compiled to WASM and run in the sandbox (ADR-0024, ADR-0029, F13.11).

## Decision
- **Rust, compiled like tools:** code in graphs is Rust, built by the same isolated tool build into
  core WASM modules with the same ABI (tool ABI v1: JSON in, JSON out, no imports) and run in the same sandbox (no file system,
  no network, fuel and memory limits). In AgentWeaver, the code editor shows Rust for grauwolf
  harnesses.
- **Node kind `code`**: parameters `input` and `output` (spec types) and `code` (body of
  `fn run(input: In) -> Result<Out, String>`; `In`/`Out` are generated from the types). Errors fail
  the node with the message.
- **Condition `on_code`**: parameters `input` (the type of the value) and `code` (body of
  `fn check(value: &In) -> bool`); passes the value through unchanged, like `on_field`.
- **Edge transformation:** optional `transform` on an edge: `{ code, output }` (body of
  `fn transform(value: In) -> Out`); the target node receives `Out`. Type checking uses `output`.
- **Declarative first:** the AgentWeaver export maps simple Kotlin conditions it can recognize
  (`field == literal`, `!field`) to `on_field`; everything else needs Rust code. Kotlin is never
  translated automatically.
- Built by `spec build`/`grauwolf tools build` together with the tools; generated crates embed the
  artifacts as for tools (ADR-0095).

## Consequences
- Graph agents with custom logic become possible without a second runtime; the sandbox guarantees
  that graph code cannot do more than tool code.
- Each code piece costs a WASM instantiation per call (microseconds to milliseconds).
- Existing Kotlin code in AgentWeaver graphs must be rewritten in Rust when switching a harness to
  grauwolf.

## Implementation notes (2026-11-13)
Decisions of the owner for the implementation, and where it differs from the text above:

- **Same ABI and sandbox as tools:** core WASM modules with tool ABI v1 (not components), the same
  forbidden-construct check, fuel and memory limits. The ADR's wording "WASM components" was
  corrected.
- **Pure functions:** code gets only its value – no history, storage, tools, model, clock or
  randomness; deterministic.
- **Interpreter and generated code run the same modules:** a generated crate embeds the WASM
  modules (`tools/<name>.wasm`, as for tools) and runs them in the sandbox instead of compiling the
  code natively; both use `grauwolf_agents::code`, and the equivalence test
  (`examples/specs/review_router.json`) compares them.
- **Node `code`:** `params { input, output, code | code_file }`; `input`/`output` are `string`, a
  named type or `array<T>`. `Err(msg)` fails the node with `msg`. The result is serialized from the
  typed `Out`, so it matches `output` by construction; the input is checked when it is read
  (`invalid input: field …`).
- **Condition `on_code`:** `params { input, code | code_file }`. A panic (a trap), an exhausted
  fuel limit or an answer that is not a boolean is an error of the run, **not** `false`.
- **Edge `transform`:** `{ input?, output, code | code_file }`, a generic serializable edge feature
  of the graph engine (`TransformFn`, `edge_transformed`, `edge_when_transformed`, typed
  `edge_with`), applied after the condition matched and before the target runs. The target's
  input is the transformed value, so checkpoints store it and a resume does not transform again;
  a failure is `EngineError::Transform`. Validation uses `output` as the edge's type; a declared
  `input` must equal what the edge delivers.
- **Outside the catalogs:** `code` and `on_code` need the built module, so, like `for_each`, they
  are not in the node and condition catalogs; validation, interpreter and generator handle them as
  structural kinds (`CODE_KIND`, `ON_CODE_KIND` in `grauwolf-spec-model`).
- **Types in the guest:** the guest crate keeps its only dependency `serde_json`; the Rust types
  of the spec types and their JSON conversions are generated (`grauwolf-toolbuild/src/code.rs`)
  instead of using serde derives, so the dependency allowlist of ADR-0030 is unchanged. Supported
  are the subset of ADR-0031 (string, integer, number, boolean, arrays, optional fields).
- **Build and names:** `AgentSpecExt::code_pieces()` lists the pieces with stable names
  (`code_<scope>_<node>`, `condition_<scope>_<from>_<to>`, `transform_<scope>_<from>_<to>`, scope
  `main` or `sub_<subgraph>`, `_2`… for further edges between the same nodes) and the pointer of
  the code. The pieces share the artifact namespace with the tools, so a piece named like a tool
  is a validation error (`DuplicateToolName` at the piece's code, 2026-11-13). They are built by `spec build`, at run start (cache like tools) and by `spec generate`;
  with `--tool-sources` each piece is a guest crate `tools/<name>/` with a `kind` (`code`,
  `condition`, `transform`) in `tools/tools.json` (omitted for tools), built by
  `grauwolf tools build`. Compile errors point to the code in the spec
  (`<file>:/graph/edges/1/transform/code:2:5: …`); `tools build --format json` reports `kind` and
  name per entry.
- **`code_file`** is resolved by the loader before validation, in the `params` of `code` nodes
  and `on_code` conditions and in `transform`, as for tools (ADR-0073).
- **Events:** no new events; a `code` node reports the usual node events, a failing piece fails
  the node or the run with its message. A transformation has no event of its own.
- **Not done:** AgentWeaver's export itself (the mapping `node-custom` → `code`,
  `node-condition`/edge condition → `on_field` where recognizable else `on_code`, edge
  transformation → `transform` is documented in the feature catalog E14); Kotlin is never
  translated.

### Live check (2026-11-13)
Ollama, `qwen3.5:9b`, `OLLAMA_THINK=false`, the example `docs/user-guide/examples/04-code.json`
(an LLM step judges a review, a `code` node scores the answer, `on_code` routes it, a `transform`
reshapes it into the reply). With the interpreter (`grauwolf spec run … --events`):

- "The headphones are great, …" → `judge` answers "This review is positive." → `score` outputs
  `{"positive":true,…}` → `{"subject":"Thank you for the praise","body":"This review is positive."}`;
- "The package arrived broken …" → "This review is negative." → `positive: false` →
  `{"subject":"We will look into it (4 words)","body":"This review is negative."}`.

The events show the path `judge` → `score` (the edge with `on_code`/`transform` has no event of
its own). A crate generated with `--service` and run with the same inputs (`POST /run`) answered
the same way (`Thank you for the praise` and `We will look into it`). A first version of the
example matched keywords as substrings and routed "damaged goods" to the praise: a reminder that
the code is plain code.
