# ADR-0121: `grauwolf-k8-ops agent deploy` – running an agent in its namespace

- **Status:** Accepted (User, 2026-11-14: "mach agent deploy"; decisions Q1–Q8 as recommended)
- **Date:** 2026-11-14
- **Related:** ADR-0120 (`grauwolf-k8-ops`: one agent, one namespace, identity, role, secrets),
  ADR-0110 (credential sources), ADR-0109 (SPIFFE), ADR-0115 (agent tree), ADR-0093/0119 (generated
  service), ADR-0096 (images), ADR-0053 (`/health`, server token).

## Context
`grauwolf-k8-ops` sets up an agent's identity, OpenBao role and secrets (ADR-0120) and proves them
with `agent check`, but it does not run the agent. Running it needs a Deployment with the SPIFFE CSI
socket, the `grauwolf.toml` of the agent, the settings of the restricted Pod Security Standard, a
service, the agent's files and its server token. Today all of that is written by hand.

## Decision
1. **What runs** (Q1). `agent deploy <name> --image <image> [--spec <file>]`.
   - With `--spec`, the image is the grauwolf CLI, and the pod runs `grauwolf spec serve` on the spec
     from a ConfigMap.
   - Without `--spec`, the image is a generated service (`spec generate --service --dockerfile`) that
     contains its spec.
   - Everything else is the same for both: port, health check, identity, configuration and secrets.
2. **The agent's files** (Q2). With `--spec`, the tool reads the whole agent tree (prompts, skills,
   sub-agents, knowledge files) with `grauwolf-agent-tree`. It stores the tree in a ConfigMap
   `grauwolf-agent`, which is mounted as a folder.
   - It refuses a tree that does not fit into a ConfigMap (about 1 MB).
   - It refuses a tree with tool code that has to be built.
   - The refusal says: build an image with `spec generate --service --dockerfile` and deploy that
     without `--spec`.
3. **Configuration** (Q3). `--env NAME=VALUE` (repeatable) sets values that are not secret in the
   pod's environment, for example the Ollama URL. Secrets keep coming from OpenBao through
   `secret set … --variable`. An `--env` for a variable that is mapped to OpenBao is an error.
4. **Server token** (Q4). On the first deploy the tool creates a random token and stores it at
   `kv/agents/<name>/server-token`. It maps the token to `GRAUWOLF_SERVER_TOKEN` in the agent's
   `grauwolf.toml`, so the service requires it. A new `secret get <agent> <key> --reveal` prints a
   secret's value; without `--reveal` it refuses.
5. **Reachability** (Q5). A ClusterIP service always. With `--host <name>`, also an Ingress for that
   host. TLS and the entry point are left to the cluster's defaults (on the owner's cluster, Kyverno and
   cert-manager add them). Without `--host`, the agent is only reachable inside the cluster.
6. **Data** (Q6). `GRAUWOLF_DATA_DIR` is an `emptyDir` by default. `--data-size <quantity>` makes it a
   PVC. There is always exactly one replica.
7. **Commands** (Q7).
   - **`agent deploy`** is repeatable. It applies only what differs and restarts the pod only when the
     spec, the image, the environment or the configuration changed. It waits until the pod is ready
     and `/health` answers. On failure it prints the last log lines.
   - **`agent status <name>`** shows the pod, its restarts, the image and the endpoint.
   - **`agent logs <name> [--follow]`** prints the pod's log.
   - **`agent undeploy <name>`** removes the Deployment, Service, Ingress, spec ConfigMap and PVC
     (the PVC only with `--purge-data`). It keeps the identity, the role and the secrets;
     `agent remove` still removes everything.
   - **`doctor`** reports deployed agents: ready or not, plus the image.
8. **Image** (Q8). `--image` is required. `deploy` uses the pull secret copied by
   `agent add --pull-secret`, or copies one itself with `--pull-secret`. A fixed grauwolf image built by
   CI is a separate step.

## Consequences
- One tool takes an agent from identity to a running service; the manifests of
  `examples/spire-openbao/` stay as documentation.
- Specs with tool code or large trees need an image of their own; the error says how.
- The server token lives in OpenBao like every other secret; nobody has to invent or store one.

