# ADR-0124: A tool catalog in the registry, richer package metadata, and a path for tool repositories

- **Status:** Accepted (User, 2026-11-16: "a process of its own for tools built as WASM and installed in the docker registry: as a user I have an overview of all tools in the registry, with url, name, description, version, security options, build date and worthwhile meta information in one central place; and tools can be made independently of the main program"; decisions Q1–Q6 as recommended)
- **Date:** 2026-11-16
- **Related:** ADR-0107 (tool packages as OCI artifacts, signatures, tool policy), ADR-0118 (the program `grauwolf-tools`), ADR-0029/0030 (tool ABI, isolated build), ADR-0096 (images), ADR-0115 (lockfile).

## Context
Tool packages can be built, packed, signed, pushed, pulled and inspected with `grauwolf-tools`, independently of the main program. Two things are missing:
- **No overview.** A user must know a package's reference; nothing lists the packages of a registry. Registries do not help everywhere: ghcr.io has no catalog endpoint.
- **Thin metadata.** The package manifest has name, version, ABI and per-tool description and arguments. The package has no description, build date, source, licence or security information.

## Decision
1. **The catalog is an artifact in the registry (Q1).**
   - A JSON index is stored as an OCI artifact, by default `<registry>/grauwolf-catalog:latest` with artifact type `application/vnd.grauwolf.catalog.v1+json`, and configurable.
   - One entry per package version. `grauwolf-tools push` adds or updates the entry of the pushed version by default; `--no-catalog` skips that.
   - `grauwolf-tools catalog sync` rebuilds the index from the registry where the registry can list repositories (`/v2/_catalog`) or from a given list of repositories.
   - Concurrent pushes: the index is read, changed and written again with the digest it was read at. If it changed in between, the update is retried.
   - A web view can be built on the index later; it is not part of this ADR.
2. **Metadata on the package (Q2).** Recorded as standard OCI annotations on the package manifest, so other tools understand them and the catalog only reads them off:
   - `org.opencontainers.image.created`, `.description`, `.source`, `.revision`, `.licenses`, `.authors`, `.version`, `.title`;
   - grauwolf's own: `vnd.grauwolf.tools.abi`, `…built-with` (grauwolf version), `…rustc`, and per module its size and SHA-256 (already a layer digest).
   - `grauwolf-tools.json` gains optional `description`, `license`, `authors` and `source`, and `pack` fills the annotations from it and from git (`source`, `revision`) when built in a repository.
   - **A catalog entry holds:** reference with digest, name, version, description, the tools with description and argument schema, build date, source and revision, licence, authors, ABI, grauwolf and rustc versions, module sizes, and the security block (decision 3).
3. **Security information (Q3).** Now:
   1. **Signature:** signed or not, by which key fingerprint(s), and whether the package satisfies the reader's tool policy. The policy is evaluated when the catalog is read, not stored as a fact.
   2. **Sandbox limits per tool:** optional `limits` in `grauwolf-tools.json` (memory, fuel, time). The runtime enforces the stricter of package and host settings.
   3. **Capabilities:** what a tool needs from the host. ABI v1 grants nothing (no network, no files), recorded as `capabilities: []`. A future ABI with host calls (e.g. HTTP) lists them, with hosts, so the catalog shows them and the egress derivation of ADR-0110 can use them.
   - Later: an SBOM of the Rust dependencies, as an OCI referrer.
4. **A path for tool repositories (Q4).**
   - `grauwolf-tools new <name>` scaffolds a package folder: `grauwolf-tools.json` with the metadata fields, `src/<tool>.rs`, a test and a README.
   - A CI template, a GitHub Actions workflow, runs build, validate, pack, sign (key from a CI secret), push and the catalog entry, using the `grauwolf-tools` image so CI needs no Rust setup.
   - The image of `grauwolf-tools` is published with the other images (ADR-0096).
5. **Commands (Q5).**
   - `grauwolf-tools catalog list | search <text> | show <name> [--version <v>]`, as text and `--json`;
   - `catalog add <ref>`, `catalog remove <name>@<version>`, `catalog sync`.
   - A spec keeps referring to a package by reference, and the lockfile pins digests. There is no "latest from the catalog" in specs for now.
6. **Who may write; signing the index (Q6).**
   - Writing needs write access to the registry, so the registry's own rights apply.
   - The index can be signed with a key from `tools keygen` (cosign-compatible, like packages); `catalog list` then shows whether its signature is valid and by whom.

