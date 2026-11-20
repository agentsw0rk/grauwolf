# ADR-0018: Node catalog as the single source of node kinds

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
Node kinds are needed by spec validation, JSON schema, interpreter, generator and predefined nodes.

## Decision
`grauwolf-graph` contains the catalog of node kinds as `NodeKindDescriptor { kind, params_schema, fn signature(&params) -> Result<(DataType, DataType)> }`. The signature depends on the parameters (e.g. `llm_request_structured` returns the type named in the params). `DataType` covers at least `String`, `Message`, `ToolCall`, `ToolResult`, `List(DataType)` and `Named(name)` for nominal types from the spec (ADR-0022). Spec validation, the DSL's JSON schema, the interpreter factory, generator templates and predefined nodes are derived from it. A conformance test checks for each catalog kind: builder node, interpreter factory and generator template exist and behave the same.

## Consequences
The consumers do not drift apart. New node kinds require an entry in the catalog plus implementation in all three entry points. Analogous catalogs exist for conditions (ADR-0021) and features (ADR-0027).

## Addendum (2026-11-09): conformance test and structural node kinds
- The conformance tests (`crates/domain/grauwolf-agents/tests/catalog_conformance.rs`, `crates/domain/grauwolf-generator/tests/catalog_conformance.rs`) only check that there is an interpreter implementation or a generator template for each catalog kind – not that they behave the same. Identical behavior is ensured by the equivalence test on `examples/specs/` (`crates/domain/grauwolf-generator/tests/equivalence.rs`, ADR-0015, ADR-0019), i.e. only for the kinds used there.
- `subgraph`, `plan`, `goap` and `parallel` are not in the node catalog. They are structural special kinds in `grauwolf-spec` (`SUBGRAPH_KIND`, `PLAN_KIND`, `GOAP_KIND`, `PARALLEL_KIND`; ADR-0034, ADR-0039, ADR-0045, ADR-0070), whose signature results from the embedded subgraphs.
- There is no feature catalog (see addendum in ADR-0027); analogous to the node catalog, only the condition catalog exists (ADR-0021).
