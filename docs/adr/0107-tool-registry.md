# ADR-0107: Tool packages from OCI registries and git repositories

- **Status:** Accepted (User, 2026-11-11: "write it as an ADR with OCI and Git" – a remote registry for tools like a Docker registry; "do the tool registry ADR-0107"). Implemented in stages: 1 OCI pull, lockfile, CLI pull/inspect/lock; 2 pack/push and credentials; 3 git source (done); 4 policy and signatures.
- **Date:** 2026-11-11

## Context
A grauwolf tool is a definition (name, description, argument schema; format of ADR-0073) plus Rust
code that is compiled to a WASM module of tool ABI v1 (ADR-0029: core module without imports, run in
the sandbox). Tools can live inline in a spec, in tool files (`code_file`, `tool_files`, ADR-0073) or
in a tools directory loaded at run time (ADR-0074). All of these are local paths: sharing a tool
between projects or teams means copying files, there are no versions, and every consumer needs the
Rust toolchain to build the tool.

Container images solved the same problem with registries: immutable, versioned artifacts addressed
by tag and digest, pulled with the same credentials everywhere. Registries also store other artifacts
than images (OCI artifacts). AgentWeaver already pins *skills* from a git repository at a commit.

## Decision
### Tool package
- A **tool package** contains one or more tools:
  - `grauwolf-tools.json`: format version, package name, version, `abi` (`1`), and per tool the
    definition (name, description, `args` schema) and the file name of its module;
  - per tool the WASM module (`<tool>.wasm`);
  - optionally the Rust sources (`src/<tool>.rs`) for review and reproducible rebuilds.
- Tool names are those of the definitions; a spec can rename them on import (`as`) to avoid
  collisions; duplicates are a validation error.

### Two sources
- **OCI** (main way, like images): a package is an OCI artifact (artifact type
  `application/vnd.grauwolf.tools.v1`, config = `grauwolf-tools.json`, one layer per module and an
  optional source layer). Reference: `registry/repository:tag` or `…@sha256:<digest>`. Any OCI
  registry works (ghcr.io, Harbor, GitLab, Docker Hub, a local `registry:2`).
- **Git** (source way, like skills): a repository with one folder per package containing
  `grauwolf-tools.json` and `src/*.rs`, pinned to a commit (`rev`, or `tag`/`branch` resolved to a
  commit and locked). Modules are built from the sources with the isolated tool build (ADR-0030);
  prebuilt modules in the folder are ignored (stage 3 notes).

### Spec
```json
"tool_packages": [
  { "oci": "ghcr.io/acme/tools/jira-helper:1.2" },
  { "oci": "ghcr.io/acme/tools/pdf@sha256:9f3c…", "tools": ["extract_text"] },
  { "git": "https://github.com/acme/grauwolf-tools.git", "path": "text", "rev": "4e1a9c2",
    "tools": ["count_words"], "as": { "count_words": "word_count" } }
]
```
- `tools` selects tools of the package (default: all); `as` renames. Validation: `https://` for git,
  no credentials in URLs, unique tool names after renaming, known ABI version.

### Lockfile and resolution
- `grauwolf.lock` next to the spec records per entry the resolved digest (OCI) or commit (git) and
  the hash of every module. `spec run`, `spec serve`, `spec build`, `spec generate` resolve missing
  entries and write the lockfile; with `--locked` they fail if it would change (CI, images).
- Downloaded and built packages land in the existing tool cache (`.grauwolf/tools`), keyed by digest
  or commit; a cached package is never fetched again.
- **Generated code:** modules are embedded like local tools (`include_bytes!`, ADR-0050); fetching
  happens at generate time, so the image build stays offline (ADR-0096).
- **Interpreter and service:** modules are loaded into the sandbox like other tools;
  `tool_packages` are resolved once at start-up, not per request.

### Credentials and trust
- **OCI auth:** the Docker credential store (`~/.docker/config.json`, `DOCKER_CONFIG`, including
  credential helpers), or `GRAUWOLF_REGISTRY_AUTH` (JSON map registry → token) for services;
  anonymous pulls for public packages.
