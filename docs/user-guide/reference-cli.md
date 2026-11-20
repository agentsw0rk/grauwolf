# CLI reference

All commands and options of `grauwolf`. Explanations with examples are in the chapters; the
"Chapter" column points there. `grauwolf <command> --help` always shows the current help.

All commands write results to stdout and messages, prompts and errors to stderr. The exit code is
`0` on success and `1` on an error (`2` for wrong arguments). Errors in specs name the file and
JSON pointer, e.g. `spec.json:/graph/nodes/0/kind: unknown node kind 'llm_answer'`. Warnings about valid but suspicious specs are printed once to stderr as `warning: <file>:<pointer>: <message>` by every command that loads a spec; they never change the exit code.

## Overview

| Command | Purpose | Chapter |
|---|---|---|
| `grauwolf spec validate <file>` | Validate a spec, including provider names, local skill directories and the whole tree of local sub-agents (cycles and sub-agent specs outside the spec's directory are errors); reports all errors with a JSON pointer, otherwise `<file>: valid`; warnings (`warning: <file>:<pointer>: …` on stderr, see the [spec reference](reference-spec.md#validation-warnings)) do not change the exit code | [1](01-getting-started.md) |
| `grauwolf spec graph <file>` | Print the flow as a Mermaid diagram (`flowchart TD`) | [4](04-flows.md) |
| `grauwolf spec egress <file> [--format text\|json] [--otel]` | List every destination the agent and its local sub-agents may reach, derived from the specs: model hosts, MCP servers over HTTP, remote agents, `fetch` hosts, Jira/Confluence, git, System One, knowledge URLs, pgvector, OpenBao (from a `grauwolf.toml` next to the spec) and, with `--otel`, the OTLP endpoint. One line per destination, or a JSON array with `scheme`, `host`, `port`, `any_port` (a `fetch` host inside the cluster has `port: null` and `any_port: true`), `purpose`, `pointer`, `phase` (`run` or `build`) and `from_env` | [13](13-setting-up-a-cluster.md#limiting-what-an-agent-may-reach-egress) |
| `grauwolf spec schema` | Print the JSON schema of the spec format | [Spec reference](reference-spec.md) |
| `grauwolf spec build <file>` | Build tool code to WASM, for the spec and its sub-agents; one line `<name>: <hash>` per tool | [3](03-tools.md) |
| `grauwolf spec run <file> <input>` | Run the agent once | [1](01-getting-started.md) |
| `grauwolf spec chat <file>` | Chat: each line from stdin is a run in the same conversation | [1](01-getting-started.md), [7](07-conversations-and-knowledge.md) |
| `grauwolf spec serve <file>` | Agent as an HTTP service (with A2A and MCP) | [9](09-operations.md) |
| `grauwolf spec mcp <file>` | Agent as an MCP server over stdin/stdout | [9](09-operations.md) |
| `grauwolf spec resume <file> --run-id <id>` | Continue an aborted run at the last checkpoint | [9](09-operations.md) |
| `grauwolf spec generate <file> --out <dir>` | Generate a Rust crate from the spec | [11](11-code-generation.md) |
| `grauwolf harness generate <file>` | Generate the project of a harness file (crate, `Dockerfile`, `app/`) into `build/<name>/`; `grauwolf-k8-ops agent deploy --harness <file>` deploys it | [11](11-code-generation.md#a-project-per-harness-grauwolf-harness-generate), [13](13-setting-up-a-cluster.md#from-a-harness-file---harness) |
| `grauwolf version [--json]` | Print `grauwolf <version>` (like `--version`). With `--json` one JSON object `{"version": "<version>", "builder_image": "ghcr.io/agentsw0rk/grauwolf-builder:<version>"}`: the CLI's version and the builder image `spec generate --dockerfile` uses by default, for tools that check both without parsing text | [9](09-operations.md#the-builder-image-offline-agent-builds-in-minutes) |
| `grauwolf spec create <requirement> --model … --out …` | Let an LLM write a spec | [11](11-code-generation.md) |
| `grauwolf tools pull <ref>` | Fetch a tool package into the cache and verify it. OCI reference: digests, and the tool policy (allowed registries, required signature). Git reference `git+<url>[//<path>]@<commit>\|@tag:<tag>\|@branch:<branch>`: host allowlist, SSH signature, fetch (`GIT_TOKEN`), build from the sources (Rust toolchain, `--build-wrapper`); prints `<ref> -> commit <40 hex>` and the tools | [3](03-tools.md#tool-packages-from-a-registry-tool_packages) |
| `grauwolf tools push <dir\|layout> <ref> [--sign-key <key>] [--catalog <c>] [--no-catalog]` | Push a package folder (packed first) or a layout written by `tools pack` to an OCI registry; prints `<ref>@sha256:…`. With `--sign-key` (a private key from `tools keygen`) the package is signed too: a cosign-compatible signature is pushed next to it. Credentials: `GRAUWOLF_REGISTRY_AUTH`, else the Docker configuration. Adds the pushed version to the tool catalog (`--catalog`, else `GRAUWOLF_TOOLS_CATALOG`, else `<registry>/grauwolf-catalog:latest`; the sign key signs the index too); `--no-catalog` skips it | [3](03-tools.md#tool-packages-from-a-registry-tool_packages) |
| `grauwolf tools pack <dir> [--out <dir>]` | Build the tools of a package folder and pack them as an OCI image layout (default `<dir>/dist`, with sources; reproducible digests) | [3](03-tools.md#tool-packages-from-a-registry-tool_packages) |
| `grauwolf tools inspect <git-ref> [--allowed-signers <file>]` | Git package (reference as for `tools pull`): fetch the commit **without building**; show commit, name, version, ABI, tools with arguments, whether it is cached (then with module hashes) and the signature (`signed by <principal> (<fingerprint>)`, `unsigned`, `signature invalid: …`, or `signed, not checked` without signers), checked against `git_allowed_signers` of the tool policy or `--allowed-signers`. The host allowlist applies; a missing signature is shown, not refused | [3](03-tools.md#tool-packages-from-a-git-repository) |
| `grauwolf tools inspect <ref> [--key <pub>]…`; also the package metadata from the manifest annotations (description, created, source, revision, licence, authors, grauwolf and rustc versions) and per tool its `limits` | Show digest, signature status (`signed by <fingerprint>`, `unsigned`, `signature invalid`; checked with the keys of the tool policy and every `--key`, a PEM public key such as cosign's `cosign.pub`), ABI, tools with arguments and modules of a package. The allowlist of the tool policy applies; a missing signature is shown, not refused |
| `grauwolf tools validate <dir>` | Check a tool package folder without building it: `grauwolf-tools.json` and `src/<tool>.rs` of every tool (the rules of `tools pack`, forbidden constructs included); reports every problem, exit code 1 if there is one | [3](03-tools.md#tool-packages-from-a-registry-tool_packages) |
| `grauwolf tools keygen <name>` | Create a signing key pair `<name>.key` (PKCS#8 PEM, unencrypted, mode 0600: keep it secret) and `<name>.pub` (public key for `oci_keys` in the tool policy); creates a missing folder (new folders get mode 0700); never overwrites | [3](03-tools.md#tool-packages-from-a-registry-tool_packages) |
| `grauwolf tools build <dir> [--format text\|json] [--force]` | Build the WASM modules of a crate generated with `spec generate --tool-sources` from `tools/tools.json` and the guest crates (hash check, isolated build, per-tool errors; JSON: `{ "tools": [ { "name", "kind", "ok", "skipped", "errors": [ { "message", "line"?, "column"? } ] } ] }`; `kind` is `tool`, or `code`, `condition`, `transform` for the code of graphs) | [11](11-code-generation.md#tool-code-as-sources---tool-sources-and-grauwolf-tools-build) |
| `grauwolf tools lock <spec>` | Resolve the `tool_packages` of a spec (OCI and git) and the git sources of its `skills` and write `grauwolf.lock` next to it; covers the sub-agents of the spec too (one lockfile per tree, keys of sub-agents carry their spec path) | [3](03-tools.md#tool-packages-from-a-registry-tool_packages) |
| `grauwolf tools new <name> [--tool <tool>] [--dir <dir>]` | Scaffold a tool package folder: `grauwolf-tools.json` with description, licence, authors, limits; `src/<tool>.rs` (default tool `hello`); its `cargo test`; a README; a GitHub Actions workflow that validates, tests, packs and on a tag `v*` signs and pushes in the `grauwolf-tools` image. Refuses a folder that is not empty | [15](15-the-tool-catalog.md#a-repository-for-your-tools-grauwolf-tools-new) |
| `grauwolf tools catalog list\|search <text>\|show <name> [--version <v>] [--catalog <c>] [--key <pub>]… [--json]` | Read the tool catalog (`--catalog <registry\|reference>`, else `GRAUWOLF_TOOLS_CATALOG`): a table or detail view of the entries, each with the verdict of your tool policy (`allowed`, `refused: <reason>`), and the index signature (`signed by <fingerprint>: valid`, `invalid`, `unsigned`; refused under `require_signatures` unless valid) | [15](15-the-tool-catalog.md#finding-tools-catalog-list-search-show) |
| `grauwolf tools catalog add <ref>\|remove <name>@<version>\|sync [--repository <repo>]… [--verbose] [--catalog <c>] [--sign-key <key>]` | Write the tool catalog: add or refresh the entry of a package, drop one version, or rebuild the index from the registry (`/v2/_catalog` or the given repositories; other artifacts are skipped and counted in the summary, `--verbose` names them). Conditional writes with up to five retries; `--sign-key` signs the written index | [15](15-the-tool-catalog.md#writing-push-catalog-add-remove-sync) |
| `grauwolf token <user>` | Create a user token for `spec serve --users` | [9](09-operations.md) |
| `grauwolf login [--tenant <id>] [--authority <url>] [--client-id <id>] [--scope <scope>] [--config-dir <dir>]` | Sign a person in with Entra (or Entra Local) by the device code flow and cache the tokens (mode 0600); `--status` shows who is signed in, `--print-token` prints the access token (refreshed when needed) | [14](14-acting-on-behalf-of-people.md#signing-in-on-the-command-line-grauwolf-login) |
| `grauwolf logout [--config-dir <dir>]` | Remove the cached tokens | [14](14-acting-on-behalf-of-people.md#signing-in-on-the-command-line-grauwolf-login) |
| `grauwolf identity serve [--port <n>] [--mtls-port <n>]` | The identity service of a trust domain (ADR-0111): exchanges people's Entra tokens for Txn-Tokens for callers with a SPIFFE identity, renews them, publishes the keys | [14](14-acting-on-behalf-of-people.md) |
| `grauwolf identity grant create\|list\|revoke …` | A person's grants for unattended runs: create (with a sign-in of its own), list, revoke | [14](14-acting-on-behalf-of-people.md#unattended-runs-grants-and-schedules) |
| `grauwolf call <url> [--spiffe-id <id>] [--grant <grt_…>] [--input <text>] [--json] [--token <t>] [--timeout <s>]` | Call an agent's `/run` once with the workload identity (or a token) and print the output; the command of a schedule | [14](14-acting-on-behalf-of-people.md#unattended-runs-grants-and-schedules) |
| `grauwolf credentials check <VAR>… [--all] [--config <file>]` | Resolve variables through the credential sources and print one line per variable, `resolved <VAR> from <file\|openbao\|environment> (N bytes)`, never the value | [12](12-credentials-from-openbao.md) |

## Command arguments

| Command | Arguments and own options |
|---|---|
| `spec validate`, `spec graph` | `<file>` – path to the spec |
| `spec egress` | `<file>`; `--format text\|json` (default `text`), `--otel` |
| `spec schema` | none |
| `spec build` | `<file>`; [build options](#build-options) |
| `spec run` | `<file>`, `<input>` – text for `input: string`, otherwise JSON matching the input type; [run options](#run-options); plus `--attach`, `--run-id`, `--user` (below) |
| `spec chat` | `<file>`; [run options](#run-options). Ends with EOF (Ctrl+D) or `/exit`; empty lines are skipped; `/attach <file>` attaches a file to the next input |
| `spec serve` | `<file>`; [run options](#run-options); plus `--addr`, `--public-url`, `--users`, `--chat-api` (below) |
| `spec mcp` | `<file>`; [run options](#run-options) |
| `spec resume` | `<file>`, `--run-id <id>` (required), `--user <id>` (as at the start); [run options](#run-options) |
| `spec generate` | `<file>`, `--out <dir>` (required), `--grauwolf-crates`, `--check`, `--service`, `--full-facade`, `--dockerfile`, `--builder-image`, `--allow-builder-mismatch`, `--tool-sources`; [build options](#build-options) |
| `spec create` | `<requirement>`, `--model` and `--out` (required), `--max-repairs`; [build options](#build-options) |
| `harness generate` | `<file>`, `--out-dir`; [build options](#build-options) |
| `token` | `<user>` |

### Options of individual commands

| Option | Command | Default | Meaning |
|---|---|---|---|
| `--attach <file>` | `spec run` | – | File as an attachment to the input (can be given multiple times, at most 20 MB each): images `.png`, `.jpg`/`.jpeg`, `.gif`, `.webp`, PDF `.pdf`, audio `.wav`, `.mp3`. Whatever the provider doesn't understand, the run rejects before the request (ADR-0081). |
| `--run-id <id>` | `spec run` | – | Saves a checkpoint under this ID before every node (letters, digits, `-`, `_`). Not with `--conversation`; with `--user` the run uses that user's memory. |
| `--run-id <id>` | `spec resume` | – (required) | Run to be continued |
| `--user <id>` | `spec run`, `spec resume` | – | User of the run: their area in the long-term memory and – with `--conversation` – their conversation. Needs `long_term_memory` in the spec or `--conversation`. |
| `--addr <address>` | `spec serve` | `127.0.0.1:8080` | Address of the service. Non-local addresses only with `GRAUWOLF_SERVER_TOKEN` or `--users`. |
| `--public-url <url>` | `spec serve` | bound address | Public base URL for the A2A agent card, e.g. behind a reverse proxy |
| `--users <file>` | `spec serve` | – | Users file `{ "<user>": "sha256:<hex>" }` with entries from `grauwolf token` |
| `--spiffe` | `spec serve` | off | Second port over mTLS with the SVID of the Workload API (client certificates required) and JWT-SVIDs on the HTTP port (`GRAUWOLF_SPIFFE`, [chapter 9](09-operations.md#agents-calling-agents-with-spiffe)) |
| `--spiffe-port <port>` | `spec serve` | `8443` | Port of the mTLS listener (`GRAUWOLF_SPIFFE_PORT`) |
| `--spiffe-users <file>` | `spec serve` | – | File `{ "<spiffe id>" \| "<prefix>/*": "<user>" }` that maps callers to users; read again when it changes (`GRAUWOLF_SPIFFE_USERS`) |
| `--chat-api` | `spec serve` | off | Also serves `POST /api/chat` and `GET /api/signature` ([chapter 9](09-operations.md#the-chat-api-apichat-and-apisignature)); reads `agent-signature.json` and `agent-names.json` next to the spec, `OLLAMA_BASE_URL` stands in for `OLLAMA_HOST` |
| `--out <dir>` | `spec generate` | – (required) | Target directory of the crate |
| `--out-dir <dir>` | `harness generate` | `build` | Folder of the generated projects; the project goes into `<dir>/<name>/`, replacing an earlier one |
| `--grauwolf-crates <dir>` | `spec generate` | checkout of this CLI | `crates/` directory of a grauwolf checkout that the crate refers to |
| `--check` | `spec generate` | off | Check the generated crate with `cargo check` (with `--service`: with feature `service`) |
| `--service` | `spec generate` | off | Also write `src/main.rs`, the HTTP service binary `agent` behind the Cargo feature `service` ([chapter 11](11-code-generation.md#as-an-http-service---service)) |
| `--full-facade` | `spec generate` | off | Depend on the facade with all its features, whatever the spec needs, so that the dependency graph is the same for every spec and pre-built dependencies fit ([chapter 11](11-code-generation.md#identical-dependencies---full-facade)) |
| `--tool-sources` | `spec generate` | off | Write the tool code as sources (`tools/<name>/`, `tools/tools.json`, `build.rs`) instead of compiling it; no Rust toolchain needed; `grauwolf tools build <dir>` builds the modules ([chapter 11](11-code-generation.md#tool-code-as-sources---tool-sources-and-grauwolf-tools-build)) |
| `--dockerfile` | `spec generate` | off | Also write `Dockerfile` and `.dockerignore` that build the service offline in the builder image, and copy the files the spec reads to `app/`; implies `--service`, `--full-facade` and `--grauwolf-crates /opt/grauwolf/crates` ([chapter 11](11-code-generation.md#a-dockerfile-for-the-builder-image---dockerfile)) |
| `--builder-image <image>` | `spec generate` | `ghcr.io/agentsw0rk/grauwolf-builder:<version of the CLI>` | Builder image in the `Dockerfile`; only with `--dockerfile` |
| `--allow-builder-mismatch` | `spec generate` | off | Leave out the version guard of the `Dockerfile`, which fails the build when the builder image is not of this CLI's version; only with `--dockerfile` |
| `--model <provider>:<model>` | `spec create` | – (required) | LLM of the builder and of the generated agent, e.g. `ollama:qwen3.5:9b` |
| `--out <file>` | `spec create` | – (required) | Target file of the spec |
| `--max-repairs <n>` | `spec create` | `3` | Maximum number of repairs |

## `grauwolf-tools`

The program `grauwolf-tools` (ADR-0118) runs the tool commands without the agent runtime: `build`,
`pack`, `push`, `pull`, `inspect`, `lock`, `keygen`, `validate`, `new` and `catalog`, with the arguments, options,
output and exit codes of `grauwolf tools <command>` (see the table above and the build options
below; `--cache`, `--locked` and `--build-wrapper` work the same). It reads the same environment
variables (`GRAUWOLF_REGISTRY_AUTH`, `GIT_TOKEN`, `GRAUWOLF_TOOL_POLICY`, `GRAUWOLF_TOOLS_CATALOG`, `SOURCE_DATE_EPOCH`, `DOCKER_CONFIG`, `HOME`, `PATH`).
`grauwolf-tools --version` prints `grauwolf-tools <version>`. The one difference: `grauwolf-tools
lock <spec>` reads the spec and its local sub-agents with the spec crate only and locks them; it
does not check providers, skill directories or the rest of the spec (`grauwolf spec validate`
does). The program ships as a container image (`ghcr.io/agentsw0rk/grauwolf-tools`), in the
builder image and with `scripts/build-macos.sh --tools`.

## `grauwolf-k8-ops`

The program `grauwolf-k8-ops` (ADR-0120) sets up SPIRE and OpenBao for agents on Kubernetes; see
[chapter 13](13-setting-up-a-cluster.md). Exit codes: 0 ok, 1 a failure (for `doctor` a failed check,
for `agent check` a failed or timed-out check, for `agent deploy` a pod that is not ready in time, for `agent build` a failed or timed-out build), 2 a usage error.

| Command | What it does |
|---|---|
| `doctor [--json]` | Read-only: prerequisites and setup, one line per check (`ok`, `warn`, `fail`, reason, next step); JSON: `[ { "section", "check", "status", "reason", "hint" } ]`. The line `identity service`: installed and ready, not installed (fine unless an agent is deployed with `--identity`), or not ready. Per agent also: `callers` (the ConfigMap parses, dangling entries), `NetworkPolicy` (matches the callers), `caller spec` (a deployed spec's calls to other agents: `auth.spiffe`, mTLS port, permission), and a warning that NetworkPolicy enforcement cannot be known from the API |
| `oidc install` | The SPIRE OIDC discovery provider in its own namespace, with the image version of the running SPIRE server and its `ClusterSPIFFEID` |
| `openbao setup [--jwks-url <url>] [--bound-issuer <issuer>]` | The JWT auth method with the provider's `jwks_url`, the KV v2 mount, and the policy `grauwolf-k8-ops` (exactly what the tool needs, including the Transit mount and keys and the role and policy `grauwolf-identity` of `identity install`; run it again after an update) |
| `openbao token [--ttl <duration>] [--renewable]` | Create a token with only the policy `grauwolf-k8-ops` (default `--ttl 8h`) and print it on stdout, nothing else; needs root or an admin token. See [Working without the root token](13-setting-up-a-cluster.md#working-without-the-root-token) |
| `agent add <name> [--namespace <ns>] [--pull-secret <ns>/<name>]` | Namespace (restricted Pod Security Standard), service account, `ClusterSPIFFEID grauwolf-agents`, ConfigMap `grauwolf-config`, OpenBao role and policy `agent-<name>` |
| `agent list [--json]` | The agents with namespace and SPIFFE ID |
| `agent remove <name> [--purge-secrets] [--yes]` | Delete the namespace, the role and the policy (and the secrets); also removes the agent from the callers (`grauwolf-callers`) and NetworkPolicies of the agents that allowed it |
| `agent check <name> --image <image> [--variable <NAME>]... [--timeout <s>]` | Run `grauwolf credentials check` for every variable mapped in the agent's configuration (`--variable` narrows it) as one Job, print its log (one line per variable; on a `permission denied` a hint with the policy and the path that was tried; for an agent deployed with `--identity` a hint when the identity service is missing or not ready), delete the Job; exit code 1 if any variable fails |
| `agent build --harness <file> [--project <dir>] [--namespace <ns>] [--kaniko-image <image>] [--registry-secret <name>] [--git-secret <name>] [--timeout <s>]` | Build the image of a harness in the cluster with Kaniko (ADR-0129): clone the committed and pushed project folder (default `build/<name>`; refused with uncommitted or unpushed changes) in a Job in namespace `grauwolf-builds` and push the harness's `image` (else the default image of `[harness] registry`); prints Kaniko's log as it comes, then `built and pushed <image>`; exit code 0 on success, 1 when Kaniko failed or `--timeout` (default 1800) passed. The Secrets must exist. See [chapter 13](13-setting-up-a-cluster.md#building-a-harness-image-agent-build---harness) |
| `build secret registry <name> --server <server> --username <user> [--namespace <ns>]` | Create or update the Secret with the credentials of a registry in the build namespace; the token comes from stdin or a hidden prompt, never from an argument, and is never printed |
| `build secret git <name> --username <user> [--namespace <ns>]` | The same for the git credentials (keys `username` and `password`) of a private repository |
| `agent deploy <name> --image <image> \| --harness <file> [--image <image>] [--spec <file>] [--env NAME=VALUE]... [--host <host>] [--port <n>] [--mtls-port <n>] [--data-size <qty>] [--cpu <qty>] [--memory <qty>] [--pull-secret <ns>/<name>] [--otel [--otel-endpoint <url>] [--otel-content]] [--events] [--identity] [--egress [--egress-strict] [--cluster-cidr <cidr>]... [--egress-dns-namespace <ns>] [--egress-dns-selector <key=value>]] [--timeout <s>]` | Run the agent: Deployment (one replica), Service, with `--host` an Ingress, with `--spec` a ConfigMap with the agent tree and, for every relative `builtin_tools.root` and `tools_dir` of the tree, a directory of the data volume mounted at that place under `/app/spec` (made by an init container; an absolute path stays as it is, `..` is refused), with `--data-size` a PVC; always with SPIFFE on: `GRAUWOLF_SPIFFE=1`, the mTLS port `--mtls-port` (default 8443) in the pod and the Service, the ConfigMap `grauwolf-callers` mounted as a directory (made empty if missing, kept otherwise, not part of the restart hash) and the NetworkPolicy `grauwolf-mtls-callers`; creates the server token if missing; with `--otel` `spec serve --otel` (an image without `--spec`: `GRAUWOLF_OTEL=1`, with `--otel-content` also `GRAUWOLF_OTEL_CONTENT=1`) and `OTEL_EXPORTER_OTLP_ENDPOINT` (`--otel-endpoint`, default `http://lgtm.lgtm.svc:4318`) and `OTEL_SERVICE_NAME` (the agent's name), `--otel-content` records contents, a deploy without `--otel` switches it off; with `--events` `spec serve --events` (an image: `GRAUWOLF_EVENTS=1`): every event as a JSON line on stderr, thus in the pod log, a deploy without `--events` switches it off; with `--identity` sets `GRAUWOLF_IDENTITY_URL` and `GRAUWOLF_IDENTITY_SPIFFE_ID` to the identity service of `identity install` (refused without it; with `--egress` it is an allowed destination; see [chapter 14](14-acting-on-behalf-of-people.md#in-a-cluster-with-grauwolf-k8-ops)); `--egress` (needs `--spec`) the NetworkPolicy `grauwolf-egress` derived from `spec egress` of the tree (DNS, cluster services by namespace and pod port, IPs as /32, external names by port only; `--egress-strict` leaves them out; a deploy without `--egress` deletes the policy; see [limiting egress](13-setting-up-a-cluster.md#limiting-what-an-agent-may-reach-egress)); waits for the rollout and `/health` (default 300 s); on failure prints the last 20 log lines and exits 1. `--harness <file>` takes the name and `image`, `port`, `env` from a [harness file](13-setting-up-a-cluster.md#from-a-harness-file---harness) (no name next to it; a flag for a setting the file sets is refused). Repeatable; restarts the pod only when something it reads changed. See [chapter 13](13-setting-up-a-cluster.md#running-an-agent) and [Traces in Tempo](13-setting-up-a-cluster.md#traces-in-tempo) |
| `agent status <name> [--json]` | Deployed or not, pod (phase, ready, restarts, waiting reason), image, endpoint in the cluster, host, `tracing: on (<endpoint>)` or `off`, `events: on` or `off`, `identity: on (<url>)` or `off` (`"identity"` in JSON), `egress: <n> destinations, <m> only port-restricted` or `egress: off` (`"egress"` in JSON), the writable folders (`writable /app/spec/work …`; `"writable"` in JSON), whether the server token exists (never its value) |
| `agent logs <name> [--follow] [--tail <n>] [--events-only]` | The pod's log (default the last 100 lines); `--follow` streams; `--events-only` prints only the JSON event lines (see [Events in the pod log](13-setting-up-a-cluster.md#events-in-the-pod-log)) |
| `agent allow <target> --from <caller> [--as <user>]` | Let `<caller>` call `<target>` over the mTLS port: writes `spiffe://<trust domain>/ns/<caller namespace>/sa/<caller>` into the ConfigMap `grauwolf-callers` (key `callers.json`) of the target, mapped to the user `--as` (default: the caller's name), and makes the NetworkPolicy `grauwolf-mtls-callers` admit the caller's namespace on the mTLS port (the HTTP port stays open). Both agents must exist. Repeatable: says `created`, `updated` or `unchanged`. See [chapter 13](13-setting-up-a-cluster.md#letting-agents-call-each-other) |
| `agent deny <target> --from <caller>` | Take it back: removes the entry and the namespace from the policy (no callers left: no rule for the mTLS port); repeatable |
| `agent callers <target> [--json]` | The caller IDs with their users, the agent each belongs to (or `no such agent`), whether the NetworkPolicy admits its namespace, and whether the policy `matches`, `differs` or is `missing` |
| `agent undeploy <name> [--purge-data] [--yes]` | Delete Deployment, Service, Ingress, the spec ConfigMap and the NetworkPolicy `grauwolf-egress`; keeps identity, role, secrets, the callers (ConfigMap and NetworkPolicy) and the PVC; `--purge-data` also deletes the PVC (asks first unless `--yes`) |
| `identity install --tenant <id> --issuer <url> --audience <aud>... --image <image> [--pull-secret <ns>/<name>] [--jwks-url <url>] [--scope <scope>] [--public-url <url>] [--policy <file>] [--transit-key <name>] [--port <n>] [--mtls-port <n>] [--cluster-cidr <cidr>]... [--egress-dns-namespace <ns>] [--egress-dns-selector <key=value>] [--grant-client-id <id>] [--timeout <s>]` | The identity service of ADR-0111 in namespace `identity` (restricted): `ClusterSPIFFEID grauwolf-identity`, service account, Deployment and Service `grauwolf-identity` (`grauwolf identity serve`, HTTP 8080, mTLS 8443), ConfigMap `grauwolf-identity-policy` (replaced by `--policy`, else kept), NetworkPolicies `grauwolf-identity-ingress` (mTLS only from agent namespaces) and `grauwolf-egress` (DNS, OpenBao, the keys host); in OpenBao the mount `transit/`, the key (`ecdsa-p256`, default `grauwolf-identity`), the policy and JWT role `grauwolf-identity`. `--audience` may be given several times (joined into `GRAUWOLF_IDENTITY_AUDIENCE`). `--pull-secret` copies that image pull secret into namespace `identity` and attaches it to the service account, as `agent deploy --pull-secret` does for an agent (for an image from a registry that needs credentials). `--grant-client-id` (the public client "grauwolf CLI") enables grants: the service gets `GRAUWOLF_IDENTITY_GRANT_CLIENT_ID` and the KV mount, its OpenBao policy also the folders `<kv>/identity/grants/` and `<kv>/identity/grant-tokens/`. Waits for the rollout and `/health`. See [chapter 14](14-acting-on-behalf-of-people.md#in-a-cluster-with-grauwolf-k8-ops) |
| `identity status [--json]` | Installed or not, ready, pod, image, SPIFFE ID, mTLS endpoint, keys URL, the settings, the callers of the policy with their purposes, the grants section (agents and triggers), and the OpenBao role, policy and Transit key |
| `identity uninstall [--delete-key] [--yes]` | Delete namespace `identity`, the ClusterSPIFFEID and the OpenBao role and policy; keeps the Transit key unless `--delete-key`; warns about agents deployed with `--identity` |
| `identity allow <agent> [--grants] [--purpose <purpose>]...` | Let the agent request Txn-Tokens on behalf of people (an entry in `callers`; with `--grants` in `grants.agents`: it may run on people's grants; without `--purpose` every purpose); repeatable |
| `identity allow --trigger <schedule or spiffe id>` | Let a workload start grant runs (`grants.triggers`; a name is the schedule `spiffe://<td>/ns/schedules/sa/<name>`, an ID may end in `/*`) |
| `identity grants [--revoke <grt_…>] [--json]` | Every person's grants from OpenBao (owner, agent, trigger, purposes, validity, last use; never a refresh token), or one grant revoked (metadata and refresh token deleted). Needs the policy of `openbao setup` from this version (it reads `<kv>/identity/grants/`) |
| `identity deny <agent> [--grants]`, `identity deny --trigger <schedule or spiffe id>` | Remove the agent (from `callers`, or with `--grants` from `grants.agents`) or the trigger from the policy |
| `schedule add <name> --agent <agent> --grant <grt_…> --cron "<cron>" --input <text> --image <image> [--pull-secret <ns>/<name>]` | A schedule (ADR-0111 stage 3): namespace `schedules` (restricted), service account `<name>` and `ClusterSPIFFEID grauwolf-schedule-<name>` (`spiffe://<td>/ns/schedules/sa/<name>`), NetworkPolicy `grauwolf-schedule-<name>` (egress: DNS and the agent's mTLS port only), the CronJob `<name>` (`grauwolf call https://<agent>.<ns>.svc:<mtls>/run --spiffe-id <agent id> --grant … --input …`, the SPIFFE CSI volume, one run at a time), and the schedule as a caller of the agent (user `schedule-<name>`, callers file and NetworkPolicy). Says what the identity policy still lacks (`identity allow <agent> --grants`, `identity allow --trigger <name>`). Repeatable. See [chapter 14](14-acting-on-behalf-of-people.md#unattended-runs-grants-and-schedules) |
| `schedule list [--json]` | The schedules: cron line, agent, grant, last run, last success |
| `schedule run-now <name>` | One run now: a Job `<name>-manual-<time>` from the CronJob |
| `schedule remove <name>` | Delete CronJob, NetworkPolicy, ClusterSPIFFEID and service account, and the schedule's entry in the agent's callers; the grant stays |
| `secret set <agent> <key> [--variable <NAME>]` | Store a value from stdin or a hidden prompt at `kv/agents/<agent>/<key>` |
| `secret get <agent> <key> --reveal` | Print the value to stdout and nothing else (a newline follows it); without `--reveal` it refuses. The only command that prints a value |
| `secret list <agent> [--json]` | Keys and versions, never values |
| `secret delete <agent> <key>` | Delete a secret with all its versions |
| `trust-domain set <domain> [--apply] [--yes] [--backup <file>] [--consumers <file>] [--from <old domain>] [--timeout <s>]` | Change the trust domain of SPIRE and everything that holds it. Without `--apply`: only the plan. With `--apply`: confirmation (`--yes` skips it), backup (default `trust-domain-backup-<UTC timestamp>.json`), configuration rewrite, recreation of the server's data, wait for the new bundle, restart of the agents and the OIDC provider, OpenBao roles, consumers (`--consumers`: TOML with `[[configmap]]` and `[[restart]]`), verification; `--timeout` per wait, default 300 s; `--from` finishes an interrupted run. `<current domain>` prints `already <domain>, nothing to do`. See [chapter 13](13-setting-up-a-cluster.md#changing-the-trust-domain) |
| `trust-domain restore <backup> [--apply] [--yes] [--backup <file>] [--timeout <s>]` | Put the cluster back as a backup of `trust-domain set` (or of an earlier restore) has it: the backup's `from` domain. Without `--apply`: only the plan (line diff of the ConfigMap `data`, roles, restarts; other changed lines are listed separately). With `--apply`: confirmation (`--yes` skips it), backup of the current state (default `trust-domain-backup-<UTC timestamp>.json`), ConfigMap `data`, recreation of the server's data if its CA lacks the domain, wait for the bundle, restarts, OpenBao roles of agents that still exist, verification. Already equal: `already as in the backup, nothing to do`, no backup. See [chapter 13](13-setting-up-a-cluster.md#restoring-from-a-backup) |

Global options (all commands):

| Option | Default | Meaning |
|---|---|---|
| `--kubeconfig <file>` | `KUBECONFIG`, `~/.kube/config` | Kubeconfig |
| `--context <name>` | the current context of the kubeconfig | Kubeconfig context; one that does not exist is an error that lists the available ones. `doctor` prints the cluster's server URL and the context in use as its first line |
| `--spire-namespace <ns>` | `spire-system` | SPIRE server and agents |
| `--oidc-namespace <ns>` | `spire-oidc` | OIDC discovery provider |
| `--openbao-namespace <ns>`, `--openbao-service <name:port>` | `openbao`, `openbao:8200` | OpenBao, reached through the API server's service proxy |
| `--openbao-addr <url>` | – | Reach OpenBao directly instead |
| `--token-file <file>` | environment variable `OPENBAO_TOKEN` | The OpenBao token (never an argument) |
| `--jwt-mount <mount>`, `--kv-mount <mount>` | `jwt`, `kv` | OpenBao mounts |
| `--audience <audience>` | `openbao` | Audience of the agents' JWT-SVIDs |
| `--agent-openbao-address <url>` | `http://<service>.<namespace>.svc:<port>` | OpenBao as the agents see it (written into their ConfigMap) |

## Build options

For all commands that build tool code: `build`, `run`, `chat`, `serve`, `mcp`, `resume`,
`generate`, `create`.

| Option | Default | Meaning |
|---|---|---|
| `--cache <dir>` | `.grauwolf/tools` | Cache directory for guest crates and built artifacts (relative to the working directory) |
| `--locked` | off | Tool packages must be in `grauwolf.lock`; fails instead of changing the lockfile, also for an entry that no agent of the tree uses (CI, images). `tools lock` has the same option |
| `--build-wrapper <part>` | – | Prefix under which `cargo` runs (`tools lock` and `tools pull` of a git package too: they build) (e.g. bwrap or docker); can be given multiple times, once per part: `--build-wrapper bwrap --build-wrapper --unshare-net` |

`tools pull`, `tools inspect`, `tools pack`, `tools push` and `tools lock` take `--cache <dir>` (default `.grauwolf/tools`); the
packages are cached in its `packages/` folder, keyed by digest.

## Run options

For `run`, `chat`, `serve`, `mcp` and `resume`; always together with the build options.

| Option | Default | Meaning | Chapter |
|---|---|---|---|
| `--stream` | off | Print text from streaming nodes (`llm_request_streaming`) immediately | [4](04-flows.md) |
| `--events` | off | All events of the run as JSON lines to stderr | [9](09-operations.md) |
| `--prices <file>` | – | Price table `{ "currency", "models": { "<provider>:<id>": { "input", "output", … } } }` per million tokens; events and spans include the costs | [2](02-models.md) |
| `--max-tokens <n>` | – | End the run as soon as it has used this many input and output tokens | [2](02-models.md) |
| `--max-cost <amount>` | – | End the run as soon as it has reached these costs; needs `--prices` | [2](02-models.md) |
| `--conversation <id>` | – | Continue and save the conversation with this ID | [7](07-conversations-and-knowledge.md) |
| `--conversations-dir <dir>` | `.grauwolf/conversations` | Directory of the conversation histories | [7](07-conversations-and-knowledge.md) |
| `--conversations-db <url>` | – | Database instead of a directory, e.g. `sqlite://chats.db` or `postgres://user@host/db` | [7](07-conversations-and-knowledge.md) |
| `--llm-cache <dir>` | – | Answer identical LLM requests from files in this directory instead of from the provider (0 tokens) | [2](02-models.md) |
| `--runs-dir <dir>` | `.grauwolf/runs` | Directory of the checkpoints | [9](09-operations.md) |
| `--runs-db <url>` | – | Database for the checkpoints instead of the directory, e.g. `sqlite://runs.db` | [9](09-operations.md) |
| `--otel` | off | Export spans via OTLP/HTTP (target from `OTEL_EXPORTER_OTLP_*`) | [9](09-operations.md) |
| `--otel-content` | off | Also record contents in the spans; only together with `--otel` | [9](09-operations.md) |
| `--confirm-tools <a,b>` | – | Ask before every call of these tools (comma-separated); in the service via `/run/stream` | [9](09-operations.md) |

### Which run option applies where?

Not every run option has an effect with every command. If you set one that would have no effect,
the command aborts instead of silently ignoring it:

| Option | `run` | `chat` | `serve` | `mcp` | `resume` |
|---|---|---|---|---|---|
| `--stream` | yes | error | error | error | yes |
| `--events`, `--prices`, `--max-tokens`, `--max-cost`, `--llm-cache` | yes | yes (per line) | yes (per request) | yes (per call) | yes |
| `--conversation` | yes | yes (otherwise conversation only in memory) | error (per request `conversation_id`) | error | error |
| `--conversations-dir`, `--conversations-db` | with `--conversation` | with `--conversation` | yes | error | error |
| `--runs-dir`, `--runs-db` | with `--run-id` | error | error | error | yes |
| `--otel`, `--otel-content` | yes | error | yes | yes | yes |
| `--confirm-tools` | yes (console) | yes (console) | yes (`/run/stream`) | error | yes (console) |

"with …" means: only together with this option, otherwise an error. The message names all
ineffective options at once, e.g.

```text
no effect with spec serve: --stream, --conversation
no effect with spec run: --runs-dir (only with --run-id)
```

In addition there are specific messages such as `--stream and --otel are not yet available in spec chat` or
`--confirm-tools reads stdin and does not fit the stdio transport of MCP`.

### Incompatible combinations with `spec run`

| Combination | Message |
|---|---|
| `--run-id` + `--conversation` | `--run-id and --conversation cannot be combined` |
| `--user` without `long_term_memory` and without `--conversation` | `--user needs a long_term_memory in the spec or --conversation` (with `spec resume`: `--user needs a long_term_memory in the spec`) |
| `--max-cost` without `--prices` | `--max-cost needs --prices: without a price table the cost is unknown` |

## Endpoints of `spec serve`

| Method and path | Purpose | Token needed? |
|---|---|---|
| `GET /health` | Answers `ok` | no |
| `POST /run` | `{ input, conversation_id?, user_id? }` → `{ output }` or status 500 with `{ error }` | yes* |
| `POST /run/stream` | like `/run`, answer as SSE: `agent`, `text`, `approval`, finally `output` or `error` | yes* |
| `POST /approvals/{id}` | `{ approved, reason?, user_id? }` → 204; unknown or another user's → 404 | yes* |
| `GET /.well-known/agent-card.json` | A2A agent card | no |
| `POST /a2a` | A2A 1.0 via JSON-RPC: `SendMessage`, `SendStreamingMessage`, `GetTask`, `CancelTask` | yes* |
| `POST /mcp` | MCP via Streamable HTTP; the agent as one tool named after the spec | yes* |

\* only if `GRAUWOLF_SERVER_TOKEN` is set or `--users` is given. Then `401` without a valid
token and `403` if the `user_id` in the body doesn't match the user token.

## `grauwolf token`

```sh
grauwolf token <user>
```

First line: the new token (`gw_` and 32 hex characters) – visible only now. Second line:
`"<user>": "sha256:<hex>"` for the users file of `spec serve --users`.

## `grauwolf login`

```sh
grauwolf login [--tenant <id>] [--authority <url>] [--client-id <id>] [--scope <scope>] [--config-dir <dir>]
grauwolf login --status [--config-dir <dir>]
grauwolf login --print-token [--config-dir <dir>]
grauwolf logout [--config-dir <dir>]
```

Signs a person in by the device code flow (RFC 8628) at `<authority>/<tenant>/oauth2/v2.0/devicecode`
and `…/token` (Entra ID and Entra Local) and caches the access and refresh token in
`<config dir>/login.json` (mode 0600). `offline_access` is added to `--scope`.

| Option | Default | Meaning |
|---|---|---|
| `--tenant <id>` | `GRAUWOLF_LOGIN_TENANT`, else the last login | The tenant |
| `--authority <url>` | `GRAUWOLF_LOGIN_AUTHORITY`, the last login, `https://login.microsoftonline.com` | Entra ID or Entra Local's address |
| `--client-id <id>` | `GRAUWOLF_LOGIN_CLIENT_ID`, else the last login | The public client ("grauwolf CLI") |
| `--scope <scope>` | `GRAUWOLF_LOGIN_SCOPE`, else the last login | The API scope, e.g. `api://grauwolf-identity/access_as_user` |
| `--identity-url <url>` | `GRAUWOLF_LOGIN_IDENTITY_URL`, else the last login | The identity service's public URL, kept for `grauwolf identity grant` |
| `--config-dir <dir>` | `GRAUWOLF_CONFIG_DIR`, `$XDG_CONFIG_HOME/grauwolf`, `~/.config/grauwolf` | Where `login.json` lives |
| `--status` | – | Who is signed in and until when; exit code 1 when nobody is or the login expired without a refresh token |
| `--print-token` | – | Only the access token on stdout (refreshed first when less than 60 s are left), for `Authorization: Bearer` |

No other output shows a token. See [chapter 14](14-acting-on-behalf-of-people.md#signing-in-on-the-command-line-grauwolf-login).

## `grauwolf credentials check`

```
grauwolf credentials check <VAR>… [--config <file>]
grauwolf credentials check --all [--config <file>]
```

Resolves the variables `<VAR>…` (with `--all` every variable mapped in the `[credentials]` section) the way a run would, through the `[credentials]` section of `--config`, else of `GRAUWOLF_CREDENTIALS`, else of `./grauwolf.toml`, and prints one line per variable: `resolved <VAR> from <file|openbao|environment> (N bytes)` on stdout, or the error that names the variable on stderr. A variable that fails does not hide the others. The value is never printed. Exit code 1 if any variable fails (its source cannot be read, or it is neither mapped nor set). See [Credentials from OpenBao with SPIFFE](12-credentials-from-openbao.md).

## `grauwolf identity serve`

```
grauwolf identity serve [--port <n>] [--mtls-port <n>]
```

Serves the identity service (ADR-0111) on the HTTP port (`--port`, else `SERVER_PORT`, default
8080) and the mTLS port (`--mtls-port`, else `GRAUWOLF_SPIFFE_PORT`, default 8443):
`POST /api/identity/token` (callers with a client certificate or a JWT-SVID for its own SPIFFE ID
or `GRAUWOLF_SPIFFE_AUDIENCE`), `GET /api/identity/jwks.json`, `GET /api/identity/config` and
`GET /health` (public). Needs `SPIFFE_ENDPOINT_SOCKET`, the `GRAUWOLF_IDENTITY_*` variables below
and an OpenBao Transit key. With `GRAUWOLF_IDENTITY_GRANT_CLIENT_ID` it also serves grants (stage 3):
`POST /api/identity/grants`, `GET /api/identity/grants` and `DELETE /api/identity/grants/{id}`
(callers: people with their Entra access token as bearer), kept in OpenBao KV. See
[Acting on behalf of people](14-acting-on-behalf-of-people.md).

## `grauwolf identity grant`

```
grauwolf identity grant create --agent <spiffe id> --trigger <spiffe id> [--purpose <p>]… [--valid 30d] [--max-run 4h] [--identity-url <url>] [--config-dir <dir>]
grauwolf identity grant list [--json] [--identity-url <url>] [--config-dir <dir>]
grauwolf identity grant revoke <grt_…> [--identity-url <url>] [--config-dir <dir>]
```

A person's grants for unattended runs (ADR-0111 stage 3), authorized by the cached `grauwolf login`.
`create` runs a device code sign-in of its own with `offline_access` (same client and scope as the
login, the login cache is not touched), sends that sign-in's refresh token to the identity service
once and keeps no copy; the service redeems it once to check that it is yours.

| Option | Default | Meaning |
|---|---|---|
| `--agent <spiffe id>` | – | The entry agent the grant is for |
| `--trigger <spiffe id>` | – | The workload that may start runs, e.g. `spiffe://<td>/ns/schedules/sa/morning` |
| `--purpose <p>` | none | A purpose of the runs (repeatable); becomes `purp` of the Txn-Token |
| `--valid <duration>` | `30d` | How long the grant is valid, at most `90d` (`d`, `h`, `m`, `s`) |
| `--max-run <duration>` | `4h` | How long one run may last (its chain's maximum lifetime), at most the service's 8 h |
| `--identity-url <url>` | `GRAUWOLF_LOGIN_IDENTITY_URL`, else `grauwolf login --identity-url` | The identity service's public URL |
| `--json` (`list`) | – | Print JSON |

`list` shows your grants (never a token), `revoke` deletes a grant's metadata and refresh token at
once. Exit code 1 when the service refuses (the reason is printed). See
[Unattended runs](14-acting-on-behalf-of-people.md#unattended-runs-grants-and-schedules).

## `grauwolf call`

```
grauwolf call <url> [--spiffe-id <id>] [--grant <grt_…>] [--input <text> | stdin] [--json] [--token <t>] [--timeout <s>]
```

Posts `{"input": …}` to an agent's `/run` (appended to a bare address) and prints `output` (text as
it is, anything else as JSON). With `SPIFFE_ENDPOINT_SOCKET` it authenticates with the workload
identity: over `https://` with mTLS (the server must be `--spiffe-id`, else any ID of the own trust
domain), over `http://` with a JWT-SVID for the audience `--spiffe-id` (required there). `--token`
sends a bearer token instead. `--grant` sends `Grauwolf-Grant`. `--json` sends the input as JSON
instead of text; `--timeout` (default 600 s) limits the wait. Exit codes: 0 the run answered, 1 the
call failed (unreachable, the agent's error, any other HTTP status), 3 refused (401 or 403).

## Environment variables

grauwolf reads each of these variables in one place (`config::VARIABLES`, see the developer guide,
chapter 3); a service started from a generated crate and the CLI use the same names.

| Variable | Effect | Chapter |
|---|---|---|
| `OLLAMA_HOST` | Address of Ollama; default `http://localhost:11434` | [2](02-models.md) |
| `OLLAMA_THINK` | `true`/`1` or `false`/`0`: turn the thinking mode of Ollama models on or off | [2](02-models.md) |
| `OPENAI_API_KEY` | Key for the `openai` provider (required for it) | [2](02-models.md) |
| `OPENAI_BASE_URL` | Different base URL for `openai`, e.g. a compatible service | [2](02-models.md) |
| `ANTHROPIC_API_KEY` | Key for the `anthropic` provider (required for it) | [2](02-models.md) |
| `GEMINI_API_KEY` | Key for the `gemini` provider (required for it) | [2](02-models.md#google-gemini) |
| `GEMINI_BASE_URL` | Different base URL for `gemini`; default `https://generativelanguage.googleapis.com` | [2](02-models.md#google-gemini) |
| `AWS_ACCESS_KEY_ID` | Access key of the `bedrock` provider; needs `AWS_SECRET_ACCESS_KEY` | [2](02-models.md#amazon-bedrock) |
| `AWS_SECRET_ACCESS_KEY` | Secret key of the `bedrock` provider | [2](02-models.md#amazon-bedrock) |
| `AWS_SESSION_TOKEN` | Session token for temporary AWS credentials (optional) | [2](02-models.md#amazon-bedrock) |
| `AWS_REGION` | Region of the `bedrock` provider; default `us-west-2` | [2](02-models.md#amazon-bedrock) |
| `AWS_DEFAULT_REGION` | Region of `bedrock` if `AWS_REGION` is not set | [2](02-models.md#amazon-bedrock) |
| `AWS_BEARER_TOKEN_BEDROCK` | Bedrock API key; replaces the access keys and wins if both are set | [2](02-models.md#amazon-bedrock) |
| `BEDROCK_BASE_URL` | Different base URL for `bedrock` (tests, VPC endpoints, proxies); default `https://bedrock-runtime.<region>.amazonaws.com` | [2](02-models.md#amazon-bedrock) |
| `<NAME>_API_KEY` | Key for an OpenAI-compatible provider (required for it): `OPENROUTER_API_KEY`, `DEEPSEEK_API_KEY`, `MISTRAL_API_KEY`, `GROQ_API_KEY`, `DASHSCOPE_API_KEY`, `XAI_API_KEY` | [2](02-models.md#openai-and-compatible-services) |
| `<NAME>_BASE_URL` | Different base URL for that provider: `OPENROUTER_BASE_URL`, `DEEPSEEK_BASE_URL`, `MISTRAL_BASE_URL`, `GROQ_BASE_URL`, `DASHSCOPE_BASE_URL` (e.g. for a different region), `XAI_BASE_URL` | [2](02-models.md#openai-and-compatible-services) |
| `RKEV_BASE_URL` | Address of the System One service (e.g. RKEV) for decisions with `decider: system_one`; without it such a spec does not start | [4](04-flows.md#decisions-routing-by-a-question) |
| `RKEV_API_KEY` | Optional key for that service (sent as `Authorization: Bearer …`) | [4](04-flows.md#decisions-routing-by-a-question) |
| `ATLASSIAN_EMAIL`, `ATLASSIAN_API_TOKEN` | E-mail address and API token for the `jira_*` and `confluence_*` built-in tools; without them such a spec does not start | [3](03-tools.md#jira-confluence-and-git) |
| `GIT_TOKEN` | Access token for the `git_*` tools with `git.use_token: true` (user `x-access-token`; without it such a spec does not start) and for `tool_packages` with `git` sources (optional, anonymous without it) | [3](03-tools.md#jira-confluence-and-git) |
| `GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL` | Author of the commits of `git_commit`; default `grauwolf agent`, `agent@grauwolf.local` | [3](03-tools.md#jira-confluence-and-git) |
| `GRAUWOLF_REGISTRY_AUTH` | Credentials for tool package registries, a JSON map registry → `"user:token"` (basic credentials, also for the token request) or a bare bearer token; without an entry the Docker configuration is consulted (see `DOCKER_CONFIG`), then packages are pulled anonymously | [3](03-tools.md#tool-packages-from-a-registry-tool_packages) |
| `GRAUWOLF_TOOL_POLICY` | Path to the policy file for tool packages (allowed registries and git hosts, required signatures; same format as `grauwolf.toml`, table `[tool_packages]`). It wins over a `grauwolf.toml` next to the spec; without either, everything is allowed and no signature is required. Applies to every command that resolves tool packages and to `tools pull` | [3](03-tools.md#tool-packages-from-a-registry-tool_packages) |
| `GRAUWOLF_TOOLS_CATALOG` | The tool catalog of `grauwolf-tools catalog …` and of `push`: a registry (`registry.example.com` = `registry.example.com/grauwolf-catalog:latest`) or a full reference `registry/repository[:tag]`. `--catalog` wins; without either, `catalog add` and `push` use the catalog of the package's registry | [15](15-the-tool-catalog.md#which-catalog) |
| `SOURCE_DATE_EPOCH` | Build date of a packed tool package (`tools pack`, `tools push` of a folder) in seconds since 1970, for reproducible builds; default: now | [15](15-the-tool-catalog.md#what-a-package-says-about-itself) |
| `GRAUWOLF_CREDENTIALS` | Path to a file with the `[credentials]` section that maps variables to credential sources (`file:`, later `openbao:`); same format as `grauwolf.toml`. It wins over a `grauwolf.toml` next to the spec. A mapped variable must not also be set in the environment | – |
| `SPIFFE_ENDPOINT_SOCKET` | Socket of the SPIFFE Workload API of the SPIRE agent (`unix:///run/spire/sockets/agent.sock`). Set by the SPIFFE CSI driver's volume in Kubernetes. The agent's JWT-SVID, with which credential sources log in to OpenBao, comes from there; without it nothing changes | – |
| `SPIFFE_FETCH_TIMEOUT` | How long to wait at start-up for the pod's SPIFFE identity (`250ms`, `30s`, `1m`; default `30s`; `0s` tries once). A new pod often starts before SPIRE has synced its registration entry; the Workload API then answers `no identity issued`, and grauwolf retries with a growing pause until the deadline | [12](12-credentials-from-openbao.md) |
| `DOCKER_CONFIG` | Directory of the Docker `config.json` that tool package registries fall back to behind `GRAUWOLF_REGISTRY_AUTH` (default `~/.docker`): `auths` (`auth`, `identitytoken`), `credHelpers` and `credsStore`; helpers run as `docker-credential-<name> get` and are looked up in `PATH`; only consulted when a registry asks for credentials | [3](03-tools.md#tool-packages-from-a-registry-tool_packages) |
| `GRAUWOLF_SERVER_TOKEN` | Shared token for `spec serve` and for generated services | [9](09-operations.md) |
| `SERVER_PORT`, `A2A_PORT`, `A2A_BASE_PATH` | Port, additional A2A port and its path prefix of a generated service (`--service`) | [11](11-code-generation.md#as-an-http-service---service) |
| `GRAUWOLF_USERS_FILE`, `GRAUWOLF_ALLOW_ANONYMOUS`, `GRAUWOLF_DATA_DIR`, `GRAUWOLF_PUBLIC_URL`, `GRAUWOLF_A2A_PUBLIC_URL` | Users file, anonymous access, base directory and public URLs of a generated service | [11](11-code-generation.md#as-an-http-service---service) |
| `GRAUWOLF_SPIFFE`, `GRAUWOLF_SPIFFE_PORT`, `GRAUWOLF_SPIFFE_USERS`, `GRAUWOLF_SPIFFE_AUDIENCE`, `GRAUWOLF_SPIFFE_TRUST_DOMAINS`, `GRAUWOLF_SPIFFE_PUBLIC_URL` | SPIFFE callers (ADR-0109, [chapter 9](09-operations.md#agents-calling-agents-with-spiffe)): `1`/`true` switches on the mTLS port (`GRAUWOLF_SPIFFE_PORT`, default `8443`) and JWT-SVIDs on the HTTP port; `GRAUWOLF_SPIFFE_USERS` is the file that maps SPIFFE IDs to users; `GRAUWOLF_SPIFFE_AUDIENCE` the audience of JWT-SVIDs (default the own SPIFFE ID); `GRAUWOLF_SPIFFE_TRUST_DOMAINS` further accepted trust domains (reserved for federation); `GRAUWOLF_SPIFFE_PUBLIC_URL` the URL of the mTLS port that its A2A agent card names (default `https://localhost:<port>`). `spec serve` has the flags `--spiffe`, `--spiffe-port`, `--spiffe-users` |
| `GRAUWOLF_IDENTITY_URL`, `GRAUWOLF_IDENTITY_SPIFFE_ID` | Delegation in an agent (ADR-0111, [chapter 14](14-acting-on-behalf-of-people.md)): the identity service where an Entra bearer token is exchanged for a Txn-Token and whose keys verify the `Txn-Token` header of callers; needs `GRAUWOLF_SPIFFE`. `GRAUWOLF_IDENTITY_SPIFFE_ID` is the service's SPIFFE ID: checked over `https://` (mTLS; default any ID of the own trust domain), the JWT-SVID audience over `http://` (required there) |
| `GRAUWOLF_IDENTITY_TENANT`, `GRAUWOLF_IDENTITY_ISSUER`, `GRAUWOLF_IDENTITY_AUDIENCE`, `GRAUWOLF_IDENTITY_SCOPE`, `GRAUWOLF_IDENTITY_JWKS_URL` | `grauwolf identity serve`: what people's Entra tokens must name – tenant (`tid`), issuer (`iss`), audience (`aud`, the grauwolf identity API: a comma-separated list, e.g. its application ID and its `api://…` URI; a token is accepted when its `aud` is any of them), scope in `scp` (default `access_as_user`); the JWKS URL (default from the issuer's OpenID discovery) | [14](14-acting-on-behalf-of-people.md) |
| `GRAUWOLF_IDENTITY_PUBLIC_URL`, `GRAUWOLF_IDENTITY_TRUST_DOMAIN`, `GRAUWOLF_IDENTITY_POLICY` | `grauwolf identity serve`: its own URL (`iss` of the Txn-Tokens), the trust domain (`aud`; default the one of its SPIFFE ID), the policy file (which callers may request tokens on behalf of people, for which purposes; read again when it changes) | [14](14-acting-on-behalf-of-people.md) |
| `GRAUWOLF_IDENTITY_TRANSIT_KEY`, `GRAUWOLF_IDENTITY_TRANSIT_MOUNT`, `GRAUWOLF_IDENTITY_OPENBAO_ADDR`, `GRAUWOLF_IDENTITY_OPENBAO_ROLE`, `GRAUWOLF_IDENTITY_OPENBAO_AUDIENCE`, `GRAUWOLF_IDENTITY_OPENBAO_MOUNT` | `grauwolf identity serve`: the OpenBao Transit key (`ecdsa-p256`) that signs the Txn-Tokens and its mount (default `transit`); OpenBao's address and the JWT login with the service's JWT-SVID (role; audience, default `openbao`; mount, default `jwt`) | [14](14-acting-on-behalf-of-people.md) |
| `GRAUWOLF_IDENTITY_GRANT_CLIENT_ID`, `GRAUWOLF_IDENTITY_GRANT_TOKEN_URL`, `GRAUWOLF_IDENTITY_GRANT_SCOPE`, `GRAUWOLF_IDENTITY_GRANT_KV_MOUNT` | `grauwolf identity serve`, grants: the public client whose refresh tokens grants hold (without it the service has no grants), Entra's token endpoint (default: from the issuer's OpenID discovery), the scope of the redemption (default `<audience URI>/<scope> offline_access`) and the KV v2 mount of the grants (default `kv`) | [14](14-acting-on-behalf-of-people.md) |
| `GRAUWOLF_LOGIN_TENANT`, `GRAUWOLF_LOGIN_AUTHORITY`, `GRAUWOLF_LOGIN_CLIENT_ID`, `GRAUWOLF_LOGIN_SCOPE`, `GRAUWOLF_LOGIN_IDENTITY_URL` | Defaults of `grauwolf login` (the flags win, the last login is the fallback); the identity URL is also read by `grauwolf identity grant` | [14](14-acting-on-behalf-of-people.md#signing-in-on-the-command-line-grauwolf-login) |
| `GRAUWOLF_CONFIG_DIR` | Directory of the token cache of `grauwolf login` (default `$XDG_CONFIG_HOME/grauwolf`, `~/.config/grauwolf`) | [14](14-acting-on-behalf-of-people.md#signing-in-on-the-command-line-grauwolf-login) |
| `GRAUWOLF_CONVERSATIONS_DIR`, `GRAUWOLF_CONVERSATIONS_DB` | Where a generated service keeps its conversations (`conversation_id`): a directory (relative to `GRAUWOLF_DATA_DIR`) or a database URL (`sqlite://…`, `postgres://…`; never printed, needs the facade feature `sql`) instead of the memory of the process; setting both is a start-up error (as `spec serve --conversations-dir` / `--conversations-db`) | [11](11-code-generation.md#as-an-http-service---service) |
| `GRAUWOLF_PRICES` | Path of the price table of a generated agent (the file of `--prices`), relative to `GRAUWOLF_DATA_DIR` / the base directory; events then carry `cost`, and `limits.max_cost` works | [11](11-code-generation.md) |
| `GRAUWOLF_CHAT_API` | `1` or `true` switches on `/api/chat` and `/api/signature` in a generated service (as `spec serve --chat-api`) | [9](09-operations.md#the-chat-api-apichat-and-apisignature) |
| `GRAUWOLF_OTEL`, `GRAUWOLF_OTEL_CONTENT` | `1` or `true` makes a generated service export its spans via OTLP/HTTP (as `spec serve --otel`; target and service name from `OTEL_EXPORTER_OTLP_*` and `OTEL_SERVICE_NAME`); `GRAUWOLF_OTEL_CONTENT=1` also records contents (as `--otel-content`). `0`/`false`/empty is off, any other value is a start-up error naming the variable; a build without the facade feature `otel` refuses `1` with `this build has no OpenTelemetry (feature `otel`)`. An incoming `traceparent` is continued (ADR-0123) | [9](09-operations.md#opentelemetry), [13](13-setting-up-a-cluster.md#traces-in-tempo) |
| `GRAUWOLF_EVENTS` | `1` or `true` makes a generated service write every event of its runs (and of its sub-agents) as one JSON line to stderr, as `spec serve --events`, so the events are in the pod log. `0`/`false`/empty is off, any other value is a start-up error naming the variable | [9](09-operations.md#events-as-json-lines), [13](13-setting-up-a-cluster.md#events-in-the-pod-log) |
| `AGENT_SIGNATURE_PATH` | Signature file for `/api/signature`; default `agent-signature.json` in the base directory | [9](09-operations.md#the-chat-api-apichat-and-apisignature) |
| `OLLAMA_BASE_URL` | In chat API mode: address of the Ollama server if `OLLAMA_HOST` is not set | [9](09-operations.md#the-chat-api-apichat-and-apisignature) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Target of the OpenTelemetry export (`--otel`) | [9](09-operations.md) |
| `OTEL_EXPORTER_OTLP_HEADERS` | Headers of the export, e.g. for credentials | [9](09-operations.md) |
| `OTEL_SERVICE_NAME` | Service name in the spans; default `grauwolf` | [9](09-operations.md) |
| `PGVECTOR_URL` | Conventional name of the variable that holds the PostgreSQL URL (`postgres://user:password@host:5432/db`) of `knowledge.store` and `long_term_memory.vector_store`; the spec names the variable in `url_env`, so any name works. Needs the pgvector extension | [7](07-conversations-and-knowledge.md#knowledge-and-memory-in-postgresql-with-pgvector) |
| `PGVECTOR_TEST_URL` | PostgreSQL with pgvector for the opt-in database tests of `grauwolf-store-pgvector` (`cargo test -p grauwolf-store-pgvector --test postgres`); without it they return early | [Developer guide](../developer-guide/README.md) |
| `CARGO`, `RUSTC` | Programs for the tool build and `generate --check`; default `cargo` and `rustc` | [3](03-tools.md) |
| `PATH`, `HOME`, `CARGO_HOME`, `RUSTUP_HOME`, `RUSTUP_TOOLCHAIN` | The only variables the tool build passes on to `cargo` | [10](10-self-made-tools.md#security) |
| any, as `${VAR}` | Replaced in `headers` of `mcp_servers` and `agent_tools`; if the variable is missing, that is an error | [8](08-external-tools-and-agents.md) |

## Default directories

All relative to the working directory in which you start `grauwolf`:

| Directory | Contents | Option |
|---|---|---|
| `.grauwolf/tools` | Cache of built tools and of pulled tool packages (`packages/`) | `--cache` |
| `.grauwolf/conversations` | Conversations (`<id>.json`, per user under `users/<hex>/<id>.json`) | `--conversations-dir` |
| `.grauwolf/runs` | Checkpoints (`<run-id>.json`) | `--runs-dir` |

Paths *in* the spec (`builtin_tools.root`, `knowledge.sources`, `tools_dir`, `code_file` …), on
the other hand, are relative to the spec file.