## Consequences
- One place lists all tools of a registry with what matters for choosing and trusting them.
- Metadata travels with the package, so a lost index can be rebuilt (`catalog sync`).
- Tool authors get a repository template and CI without knowing grauwolf internals.
- The catalog is only as current as the last push or sync; a package pushed without the catalog step is missing until `sync`.

## Implementation notes (2026-11-16)
- **Metadata (Q2):** `grauwolf-tools.json` has optional `description`, `license`, `authors`, `source`; per tool `limits` (`memory_mib`, `fuel`, `time_ms`; 0 is refused) and `capabilities` (must be empty under ABI v1). Unset fields are not written, so manifests without them keep their bytes and digests. `pack` writes the annotations of decision 2 (`grauwolf_toolpkg::package_annotations`): `created` from `SOURCE_DATE_EPOCH`, else the clock; `source` from the manifest, else the `origin` remote of the git checkout (credentials removed); `revision` = the checked-out commit (`grauwolf_git::checkout_of`, libgit2, no new dependency); `rustc` from the tool builder. Module sizes and digests are the layer descriptors. `inspect` shows them; older packages pull and inspect unchanged.
- **Limits (Q3.2):** `ToolRuntime::instantiate_limited`; the sandbox applies the stricter of host and package per limit. The host had no time limit before: `Limits.time` (default none) uses wasmtime's epoch interruption with a 5 ms ticker per engine. The interpreter and generated crates (`generate_lib_limited`) pass the package limits.
- **Catalog (Q1, Q5):** domain `grauwolf_tools::catalog` (index `{ format, updated, entries }`, entries keyed `<name>@<version>`, the reference pinned by digest), registry access in `grauwolf_toolpkg::catalog`, commands in `grauwolf-tools-app`. Where: `--catalog <registry | reference>`, else `GRAUWOLF_TOOLS_CATALOG`, else (for `add` and `push`) the default catalog of the package's registry; no `grauwolf.toml` setting. `sync` *rebuilds* the index (it replaces the entries) from `/v2/_catalog` or `--repository`, skipping tags `sha256-*` and other artifacts (counted in the summary, named with `--verbose`; revised 2026-11-16: manifests are read with an Accept that covers OCI/docker indexes and manifests, so ordinary images are classified by media type instead of failing the digest check).
- **Concurrency:** read with digest → change → `PUT` with `If-Match` (`If-None-Match: *` for a new index) → read back; `412` or a moved digest restarts the round, at most 5 rounds. A change that is already there writes nothing. On registries that ignore the condition, a write that lands between another writer's write and its read-back can still be lost; `catalog sync` repairs it.
  **Revised 2026-11-16 (live check against `registry:2`, which ignores `If-Match`):** the digest read-back was not enough, because each writer read back its own write immediately and two parallel `catalog add` lost one entry. Now the first write is a probe with an `If-Match` that cannot hold; success means the registry ignores conditions, and the command prints `note: <registry> ignores conditional writes; concurrent catalog writers are checked after the fact`. Such a write is checked after a random wait (300–1000 ms, `GRAUWOLF_TOOLS_CATALOG_SETTLE_MS`): the index is read again and must hold *our change* (entry with our digest for `add`/`sync`, no entry for `remove`) – not the same whole-index digest, since others may add. Otherwise the round is repeated (still at most 5). Remaining risk: a writer that overwrites after the check; with such registries prefer one writer at a time (e.g. one CI job) and repair with `catalog sync`.
- **Policy (Q3.1):** the entry stores the package's cosign signatures (payload and signature) and the fingerprints of the writer's keys; the reader's policy is applied to the stored signatures at read time (`entry_verdict`) and never stored.
- **Index signature (Q6):** `--sign-key` on `catalog add|remove|sync`, and `push --sign-key` signs the index with the package key. `catalog list` prints `index: signed by <fp>: valid | signed: invalid (…) | unsigned | … not checked`. Under `require_signatures` the reading commands refuse an index that is not validly signed by a policy key or `--key` – unsigned included, not only invalid.
- **`new` (Q4):** scaffold with manifest, example tool, a `cargo test` crate that includes the code body, README, `.gitignore` and a GitHub Actions workflow in `ghcr.io/agentsw0rk/grauwolf-tools` (validate, test, pack with `SOURCE_DATE_EPOCH` of the commit; on a tag `v<version>` push with `--sign-key` from a secret, which adds the catalog entry). The image now contains `git`. **Open:** publishing the `grauwolf-tools` image with the other images is not done here; the workflow template assumes `ghcr.io/agentsw0rk/grauwolf-tools:latest`.
- Docs: user guide chapter 15 (EN) and 15 (DE) instead of a section in chapter 10, which is about tools the agent builds at run time; developer guide 4.10.
