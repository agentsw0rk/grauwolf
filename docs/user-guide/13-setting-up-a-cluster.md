# 13. Setting up a cluster with grauwolf-k8-ops

[Chapter 12](12-credentials-from-openbao.md) explains how an agent gets its credentials from OpenBao
with its SPIFFE identity, and the [example](../../examples/spire-openbao/README.md) lists the manual
steps. `grauwolf-k8-ops` does these steps for you: it is a small program (separate from `grauwolf`,
so the Kubernetes client stays out of the agent runtime) that talks to your cluster with your
kubeconfig and to OpenBao with an admin token. Every command can be run again; it changes only what
differs and says `created`, `updated` or `unchanged` for each object.

What it does **not** do: install SPIRE, spire-controller-manager, the SPIFFE CSI driver or OpenBao.
`doctor` checks that they are there.

## The model: one agent, one namespace

An agent called `reviewer` gets

- a namespace `reviewer` (override with `--namespace`) with the labels `grauwolf.io/agent=reviewer`
  and `app.kubernetes.io/managed-by=grauwolf-k8-ops`, and the restricted Pod Security Standard
  enforced;
- a service account `reviewer`, so its pods get the SPIFFE ID
  `spiffe://<trust domain>/ns/reviewer/sa/reviewer` (one `ClusterSPIFFEID` called `grauwolf-agents`
  selects all namespaces with the agent label);
- an OpenBao role and policy `agent-reviewer`: the role accepts exactly that SPIFFE ID, the policy
  only reads `kv/data/agents/reviewer/*` – the agent cannot read another agent's secrets;
- a ConfigMap `grauwolf-config` with the `[credentials]` section of its `grauwolf.toml`
  (see [chapter 12](12-credentials-from-openbao.md) and the [configuration reference](reference-config.md)).

The tool never touches a namespace without these labels: `agent add` refuses an existing namespace
that is not the agent's, `agent remove` only deletes the agent's own.

## What you need

- A kubeconfig that may create namespaces, ConfigMaps, Jobs and `ClusterSPIFFEID`s (`doctor` checks
  with access reviews). `--kubeconfig <file>`, else `KUBECONFIG`, else `~/.kube/config`; `--context
  <name>` picks another context than the current one (a context that does not exist is an error that
  lists the available ones). `doctor` starts with a line `Cluster <server URL> (context <name>)`, so you see
  which cluster it is about before it checks anything; the other commands do not repeat it (their output is
  often pasted or captured).
- A running SPIRE (server, agents, spire-controller-manager, SPIFFE CSI driver) and OpenBao,
  initialized and unsealed.