- **Git auth:** `GIT_TOKEN` as for the git tools (ADR-0104), passed to libgit2 in memory.
- **Integrity:** digests (OCI) and commits (git) are verified; module hashes from the lockfile are
  checked on every load.
- **Policy** (`grauwolf.toml` or env `GRAUWOLF_TOOL_POLICY`): allowed registries/hosts (default:
  any), require signatures (default: off). OCI packages carry a cosign-compatible signature
  (verified with a public key; keyless Sigstore with an identity is a later addition); a git package
  can require a signed commit or tag (SSH signatures, checked against an allowed-signers file).
- The sandbox stays the main protection: ABI v1 modules have no imports, so a package cannot reach
  files or the network. Capabilities (ABI v2, e.g. allowed HTTP hosts) will be shown by `tools
  inspect` and must be granted in the spec when they arrive.

### CLI
- `grauwolf tools pull <ref>` – fetch and verify into the cache.
- `grauwolf tools inspect <ref>` – show tools, arguments, ABI, digest, signature status.
- `grauwolf tools pack <dir>` – build a package folder (definitions + sources) into modules and a
  local artifact.
- `grauwolf tools push <dir> <oci-ref>` – pack and publish to an OCI registry (prints the digest);
  `--sign-key <key>` signs the package with a key made by `grauwolf tools keygen <name>` (no cosign
  binary needed).
- `grauwolf tools lock <spec>` – resolve and write `grauwolf.lock`.

### Implementation
- New adapter crate `grauwolf-toolpkg`: OCI distribution client (registry API v2 over reqwest:
  token auth, manifests, blobs, push with chunked upload) and git fetch via `git2`; the domain only
  sees resolved tool definitions and module bytes through a port `ToolPackageSource`.
- Signature verification with small own code (ECDSA P-256 and SSH signatures from RustCrypto and
  `ssh-key`), not the `sigstore` crate and without a Cargo feature (see the stage 4 notes).

## Consequences
- Tools can be shared, versioned and reviewed like images; teams reuse the registries and logins they
  already have; builds stay reproducible through the lockfile and offline through generate-time
  fetching.
- AgentWeaver can offer a tool node "from registry" (browse, pick a version, show arguments) with the
  same mechanism as its skill registry; its export only writes `tool_packages` and the lockfile.
- A new attack surface (third-party code) is limited by the sandbox, digests and optional
  signatures; the policy file decides how strict a deployment is.
- Package format version 1 is tied to tool ABI v1; ABI v2 packages need a new format version.

## Implementation notes

### Stage 1 (2026-11-11): OCI pull, lockfile, `tools pull|inspect|lock`
- **Where things live:** the package format (`PackageManifest`, `ResolvedTool`) in `grauwolf-tools`
  (domain, serde only); `tool_packages` and the reference syntax (`OciReference`) in `grauwolf-spec`;
  registry client, cache, lockfile and resolution in the new adapter `grauwolf-toolpkg`. The facade
  feature `packages` (default, implies `sandbox`) wires it: `grauwolf::packages::resolve_tools`.
- **Own registry client instead of `oci-client`:** `oci-client` 0.18 builds against reqwest 0.13, but
  pulls a second major version of `sha2` plus `oci-spec`, `chrono`, `olpc-cjson` and others for what
  are a few requests, and we want exact control of digest checks, media types, the token flow and
  plain http for local registries only. The client is about 350 lines; stage 2 adds push to it.
- **Package manifest:** `grauwolf-tools.json` has `format`, `name`, `version`, `abi`, and per tool
  `name`, `description`, `args`, `module` (plain file name). OCI layout as decided: artifact type
  `application/vnd.grauwolf.tools.v1`, config `…tools.config.v1+json`, module layers
  `…tools.module.v1+wasm` titled by file name, optional `…tools.source.v1.tar+gzip`; OCI image
  manifests and the deprecated artifact manifests are accepted.
- **References:** `registry/repository[:tag][@sha256:…]` with an explicit registry (a dot, a port or
  `localhost` in the first part; there is no Docker Hub default); without tag and digest the tag is
  `latest`. Plain http only for `localhost` and `127.0.0.1` registries.