## Implementation notes
- **Commands.** `agent deploy <name> --image <image> [--spec FILE] [--env NAME=VALUE]... [--host HOST] [--port N]
  [--data-size QTY] [--cpu QTY] [--memory QTY] [--pull-secret NS/NAME] [--timeout SECONDS]`,
  `agent status <name> [--json]`, `agent logs <name> [--follow] [--tail N]`,
  `agent undeploy <name> [--purge-data] [--yes]`, `secret get <agent> <key> --reveal`.
  Code: `deploy.rs` (objects, wait, status, logs, undeploy), `agent_files.rs` (the tree as a ConfigMap).
- **Deployment.** Name and service account `<name>`, labels `app.kubernetes.io/name=grauwolf`,
  `grauwolf.io/agent`, `app.kubernetes.io/managed-by`. One replica with strategy `Recreate` (a PVC can be used
  by one pod). Pod security as in `check.rs`, plus `fsGroup` 65532 (for the data volume) and no service
  account token. The CSI volume `csi.spiffe.io` with `SPIFFE_ENDPOINT_SOCKET`, and `SPIFFE_FETCH_TIMEOUT=2m`
  (the default is 30 s; a first identity can take longer). `grauwolf-config` is mounted at `/etc/grauwolf` and
  `GRAUWOLF_CREDENTIALS=/etc/grauwolf/grauwolf.toml`. Container port `http`, default 8080, `--port`. Startup
  (5 min), readiness and liveness probes on `/health`, which `spec serve` and the generated service answer
  without a token. `--cpu` and `--memory` set request and limit to the same value, and only when given.
  The pull secret is the one `agent add --pull-secret` attached; `deploy --pull-secret` copies and attaches it too.
- **A restart only when something changed.** The pod template carries `grauwolf.io/config-hash`: the hash
  of the pod spec (image, command, environment, mounts, resources), the text of `grauwolf.toml` and the
  ConfigMap with the agent files. `ensure` applies an object only when its own applied-hash differs, so an
  unchanged deploy writes nothing. A `secret set --variable` changes `grauwolf.toml`, so the next deploy restarts.
- **Working directory and writable folders.** `spec serve` reads files relative to the spec's folder and the
  conversations relative to the working directory. The tree is mounted read-only, so the working directory is
  `/data` (the `emptyDir` or the PVC). Folders the agent writes (`builtin_tools.root`, `tools_dir`, the folder of
  `long_term_memory.store` and `knowledge.index`) are directories of the data volume mounted at their place (see the notes on writable folders
  below; first versions refused the store and the index). For a generated service
  `GRAUWOLF_DATA_DIR` stays `/app` (set by the image), `SERVER_PORT` is set, and with `--data-size`
  `GRAUWOLF_CONVERSATIONS_DIR=/data/conversations`.
- **Agent tree (deviation from "prefer `items`").** The files are in the ConfigMap `grauwolf-agent` under flat
  keys `<index>-<name>` (valid ConfigMap keys; text in `data`, other files base64 in `binaryData`) and mounted with
  one `subPath` volume mount per file at `/app/spec/<relative path>`. A volume with `items` would work for the
  paths, but Kubernetes projects such files as symlinks into a hidden directory of the volume, and the skill check of
  `grauwolf-builtin-tools` refuses a symlink that leads out of its skill folder. A `subPath` mount is a plain
  file. A change of the files restarts the pod through the hash (a `subPath` file is not updated in place anyway).
  Read with `grauwolf_agent_tree::AgentTree::read` (all errors of the tree, sub-agents included); files are the
  spec of every node and its `companions()`. The size limit is 1,000,000 bytes of data (base64 counted for binary
  files). Refused, with the hint to build an image: tools with code (`tools`, graph code), `tool_packages`,
  a `tools_dir` that holds tool files, git skills, a tree that is too large (and, as first made, `long_term_memory.store`, `knowledge.index`: lifted, see the notes on writable folders). The app crate may depend on
  `grauwolf-agent-tree` (an adapter, ADR-0016 and `arch::K8S_OPS_APP` only forbid app crates); it pulls the tool
  build crates into the binary.
