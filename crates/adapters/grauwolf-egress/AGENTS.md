# grauwolf-egress

**Layer:** `adapters` · **Status:** ADR-0110 stage 6 (addendum B). `derive_tree(&AgentTree, &Options)` lists every destination of an agent tree: `Destination { scheme, host, port, any_port, purpose, pointer, phase, from_env }` (`Phase::Run | Build`), sorted (run before build, then scheme, host, port), merged (same destination: purposes joined with `; `, pointers with `, `), host names lower case. `derive_spec(spec, origin, &Options)` does one spec; `to_json` / `to_text` write the formats of `grauwolf spec egress` and `app/egress.json`; `resolve` / `resolve_all(destinations, lookup)` replace the address of an entry by the value its `from_env` variable has (what `grauwolf-k8-ops agent deploy --egress` knows); `credentials_file(tree)` reads the `grauwolf.toml` next to the root spec.

## Purpose
One derivation for the CLI (`spec egress`, `spec generate --dockerfile`) and for `grauwolf-k8-ops` (`agent deploy --egress`): the spec (domain) knows where it points to, `providers` (this crate) knows the default address of every model provider, and neither app may depend on the host. `providers::PROVIDER_ADDRESSES` is the one table of default addresses; the test `grauwolf-host/tests/egress_addresses.rs` keeps it equal to the provider adapters and `OPENAI_COMPATIBLE`.

## Rules of the derivation
- Never guess: an address only a variable knows (`RKEV_BASE_URL`, `PGVECTOR_URL`, `OTEL_EXPORTER_OTLP_ENDPOINT`) has `scheme`, `host` and `port` `None` and `from_env`; an address with a default (a provider's) has the default and `from_env` names the variable that may replace it. `bedrock` has the default region and `from_env: AWS_REGION` (`BEDROCK_BASE_URL` also replaces it, see `resolve`).
- Build-phase entries (tool package registries, git sources of packages and skills) are listed with `phase: build`; they are not needed at run time.
- The OTLP endpoint is only listed with `Options::otel`; OpenBao only from `Options::credentials_toml`.

- A `fetch.allowed_hosts` entry inside the cluster (`target::is_cluster_host`: `<svc>.<ns>.svc[...]`, `*.<ns>.svc`) has `port: None` and `any_port: true` (the spec does not name a port; the Service decides, `target` prints `:*`); an external one keeps 443, and 80 with `allow_http`. Other kinds carry explicit ports or their scheme's default.

## Features / ADRs
ADR-0110 (addendum B, stage 6), ADR-0115

## Allowed dependencies
- grauwolf crates: grauwolf-agent-tree, grauwolf-spec; dev: grauwolf-test
- External: serde_json, toml

No dependency on the host or on a provider adapter.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-egress`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- A new place in the spec that names a destination is a new case in `derive.rs` with a test in `tests/derive.rs` (and the example specs show it in `grauwolf-cli/tests/egress.rs`).
- Update this file when purpose, API, dependencies, or invariants change.