- An OpenBao token that may configure auth methods, mounts, policies and roles: `export
  OPENBAO_TOKEN=...` or `--token-file <file>` (never an argument). The root token works, but a
  restricted one is enough after the first `openbao setup`
  ([below](#working-without-the-root-token)). By default OpenBao is reached
  through the Kubernetes API's service proxy (`/api/v1/namespaces/openbao/services/http:openbao:8200/proxy/`),
  so no ingress is needed; `--openbao-addr http://host:8200` reaches it directly.

The defaults fit the [example](../../examples/spire-openbao/README.md): SPIRE in `spire-system`,
the OIDC discovery provider in `spire-oidc`, OpenBao in `openbao` (service `openbao:8200`), mounts
`jwt` and `kv`, audience `openbao`. Each can be changed with a global option (`--spire-namespace`,
`--oidc-namespace`, `--openbao-namespace`, `--openbao-service`, `--jwt-mount`, `--kv-mount`,
`--audience`; `--agent-openbao-address` is the address written into the agents' configuration).

## Walkthrough

```console
$ grauwolf-k8-ops doctor
Prerequisites (not installed by grauwolf-k8-ops)
  ok    cluster: reachable, Kubernetes v1.34.1+k3s1
  ok    rights on namespaces: get, list, create, delete allowed (agent add/remove)
  ...
  ok    SPIRE server: StatefulSet spire-system/spire-server ready (1/1)
  ok    OpenBao: reachable, initialized, unsealed (version 2.7.2)
  ok    OpenBao token: valid; may configure the auth method, mount, policies, roles and agent secrets
  warn  OpenBao token policy: working with the root token
        next: create a restricted one with `grauwolf-k8-ops openbao token`
Setup (installed by grauwolf-k8-ops)
  fail  OIDC discovery provider: Deployment spire-oidc/spire-oidc-discovery-provider not found
        next: run `grauwolf-k8-ops oidc install`
  fail  JWT auth method: auth/jwt/ is not enabled
        next: run `grauwolf-k8-ops openbao setup`
```

`doctor` changes nothing. It prints one line per check – `ok`, `warn` or `fail` with the reason and,
where there is one, the next step – in two sections: the **prerequisites** (what the tool does not
install: cluster, rights, SPIRE, CSI driver, CRD, controller manager, OpenBao and its health, the
token's rights) and the **setup** (what it installs: the discovery provider and its keys, the JWT
auth method and its `jwks_url`, the KV mount, the `ClusterSPIFFEID`, the trust domain in every place
that holds it, and for each agent its namespace, service account, role, policy and ConfigMap). The
exit code is 1 when a check fails; warnings do not count. `--json` prints an array of
`{section, check, status, reason, hint}`.

```console
$ grauwolf-k8-ops oidc install
trust domain agentswork.de, SPIRE 1.15.5; discovery provider in namespace spire-oidc
Namespace spire-oidc: created
...
$ grauwolf-k8-ops openbao setup
auth method jwt/ (jwks_url http://spire-oidc-discovery-provider.spire-oidc.svc/keys): created
KV v2 mount kv/: created
OpenBao policy grauwolf-k8-ops (what this tool needs, for `openbao token`): created
$ grauwolf-k8-ops agent add reviewer
trust domain agentswork.de; identity spiffe://agentswork.de/ns/reviewer/sa/reviewer
namespace reviewer: created
service account reviewer/reviewer: created
ClusterSPIFFEID grauwolf-agents: created
ConfigMap reviewer/grauwolf-config: created
OpenBao policy agent-reviewer (reads kv/data/agents/reviewer/*): created
OpenBao role auth/jwt/role/agent-reviewer (subject spiffe://agentswork.de/ns/reviewer/sa/reviewer): created
$ grauwolf-k8-ops secret set reviewer jira --variable ATLASSIAN_API_TOKEN
Value for reviewer/jira (hidden):
secret kv/agents/reviewer/jira#value: stored (version 1, 32 bytes)
variable ATLASSIAN_API_TOKEN -> openbao:kv/agents/reviewer/jira#value in reviewer/grauwolf-config: updated
$ grauwolf-k8-ops agent check reviewer --image registry.example.com/grauwolf:1.0
running Job reviewer/grauwolf-check-3fa9c1 (check of ATLASSIAN_API_TOKEN, image registry.example.com/grauwolf:1.0)
Job grauwolf-check-3fa9c1: deleted
agent reviewer: resolved ATLASSIAN_API_TOKEN from openbao (32 bytes)
```

1. **`oidc install`** installs (or adopts, if you created it from the example) the SPIRE OIDC
   discovery provider in its own namespace (spire-controller-manager ignores `spire-system`), with
   the image version of the running SPIRE server and its own `ClusterSPIFFEID`. The trust domain
   comes from the SPIRE server's configuration, the one source of truth.
2. **`openbao setup`** enables the JWT auth method with the provider's `jwks_url` (`--jwks-url`,
   `--bound-issuer` if the SPIRE server sets `jwt_issuer`) and the KV v2 mount, and writes the policy
   `grauwolf-k8-ops` (see [Working without the root token](#working-without-the-root-token)).
3. **`agent add <name>`** creates what the model above lists. `--pull-secret <namespace>/<name>`
   copies an existing image pull secret into the agent's namespace and attaches it to the service
   account. `agent list [--json]` shows the agents and their SPIFFE IDs.
4. **`secret set <agent> <key>`** stores a value at `kv/agents/<agent>/<key>` (field `value`). The
   value is read from **stdin** (`printf %s "$TOKEN" | grauwolf-k8-ops secret set ...`) or, on a
   terminal, from a hidden prompt – never from an argument or the environment, and never printed.
   `--variable NAME` also maps the configuration variable `NAME` to the secret in the agent's
   ConfigMap. `secret list <agent> [--json]` shows keys and versions, never values; `secret delete
   <agent> <key>` removes all versions.
5. **`agent check <name> --image <image>`** runs `grauwolf credentials check` as a Job in the agent's
   namespace under the restricted standard (numeric user 65532, seccomp `RuntimeDefault`, all
   capabilities dropped), waits (`--timeout`, default 120 s), prints the log and deletes the Job –
   also when it fails or times out. It checks every variable mapped in the agent's configuration, in
   one Job, and prints one line per variable; `--variable <NAME>` (repeatable) narrows it. If a
   variable fails with `permission denied`, a hint names the agent's policy (it reads only
   `kv/data/agents/<name>/*`) and the path that was tried. The exit code is 1 when any variable fails.
6. **`agent remove <name>`** deletes the namespace and the OpenBao role and policy, and with
   `--purge-secrets` the secrets under `kv/agents/<name>/`. It prints what it removes and asks for
   confirmation unless `--yes` is given.

## Working without the root token

Everything above works with the root token, but the tool needs far less. `openbao setup` also
writes the policy `grauwolf-k8-ops`, which allows exactly what the commands do and nothing else:
the JWT auth method and its configuration (`sys/auth/jwt`, `auth/jwt/config`), the KV mount
(`sys/mounts/kv`), the policies and roles `agent-*` (and reading its own policy), the secrets below
`kv/data/agents/*` and `kv/metadata/agents/*`, and the few reads of `doctor` (`sys/health`,
`sys/capabilities-self`, `auth/token/lookup-self`, the legacy role `grauwolf-agents`). There is no
`sys/*` wildcard, no access to other secrets and no right to create tokens. The paths follow
`--jwt-mount` and `--kv-mount`.

Use it like this: run `openbao setup` **once** with the root token (or an admin token), create a
restricted token, and work with that from then on:

```console
$ export OPENBAO_TOKEN=<root token>
$ grauwolf-k8-ops openbao setup
...
OpenBao policy grauwolf-k8-ops (what this tool needs, for `openbao token`): created
$ export OPENBAO_TOKEN=$(grauwolf-k8-ops openbao token --ttl 8h)
token with the policy grauwolf-k8-ops, valid for 8h; use it with `export OPENBAO_TOKEN=...` or --token-file
$ grauwolf-k8-ops doctor
...
  ok    OpenBao token policy: the token has the policy grauwolf-k8-ops
```

`openbao token [--ttl <duration>] [--renewable]` (default `--ttl 8h`) creates a token that has only
that policy (not even `default`) and prints it on stdout, nothing else, so a command substitution
captures it; the note goes to stderr. It needs a token that may create tokens (root or an admin
token) and the policy to exist (`openbao setup`). The token has no parent when the token in use
may create such tokens (root), so it outlives it; otherwise it is a child of it. `doctor` warns
`working with the root token` (next step: create a restricted one with `grauwolf-k8-ops openbao
token`) while the token in use has the policy `root`, and says `ok` for a token with the policy
`grauwolf-k8-ops`. If the tool's needs change in a later version, run `openbao setup` again with
the root token: it updates the policy (the restricted token cannot write it).

## Running an agent

`agent deploy` takes an agent that has an identity (`agent add`) to a running service
([ADR-0121](../adr/0121-grauwolf-k8-ops-agent-deploy.md)). The example is
[`01-hello.json`](examples/01-hello.json), an agent that talks to an Ollama server in the cluster:

```console
$ grauwolf-k8-ops agent deploy reviewer --image registry.example.com/grauwolf:1.0 \
    --spec examples/01-hello.json --env OLLAMA_HOST=http://ollama.ollama.svc:11434 \
    --host reviewer.example.com
agent reviewer (namespace reviewer): image registry.example.com/grauwolf:1.0, grauwolf spec serve examples/01-hello.json
server token kv/agents/reviewer/server-token: created (read it with `grauwolf-k8-ops secret get reviewer server-token --reveal`)
ConfigMap reviewer/grauwolf-config (GRAUWOLF_SERVER_TOKEN -> the server token): updated
ConfigMap reviewer/grauwolf-callers (no callers yet): created
ConfigMap reviewer/grauwolf-agent (1 file(s), 1 KiB): created
Deployment reviewer/reviewer: created
Service reviewer/reviewer (ports 8080, 8443 mTLS): created
Ingress reviewer/reviewer (host reviewer.example.com): created
NetworkPolicy reviewer/grauwolf-mtls-callers (port 8443 from nobody; port 8080 from anywhere): created
waiting for the rollout and /health (at most 300 s)
agent reviewer: ready, /health answers ok
endpoint http://reviewer.reviewer.svc:8080
mTLS endpoint https://reviewer.reviewer.svc:8443
host reviewer.example.com
$ grauwolf-k8-ops agent status reviewer
agent reviewer (namespace reviewer): deployed, ready
pod reviewer-7d9f-x1: Running, ready, 0 restart(s)
image registry.example.com/grauwolf:1.0
endpoint http://reviewer.reviewer.svc:8080
mTLS endpoint https://reviewer.reviewer.svc:8443
host reviewer.example.com
server token: exists
$ grauwolf-k8-ops secret get reviewer server-token --reveal   # the bearer token of the service
$ grauwolf-k8-ops agent logs reviewer --follow
$ grauwolf-k8-ops agent undeploy reviewer
```

Call the service with `Authorization: Bearer <server token>`, from inside the cluster (the endpoint) or
through the Ingress of `--host`. The Kubernetes API's service proxy (`kubectl proxy`, the
`…/services/http:<name>:8080/proxy/` path) is not a way in for calls that need the token: the API
server drops the `Authorization` header, so the service answers 401. `/health` needs no token and
works through it.

What it makes, in the agent's namespace:

- **A Deployment** `<name>` with one replica (strategy `Recreate`, so a volume is never used by two
  pods). The pod follows the restricted Pod Security Standard (numeric user 65532, seccomp
  `RuntimeDefault`, all capabilities dropped, no privilege escalation, no service account token),
  mounts the SPIFFE CSI volume (`SPIFFE_ENDPOINT_SOCKET`, `SPIFFE_FETCH_TIMEOUT=2m` for the start-up
  race) and the ConfigMap `grauwolf-config` as `grauwolf.toml` (`GRAUWOLF_CREDENTIALS` points at it),
  and has a startup, readiness and liveness probe on `/health`. `--cpu` and `--memory` set request
  and limit; without them nothing is set.
- **What runs.** With `--spec <file>` the image is the grauwolf CLI and the pod runs `grauwolf spec
  serve` on the spec and its whole agent tree (prompts, skills, sub-agents, knowledge and other
  files), which `deploy` stores in the ConfigMap `grauwolf-agent` and mounts read-only under
  `/app/spec`, one plain file each. Without `--spec` the image is a generated service (`spec
  generate --service --dockerfile`) that contains its spec and keeps its own entry point; `--port`
  becomes its `SERVER_PORT`. The container's working directory is `/data`.
- **Callers by SPIFFE ID** ([ADR-0109](../adr/0109-workload-identity-with-spiffe.md)). The pod always runs
  with `GRAUWOLF_SPIFFE=1`: next to the HTTP port it has a second container port `mtls` (`--mtls-port`,
  default 8443, `GRAUWOLF_SPIFFE_PORT`) that requires a client certificate, and
  `GRAUWOLF_SPIFFE_PUBLIC_URL=https://<name>.<namespace>.svc:<mtls port>`, which its A2A agent card names.
  The ConfigMap `grauwolf-callers` (made empty, `{}`, if it is missing, and never replaced by a later deploy)
  is mounted as a **directory** at `/etc/grauwolf/callers`, and `GRAUWOLF_SPIFFE_USERS` points at
  `callers.json` in it. A directory, not a `subPath` of one file, because only a directory follows an update
  of the ConfigMap, and the service reads the file again when it changes. The probes stay on the HTTP port.
  The ConfigMap is not part of the hash below: `agent allow` and `deny` take effect without a restart.
  `deploy` also keeps the NetworkPolicy `grauwolf-mtls-callers` at the ports of this deploy; see
  [Letting agents call each other](#letting-agents-call-each-other).
- **The Service** `<name>` (ClusterIP, port `--port`, default 8080, and the port `mtls`) and, with `--host`, an
  **Ingress** for that host. It has no TLS block and no class: the cluster's defaults add them (on the
  owner's cluster Kyverno and cert-manager do). Without `--host` the agent is only reachable inside the
  cluster, at `http://<name>.<namespace>.svc:<port>`. Running `deploy` without `--host` removes an
  Ingress from an earlier run.
- **Data.** `/data` is an `emptyDir` by default (lost with the pod); `--data-size 1Gi` makes it a
  PersistentVolumeClaim `<name>-data`, and a generated service then keeps its conversations there
  (`GRAUWOLF_CONVERSATIONS_DIR`).
- **Writable folders (`--spec`).** The spec folder is read-only, but an agent writes into some of its
  folders. For every **relative** folder of the tree (the root spec and its sub-agents, each relative to
  its own spec) that the agent writes, `deploy` mounts a directory of the data volume at exactly that place
  under `/app/spec`, so the spec stays as it is: `builtin_tools.root` (`"root": "work"` gives
  `/app/spec/work`), `tools_dir` (for `self_tools` with `save`) and the folder of `long_term_memory.store` and of
  `knowledge.index` (`"store": "state/memory.json"` gives `/app/spec/state`; a copy of the file that a local run
  left in your spec folder is not shipped, the agent starts with an empty store and rebuilds the index). The directory is
  `files/<path>` of the data volume, so it survives a restart with `--data-size` and is an `emptyDir`
  without. An init container makes the directories before the agent starts (as the agent's user; a
  directory that kubelet makes for a `subPath` would belong to root and could not be written), with the
  same restricted settings and the image of the agent, which needs `mkdir`; the grauwolf image has it. The
  output shows a line `writable: …` per folder and `agent status` lists them. Rules:
  an **absolute** path is left as it is and must exist in the image or lie under `/data`, else `deploy`
  warns; a path that **leaves the spec folder** (`..`) is refused; a `root` without a value or `"."` is the
  spec folder itself, which cannot be writable (use a sub-folder). A mount never hides spec files: a `root`
  that holds files of the spec stays a read-only folder of the spec (with a warning, so use another place
  for what the agent writes), a `tools_dir` that holds tool files is refused (they have to be built), and a
  `store` or `index` is refused if the file is in the spec's own folder or in a folder with other files of the
  spec (put it in a sub folder of its own, such as `state/memory.json`).
- **`--env NAME=VALUE`** (repeatable) sets values that are **not secret**, such as the Ollama URL. They
  are stored in the Deployment, not in a Secret, and everybody who can read the Deployment sees them.
  A name that the agent's `grauwolf.toml` maps to OpenBao is refused; a name that is not a configuration
  variable of grauwolf only draws a warning. Secrets keep going through `secret set --variable`.
- **The server token.** The first deploy creates 32 random bytes (hex) at
  `kv/agents/<name>/server-token` and maps `GRAUWOLF_SERVER_TOKEN` to it in `grauwolf.toml`, so the
  service requires `Authorization: Bearer <token>` (`/health` stays open). A redeploy and `undeploy` keep it;
  `secret get <name> server-token --reveal` prints it.
- **`--pull-secret <namespace>/<name>`** copies a pull secret and attaches it to the service account
  (as `agent add` does).
- **`--otel`** sends the agent's traces to a collector; see [Traces in Tempo](#traces-in-tempo).

`deploy` can be run again: it applies only what differs. A hash of the pod, the configuration and the
agent's files is an annotation of the pod template, so the pod restarts exactly when the spec, the
image, the environment, the port or the configuration changed (also after `secret set --variable`) and
not otherwise. Then it waits (`--timeout`, default 300 s) until the rollout is done and `/health`
answers through the API server's service proxy. If the pod crashes, cannot pull its image or does not
become ready in time, it prints the reason and the last 20 lines of the log and exits with 1; `agent
logs` and `agent status` look further. `agent undeploy` removes the Deployment, Service, Ingress and the
spec ConfigMap and keeps the identity, the role, the secrets, the callers (ConfigMap and NetworkPolicy) and the data volume (`--purge-data` deletes
the volume after a confirmation); `agent remove` still removes everything, deployed or not.
`doctor` has a line per deployed agent (ready or not, with its image) and, for an agent with tracing on,
one that says whether the Service of the collector exists.

**What `--spec` refuses** (the error says: build an image with `spec generate --service --dockerfile` and
deploy that without `--spec`): tools with code, code in the graph, tool packages and git skills (they
have to be built or fetched), a `tools_dir` that holds tool files, a path of the spec that leaves the spec
folder (`..`), a tree larger than about 1 MiB (a ConfigMap's limit), and a `long_term_memory.store` or
`knowledge.index` that is in the spec's own folder or in a folder with other spec files (the error suggests a sub
folder of its own).

### From a harness file: `--harness`

A [harness file](11-code-generation.md#a-project-per-harness-grauwolf-harness-generate) can hold the
settings of a generated service instead of flags ([ADR-0128](../adr/0128-harness.md)):

```json
{ "version": 1, "name": "reviewer", "spec": "../agents/reviewer.json",
  "image": "registry.example.com/reviewer:1.0", "port": 8080,
  "env": { "OLLAMA_HOST": "http://ollama.ollama.svc:11434" } }
```

```console
$ grauwolf harness generate harnesses/reviewer.harness.json      # build/reviewer/, build and push the image
$ grauwolf-k8-ops agent deploy --harness harnesses/reviewer.harness.json
```

The agent is the harness's `name` (give no name next to `--harness`). `image`, `port` and `env`
take the place of `--image`, `--port` and `--env` (without `image`: `<registry>/<name>:<version>`
with `[harness] registry` of the nearest `grauwolf.toml`; without `port`: 8080); the values of `env` go through the same checks.
A flag for a setting the harness file sets is refused (`--image: the harness file sets `image`
already; …`), so every value has one source. All other flags (`--host`, `--otel`, `--egress`, …)
work as before. The agent needs its identity first (`agent add reviewer`), as without a harness.

### Building a harness image: `agent build --harness`

The image of a harness can be built in the cluster, without Docker on your machine
([ADR-0129](../adr/0129-building-harness-images-in-the-cluster.md)): a Kaniko Job clones the
generated project from your git repository and pushes the image to your registry. The same build is
available in the studio; this command works without it.

```console
$ grauwolf harness generate harnesses/reviewer.harness.json      # build/reviewer/
$ git add build/reviewer && git commit -m "Generate reviewer" && git push
$ printf '%s' "$GHCR_TOKEN" | grauwolf-k8-ops build secret registry ghcr --server ghcr.io --username arek
$ grauwolf-k8-ops agent build --harness harnesses/reviewer.harness.json --registry-secret ghcr
...                                                               # Kaniko's log, line by line
built and pushed ghcr.io/arek/reviewer:0.1.0
$ grauwolf-k8-ops agent deploy --harness harnesses/reviewer.harness.json
```

The project folder is `build/<name>` of the working directory (`--project <dir>` names another one).
It must be committed and pushed: the build clones that commit, so the command refuses a folder with
uncommitted changes, a commit that is not on the remote, and a remote a cluster cannot clone. The image
pushed is the harness's `image`, else `<registry>/<name>:<version>` with `[harness] registry` of the
nearest `grauwolf.toml`; with neither there is nothing to push and the command says so. The Job
runs in the namespace `grauwolf-builds` (`--namespace`), which the command creates (not at the restricted
Pod Security level: Kaniko runs as root), with the Kaniko image of `--kaniko-image`; it is deleted when it
ends and gives up after `--timeout` seconds (default 1800). Exit code 0 when the image is pushed, 1 when
Kaniko failed or the time was up.

Credentials are Secrets of the build namespace, made by `build secret` and named with `--registry-secret`
and `--git-secret` (a Secret that does not exist is an error naming it):

- `build secret registry <name> --server <server> --username <user>`: the push credentials of a registry.
- `build secret git <name> --username <user>`: for a private repository.

The token is read from stdin or a hidden prompt, never from an argument, and never printed. Run the
command again with a new token to replace it (`created`, `updated` or `unchanged`).

### Events in the pod log

`agent deploy --events` writes every event of the agent's runs as one JSON line to stderr, so the events
are in the pod log, where a log collector (Loki, for example) picks them up. It is the cluster side of
`--events` of `spec serve` ([chapter 9](09-operations.md#events-as-json-lines)): with `--spec` the pod runs
`grauwolf spec serve … --events`; for an image without `--spec` the generated service gets
`GRAUWOLF_EVENTS=1` ([chapter 11](11-code-generation.md#as-an-http-service---service)), which also covers its
sub-agents. Events carry the input and the output of the agent, prompts and tool arguments: they may
contain personal data, and the pod log is no place for secrets.

```console
$ grauwolf-k8-ops agent deploy reviewer --image registry.example.com/grauwolf:1.0 --events
…
events: on, JSON lines on stderr (the pod log)
$ grauwolf-k8-ops agent logs reviewer --events-only | jq -c '{event, path}'
{"event":"agent_starting","path":null}
…
```

- **Like every other option, it is on only while it is given.** A deploy without `--events` removes the flag
  or the variable again; the setting is part of the pod template, so the pod restarts exactly when you
  turn it on or off.
- **`--env GRAUWOLF_EVENTS=…`** is refused together with `--events`; without `--events` it passes like any
  other `--env`.
- **`agent logs --events-only`** prints only the lines that are JSON events (they start with
  `{"event":`), without the other log lines; `agent logs` itself is unchanged.
- **`agent status`** shows `events: on` or `events: off`; `status --json` has `events` (`true`/`false`).
- Images built with an older grauwolf do not know `GRAUWOLF_EVENTS` and ignore it; build the image again.

### Traces in Tempo

With `--otel` the pod exports its traces ([chapter 9](09-operations.md#opentelemetry), ADR-0049) over
OTLP/HTTP. That is how you see in a Grafana Tempo which agent called which, and who the caller was:

```console
$ grauwolf-k8-ops agent deploy reviewer --image registry.example.com/grauwolf:1.0 \
    --spec examples/01-hello.json --otel
agent reviewer (namespace reviewer): image registry.example.com/grauwolf:1.0, grauwolf spec serve examples/01-hello.json
tracing: on, OTLP/HTTP to http://lgtm.lgtm.svc:4318
…
$ grauwolf-k8-ops agent status reviewer
…
tracing: on (http://lgtm.lgtm.svc:4318)
```

| Option | Meaning |
|---|---|
| `--otel` | The pod runs `grauwolf spec serve … --otel` with `OTEL_EXPORTER_OTLP_ENDPOINT=<endpoint>` and `OTEL_SERVICE_NAME=<agent name>`. For an image without `--spec`, the service gets `GRAUWOLF_OTEL=1` in its environment instead of the flag. |
| `--otel-endpoint <url>` | The OTLP/HTTP base URL of the collector; the SDK adds `/v1/traces`. Default `http://lgtm.lgtm.svc:4318`: the port `otel-http` of a Service `lgtm` in the namespace `lgtm`, as a Grafana LGTM stack (Loki, Grafana, Tempo, Mimir) is often installed. Pass your own URL when the collector is somewhere else; `--help` shows the default. |
| `--otel-content` | Also record prompts, answers and tool arguments and results in the spans (an image: `GRAUWOLF_OTEL_CONTENT=1`). They may contain personal data. |

- **Like every other option, tracing is on only while it is given.** A deploy without `--otel` removes the
  flag and both variables again. The settings are part of the pod template, so the pod restarts exactly
  when you turn tracing on or off or change the endpoint or `--otel-content`, and not on a repeat.
- **`--env` and the reserved variables.** With `--otel`, `--env OTEL_EXPORTER_OTLP_ENDPOINT=…`, `--env
  OTEL_SERVICE_NAME=…`, `--env GRAUWOLF_OTEL=…` and `--env GRAUWOLF_OTEL_CONTENT=…` are refused (use
  `--otel-endpoint`; the service name is the agent's name). Without
  `--otel` they pass like any other `--env`, and `OTEL_EXPORTER_OTLP_HEADERS` always does: it is stored in
  the Deployment like every `--env` value, so it is no place for a secret.
- **An image without `--spec`** works too: a generated service (`spec generate --service`) has no flags,
  so `--otel` sets `GRAUWOLF_OTEL=1` (and with `--otel-content` `GRAUWOLF_OTEL_CONTENT=1`) in its
  environment ([chapter 11](11-code-generation.md#as-an-http-service---service)). Images built with an
  older grauwolf do not know the variable and ignore it, so they export nothing; build the image again.
- **The network.** The NetworkPolicy `grauwolf-mtls-callers` only restricts *ingress* (its `policyTypes` is
  `Ingress`), so the pod's way out to the collector is not touched by it. If your cluster has a default
  deny of egress of its own, allow the collector's port there.
- **`status`** shows `tracing: on (<endpoint>)` (with `, with contents` if `--otel-content` is on) or
  `tracing: off`; `status --json` has `tracing` with `endpoint` and `content`, or `null`.
  **`doctor`** has the line `agent <name>: tracing` for an agent with tracing on. When the endpoint is
  `http://<service>.<namespace>.svc[…][:port]`, it is `ok` if that Service exists and a warning if
  it does not (the traces would be lost). It never contacts the collector; another URL is not checked.

**What you see in Tempo.** Each request to the agent is a trace with the span `invoke_agent reviewer`
(the service is the agent's name), below it `node …`, `chat <model>` and `execute_tool …` spans. When
another agent called over SPIFFE ([next section](#letting-agents-call-each-other)), the `invoke_agent`
span has the attribute `grauwolf.caller` with the caller's SPIFFE ID. In Grafana's Explore with the
Tempo data source, TraceQL finds those calls:

```
{ span.grauwolf.caller = "spiffe://agentswork.de/ns/coordinator/sa/coordinator" }
```

Without a filter, `{ resource.service.name = "reviewer" && span.grauwolf.caller != nil }` lists every
call that came from another agent.

**One trace from caller to callee.** A call from one agent to another is one trace, not two: the
calling agent sends the W3C header `traceparent` with its A2A or MCP request
([chapter 9](09-operations.md#opentelemetry)), and the called agent's `invoke_agent` span continues the
trace. Deploy both agents with `--otel` (to the same collector). In the trace, `invoke_agent coordinator`
has the `execute_tool reviewer` span below it, and `invoke_agent reviewer` is a child of that span. The
services stay separate (`coordinator`, `reviewer`), so a structural TraceQL query finds the traces that
reach from one to the other:

```
{ resource.service.name = "coordinator" } >> { resource.service.name = "reviewer" }
```

`grauwolf.caller` is still on `invoke_agent reviewer`, so the search above works as before. An agent
without `--otel` neither sends nor continues a trace; its callees then start traces of their own.

## Limiting what an agent may reach (egress)

Everything an agent may reach follows from its spec: the model hosts, the MCP servers and remote
agents, the `fetch` hosts, Jira, git, OpenBao. `grauwolf spec egress` lists these destinations, and
`agent deploy --egress` turns them into a NetworkPolicy `grauwolf-egress`, so that a `fetch` to a
host that is not in the spec, or a direct connection attempt, is blocked by the cluster.

```sh
grauwolf spec egress reviewer.json
```

```text
run   http://localhost:11434  model (ollama)  [/model/provider]  (from OLLAMA_HOST)
run   https://acme.atlassian.net:443  Jira  [/builtin_tools/jira/base_url]
run   http://peer.team.svc:8080  agent peer (A2A)  [/agent_tools/peer/url]
```

`--format json` prints an array with `scheme`, `host`, `port`, `any_port`, `purpose`, `pointer` (with the
spec file for a sub-agent), `phase` and `from_env`; `spec generate --dockerfile` writes the same to
`app/egress.json`. The rules of the list:

- **Never a guess.** An address that only a variable knows (`RKEV_BASE_URL`, a pgvector `url_env`, the
  OTLP endpoint with `--otel`) has no host, only `from_env`. An address with a default (a model
  provider's: `api.openai.com`, `localhost:11434` for Ollama, the default region of `bedrock`) has the
  default, and `from_env` names the variable that may replace it (`OPENAI_BASE_URL`, `OLLAMA_HOST`,
  `AWS_REGION`, …).
- **A `fetch` host inside the cluster has no fixed port.** `fetch.allowed_hosts` names hosts without
  ports, so for an external host the list assumes 443 (and 80 with `allow_http`). A cluster host
  (`<service>.<namespace>.svc`, `*.<namespace>.svc`) may be reached on any port its Service has, for
  example the mTLS port of another agent: its entry has `"port": null` and `"any_port": true`, and the text
  format shows `https://target.target.svc:*`.
- `phase: build` marks what is only needed while the image is built: tool package registries and git
  sources of packages and skills. They are not needed at run time and `deploy` does not allow them.
- Duplicates are merged (the purposes are joined), host names are lower case, the order is fixed.

With `agent deploy --egress` (needs `--spec`), `deploy` resolves `from_env` with what it knows: the
`--env` values, the OpenBao address of the agent's configuration and the `--otel-endpoint`. It then
writes the NetworkPolicy `grauwolf-egress` for the agent's pods:

| Destination | Rule |
|---|---|
| DNS | UDP and TCP 53 to the pods labelled `k8s-app=kube-dns` in `kube-system` (k3s; `--egress-dns-namespace` and `--egress-dns-selector` change it); always |
| In the cluster (`<service>.<namespace>.svc`) | The namespace (label `kubernetes.io/metadata.name`) and the port. A NetworkPolicy applies to the **pod** port, so `deploy` reads the Service and allows its `targetPort`; if the Service is not there, it allows the service port and warns. A `fetch` host in the cluster has no fixed port ("any port"): `deploy` allows the pod ports of **all** ports of its Service; for a pattern (`*.<namespace>.svc`) or a Service that is not there yet (with a warning) it allows the whole namespace without a port restriction |
| IP address (for example `OLLAMA_HOST=http://10.1.2.3:11434`) | `ipBlock` `/32` (`/128`) and the port |
| `localhost` | No rule: it stays inside the pod |
| External host name | By port only, see below |

**The limit: no names.** A plain NetworkPolicy matches addresses, not names. An external host such as
`acme.atlassian.net` therefore cannot be allowed by itself. By default the policy allows its port (443)
to every address outside the cluster's pod and service ranges (`--cluster-cidr`, default the k3s ranges
`10.42.0.0/16` and `10.43.0.0/16`), and `deploy` warns with the names that are only port-restricted;
`agent status` shows `egress: <n> destinations, <m> only port-restricted`, and `doctor` lists them.
With `--egress-strict` they are left out and **blocked**; deploy says which. Matching names needs a
policy engine that knows FQDNs or an egress gateway.

More limits:

- **Ports of external `fetch` hosts.** The list and the policy assume 443 (and 80 with `allow_http`) for
  an external `fetch` host, because `allowed_hosts` has no ports. A `fetch` to an external host on another
  port is not covered and is blocked.
- **The first seconds of a pod.** NetworkPolicies are applied asynchronously by the network plugin or
  policy controller. On k3s, a new pod was unrestricted for a few seconds after it started, until the
  controller had applied the rules: a probe started right away reached `example.com:80` and other hosts
  the policy blocks, the same probe after 20 seconds did not. So the first seconds of a pod may not be
  covered. This is a property of the cluster's policy controller, not of grauwolf; do not start work that
  must not leave the cluster in that window.

```sh
grauwolf-k8-ops agent deploy reviewer --image registry.example.com/grauwolf:1.0 \
  --spec reviewer.json --env OLLAMA_HOST=http://10.1.2.3:11434 --egress
```

```text
NetworkPolicy reviewer/grauwolf-egress (4 destinations, 1 only port-restricted): created
note: egress: peer.team port 8080 is port 9090 of its pods: the policy allows 9090
warning: egress: 1 external destination(s) can only be restricted by port, not by name (NetworkPolicies match addresses): acme.atlassian.net:443; `--egress-strict` leaves them out
```

The policy is part of what `deploy` keeps in sync: a redeploy updates it, one without `--egress`
deletes it, `agent undeploy` removes it and `agent remove` removes it with the namespace. It is not in
the pod template, so changing it restarts nothing. It applies to the agent's pods only; the
`grauwolf-mtls-callers` policy (who may call the agent) stays separate. A destination whose address
comes from OpenBao (a mapped variable) or that no `--env` names stays unknown: `deploy` warns and no
rule allows it. A cluster that does not enforce NetworkPolicies ignores the policy; `doctor` checks that.

## Letting agents call each other

An agent can call another agent of the cluster over A2A or use it as an MCP server, and the target then
knows **who** is calling: the caller proves its SPIFFE ID with a certificate (mTLS, [chapter 9](09-operations.md)),
and the target maps that ID to a user ([ADR-0109](../adr/0109-workload-identity-with-spiffe.md), decisions Q7-Q9).
Nobody may call an agent until its owner says so.

### Walkthrough: coordinator calls reviewer

Two agents: `reviewer` (the target, [`13-reviewer.json`](examples/13-reviewer.json)) and `coordinator`
(the caller, [`13-coordinator.json`](examples/13-coordinator.json)), which passes drafts on to the
reviewer. The agents run in the namespaces `reviewer` and `coordinator` and the trust domain is
`agentswork.de`, as everywhere in this chapter.

1. **Create both agents** (identity, namespace, OpenBao role; see the walkthrough above):

   ```console
   $ grauwolf-k8-ops agent add reviewer
   $ grauwolf-k8-ops agent add coordinator
   ```

2. **Let the coordinator call the reviewer:**

   ```console
   $ grauwolf-k8-ops agent allow reviewer --from coordinator
   agent reviewer (namespace reviewer) allows spiffe://agentswork.de/ns/coordinator/sa/coordinator as user coordinator: created
   ConfigMap reviewer/grauwolf-callers: created
   NetworkPolicy reviewer/grauwolf-mtls-callers (port 8443 from namespace coordinator; port 8080 from anywhere): created
   ```

3. **Write the coordinator's spec.** The entry in `agent_tools` has the reviewer's service address on the
   **mTLS port over https** and the reviewer's SPIFFE ID, `spiffe://<trust domain>/ns/reviewer/sa/reviewer`
   (`agent list` prints it):

   ```json
   "agent_tools": {
     "reviewer": {
       "url": "https://reviewer.reviewer.svc:8443",
       "description": "Reviews a short text and names its main problems",
       "auth": { "spiffe": { "id": "spiffe://agentswork.de/ns/reviewer/sa/reviewer" } }
     }
   }
   ```

   The coordinator presents its certificate and accepts the server only if it has exactly that ID.

4. **Deploy both**, the reviewer first (a model server such as Ollama must be reachable, see
   [Running an agent](#running-an-agent)):

   ```console
   $ grauwolf-k8-ops agent deploy reviewer --image registry.example.com/grauwolf:1.0 \
       --spec examples/13-reviewer.json --env OLLAMA_HOST=http://ollama.ollama.svc:11434
   $ grauwolf-k8-ops agent deploy coordinator --image registry.example.com/grauwolf:1.0 \
       --spec examples/13-coordinator.json --env OLLAMA_HOST=http://ollama.ollama.svc:11434
   ```

   If the coordinator's spec did not fit (no `auth`, a wrong ID, `http://` or port 8080, or no `allow`), the
   deploy of the coordinator would print a warning for each problem, for example
   `warning: coordinator: agent_tools.reviewer (agent reviewer) calls agent reviewer, which has not allowed coordinator (…)
   (run `grauwolf-k8-ops agent allow reviewer --from coordinator`)`; the deploy goes on.

5. **Call the coordinator** from inside the cluster with its server token (`secret get coordinator
   server-token --reveal`), as in [Running an agent](#running-an-agent):

   ```console
   $ curl -s http://coordinator.coordinator.svc:8080/run -H "Authorization: Bearer $TOKEN" \
       -H 'Content-Type: application/json' -d '{ "input": "Review: our new login page is fast but ugly." }'
   ```

   The coordinator sends the text to the reviewer over A2A through the mTLS port; the answer is the
   reviewer's review in two sentences.

6. **See who called.** The reviewer maps the coordinator's ID to the user `coordinator`, so the call has
   its own conversations, memory and approvals there, and the reviewer's `agent_starting` event has the
   field `caller` with `spiffe://agentswork.de/ns/coordinator/sa/coordinator`
   ([chapter 9](09-operations.md#agents-calling-agents-with-spiffe)): in the `agent` events of
   `/run/stream`, with `spec serve --events`, and as the attribute `grauwolf.caller` of the span
   `invoke_agent` with OpenTelemetry. Deploy both agents with `--otel` to see the call as one trace in Tempo
   ([Traces in Tempo](#traces-in-tempo)): `{ span.grauwolf.caller = "spiffe://agentswork.de/ns/coordinator/sa/coordinator" }`.
   `agent deploy` starts `spec serve` without `--events`, so `agent logs` does not show the events.

7. **Counter-checks.** `agent deny reviewer --from coordinator` and the next call fails: with the
   NetworkPolicy enforced the connection times out, otherwise the reviewer answers `403`. A caller
   that was never allowed gets the same. `grauwolf-k8-ops doctor` lists the callers, the policy and
   the coordinator's calls (below).

### The commands

```console
$ grauwolf-k8-ops agent allow reviewer --from coordinator
agent reviewer (namespace reviewer) allows spiffe://agentswork.de/ns/coordinator/sa/coordinator as user coordinator: created
ConfigMap reviewer/grauwolf-callers: created
NetworkPolicy reviewer/grauwolf-mtls-callers (port 8443 from namespace coordinator; port 8080 from anywhere): created
$ grauwolf-k8-ops agent callers reviewer
agent reviewer (namespace reviewer): 1 caller(s)
  spiffe://agentswork.de/ns/coordinator/sa/coordinator  user coordinator  agent coordinator  network policy: admitted
NetworkPolicy reviewer/grauwolf-mtls-callers: matches
$ grauwolf-k8-ops agent deny reviewer --from coordinator
```

- **`agent allow <target> --from <caller> [--as <user>]`** writes the caller's SPIFFE ID
  (`spiffe://<trust domain>/ns/<caller namespace>/sa/<caller>`, the trust domain comes from the SPIRE
  server as everywhere in the tool) into the ConfigMap `grauwolf-callers` in the target's namespace, key
  `callers.json`: `{ "<SPIFFE ID>": "<user>" }`. The user is the caller's name unless `--as` says
  otherwise; it is the user of the calls in the target's conversations, memory and approvals. Both agents
  must exist (`agent add`). The command is repeatable and says `created`, `updated` or `unchanged`.
- **The NetworkPolicy `grauwolf-mtls-callers`** is kept in step with the map. It selects the pods
  labelled `grauwolf.io/agent=<target>` and has two ingress rules: the HTTP port (8080) from anywhere (the
  Ingress, the probes, people with tokens) and the mTLS port (8443) only from the namespaces of the
  callers (`kubernetes.io/metadata.name`). With no callers there is no rule for the mTLS port at all: a
  rule without `from` would admit everybody. A policy that was changed by hand is put right by the next
  `allow` or `deny`.
- **`agent deny <target> --from <caller>`** removes the entry and the namespace from the policy, also for
  a caller that no longer exists. It is repeatable too.
- **`agent remove <name>`** also takes the agent out of the callers and NetworkPolicies of the other agents
  that allowed it, so no entry dangles: the confirmation lists them, and the output says
  `ConfigMap <ns>/grauwolf-callers: <name> removed from the callers of <other>`.
- **`agent callers <target> [--json]`** lists the IDs with their users, the agent each belongs to
  (`no such agent` for a dangling entry), whether the policy admits its namespace, and whether the policy
  `matches`, `differs` from what the callers ask for, or is `missing`.

### The check of the calling spec

When a spec that `agent deploy --spec` takes to the cluster has an
`agent_tools` or `mcp_servers` entry whose URL is `<name>.<namespace>.svc` (or `….svc.cluster.local`) of
another agent set up with this tool, `deploy` prints a warning, never an error, if

- the entry has no `auth.spiffe`, or its `id` (`ids`, `trust_domain`) is not that agent's SPIFFE ID;
- the URL is not `https` on the target's mTLS port (the check is skipped for `"mode": "jwt"`, which goes to the HTTP port);
- the target has not allowed this caller (`agent allow`; only for entries that carry `auth.spiffe`).

`doctor` shows the same per deployed agent as the line `agent <name>: caller spec`, read from the agent's
`grauwolf-agent` ConfigMap; an agent deployed from an image without `--spec` has no such ConfigMap, and the
line says that its calls are not checked. Further lines per agent: `callers` (the ConfigMap exists and
parses; a warning for each entry that names an agent that no longer exists or another trust domain) and
`NetworkPolicy` (it matches the callers). **`doctor` cannot tell whether the cluster enforces
NetworkPolicies**: the API accepts them either way, and a network plugin that ignores them lets every pod
reach the mTLS port (the service itself still answers `403` to a caller it does not know, and refuses a pod
without a certificate). So it ends with the warning `NetworkPolicy enforcement`; to be sure, start a pod
in a namespace that no agent allows and try the port, `nc -zv -w 3 <agent>.<namespace>.svc 8443` must time out,
or run the live check.

**How fast `allow` and `deny` take effect.** Kubernetes copies a changed ConfigMap into a running
pod with a delay, usually 30–90 seconds, and the service picks the file up within 2 seconds after
that. In the live check a `deny` gave the first `403` after about 40 seconds, without a restart.
A NetworkPolicy change does not cut connections that are already open, such as the connection pool of
a calling agent, so a removed caller can still reach the port for a while. The callers file is what
refuses it.

## Acting on behalf of people

`grauwolf-k8-ops identity install` runs the identity service of
[ADR-0111](../adr/0111-delegation-with-transaction-tokens.md) in namespace `identity`, with its own
SPIFFE identity and an OpenBao Transit key that signs the Txn-Tokens; `agent deploy --identity`
connects an agent to it, `identity allow <agent>` lets an entry agent request tokens for people.
[Chapter 14](14-acting-on-behalf-of-people.md#in-a-cluster-with-grauwolf-k8-ops) explains it, and
[`examples/delegation/`](../../examples/delegation/README.md) walks through it with Entra Local and
`grauwolf login`. After an update of `grauwolf-k8-ops` run `openbao setup` again: its policy now
covers the identity service.

## Changing the trust domain

`trust-domain set <domain>` changes the trust domain of a running SPIRE, with everything that
holds it. It is the one change in this chapter that cannot be undone by running a command again:
**every SPIFFE ID changes** (`spiffe://old/ns/reviewer/sa/reviewer` becomes
`spiffe://new/ns/reviewer/sa/reviewer`), every workload gets a new SVID, and the SPIRE server
starts with an empty database and a new CA, because the CA of a trust domain cannot be renamed.
Everything that still names the old domain and is not part of the change **stops working**.

```console
$ grauwolf-k8-ops trust-domain set agentswork.de --consumers trust-domain-consumers.toml
```

Without `--apply` this only prints the plan and changes nothing:

- where the domain is now (SPIRE server, agent and controller-manager configuration, the OIDC
  provider, the CA in the server's bundle, the bound subject of every `agent-*` role in OpenBao);
- every ConfigMap with the exact lines that change (`- trust_domain = "old"` / `+ trust_domain =
  "new"`): in the SPIRE server and agent configuration only the `trust_domain` assignment, in the
  controller manager `trustDomain:`, in the OIDC provider the `trust_domain` of `workload_api`;
  lines that still mention the old domain afterwards (a `jwt_issuer`, say) are listed as a warning,
  they are not touched;
- the volume to recreate (scale the StatefulSet to 0, delete the PVC, scale back) and the wait for
  the new bundle;
- the restarts (agents, OIDC provider, spire-controller-manager if it is a Deployment) and the
  OpenBao roles with old and new subject;
- the consumers (below), what is left alone, and what the change means.

With `--apply` it asks for confirmation (`--yes` skips it; without a terminal and without `--yes`
it cancels), then does these steps in this order:

1. **Backup**: `--backup <file>` (default `trust-domain-backup-<UTC timestamp>.json` in the working
   directory; an existing file is never overwritten, and if it cannot be written nothing is
   changed). It holds the full objects of every ConfigMap that is rewritten, of the StatefulSet,
   DaemonSet and Deployments that are restarted, and the OpenBao roles. No Secret is read and
   no secret value is in it. The file is created readable by its owner only.
2. **Configuration** is rewritten as the plan showed.
3. **The server's data is recreated**: the StatefulSet is scaled to 0 (the replica count is kept in
   the annotation `grauwolf.io/trust-domain-replicas` meanwhile), the tool waits until its pod is
   gone, deletes the PVC of the data volume, scales back and waits until it is ready.
4. **It waits** until the bundle ConfigMap (the `k8sbundle` notifier's; name and namespace from the
   server configuration, default `spire-bundle`) has a CA with the URI SAN `spiffe://<new>`. Only
   then the SPIRE agents (restart annotation on the DaemonSet), spire-controller-manager (if it is a
   Deployment) and the OIDC provider are restarted, each waiting until it is ready.
5. **OpenBao**: the `bound_subject` of every agent role is set to the new SPIFFE ID (read, change,
   write: all other fields stay).
6. **Consumers** are rewritten and restarted.
7. **Verification**: every place holds the new domain (the agreement `doctor` checks for the trust domain). The exit code is 1
   if not, and the error says which step failed and where the backup is.

`--timeout <seconds>` (default 300) applies to each wait; a timeout is an error that names what it
waited for.

To go back to the state before a change, see [Restoring from a backup](#restoring-from-a-backup).

**It refuses** (with the reason, nothing is changed) when the SPIRE server's data cannot simply be
recreated – the `DataStore "sql"` is not `sqlite3`, or its file or the keys of the `KeyManager
"disk"` are not on a volume claim template of the StatefulSet – and when the configuration contains
federation (`federation { ... }` or `federates_with`): changing federated trust domains is out of
scope. It also refuses when the cluster holds more than one domain besides the new one, a
consumer that does not exist, and a StatefulSet that is scaled to 0 without the memo.

**Repeating a run.** `trust-domain set <current domain>` prints `already <domain>, nothing to do`.
If a run failed midway (a timeout, a lost connection), run the same command again: the plan shows
the mixed state (places that already hold the new domain are left alone), and `--apply` finishes the
job. The server's volume is only recreated while its bundle CA still has the old domain. Keep the
first backup: a second run writes a new one that already contains part of the change. When nothing
in the cluster names the old domain any more but a consumer was not finished, pass `--from <old
domain>` with the consumers file.

### Consumers

Other systems hold the trust domain too (an ACL that names agents by SPIFFE ID, a server's
configuration). The tool cannot know them, so you list them in a TOML file
([example for scion](../../examples/k8-ops/trust-domain-consumers.toml)):

```toml
[[configmap]]            # rewritten
namespace = "scion"
name = "scion-server"
keys = ["acl.toml", "scion-server.toml"]   # optional: all keys of the ConfigMap by default

[[restart]]              # restarted after the new bundle is in place
kind = "Deployment"      # Deployment | StatefulSet | DaemonSet
namespace = "scion"
name = "scion-server"
```

In a consumer's text only `spiffe://<old>` followed by `/`, a quote, white space or the end of the
text, and the exact quoted strings `"<old>"` and `'<old>'` are replaced. A bare occurrence
(`foo.<old>`, `https://<old>/`) stays, and the plan lists such lines as a warning, because they may
need a decision by hand. The ConfigMaps are rewritten after the new bundle is in place, and the
workloads restarted after that.

**If a consumer is missing from the file** it keeps the old domain: its ACLs do not match the new
SPIFFE IDs and its workloads cannot verify the new SVIDs, so it breaks until you change it. Look for
the old domain in the cluster before you apply (`kubectl get configmaps -A -o yaml | grep old.example`).

### Restoring from a backup

`trust-domain restore <backup>` puts the cluster back as a backup of `trust-domain set` has it: the
trust domain the cluster had before that `set` (the backup's `from`).

```
$ grauwolf-k8-ops trust-domain restore trust-domain-backup-20261114T153000Z.json
$ grauwolf-k8-ops trust-domain restore trust-domain-backup-20261114T153000Z.json --apply
```

Options: `--apply`, `--yes`, `--backup <file>` (where the backup of the current state goes) and
`--timeout <seconds>`, as for `set`. Without `--apply` it only prints the plan; with it, it asks
(`--yes` skips the question; without a terminal and without `--yes` it cancels). A file that is not
a backup of this tool (not JSON, written by another program, a format it does not know, no valid
`from`) is refused with the reason.

**The plan** shows, per ConfigMap of the backup and per key, the lines that differ from the backup
(`-` is what the cluster has now, `+` what the restore writes), the SPIRE server's data if it has to
be recreated, the restarts and the OpenBao roles with the fields that change. A restore writes back
whole keys, so it also undoes every other change made to those lines since the backup; such
lines (those that do not mention either trust domain) are listed on their own in a warning, so look
at them before you apply.

**What it does**, in this order (the same as `set`, in the other direction):

1. **Backup of the current state** (`--backup`, default `trust-domain-backup-<UTC timestamp>.json`;
   the same format, never overwrites a file, nothing is changed if it cannot be written). A restore
   can be undone by restoring *that* file.
2. **ConfigMaps**: only the `data` of the ConfigMaps in the backup is written (a merge patch with the
   backup's keys; keys that are in the cluster but not in the backup stay). A ConfigMap that no
   longer exists is refused by name.
3. **The server's data is recreated** if its CA does not have the restored domain (as `set` does:
   scale to 0, delete the volume claim, scale back).
4. **Waits** for a CA of the restored domain in the bundle, then restarts the SPIRE agents,
   spire-controller-manager (if there is one), the OIDC provider and the consumer workloads
   listed in the backup.
5. **OpenBao roles**: every field of the roles in the backup is written back (read, change, write),
   but only for agents that still exist. The role of an agent that was removed since is skipped
   (the plan says so) and not given back. Roles of agents added since are left as they are; the
   verification reports them as warnings, and `agent add <name>` binds them to the current domain.
6. **Verification** as for `set`: the restored domain in every place, every role bound to the right
   subject. The exit code is 1 otherwise, naming the failed step and both files: the backup it
   restores from and the backup of the state before.

It refuses what `set` refuses where it applies: federation, and (when the server's data has to be
recreated) a datastore that is not SQLite on a volume claim template.

**What it does not restore:** metadata, labels and annotations of the ConfigMaps (only `data`);
workloads other than by a restart; Secrets (none is in a backup); agents added or removed since;
and the SPIRE server's old database and CA: the data is recreated, so every workload gets a new
SVID again.

**Repeating a run.** If the current state already equals the backup (its ConfigMap data, its roles
and a CA of its domain in the bundle) it prints `already as in the backup, nothing to do`, exits 0
and writes no backup. A restore that was interrupted (a timeout, a lost connection) is finished by
running it again with the same backup file and a new `--backup` file; the plan shows what is
left.

By hand (a backup whose ConfigMaps no longer exist, for instance): the backup is plain JSON with the
objects as they were. Extract an object (`jq '.configmaps[] | select(.metadata.name=="spire-server")' backup.json`),
create or patch it (`kubectl patch configmap spire-server -n spire-system --type merge -p '{"data": {...}}'`),
and run `restore` again, or restart the workloads and write the roles (`openbao_roles`: `mount`,
`name`, `data`) back with `bao write auth/<mount>/role/<name> @-`.

## When something is wrong

Run `doctor`: its hints name the command that fixes the line. Typical cases: *the trust domain is
not the same everywhere* (the SPIRE server's value is the reference; if a `trust-domain set` was interrupted, run it again to
finish it), *an agent's role is bound to another subject* (`agent add <name>` again), *the legacy shared
role `grauwolf-agents`* (from the example: it lets its agents read all agents' secrets; per-agent roles replace it).

A call to another agent or an MCP server with the wrong `auth.spiffe` fails like this:

```text
A2A over HTTP: TLS handshake with https://reviewer.reviewer.svc:8443/.well-known/agent-card.json failed: the server is spiffe://agentswork.de/ns/reviewer/sa/reviewer, expected spiffe://agentswork.de/ns/intruder/sa/intruder; the call was not executed
```

Set `auth.spiffe.id` to the identity the message names (`grauwolf-k8-ops agent list` shows it).
"no connection … (refused or given up …)" instead means the port is not reachable: the agent is not
deployed, or a NetworkPolicy keeps the caller out (`grauwolf-k8-ops agent callers <target>`).

## Limits of this stage

- `trust-domain restore` restores only what a backup of `trust-domain set` holds (ConfigMap data, roles); it cannot bring back the SPIRE server's old database and CA.
- The program is not part of the container images yet.
- `trust-domain set` and `restore` rewrite the roles of agents, not the role `grauwolf-identity` of the identity service: run `identity install` again afterwards (it binds the role to the current trust domain).
- What an agent may use its identity for besides calling agents and MCP servers (`fetch.auth`, `git.auth`, `workload_api`) is configured in the spec ([chapter 3](03-tools.md)); `agent deploy` does not change for it, but the pod needs the SPIFFE socket as for every identity use. [`examples/test-git-server/`](../../examples/test-git-server/README.md) has a test git server to try `git.auth` on your cluster.
- One replica only; `agent deploy` does not scale, set up autoscaling or manage TLS.
- A repeated run compares with what the tool applied last (the annotation `grauwolf.io/applied-hash`
  on each object), so it does not notice an edit made by hand to an object it manages; `doctor`
  checks the parts that matter for access (role, policy, labels).

For the internals (the crates, `FakeCluster`, how a command is added, the traps found in live checks) see [developer guide chapter 9](../developer-guide/09-identity-credentials-and-the-cluster-tool.md).