- **Server token.** 32 bytes from `getrandom`, hex (64 characters), stored with the same call as `secret set`
  (`kv_put_field`, field `value`) only if the secret does not exist, then `GRAUWOLF_SERVER_TOKEN` is mapped in
  `grauwolf.toml` (the secret first, so the configuration never points at nothing). If the variable is already mapped
  to something else, `deploy` keeps that and creates no token. `undeploy` and redeploys keep the token.
- **`--env`.** Names must be valid variable names; a name that is mapped to OpenBao in the agent's `grauwolf.toml` is
  an error, so are the names `deploy` sets itself (`SPIFFE_*`, `GRAUWOLF_CREDENTIALS`, `GRAUWOLF_DATA_DIR`,
  `GRAUWOLF_CONVERSATIONS_DIR`, `SERVER_PORT`, `GRAUWOLF_SERVER_TOKEN`) and a name given twice. A name that is not in
  `grauwolf_config::VARIABLES` is a warning on stderr (so external names such as `HTTP_PROXY` work). The values are in the
  Deployment, not in a Secret, because they are not secret; the docs say so.
- **Ingress.** `networking.k8s.io/v1`, one rule for `--host`, path `/` (Prefix) to the service port; no TLS block, no
  class (the cluster's defaults add them). `deploy` without `--host` deletes an Ingress of an earlier run, and without `--spec`
  it deletes the ConfigMap `grauwolf-agent` of an earlier run.
- **Waiting.** Polls every 250 ms: the Deployment's `observedGeneration`, `updatedReplicas`, `readyReplicas`, then
  `GET /api/v1/namespaces/<ns>/services/http:<name>:<port>/proxy/health` through the API server. Fails at once when the
  pod's container waits with `CrashLoopBackOff`, `ImagePullBackOff`, `ErrImagePull`, `InvalidImageName`,
  `CreateContainerConfigError` or `CreateContainerError`; otherwise at `--timeout` (default 300 s). Both print the pod's
  state and the last 20 log lines (the previous run's if the current one is empty) on stderr and exit with 1.
- **`status`, `logs`.** `status` prints deployed or not, ready, the pod (phase, ready, restarts, waiting reason), image,
  in-cluster endpoint, host and whether the server token exists (`unknown` without an OpenBao token; never its value);
  `--json` has the same. `logs` streams with `pod_log_stream` (kube-rs `Client::send` and the response body, no extra
  feature): complete lines, `--tail` default 100, `--follow` until the pod or the connection ends.
- **`undeploy`, `remove`, `doctor`.** `undeploy` lists what it deletes (Deployment, Service, Ingress, `grauwolf-agent`;
  the PVC with `--purge-data`, after a confirmation unless `--yes`) and what it keeps. `agent remove` lists a deployed agent
  as part of the namespace and deletes it with it (the namespace carries everything). `doctor` has a line
  `agent <name>: deployment` for a deployed agent: `ok` with the image, or `fail` (not ready) with the way to look further;
  an agent that is not deployed has no line.
- **`secret get <agent> <key> --reveal`** prints the value and a newline to stdout and nothing else (untrimmed); without
  `--reveal` it refuses; errors name agent and key only.
- **Tests** (`tests/deploy.rs`, `tests/commands.rs`; `FakeCluster::set_deployment_outcome` and `pod_log_stream` in
  `grauwolf-k8s`): the Deployment, Service and ConfigMaps of `deploy` with and without `--spec` (restricted settings, CSI
  volume, environment, config mount, file mounts), no write on a repeat and a new template hash on a change, the refusals
  (before any write), the token (created once, kept, mapped, never printed), Ingress and PVC, ready, crash loop, timeout and
  unhealthy, `status` and `logs`, `undeploy` (and `--purge-data`), `remove`, `doctor`, `secret get`.
- **Open points.** Not checked against a real cluster yet. A `subPath` mount of many files may need a look at the number of
  mounts for very large trees. `--spec` with a `long_term_memory.store` or `knowledge.index` that has no folder of its own is refused (see the notes on writable folders).

## Implementation notes: writable folders of the spec tree (2026-11-15, closes the open point of the ADR-0110 live check)
- **Decision (owner: "close gap 2"):** refusing a relative `builtin_tools.root` or a `tools_dir` would have made `--spec` useless for
  agents that write files. Instead `deploy` leaves the spec as it is and mounts a directory of the data volume
  (`subPath: files/<relative path>`, the `emptyDir` or the PVC) at exactly that place under `/app/spec`, for every relative
  `builtin_tools.root` and `tools_dir` of the tree (root spec and sub-agents, relative to their own spec; `self_tools.save` writes into
  the `tools_dir`, so there is no third place). The paths are normalized lexically: `./work/`, `a/../work` and `work` are one place.
- **Rules:** an absolute path stays as it is (a warning unless it is `/data` or below, because it must exist in the image); a path that leaves the
  spec folder (`..` above the root spec's folder) is refused with a hint, before anything is written; `root` unset or `.` is the spec's own folder
  and cannot be mounted over, so it stays as it is.
- **Spec files under a writable folder (decision):** a mount hides what is below it, so it is never placed over files of the spec. Seeding the
  folder from the ConfigMap with an init container was rejected (it needs a shell and `cp` in the image and a rule for the next redeploy: overwrite what
  the agent wrote, or keep it and miss spec updates). Instead: a `root` that holds spec files stays a read-only folder of the spec with a warning,
  exactly as before (read-only data in the root keeps working); a `tools_dir` that holds tool files is refused with the old hint (the tools have to be
  built, and `tools_dir` is not a companion path, so the files would not be shipped).
- **Permissions:** the directories are made before the agent starts by an init container `data-dirs` (the agent's image, `mkdir -p /data/files/…`,
  same restricted container settings, same `--cpu`/`--memory`), as user 65532 with `fsGroup` 65532. A `subPath` directory that kubelet makes itself belongs
  to root with mode 0750, so the agent could not write into it. The image needs `mkdir` (the grauwolf CLI image, a Debian base, has it).
  No init container without a writable place.
- **`long_term_memory.store` and `knowledge.index` (lifted refusals):** the same mechanism works, so these two are no longer refused. The
  file is written inside its parent folder, so that folder is the writable place (`"store": "state/memory.json"` gives `/app/spec/state`, which
  also holds what the stores write next to the file). It is clean only if the folder is the file's own: a file in the spec's own folder
  (`"store": "memory.json"`) would need the whole spec folder, and a folder with other files of the spec would hide them; both are refused with the hint
  to use a sub folder of its own (`state/<name>`). A local copy of the store or the index (left by a local run) is not shipped: the agent starts
  empty and rebuilds the index. Mounting a single file by `subPath` was rejected: kubelet makes a missing target a directory, and the stores write
  temporary files next to the file. An absolute path warns unless under `/data`.
- **Hash and status:** the mounts and the init container are part of the pod spec, so the hash covers them (a new `root` restarts the pod). The output has
  a line `writable: <path> (<what asks for it>; the data volume, files/<path>)`; `agent status` lists the paths (`writable` in `--json`).
- **Tests** (`tests/deploy.rs`): root, `tools_dir`, sub-agents, absolute path (with and without warning), `..`, no init container without a place, hash, root with spec files, `tools_dir` with files, store and index (folder, local copy not shipped, own folder and shared folder refused, absolute path).

## Implementation notes: callers by SPIFFE ID (2026-11-15, ADR-0109 Q7-Q9)
- **`agent deploy` always has SPIFFE on:** `GRAUWOLF_SPIFFE=1`, the container port and Service port `mtls` (`--mtls-port`, default 8443, must differ from `--port`), `GRAUWOLF_SPIFFE_PORT`, `GRAUWOLF_SPIFFE_USERS=/etc/grauwolf/callers/callers.json`, `GRAUWOLF_SPIFFE_PUBLIC_URL=https://<name>.<ns>.svc:<mtls port>`; the four names are reserved for `--env`. Probes, `--port` and the Ingress are unchanged; the output has a line `mTLS endpoint https://…`, and `status` too.
- **The callers ConfigMap is outside the restart hash.** `grauwolf-callers` is mounted as a directory (a `subPath` would not see updates), made as `{}` only if missing and never replaced by a redeploy, kept by `undeploy`. The hash covers the pod spec, `grauwolf.toml` and the agent files only, so `agent allow|deny` apply without a restart (test: `changing_only_the_callers_does_not_restart_the_pod`). Restricted-PSS: a read-only ConfigMap volume needs nothing more.
- **NetworkPolicy `grauwolf-mtls-callers`:** `deploy` syncs it to its ports (new, it admits nobody on the mTLS port); `agent allow|deny` change its callers. See the notes in ADR-0109.
- **Warnings:** after the checks and before the first write, `deploy --spec` warns about calls to other agents that cannot work (no `auth.spiffe`, wrong ID, not the mTLS port over https, not allowed). `undeploy` keeps the callers (ConfigMap and policy); `agent remove` cleans the other agents' callers; `doctor` has the lines `callers`, `NetworkPolicy`, `caller spec` and the warning about enforcement.
- **Open points:** no `--events` for the pod (`--otel` since the notes below); the Deployment and the policy are not checked against a real cluster (the live check of ADR-0109 Q10 does).

## Implementation notes: traces with `--otel` (2026-11-15, ADR-0049, ADR-0109 `caller`)
- **Why:** the `caller` of a call between agents is the attribute `grauwolf.caller` of the `invoke_agent` span (`OtelFeature`); the live check could not see it because the pod exported nothing. Owner's request: "`agent deploy --otel`, so that `caller` becomes visible in your Tempo" (a Grafana LGTM stack in namespace `lgtm`, Service `lgtm`, ports `otel-grpc` 4317 and `otel-http` 4318; grauwolf exports OTLP/HTTP).
- **Options:** `--otel`, `--otel-endpoint <url>`, `--otel-content`; the last two need `--otel`. With `--spec` the container runs `grauwolf spec serve … --otel [--otel-content]` with `OTEL_EXPORTER_OTLP_ENDPOINT=<endpoint>` (the base URL; the SDK adds `/v1/traces`) and `OTEL_SERVICE_NAME=<agent name>`. The endpoint must be an `http://` or `https://` URL.
- **Decision: the default endpoint is `http://lgtm.lgtm.svc:4318`**, not "required". A required endpoint would make the first try a usage error; the default is the owner's stack and the usual name of one, and it is not silent: `--help` shows `[default: …]`, the output prints `tracing: on, OTLP/HTTP to <url>`, `status` shows it and `doctor` warns when its Service does not exist. User guide chapter 13 documents it.
- **Superseded by ADR-0123 (decision 4): the refusal is gone.** Without `--spec`, `--otel` sets `GRAUWOLF_OTEL=1` (and `GRAUWOLF_OTEL_CONTENT=1` with `--otel-content`) next to the endpoint and service name; `status` reads the switches back; `--env` refuses `GRAUWOLF_OTEL` and `GRAUWOLF_OTEL_CONTENT` with `--otel` as well. The original decision, kept for the record: a generated service image (no `--spec`) was refused with `--otel`. A generated service has no tracing: `grauwolf-server`'s `service::main` reads `SERVER_PORT`, the token and the SPIFFE variables but never builds an `OtelTracing`, and the generated crate has no `--otel`. A switch such as `GRAUWOLF_OTEL=1` would need the service to create `OtelTracing::init_with`, add its feature to every run (`GeneratedAgent::run` takes the features of the request) and to the provisioned agents' features, flush it on shutdown, and the generator's `Cargo.toml` template and its equivalence tests would change: a change of its own, not part of this one. Reported as open.
- **Settings are part of the pod template**, so the existing hash restarts the pod exactly when tracing is turned on or off or the endpoint or `--otel-content` changes. Nothing is stored apart from the Deployment: a deploy without `--otel` removes the flag and both variables, like every other option (documented).
- **`--env`:** `OTEL_EXPORTER_OTLP_ENDPOINT` and `OTEL_SERVICE_NAME` are refused with `--otel` ("set by `agent deploy --otel`"), allowed without it (a user may point an agent at a collector by hand, and the pod does not read them without `--otel`; `OTEL_EXPORTER_OTLP_HEADERS` was never reserved).
- **`status`** (`tracing: on (<endpoint>)`, `, with contents`, or `tracing: off`; `--json`: `tracing` is `{endpoint, content}` or `null`), read back from the pod template. **`doctor`** has `agent <name>: tracing` for an agent with tracing on: `ok` if the Service of `http://<svc>.<ns>.svc[…][:port]` exists, a warning if not, `ok (not checked)` for any other URL. It never contacts the collector.
- **NetworkPolicy:** `grauwolf-mtls-callers` has `policyTypes: ["Ingress"]` only (`callers.rs`), so egress to the collector is not restricted by grauwolf; a cluster with its own default deny of egress has to allow the collector.
- **Tests:** `tests/deploy.rs` (`otel_*`, `env_cannot_set_the_otel_variables…`, `tracing_changes_the_hash…`, `status_shows_whether_tracing_is_on`, `doctor_checks_that_the_service_of_the_collector_exists…`).
- **Live check should look at:** `agent deploy … --spec … --otel` with a call from another agent over SPIFFE; in Tempo `{ span.grauwolf.caller = "spiffe://…" }` finds the `invoke_agent` span of the called agent; the spans arrive under the service name of the agent; a redeploy without `--otel` restarts the pod and traces stop.

## Implementation notes: `--events` (2026-11-15)
- **Why:** the events of an agent's runs (prompts, tool calls, cost) are what one looks at when a run goes wrong, and the pod log is where a log collector finds them. `spec serve --events` writes them as JSON lines to stderr; the open point above ("no `--events` for the pod") is closed.
- **Option:** `agent deploy --events`. With `--spec` the container runs `spec serve … --events`. For an image (no `--spec`) the generated service gets the new switch `GRAUWOLF_EVENTS=1|true` (`ServiceConfig.events`, a variable of `config::VARIABLES` and of reference-cli; any other value than `0`/`false`/empty is a start-up error naming the variable), like `GRAUWOLF_OTEL` in ADR-0123. The service attaches one `JsonlEventLog` on stderr to the main agent and to every sub-agent, so the lines match what `spec serve --events` writes.
- **Part of the pod template**, hence of the hash: a deploy without `--events` removes the flag or the variable and restarts the pod; a repeat with it changes nothing. `--env GRAUWOLF_EVENTS` is refused only together with `--events`.
- **`agent status`** shows `events: on` or `events: off` (`--json`: `events`), read back from the pod template (`events_of`).
- **Decision: `agent logs --events-only`** exists. It keeps the lines that start with `{"event":` (the pod's log mixes them with start-up messages and errors), so `agent logs reviewer --events-only | jq` works. It is a filter on the same stream, so `--follow` and `--tail` work as before (`--tail` counts log lines, not events). `agent logs` without it is unchanged.
- **Tests:** `tests/deploy.rs` (`events_*`, `status_shows_whether_events_are_on`, `logs_events_only_…`), the host's `ServiceConfig`, the server's `an_event_log_sees_the_events_of_the_agent_and_of_its_sub_agents`, and the slow equivalence case `generated_services_answer_health_and_run` (a service built from a spec with `GRAUWOLF_EVENTS=1` prints `agent_starting` on stderr).
- **Live check should look at:** `agent deploy reviewer --spec … --events`, one call, and `agent logs reviewer --events-only` shows the JSON lines; the same for an image built with this version; a deploy without `--events` removes them again.

## Implementation notes: `--identity` (2026-11-16, ADR-0111 Q5)
`agent deploy --identity` sets `GRAUWOLF_IDENTITY_URL` (the mTLS URL of `grauwolf-identity` in
namespace `identity`) and `GRAUWOLF_IDENTITY_SPIFFE_ID` in the pod (part of the hash; a deploy
without it removes them) and is refused while `identity install` has not run. `--env` refuses both
names. With `--egress` the identity service is a destination (its namespace, the mTLS pod port).
`status` shows `identity: on (<url>)`/`off`, `agent check` prints a hint when the service is
missing or not ready, `doctor` fails its `identity service` line then.

## Live check (2026-11-14, image built from `612a027`)
- **First deploy.** `agent add reviewer`, then `agent deploy reviewer --spec docs/user-guide/examples/01-hello.json --env OLLAMA_HOST=… --env OLLAMA_THINK=false`. It created the server token in OpenBao, the ConfigMaps, the Deployment and the Service. The pod was ready, with `/health` ok, after 8 s.
- **Redeploys.** A second identical deploy wrote nothing (`unchanged`). A deploy with a changed `OLLAMA_HOST` gave `Deployment … updated` and a new pod.
- **status and logs.** `agent status` showed the pod running and ready with 0 restarts, the image, the endpoint and `server token: exists`. `agent logs` printed the service's start line.
- **Calls from inside the cluster.** A short Job ran in the agent's namespace under the restricted standard, with the token taken from a temporary Secret:
  - `/run` without a token, or with a wrong one, answered 401;
  - with the token, `/run` answered 200 `{"output":"Hello, friend! How is your day?"}` (Ollama `qwen3.5:9b` on the owner's LAN).
- **Finding: the API server proxy.** The API server's service proxy drops the `Authorization` header. A call through it with the token gets 401, while `/health` works. This is documented in user guide chapter 13. Token-protected calls go through the Ingress (`--host`) or come from inside the cluster.
- **Teardown.** `doctor` showed `agent reviewer: deployment … ready`. `agent undeploy` removed the Deployment, Service and spec ConfigMap and kept the identity and `server-token`. After it, `agent status` said "not deployed". `agent remove --purge-secrets --yes` removed the rest.
- **Not checked live.** The Ingress (`--host`) and the PVC (`--data-size`).

## Live check of `--otel` (2026-11-15, image built from `cf048f8`, Tempo of the owner's LGTM stack)
- `agent deploy reviewer` and `coordinator` were run with `--otel`, at the default endpoint `http://lgtm.lgtm.svc:4318`. `reviewer` allowed `coordinator`. `agent status` showed `tracing: on (…)`, and `doctor` showed the tracing line as ok because Service `lgtm/lgtm` exists.
- One call to `coordinator` produced a call of `reviewer` over SPIFFE mTLS. The TraceQL search `{ span.grauwolf.caller = "spiffe://agentswork.de/ns/coordinator/sa/coordinator" }` found the trace of service `reviewer`. Its root span `invoke_agent reviewer` has the attributes `gen_ai.agent.name=reviewer` and `grauwolf.caller=spiffe://agentswork.de/ns/coordinator/sa/coordinator`, and its children are `node call_llm` and `chat qwen3.5:9b`. The call of `coordinator` is a trace of its own, root span `invoke_agent coordinator`.
- **Open point (done in ADR-0123):** the two traces were not linked. The A2A and MCP clients did not send a W3C `traceparent`, and the server did not continue one, so caller and callee appeared as separate traces, related only through `grauwolf.caller` and the time. ADR-0123 decisions 1 and 2 link them: caller and callee are one trace.
- A redeploy without `--otel` restarted the pod, and `status` then showed `tracing: off`. Everything was removed afterwards.

## Live check of the writable folders (2026-11-15, images built from `6f69c64`)
- **`writer`** used `"root": "work"`, `git.auth` against the test git server, and was deployed with `--data-size 1Gi`. `deploy` printed `writable: /app/spec/work (builtin_tools.root of writer.json; the data volume, files/work)` and created the PVC. The init container `data-dirs` completed (exit 0), and the agent cloned, committed and pushed (`Pushed test-writer2`).
- **`memo`** used `"long_term_memory.store": "state/memory.json"` with `nomic-embed-text`. `deploy` printed `writable: /app/spec/state …`. `remember` answered `Remembered.`, and a second request recalled "my favourite colour is teal".
- **Persistence.** A redeploy of `writer` with an extra tool created a new pod (`writer-5f9867d67-…` → `writer-6598fcf89c-…`). It read `repo/hello-writer.txt` (`hello from writer`) written by the old pod, so the folder lives on the PVC.
- Everything was removed afterwards: agents and their PVC with the namespaces, the `git-test` namespace, the ClusterSPIFFEID, the build namespace and the images. The restricted token was revoked.