- **Spec validation without network:** names listed in `tools` (after `as`) are checked for
  uniqueness and against all other tools and count as known tools (subgraph tool lists). Tools of an
  entry without `tools`, unknown package tool names and **the ABI version are only checked when the
  package is resolved** (the ABI is in the package, not in the spec). `git` entries are reserved and
  rejected with `UnsupportedToolSource`.
- **Lockfile:** JSON (`lock_version`, `packages`: reference as written → `digest`, `modules` file →
  `sha256:…`), sorted, pretty, final newline. A locked entry is fetched by digest, never by tag;
  entries no longer in the spec are dropped; `--locked` fails before any request when an entry is
  missing or stale. Every resolve compares the module hashes with the lock.
- **Cache:** `<tool cache>/packages/sha256-<hex>/manifest.json` plus `blobs/<hex>`, atomic, verified
  on every load; a damaged entry is an error (no silent refetch).
- **Interpreter and generator:** package tools are not part of `external_tools()`; the interpreter
  takes them with `with_package_tools` and offers all of them. The generator works on a spec copy in
  which they are tools without code (`GenerateOptions::package_tools`), so generated crates, their
  `SPEC_JSON` and images know no registry.
- **`--locked`** is a build option, so it is accepted by every command that has build options
  (`run`, `chat`, `serve`, `mcp`, `resume`, `build`, `generate`, `create`) and by `tools lock`.
- **Test registry:** `grauwolf_toolpkg::testing::TestRegistry` (feature `test-registry`) is an
  in-memory registry with manifests, blobs and an optional bearer-token challenge; the example
  `serve_test_package` serves one package for live checks. Stage 2 extends it with push.
- **Live check (no external programs):** the `add` tool of `examples/specs/calculator.json` was
  built to a real WASM module, stored in the in-memory registry on 127.0.0.1 with the example, then
  `grauwolf tools pull|inspect 127.0.0.1:<port>/test/add:1.0`, a spec with `tool_packages` run with
  Ollama `qwen3.5:9b` (the model called `add`, answer 5555 for 1234 + 4321), the same spec with
  `--locked` after the registry was stopped (cache + lock only), and `spec generate --locked
  --dockerfile` / `--service --check` (the generated crate compiles with the embedded module).
- **For stage 2:** `RegistryClient` has only GET paths; the credential lookup is
  `RegistryAuth::credential` (add the Docker config and helpers there; keep secrets out of error
  messages); `TestRegistry` stores through `push` and has no upload endpoints yet; `PackageManifest`
  has `to_json`, the layout code is in `layout.rs` (`Layout::parse`/`assemble`) and needs a writer
  side; `pack` needs the module/source layer building.

### Stage 2 (2026-11-11): `tools pack`, `tools push`, registry credentials
- **Pack format: an OCI image layout directory** (`oci-layout`, `index.json` with the one manifest,
  `blobs/sha256/<hex>`), default `<dir>/dist`, not a tarball. A directory needs no archive writer
  or reader, can be inspected and diffed, and is what `tools push` reads back (digests are verified
  on read). The layout holds exactly the blobs of the manifest; the manifest is an OCI image
  manifest with the stage 1 artifact type and media types. `tools push` also accepts a package
  folder and packs it first.
- **Where things live:** the pure parts are in `grauwolf-toolpkg` (`PackedPackage`, layout writer
  and reader, `source_archive`); building the modules is orchestration in the facade
  (`grauwolf::packages::pack_folder`, using `ToolBuilder`), because adapters must not depend on
  each other. The folder manifest is read by `PackageManifest::from_source_json` (`format`, `abi`
  and the module file names are optional; a module defaults to `<tool>.wasm`).
- **Package folder:** `grauwolf-tools.json` plus `src/<tool>.rs` with the code body in the format of
  `code_file` (ADR-0073). Every tool is built with the isolated tool build (ADR-0030).
- **Deterministic:** entries are sorted, the source layer is a ustar archive with fixed mtime, uid
  and gid inside a gzip stream of *stored* deflate blocks written by hand (CRC-32 included), so no
  `tar` or `flate2` dependency is needed and the same folder gives the same digests, also from a
  fresh build cache. Cost: the source layer is not compressed; it is small text.
