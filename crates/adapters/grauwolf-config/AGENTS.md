# grauwolf-config

**Layer:** `adapters` · **Status:** split out of the host's `config` module for ADR-0118 so that the lean `grauwolf-tools` program reads its configuration without the host. Holds the `Env` port (every `Fn(&str) -> Option<String> + Send + Sync` is an `Env`) with its adapters `ProcessEnv` (the process environment) and `MapEnv` (a fixed map, for tests), the `Lookup` type and the helpers `non_empty` / `non_blank`, the section `RegistryConfig` (`GRAUWOLF_REGISTRY_AUTH`, `DOCKER_CONFIG`, `HOME`, `PATH`, `GIT_TOKEN`, `GRAUWOLF_TOOL_POLICY`; ADR-0107) and `VARIABLES` (since ADR-0110 also `GRAUWOLF_CREDENTIALS` and `SPIFFE_ENDPOINT_SOCKET`, read by `grauwolf-credentials` and `grauwolf-spiffe` through the `Env` port), the one table of every environment variable grauwolf reads (also those of sections that live in the host). `grauwolf-host::config` re-exports all of it, so the public paths are unchanged.

`RegistryConfig::tools_catalog` = `GRAUWOLF_TOOLS_CATALOG` (ADR-0124, the catalog of `grauwolf-tools catalog`); `SOURCE_DATE_EPOCH` (build date of packed tool packages, read by `grauwolf-toolpkg::setup::pack_folder`) is listed in `VARIABLES` as well.

## Purpose
The configuration pieces that both the agent runtime and the tool commands need, below the host: the environment port, the registry/git/policy section and the table of variables (ADR-0108 stage 4b, ADR-0118).

## Features / ADRs
ADR-0107, ADR-0108, ADR-0118

## Allowed dependencies
- grauwolf crates: none
- External: none (std only)

## Invariants
- Single source: a variable is added to `VARIABLES` here, whichever crate reads it; the host test `config_reads_only_variables_of_the_table` and `every_variable_appears_in_the_cli_reference` keep the table equal to what `Config` reads and to `docs/user-guide/reference-cli.md`.
- The process environment is read only through `ProcessEnv`; sections take an `Env` or a lookup function, so tests use `MapEnv`.
- No dependency on any other grauwolf crate: this crate must stay a leaf.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-config`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.

ADR-0111: `GRAUWOLF_IDENTITY_URL` and `GRAUWOLF_IDENTITY_SPIFFE_ID` (section `service`, agents) and the `GRAUWOLF_IDENTITY_*` variables of `grauwolf identity serve` (section `identity`). `GRAUWOLF_LOGIN_TENANT|AUTHORITY|CLIENT_ID|SCOPE` and `GRAUWOLF_CONFIG_DIR` (section `login`) are read by `grauwolf login` in the CLI, listed here so the table stays complete.
