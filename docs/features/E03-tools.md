# E03 – Tools


| ID | Feature | Rust approach | Prio |
|----|---------|-------------|------|
| F03.1 | Typed tool (args → result) | `trait Tool { type Args: DeserializeOwned + JsonSchema; type Output: Serialize; const NAME; fn description(); async fn call(&self, args, ctx) -> Result<Output> }` | M1 |
| F03.2 | SimpleTool (returns a string) | Blanket: `Output = String` | M1 |
| F03.3 | Tool descriptor (name, description, parameters, JSON schema) | Automatically from `schemars`; conversion per provider format | M1 |
| F03.4 | Custom schema generation | Trait override `fn schema()` | M2 |
| F03.5 | Annotation-based tools (`@Tool`, `@LLMDescription`, `ToolSet`) | **proc macro** `#[tool]` on `async fn` and `#[toolset]` on `impl` blocks; description from doc comments | M1 |
| F03.6 | Tool registry (building, merging, subsets) | `ToolRegistry::builder().tool(X).toolset(Y).build()`; `merge`, `subset(&[names])` | M1 |
| F03.7 | Type-erased execution (JSON in, JSON out) | internal `trait DynTool` with `call_json(Value)`, blanket impl for all `Tool`s | M1 |
| F03.8 | Report validation errors back to the LLM | `ToolError::{InvalidArgs, Execution}` → error text as tool result | M1 |
| F03.9 | Tool reads agent context / call metadata | `ToolContext` parameter (`call_id`, storage access, agent ID) | M1 |
| F03.10 | Custom result format for the LLM | `fn render(&Output) -> String` override, default JSON | M1 |
| F03.11 | Parallel tool execution | `futures::future::join_all` (concurrent within the task; ADR-0026) | M1 |
| F03.12 | Agent as a tool | `agent.into_tool(name, description)` – requires `Input: DeserializeOwned + JsonSchema` | M2 |
| F03.13 | Built-in: SayToUser, AskUser, Exit | Adapter crate `grauwolf-builtin-tools` (IO does not belong in the domain), swappable IO handler | M2 |
| F03.14 | Built-in: ReadFile, WriteFile, EditFile (patch), ListDirectory | via a `FileSystemProvider` trait (sandbox/root dir) | M2 |
| F03.15 | Built-in: RegexSearch | `regex` + `ignore` crate (gitignore-aware walk) | M2 |
| F03.16 | Built-in: shell command with confirmation handler | `tokio::process`, `trait ConfirmationHandler` | M2 |
| F03.17 | Rollback-capable tools (undo on checkpoint restore) | see E07 | M2 |
| F03.18 | **Tool packages from OCI registries** (`tool_packages` with `oci`, tag or digest) | Adapters `grauwolf-oci` (client) and `grauwolf-toolpkg`, OCI distribution API, Docker credential store ([ADR-0107](../adr/0107-tool-registry.md)); **stages 1–2 done:** pull and push (anonymous, bearer token), credentials from `GRAUWOLF_REGISTRY_AUTH`, the Docker config and credential helpers, cache by digest; git source in F03.19 (stage 3); policy and signatures in F03.22 (stage 4) | M3 |
| F03.19 | Tool packages from git repositories (pinned commit, built from sources) | `{ git, path?, rev \| tag \| branch }`; shared adapter `grauwolf-git` (libgit2, own smart-HTTP transport, `GIT_TOKEN`), isolated tool build (ADR-0107 stage 3); lock key `git+<url>//<path>@<ref>`, cache per commit and path; `tools pull\|inspect` accept `git+<url>[//<path>]@<rev>\|tag:…\|branch:…` too (inspect without building, with the signature status); **done** | M3 |
| F03.20 | Lockfile `grauwolf.lock` and `--locked` | digests/commits and module hashes (ADR-0107); **done** for OCI (stage 1) and git commits (stage 3) | M3 |
| F03.21 | `grauwolf tools pull/inspect/pack/push/lock` | CLI (ADR-0107); **all five done:** `pull`, `inspect`, `lock` (stage 1), `pack` (OCI image layout directory, reproducible) and `push` (stage 2); stage 4 adds `tools keygen`, `push --sign-key` and the signature status of `inspect` | M3 |
| F03.22 | Trust policy: allowed registries and git hosts, required signatures | `grauwolf.toml` / `GRAUWOLF_TOOL_POLICY` (`[tool_packages]`, ADR-0107 stage 4): allowlists apply to cache and lockfile hits too; cosign-compatible signatures of OCI packages with public keys (`tools keygen`, `push --sign-key`; keyless later, no `sigstore` crate); SSH signatures of git commits and tags against an allowed-signers file, checked before building and again from the cache; **done** | M3 |
| F03.23 | The lean program `grauwolf-tools`: `build`, `pack`, `push`, `pull`, `inspect`, `lock`, `keygen` and `validate <package folder>`, as a library used by `grauwolf tools` too | App crate `grauwolf-tools-app`, `grauwolf-config`, `grauwolf-toolpkg::{setup, tree_lock}` ([ADR-0118](../adr/0118-grauwolf-tools-binary.md)); image, CI job, macOS script | M3 |

## Rust specifics
- Rust doc comments as tool/parameter descriptions would be more elegant than `@LLMDescription`.
- `dyn Tool` with associated types is not object-safe → two levels: typed `Tool` for users, `DynTool` internally.