- **Source layer:** all files of `src/` plus `grauwolf-tools.json`, titled `src.tar.gz`, always the
  last layer.
- **Push:** distribution API v2 with monolithic uploads: `HEAD` per blob first (existing blobs are
  skipped), `POST /v2/<repo>/blobs/uploads/` and `PUT <location>&digest=`, then the manifest `PUT`
  with the image manifest media type; prints `<ref>@sha256:…`. A reference without tag is pushed
  as `latest`. An upload `Location` that points to another host is refused (credentials must not
  leave the registry). Bearer tokens are requested with the scope `repository:<repo>:push,pull`.
  Cross-repository mount and chunked uploads are not implemented (no need so far).
- **Credential order:** `GRAUWOLF_REGISTRY_AUTH`, then the Docker configuration
  (`$DOCKER_CONFIG/config.json`, else `~/.docker/config.json`): `credHelpers.<registry>`, then
  `credsStore`, then `auths.<registry>` (`auth` as base64 `user:password`, or `identitytoken`).
  Registry names are normalized like Docker (`docker.io` ↔ `https://index.docker.io/v1/`, ports
  kept). Helpers run as `docker-credential-<name> get` (found via `PATH`, registry on stdin, 20 s
  timeout, off the async workers); their output is never shown or logged, a missing helper is an
  error that names the helper. The lookup is lazy: only a `401` challenge (or a static bearer
  token) consults the credentials, so anonymous pulls and pushes never run a helper. An
  `identitytoken` is sent as bearer token after a challenge; it is not exchanged at the realm.
- **Test registry:** `TestRegistry` now has blob `HEAD`/`POST`/`PATCH`/`PUT`, the manifest `PUT`
  (referenced blobs must exist) and a token endpoint that issues a distinct push token for a
  `push` scope; the example `serve_empty_registry` runs one for live checks of `tools push`.
- **Live check (no external programs):** `tools pack` of `docs/user-guide/examples/03-package`
  (two tools built to WASM), `tools push <folder>/dist 127.0.0.1:<port>/team/add:1.0` into the
  example registry, `tools pull` and `tools inspect` (same digest), then a spec with the
  reference on Ollama `qwen3.5:9b`: `add` gave 5555 for 1234 + 4321 and `count_words` counted 5
  words; the same spec with `--locked` ran after the registry was stopped.
- **Open for the later stages:** git source (stage 3); `grauwolf.toml` policy, signatures and
  `--sign` (stage 4); exchanging an `identitytoken` at the token realm; chunked uploads.

### Implementation note (ADR-0108, stage 3b)
The registry client, `RegistryAuth` with the Docker configuration, push, the OCI image layout
reader/writer and the in-memory test registry live in the adapter `grauwolf-oci` since stage 3b;
`grauwolf-toolpkg` keeps the package format, packing, cache, lockfile and resolution and
re-exports the client names. `grauwolf_toolpkg::testing::TestRegistry` wraps
`grauwolf_oci::testing::TestRegistry`; the example `serve_empty_registry` moved to `grauwolf-oci`
(`cargo run -p grauwolf-oci --features test-registry --example serve_empty_registry`),
`serve_test_package` stays in `grauwolf-toolpkg`.

### Stage 3 (2026-11-12): git sources
- **Spec:** `{ "git": url, "path"?, exactly one of "rev" | "tag" | "branch", "tools"?, "as"? }`.
  `rev` is a commit, full or abbreviated (7 to 40 lowercase hex digits); `tag`/`branch` are resolved
  to a commit and locked. Validation (`GitSource::from_entry` in `grauwolf-spec-model`, error
  `InvalidGitSource { reason }` at `/tool_packages/<i>/<field>`, which replaces
  `UnsupportedToolSource`): `https://`, `http://` only for `localhost` and `127.0.0.1`, no user
  info (the reason never repeats the URL), `path` relative without `..`, empty or `.` parts,
  none or several of rev/tag/branch is an error, and git fields next to `oci` are an error.
