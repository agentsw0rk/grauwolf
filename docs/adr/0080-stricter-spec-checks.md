# ADR-0080: Stricter checking of specs and errors with JSON pointers

- **Status:** Accepted (User, 2026-11-09: "do 1 and 2", i.e. the gaps in `spec validate` and errors without a JSON pointer)
- **Date:** 2026-11-09

## Context
Some errors surfaced only during a run, or came without a location in the spec: a typo in the provider (`olama`), type schemas that the generator rejects while the interpreter only fails at runtime (`Schema without "type"`), unknown keys in GOAP actions, specs of sub-agents. Format errors came as English serde messages without a pointer, and errors when resolving files in another format; `spec schema` did not know `code_file`/`tool_files`.

## Decision
- **Types:** `validate` checks each schema in `types` against the common subset of interpreter and generator (ADR-0031): top level `"type": "object"`, field names made of lowercase letters/digits/`_`, field types `string`, `integer`, `number`, `boolean`, `array` with `items` from the same subset. Deviations → `UnsupportedSchema { reason }` at the pointer of the schema.
- **GOAP:** unknown keys in `actions[i]` and `goals[i]` → `InvalidParams { kind: "goap" }`.
- **Format errors:** `AgentSpec::from_json` deserializes via `serde_path_to_error`; errors become `SpecError::Format { pointer, message }` with a German message for the common cases (unknown field, missing field, wrong type).
- `serde_path_to_error` is thereby on the list of permitted libraries of the domain layer (`DOMAIN_EXTERNAL_ALLOWLIST`, ADR-0016): a pure serde helper without input/output.
- **One format:** messages with a pointer are written by `load_spec` as `<file>:<pointer>: <message>`, otherwise `<file>: <message>`, also for the resolution of `code_file`/`tool_files` and strategies.
- **Schema:** `grauwolf::loader::spec_schema()` adds `tool_files` and `code_file` to the schema (then without the required field `code`); `spec schema` outputs it.
- **Providers:** `AgentSpec::provider_uses()` returns every place that names a provider, with its pointer. The spec remains open for custom providers (ADR-0005); `spec validate` checks against the ones built into the facade (`grauwolf::providers::BUILTIN`) and reports `<pointer>: unknown provider '…' (built-in: …)`.
- **Sub-agents:** `spec validate` also checks local specs from `agent_tools` recursively (cycles are detected, each file once) and reports their errors with their file name.

## Consequences
- Specs that the interpreter previously accepted with other schemas (e.g., nested objects) are now invalid; interpreter and generator thus behave the same (equivalence).
- Custom providers (Rust API) are reported as unknown by `spec validate`; the CLI check is not intended for them.
