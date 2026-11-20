# xtask

**Layer:** Tooling (outside the layers) · **Status:** `check-arch` (layer rules from ADR-0016), `check --full` and `check --affected` (ADR-0108 stage 1)

## Purpose
Development tools, invoked via `cargo xtask <command>` (alias in `.cargo/config.toml`).

## Commands
- `cargo xtask check-arch` – the layer rules (ADR-0016) and the app order (ADR-0108 stage 4d): host must not depend on server, CLI or facade; server not on CLI or facade; CLI not on the facade; domain, adapters and support never on host, server, CLI or facade (dev-dependencies included; `arch::APP_DEV_DEPENDENCY_EXCEPTIONS` is empty); the spec crates (ADR-0108 stage 6): `grauwolf-spec-model` depends on no other spec crate, `grauwolf-strategies` only on the model (and graph), nothing below `grauwolf-spec` (graph, model, strategies) depends on it, dev-dependencies included; the lean tool program (ADR-0118, package `grauwolf-tools-app`, binary `grauwolf-tools`) depends on no other app crate and only the CLI may depend on it (host and server must not, `arch::TOOLS_APP`), and `tests/workspace_architecture.rs` keeps its normal dependency closure (`arch::normal_dependency_count` over `cargo metadata --offline --filter-platform <host>`) at most `arch::TOOLS_APP_MAX_PERCENT_OF_CLI` (75%) of the CLI's – measured 61%, a host dependency would give about 98%; the operator program (ADR-0120, package `grauwolf-k8-ops-app`, binary `grauwolf-k8-ops`) depends on no other app crate and no app crate depends on it (`arch::K8S_OPS_APP`); grauwolf studio (ADR-0126, package `grauwolf-studio`) may depend on the host and the server, and no app crate depends on it (`arch::STUDIO`); the facade `grauwolf` has no logic (`arch::facade_logic_violations` rejects item keywords in `crates/apps/grauwolf/src`). Messages name the edge and the rule.
- `cargo xtask check --full` – `fmt --check`, clippy, `cargo test --workspace -- --include-ignored`, then clippy and fast tests of every build of `check::FEATURE_MATRIX` (packages with `--no-default-features`, optionally a few features on), layer rules.
- `cargo xtask check --affected [--base <rev>]` – the same for the packages changed since `<rev>`
  (default `HEAD`, working tree and untracked files included) and everything that depends on them,
  fast tests only. Selection rules in `src/affected.rs`; plans and runner in `src/check.rs`.

## Invariants
- The rules are a pure function `arch::check(&[Package])` (plus `arch::facade_logic_violations(&[(file, source)])` for the facade) and are covered by unit tests with synthetic packages.
- `tests/workspace_architecture.rs` checks the real workspace; a violation makes `cargo test --workspace` fail.
- Formulate new rules from ADRs first as a test case in `tests/arch_rules.rs`.
- The selection (`affected::select`, `changed_paths`, `members_from_metadata`) and the plans
  (`check::full_plan`, `affected_plan`, `parse_args`) are pure and tested with fake workspaces in
  `tests/affected.rs` and `tests/check.rs`; only `check::run` and `check::affected` touch cargo and git.
- New files that other packages read from outside their own directory (like `examples/`) belong in
  `affected::readers_of`, otherwise `--affected` does not test their readers (today: the user guide examples → host, `examples/` → the spec crates and apps, `studio/web/lib/api/` → `grauwolf-studio`).
- Integration tests that need an optional feature declare it with `required-features` (`[[test]]` in the crate's `Cargo.toml`), so the feature-matrix builds skip them; new `#[cfg(not(feature = …))]` code needs a matrix build that compiles it.
- Slow tests carry `#[ignore = "slow: <reason>"]` and run with `--include-ignored` (developer guide 2.2).