- **Shared adapter `grauwolf-git`** (as `grauwolf-oci` in ADR-0108 stage 3b): the libgit2 settings
  and the own smart-HTTP transport (no redirects, token only to the host of the URL) moved there
  unchanged from `grauwolf-tools-git`, plus `fetch_files` (one commit into a bare scratch
  repository in the temp directory, shallow first and then the branch/tag tips; abbreviated
  commits need the tips; returns the 40-hex commit and the selected files of the folder, at most
  16 MiB, symbolic links and submodules skipped). `grauwolf-tools-git` and `grauwolf-toolpkg`
  build on it; adapters still do not depend on each other.
- **Resolution** (`PackageResolver::with_git(token, builder)`): fetch the commit, read
  `<path>/grauwolf-tools.json` (`PackageManifest::from_source_json`) and `src/<tool>.rs`, build
  every module through the trait `ModuleBuilder` (`pack_sources`, shared with `tools pack`; the
  host implements the trait with `ToolBuilder`, so adapters do not depend on `grauwolf-toolbuild`)
  and store the result like an OCI entry.
- **Deviation from the decision text above:** prebuilt `.wasm` files in the folder are **ignored**
  (the ADR said they would be used if their hashes match a rebuild). Rebuilds are not
  bit-identical across toolchains, so a comparison would fail spuriously; the sources are the
  only trusted input. Consequence: with a lock but without a cache entry, a build on another
  toolchain can produce other bytes, and resolving then fails with `the modules of … do not match
  the lockfile`. Git packages are reproducible by commit, not bit for bit.
- **Lockfile:** a separate section `git` (absent without git entries, so existing lockfiles do not
  change; `packages` is still written): key = the entry as written,
  `git+<url>[//<path>]@<rev>`, `…@tag:<tag>` or `…@branch:<branch>`, value `commit` (40 hex) and
  `modules` (file → `sha256:…`). A locked entry is fetched by commit, never by tag/branch; a
  full `rev` or locked commit is looked up in the cache before any network access; `--locked`
  fails before any request when an entry is missing or stale; module hashes are compared on every
  resolve.
- **Cache:** `<tool cache>/packages/git-<commit>[-<16 hex of sha256(path)>]/` with `manifest.json`
  and `blobs/<hex>`, the same verified format as OCI entries (a damaged entry is an error), so a
  cached, locked package needs neither network nor Rust toolchain.
- **Auth:** `GIT_TOKEN` from `RegistryConfig::git_token` (`grauwolf::config`), HTTP basic with the
  user `x-access-token`, sent only to the host of the URL, masked in every error text; anonymous
  when unset.
- **Where it works:** everything that resolves `tool_packages` (`spec run|chat|serve|mcp|resume|
  build|generate|create`, `tools lock`; `tools lock` now takes the build options, including
  `--build-wrapper`, because it builds). Generated crates embed the modules as for OCI. `tools
  pull|inspect` stay OCI-only in this stage.
- **Tests without network:** `grauwolf_git::testing::TestGitServer` (feature `test-git-server`) is
  a small smart-HTTP upload-pack server over a bare repository built with libgit2 (packs from
  `PackBuilder`, no shallow support so the client fallback is exercised, optional required token,
  request log); the example `serve_folder` serves a folder as a repository for live checks.
  Covered: rev (abbreviated), tag, branch locked to a commit, lock reuse with the server stopped
  and no builder call, `--locked` stale and missing before any request, hash mismatch, selection
  and renaming, repository root as folder, missing manifest, token required and never in errors,
  damaged cache entry, and (slow) a real WASM build of `docs/user-guide/examples/03-package`.
- **Live check (no external programs):** the example `serve_folder` served `03-package` as a
  repository (tag `v1`, folder `text`); a spec with `{ "git": "http://127.0.0.1:<port>/tools.git",
  "path": "text", "tag": "v1", "tools": ["add"] }` ran with Ollama `qwen3.5:9b`
  (`OLLAMA_THINK=false`): the tool was built from the sources, the lockfile recorded the commit and
  module hashes, and the model answered 5555 for 1234 + 4321. After the server was stopped, the
  same spec with `--locked` (and a `PATH` without cargo) ran from the cache and the lock and gave
  5555 again.
- **Open:** shallow fetches work only against servers that support them (GitHub and GitLab do),
  otherwise the history of the fetched refs is downloaded.

### Stage 4 (2026-11-12): tool policy and signatures
- **Decisions (owner):** cosign-compatible OCI signatures with public keys, keyless signing later;
  an own signing key (`tools keygen`, `tools push --sign-key`) instead of calling a cosign binary;
  git: SSH signatures checked against an allowed-signers file; the policy comes from a
  `grauwolf.toml` next to the spec or from `GRAUWOLF_TOOL_POLICY` (which wins). Without a policy
  everything stays allowed and unsigned.
- **Policy:** table `[tool_packages]` with `allowed_registries`, `allowed_git_hosts` (hosts or
  prefixes of whole path parts; missing = any, empty = none), `require_signatures`, `oci_keys`
  (PEM files) and `git_allowed_signers`, files relative to the policy file; unknown keys are errors,
  `require_signatures` needs one of the key settings. Parsing is pure (`ToolPolicy` in
  `grauwolf-tools`); `Policy::load` in `grauwolf-toolpkg` reads the files; the host loads it
  (`PackageResolver::with_policy`). The allowlists are checked first, before the cache, the
  lockfile or the network, so tightening the policy also refuses packages that are already cached or
  locked. Reference: `docs/user-guide/reference-config.md`.
- **Deviation: no Cargo feature and no `sigstore` crate.** The signature code is small and part of
  `grauwolf-oci` (`cosign`) and `grauwolf-git` (`sshsig`), always built in; the `sigstore` crate
  would pull a large dependency tree for keyless features that are not in scope.
- **OCI signatures:** the format of cosign: a small image under the tag `sha256-<hex>.sig` in the
  same repository, one layer per signature, layer media type
  `application/vnd.dev.cosign.simplesigning.v1+json`, the signature (ECDSA P-256 over SHA-256,
  base64, DER) in the annotation `dev.cosignproject.cosign/signature`; the payload names the
  identity `registry/repository` and the manifest digest. Verification accepts any signature of any
  key of `oci_keys` whose payload names this repository and digest. Keys: PEM, public as SPKI, private
  as unencrypted PKCS#8 (`tools keygen` writes mode 0600 and never overwrites).
- **Cache and offline:** the signatures are stored with the cache entry (`signatures.json`) and
  tried first, so a changed key list applies without network; they are fetched when they do not
  satisfy the policy, which `--locked` forbids for an entry cached earlier (the error says so).
- **`tools inspect`** shows `signed by <fingerprint>`, `unsigned` or `signature invalid` (checked
  with the policy keys and every `--key`); it applies the allowlists but shows a missing
  signature instead of refusing the package.
- **Git signatures:** SSH signatures only (GPG and X.509 are errors); the signature is read out of
  the raw commit (`gpgsig` header) or annotated tag object, namespace `git`. `git_allowed_signers`
  has the format of git's `gpg.ssh.allowedSignersFile`; **principals are matched against the e-mail
  of the committer or tagger** (patterns `*`, `?`), the only supported option is `namespaces`, every
  other option and negated principals are errors. `rev`/`branch` entries need a signed commit; a
  `tag` entry a signed annotated tag or else a signed commit; a lightweight tag on an unsigned
  commit is refused. The check runs before anything is built. The raw objects are stored with the
  cache entry (`commit.object`, `tag.object`) and checked again from there (also after a policy
  change, without network); if neither fresh nor stored objects pass, they are fetched again
  (`Wanted::Tag` for tag entries, else `Wanted::Commit`; the commit must be the same), which
  `--locked` forbids. A git entry under `require_signatures` without `git_allowed_signers` is an
  error that says so.
- **Tests:** `tests/policy.rs`, `tests/signatures.rs`, `tests/git_signatures.rs` in
  `grauwolf-toolpkg`, `tests/sshsig.rs` in `grauwolf-git`, `grauwolf-host/tests/tool_policy.rs`,
  `grauwolf-cli/tests/tool_packages.rs`; the example policy of the user guide is parsed by a test.
- **Live check:** a registry from the example `serve_empty_registry`; `tools keygen`, then
  `tools push docs/user-guide/examples/03-package … --sign-key` to `team/text-and-math:1.0`,
  an unsigned copy `team/unsigned:1.0`, a copy signed with another key `team/wrongkey:1.0` and a
  signed copy in `other/`. A `grauwolf.toml` with `allowed_registries = ["<host>/team"]`,
  `require_signatures = true` and the public key ran with Ollama `qwen3.5:9b`
  (`OLLAMA_THINK=false`): the signed package was used and the model answered 5555 for
  1234 + 4321, also with the registry stopped (cache and lock); the unsigned package was refused
  (`does not satisfy the tool policy (require_signatures): unsigned`), the one signed with another
  key too (`signature invalid: no signature matches a trusted key`), and `other/` was refused
  by `allowed_registries`; `tools inspect` showed `unsigned` and `signed by SHA256:…`. A git
  package from the example `serve_folder` (unsigned commits) was refused with `commit <id>:
  unsigned` under a policy with `git_allowed_signers`. A signed git repository could not be made
  by hand (no `ssh-keygen` in the container); the acceptance is covered by the tests with
  `TestSigner`.
- **Open:** keyless signing (Fulcio identity, Rekor), encrypted private keys, GPG/X.509 git
  signatures.

### `tools pull|inspect` for git sources (2026-11-12)
- **Reference syntax = the lock key:** `git+<url>[//<path>]@<rev>`, `…@tag:<tag>` or
  `…@branch:<branch>`, parsed by `GitSource::parse_reference` (`grauwolf-spec-model`), which shares
  the validation of a spec entry (https, http only for localhost/127.0.0.1, no credentials, relative
  folder without `..`, rev of 7 to 40 hex digits). Everything not starting with `git+` stays an OCI
  reference; a missing `@…` is an error with an example. The parse errors never repeat the URL.
- **`tools pull`:** `PackageResolver::pull_git` is `git_package` without a lockfile, so policy
  (allowlist, `require_signatures` with the SSH signers, before anything is built), fetch, build and
  cache are the code of spec resolution. Output `<ref> -> commit <40 hex>` and the tools; it takes
  `--build-wrapper`. A spec naming the commit as `rev` then resolves (and `tools lock` locks) from
  the cache without network or toolchain.
- **`tools inspect`:** `PackageResolver::inspect_git` fetches the commit and reads the manifest from
  the sources; it never builds. It shows commit, package, version, ABI, whether the package is
  cached (with module hashes) and the signature as `signed by <principal> (<fingerprint>)`
  (the principal is the e-mail of the committer or tagger, new field `Verified::identity`),
  `unsigned`, `signature invalid: …`, or `signed, not checked` without signers. Signers: the
  policy's `git_allowed_signers`, or `--allowed-signers <file>` (instead; `--key` is for OCI and
  refused). The allowlist applies, a missing signature is shown instead of refusing. A full `rev`
  that is cached with its signed objects is answered without network.
- **Tests:** `grauwolf-toolpkg/tests/git_inspect.rs` (pull caches and the spec then resolves offline,
  disallowed host, unsigned refused by pull, inspect without a build and without touching the cache,
  cached module hashes, signed/unsigned/foreign signer, signed tag, token not in errors),
  `grauwolf-spec/tests/git_reference.rs` (parsing), `grauwolf-cli/tests/tool_packages_git.rs`
  (from the outside with an empty `PATH`; the building test is `#[ignore = "slow: …"]`).
- **Live check:** the example `serve_folder` served `03-package` (tag `v1`, folder `text`) on
  127.0.0.1. `tools inspect git+http://127.0.0.1:<port>/tools.git//text@tag:v1` with an empty
  `PATH` showed the commit, `signature: unsigned`, `text-and-math 1.0.0`, `cache: not cached` and
  both tools with their arguments; `tools pull` built the modules and printed
  `<ref> -> commit 67efe731…`; a second `inspect` said `cache: cached`. A reference without `@…`
  and one with `user:pw@` were refused with an example and without echoing the URL. A signed
  repository could not be made by hand (no `ssh-keygen`); the signed cases are covered by the
  tests with `TestSigner`.

