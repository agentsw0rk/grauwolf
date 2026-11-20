# 9 Operations

So far you have started your agents on the command line: one input, one answer, done. In this
chapter you turn an agent into a **service** that other programs reach over HTTP. You learn

- how `grauwolf spec serve` provides an agent and which endpoints there are,
- how you secure the service with tokens and assign runs to individual users,
- how a human approves sensitive tool calls – on the console and in the service,
- how you integrate the agent into Claude Desktop,
- how you observe runs (events, OpenTelemetry) and resume interrupted runs.

The example is a small notebook that reads and writes Markdown files
([`examples/09-notebook.json`](examples/09-notebook.json)):

```json
{
  "spec_version": 1,
  "name": "notebook",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You manage notes as Markdown files. Read existing notes before you change them. Answer briefly and in English.",
  "builtin_tools": {
    "tools": ["list_directory", "read_file", "write_file"],
    "root": "09-operations/archive"
  },
  "strategy": { "kind": "single_run" }
}
```

You know the built-in file tools from [chapter 3](03-tools.md); they only work under `root`,
here in the folder `docs/user-guide/examples/09-operations/archive/`.

## The agent as an HTTP service

```bash
grauwolf spec serve docs/user-guide/examples/09-notebook.json
```

On stderr you see roughly:

```text
grauwolf service for notebook on http://127.0.0.1:8080
```

The service loads the spec **once** at startup – including tools, MCP servers and knowledge – and
then serves any number of requests side by side; every run gets a fresh context. Press Ctrl+C to
stop it.

You choose the address with `--addr`; the default is `127.0.0.1:8080`. Without security (see
below) the service only starts on a local address.

### `GET /health` – is the service alive?

```bash
curl http://127.0.0.1:8080/health
```

Answer: `ok`. The endpoint is always open and suits load balancers and container checks.

### `POST /run` – one run, one answer

The body is JSON with the field `input` – text for `input: string`, otherwise a JSON value of the
spec's input type ([chapter 5](05-structured-data.md)). Don't forget the header
`Content-Type: application/json`.

```bash
curl -s http://127.0.0.1:8080/run \
  -H 'Content-Type: application/json' \
  -d '{ "input": "What is on my shopping list?" }'
```

Answer, roughly:

```json
{ "output": "Your shopping list has bread and apples." }
```

If the run fails, you get status `500` with `{ "error": "…" }`.

Three optional fields extend the request:

| Field | Meaning |
|---|---|
| `conversation_id` | Continues this conversation ([chapter 7](07-conversations-and-knowledge.md)). Letters, digits, `-` and `_` are allowed; otherwise status `400`. |
| `user_id` | User of the run: selects their conversation and – with `long_term_memory` – their area in the memory. |
| `attachments` | Attachments of the input: `[{ "media_type": "image/png", "data": "<base64>" }]` or with `"url"` instead of `"data"`; if both are missing or both are present, status `400`. Which kinds a provider understands is described in [chapter 2](02-models.md#images-pdfs-and-audio); the run rejects anything else before the request. |

As with `spec run`, conversations live in `.grauwolf/conversations` (option `--conversations-dir`)
or in a database (`--conversations-db sqlite://chats.db` or `postgres://…`).

### `POST /run/stream` – follow along live

Same body as `/run`, but the answer is a stream of **Server-Sent Events** (SSE, a simple format in
which the server sends named events one after another). With `curl -N` you see them immediately:

```bash
curl -N http://127.0.0.1:8080/run/stream \
  -H 'Content-Type: application/json' \
  -d '{ "input": "Create a note vacation.md with three packing tips." }'
```

Roughly:

```text
event: agent
data: {"event":"agent_starting","input":"Create a note vacation.md with three packing tips."}

event: agent
data: {"event":"node_starting","path":["call_llm"],"input":"Create a note …"}

event: agent
data: {"event":"tool_call_starting","call":{"id":"call_1","tool":"write_file","args":{"path":"vacation.md","content":"…"}}}

event: output
data: "I created vacation.md with three packing tips."
```

| Event | Content |
|---|---|
| `agent` | Every event of the run as JSON with the field `event` (`agent_starting`, `node_starting`, `llm_call_completed`, `tool_call_starting`, `agent_completed` …) – the same events as with `--events`. |
| `text` | Pieces of text, as soon as an `llm_request_streaming` node delivers them. |
| `approval` | A tool is waiting for approval (see [Approvals](#approvals-a-human-decides)). |
| `output` | At the end: the result as JSON. |
| `error` | At the end: the error message as JSON text. |

### A2A and MCP on the same service

The same service speaks two more protocols that other agents and tools can use to reach it
([chapter 8](08-external-tools-and-agents.md) shows the other side):

- **A2A** (Agent-to-Agent): The *agent card* at `GET /.well-known/agent-card.json` describes the
  agent (name, description from the system prompt, address of the endpoint). Tasks are sent as
  JSON-RPC to `POST /a2a` (methods `SendMessage`, `SendStreamingMessage`, `GetTask`,
  `CancelTask`).
- **MCP** (Model Context Protocol): `POST /mcp` offers the agent as one tool named after the
  spec; with `input: string` its argument is called `request`. In addition there are
  **resources** – the spec itself (`grauwolf://<name>/spec`) and the documents from
  `knowledge.sources` (`grauwolf://<name>/knowledge/<path>`) – and a **prompt** `<name>` with the
  argument `request`: the agent's system prompt together with the task, for programs that use
  prompts directly.

A2A and MCP run like `/run`: with the tools and the memory of the user from the token and with
the freshly loaded tools directory.

```bash
curl http://127.0.0.1:8080/.well-known/agent-card.json

curl -s http://127.0.0.1:8080/a2a -H 'Content-Type: application/json' -d '{
  "jsonrpc": "2.0", "id": 1, "method": "SendMessage",
  "params": { "message": { "role": "ROLE_USER", "messageId": "m1",
                           "parts": [ { "text": "Which notes are there?" } ] } } }'

curl -s http://127.0.0.1:8080/mcp -H 'Content-Type: application/json' -d '{
  "jsonrpc": "2.0", "id": 1, "method": "tools/call",
  "params": { "name": "notebook", "arguments": { "request": "Which notes are there?" } } }'
```

The A2A answer contains a `task` with `status.state` `TASK_STATE_COMPLETED` and the answer in the
artifact `answer`. The `contextId` of a message becomes the conversation.

The agent card names the bound address, e.g. `http://127.0.0.1:8080/a2a`. If the service runs
behind a reverse proxy, give the public address:

```bash
grauwolf spec serve docs/user-guide/examples/09-notebook.json --public-url https://agents.example.com
```

### The chat API: `/api/chat` and `/api/signature`

Some platforms – AgentWeaver, for example – talk to a deployed agent through two fixed routes.
They are off by default; `spec serve --chat-api` (generated services: `GRAUWOLF_CHAT_API=1`)
switches them on.

- `POST /api/chat` runs the agent once, like `/run`, but without conversation:
  - `input: string` – the request is `{"message": "…"}` and the answer `{"response": "…"}`.
  - A named input or output type – the request body *is* the input object and the answer *is*
    the output object.
  - Errors are `{"error": "…"}`: `400` for an unreadable body or one of the wrong shape, `500`
    if the run failed. Budgets and limits apply as for `/run`; tools that need an approval are
    refused, because this route cannot ask anybody.
- `GET /api/signature` answers `{"signature": …, "observations": …}` and needs **no token**,
  because it contains no secrets. `signature` is the content of `agent-signature.json` (path from
  `AGENT_SIGNATURE_PATH`, otherwise the file in the directory of the spec – `/app` in an image),
  `null` if it is missing or unreadable. `observations` is what the running agent has seen since
  its start: `startedAt`, `models` (per agent the model IDs the provider reported, with
  `firstSeen` and `lastSeen`) and `mcpTools` (per MCP server the tools it offered, each with name,
  description and a `schemaHash`). A new entry appears only when the value changes. For Ollama a
  model is reported as `<model>@<first 12 hex of its digest>` (looked up at the Ollama server,
  cached for ten minutes).

Authentication of `/api/chat` is the one of `/run`. A platform whose cron jobs send no token needs
`GRAUWOLF_ALLOW_ANONYMOUS=true` inside a trusted network. In this mode `OLLAMA_BASE_URL` is used
when `OLLAMA_HOST` is not set. With `spec serve`, sub-agents are listed under their own names (a generated service lists its main agent only); a file
`agent-names.json` next to the spec, `{"agents": {"<spec name>": "<agent name>"}}`, renames
agents in the signature.

```bash
grauwolf spec serve docs/user-guide/examples/09-notebook.json --chat-api
curl -s http://127.0.0.1:8080/api/chat -d '{"message": "Which notes are there?"}'
curl -s http://127.0.0.1:8080/api/signature
```

## Securing the service

### A shared token

Set the token in the environment variable `GRAUWOLF_SERVER_TOKEN` – deliberately not as an
argument, so it does not end up in the shell history or the process list:

```bash
export GRAUWOLF_SERVER_TOKEN="$(openssl rand -hex 32)"
grauwolf spec serve docs/user-guide/examples/09-notebook.json --addr 0.0.0.0:8080
```

Now `/run`, `/run/stream`, `/approvals/…`, `/a2a` and `/mcp` require the header
`Authorization: Bearer <token>`; otherwise the service answers with `401` and
`{ "error": "token missing or wrong" }`. Only `/health` and the agent card under
`/.well-known/…` stay open.

```bash
curl -s http://server:8080/run \
  -H "Authorization: Bearer $GRAUWOLF_SERVER_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{ "input": "Hello", "user_id": "anna" }'
```

Without a token and without a users file, the service refuses to start on an address reachable
from outside:

```text
0.0.0.0:8080 is reachable from outside: set a token in GRAUWOLF_SERVER_TOKEN, pass --users or bind to 127.0.0.1
```

The service only speaks HTTP. For HTTPS, put a reverse proxy (e.g. nginx, Caddy) in front of it.

### User tokens

The shared token is meant for a trusted backend: whoever has it may give any `user_id`. If end
users should access the service directly, give each of them an **own token**:

```bash
grauwolf token anna
```

Output, roughly:

```text
gw_5d1c0e7a9b3f4a2e8c6d0b1a2f3e4d5c
"anna": "sha256:9f2b…"
```

The first line is the token – give it to Anna; it is only shown now. The second line goes into the
**users file**, which only contains the SHA-256 hashes (template:
[`examples/09-operations/users.json`](examples/09-operations/users.json)):

```json
{
  "anna": "sha256:9f2b…",
  "ben": "sha256:41c7…"
}
```

```bash
grauwolf spec serve docs/user-guide/examples/09-notebook.json --addr 0.0.0.0:8080 \
  --users docs/user-guide/examples/09-operations/users.json
```

A request with Anna's token automatically runs as `anna`: the `user_id` is set from the token. If
the body names a different `user_id`, the service answers with `403`
(`user_id does not match the token`). You can combine the shared token and the users file.

### What belongs to whom?

- **Conversations** belong to the user: `conversation_id` `trip` from Anna and `trip` from Ben are
  two different conversations (in the directory under `users/<hex>/trip.json`). Without
  `user_id` it is a shared conversation.
- **Memory**: With `long_term_memory` in the spec, `remember`/`recall` and the automatic memory
  work in the user's area ([chapter 7](07-conversations-and-knowledge.md)).
- Without a token the service trusts the `user_id` in the body. Then run it only locally or behind
  a backend that sets the `user_id`.

With A2A, a task with a user token belongs to the token's user: the `contextId` leads into their
own conversation, even if another user picks the same `contextId`, and `GetTask` and `CancelTask`
only find their own tasks – other users' tasks count as unknown (`-32001`). Task and context IDs
are random (128 bits) and cannot be guessed; at most the last 1000 tasks are kept. With the shared
token or without protection, all callers share one area.

Since A2A and MCP run like `/run`, a user sees the same tools and the same memory through all
three: with A2A the user is the owner of the task, with MCP the user from the token.

### Agents calling agents with SPIFFE

Where a cluster runs SPIRE, agents need no shared secret to call each other (ADR-0109): every agent has a SPIFFE identity (`spiffe://agentswork.de/ns/reviewer/sa/reviewer`) that the Workload API at `SPIFFE_ENDPOINT_SOCKET` hands out as a short-lived, automatically rotated certificate. User tokens stay for people; SPIFFE is an additional way in.

**Calling.** In the caller's spec, `auth.spiffe` on an `agent_tools` or `mcp_servers` entry with a `url`:

```json
"agent_tools": {
  "reviewer": {
    "url": "https://reviewer.reviewer.svc:8443",
    "auth": { "spiffe": { "id": "spiffe://agentswork.de/ns/reviewer/sa/reviewer" } }
  }
},
"mcp_servers": {
  "tickets": {
    "url": "https://tickets-mcp.tools.svc/mcp", "tools": ["search"],
    "auth": { "spiffe": { "mode": "jwt", "audience": "tickets-mcp" } }
  }
}
```

With the default `mtls` the agent presents its certificate and accepts the server only if its SPIFFE ID is the given `id`, one of `ids`, or any ID of `trust_domain`; the URL must be `https://`. With `"mode": "jwt"` it sends a fresh JWT-SVID for `audience` as `Authorization: Bearer`, for targets behind a TLS-terminating ingress. The same `auth` serves `agent` nodes and `for_each` over remote agents, in the interpreter and in generated code. Without `SPIFFE_ENDPOINT_SOCKET`, loading the spec fails and names the sub-agent or server. Reference: [`auth.spiffe`](reference-spec.md#authspiffe-in-agent_toolsid-and-mcp_serversid).

**Being called.** `GRAUWOLF_SPIFFE=1` (`spec serve --spiffe`) adds a second listener:

| Port | Who may come | How the caller is known |
|---|---|---|
| HTTP, `8080` | people with tokens, the ingress, probes, and workloads with a JWT-SVID | `Authorization: Bearer <JWT-SVID>` is checked by the Workload API against `GRAUWOLF_SPIFFE_AUDIENCE` (default: the service's own SPIFFE ID) |
| mTLS, `GRAUWOLF_SPIFFE_PORT` (default `8443`) | workloads with a certificate of an accepted trust domain; clients **without** a valid certificate are refused in the handshake | the certificate's SPIFFE ID |

The mTLS port serves the same routes as the HTTP port, A2A (`/a2a`) and MCP included, so another agent calls this one over mTLS at `https://<service>:8443`. Its A2A agent card names the mTLS URL as the endpoint (`GRAUWOLF_SPIFFE_PUBLIC_URL`, default `https://localhost:<port>`; set it to the address callers use, for example the service name in the cluster), because an A2A client sends its messages to the endpoint of the card. The extra A2A port of a generated service (`A2A_PORT`) stays a plain port: JWT-SVIDs work there, mTLS does not – use the mTLS port for that.

`/health` is open on both ports. Without `GRAUWOLF_SPIFFE` nothing changes; with it but without a socket the start fails after `SPIFFE_FETCH_TIMEOUT` and names `SPIFFE_ENDPOINT_SOCKET`. Only the own trust domain is accepted; `GRAUWOLF_SPIFFE_TRUST_DOMAINS` is reserved for federation.

**MCP is protected the same way in both directions.** Calling an MCP server over HTTP uses `mcp_servers.<name>.auth.spiffe` (`mtls` with `id`, `ids` or `trust_domain`, or `jwt` with `audience`), exactly as for `agent_tools`, for Streamable HTTP and for `"transport": "sse"` alike. Being called as an MCP server is the route `/mcp`: it is served on the mTLS port behind the same caller check (certificate, then JWT-SVID, then the mapping file), so a desktop assistant or another agent that uses your agent as a tool is authenticated, mapped to a user and shown as `caller` like any A2A caller. An agent that calls another agent as an MCP tool:

```json
"mcp_servers": {
  "reviewer": {
    "url": "https://reviewer.reviewer.svc:8443/mcp",
    "tools": ["reviewer"],
    "auth": { "spiffe": { "id": "spiffe://agentswork.de/ns/reviewer/sa/reviewer" } }
  }
}
```

If the server presents another identity than `id`, the call fails in the handshake and the message names both: `TLS handshake with https://reviewer.reviewer.svc:8443/mcp failed: the server is spiffe://…/sa/reviewer, expected spiffe://…/sa/other; the call was not executed`. `no connection to … (refused or given up after 30 s)` is a different case: nothing answers on that port. See [chapter 8](08-external-tools-and-agents.md#securing-the-connection-with-spiffe).

**Who is that?** An authenticated ID is not yet authorized. `GRAUWOLF_SPIFFE_USERS` (`--spiffe-users`) names a file that maps IDs to users:

```json
{
  "spiffe://agentswork.de/ns/coordinator/sa/coordinator": "coordinator",
  "spiffe://agentswork.de/ns/batch/*": "batch-jobs"
}
```

An entry is an exact ID or a prefix ending in `/*` that matches whole path segments only (`…/ns/batch/*` covers `…/ns/batch/sa/x` but not `…/ns/batchy/sa/x`); the most specific entry wins. An authenticated ID without an entry gets `403`, and so does a `user_id` in the body that differs from the mapped user. The mapped user is the request's user exactly as with a user token: conversations, memory, approvals and per-user tool stores stay separate ([What belongs to whom?](#what-belongs-to-whom)). The file is read at start and again every two seconds when it changes, also when a Kubernetes ConfigMap update swaps its `..data` symlink; an invalid new file is reported and the old one stays. Without the file every caller with an ID is refused.

**In the events and spans.** A call authenticated by a SPIFFE ID (certificate or JWT-SVID) adds the field `caller` – the caller's SPIFFE ID, never a token – to the `agent_starting` event, so `--events`, the `agent` events of `/run/stream`, logs and the OpenTelemetry span `invoke_agent` (attribute `grauwolf.caller`) show who called the agent. Calls with a user token or the shared token have no `caller`. Over A2A and MCP the same field is set.

```json
{"event":"agent_starting","input":"Review this.","caller":"spiffe://agentswork.de/ns/coordinator/sa/coordinator"}
```

Decision order on the HTTP port: a client certificate (mTLS port only), then a Bearer that is a JWT (three parts, not `gw_…`, not the shared token), then the shared token and user tokens as before.

## Container images

The repository contains Dockerfiles in `docker/` and a GitHub workflow (`.github/workflows/images.yml`)
that builds and checks them. Build from the repository root.

### The CLI image

```bash
docker build -f docker/cli.Dockerfile -t grauwolf .
docker run --rm grauwolf --version
docker run --rm -v "$PWD:/work" -w /work grauwolf spec validate examples/specs/echo.json
docker run --rm -p 8080:8080 -v "$PWD:/work" -w /work -e GRAUWOLF_SERVER_TOKEN=secret \
  grauwolf spec serve examples/specs/echo.json --addr 0.0.0.0:8080
```

The default target `runtime` is `debian:bookworm-slim` with `ca-certificates`, runs as the non-root user `nonroot` (uid 65532) and has the entry point `grauwolf`.
It contains **no Rust toolchain**: that is enough for `spec validate`, `serve`, `run`, `chat` and
`mcp` of specs without `tools` of their own. Tool code is compiled when a spec is loaded, so for
such specs (and for `spec generate`) use the target `toolchain`, which adds Rust and the
`wasm32-unknown-unknown` target (`--target toolchain`; the image is large).

### An agent image from a spec

`docker/agent.Dockerfile` generates the crate with `--service` (chapter 11), builds it in release
mode and copies only the binary `agent` into a small image:

```bash
docker build -f docker/agent.Dockerfile --build-arg SPEC=examples/specs/echo.json \
  --target runtime -t echo-agent .
docker run --rm -p 8080:8080 -e GRAUWOLF_SERVER_TOKEN=secret echo-agent
curl localhost:8080/health
```

| Build argument / target | Meaning |
|---|---|
| `--build-arg SPEC=<path>` | Spec inside the build context (default `examples/specs/echo.json`) |
| `--target runtime` | Default: `gcr.io/distroless/cc-debian12:nonroot` – no shell, user 65532; also for specs with git tools (libgit2 is part of the binary) |

The whole directory of the spec is copied to `/app`, which is the working directory and
`GRAUWOLF_DATA_DIR`: files that the spec names relative to itself (knowledge sources, the memory
`store`, the root of the file tools) must be next to it. `/app` is writable for the service user;
mount a volume there (`-v agent-data:/app`) if long-term memory should survive; the same holds for conversations with `GRAUWOLF_CONVERSATIONS_DIR=conversations` (by default they stay in the memory of the process; `GRAUWOLF_CONVERSATIONS_DB` takes a database URL instead). Tools are
compiled to WASM during the build and embedded in the binary.

The service listens on port 8080 (`ENV SERVER_PORT=8080`, `EXPOSE 8080`) and is configured
through the environment variables from [chapter 11](11-code-generation.md#as-an-http-service---service):
`GRAUWOLF_SERVER_TOKEN` (or `GRAUWOLF_USERS_FILE`), provider keys such as `OPENAI_API_KEY` or
`OLLAMA_HOST`, `A2A_PORT` and so on. Because it binds `0.0.0.0`, it refuses to start without a
token unless you set `GRAUWOLF_ALLOW_ANONYMOUS=true`. SIGTERM (`docker stop`, Kubernetes) ends it
after running requests have finished.

### The builder image: offline agent builds in minutes

`docker/agent.Dockerfile` compiles all dependencies on every build and needs crates.io. For a
platform that builds many agents (with kaniko, say, with a time limit and no network except the
registry) there is the **builder image**, `ghcr.io/agentsw0rk/grauwolf-builder:<version>`:

```bash
docker build -f docker/builder.Dockerfile --build-arg GRAUWOLF_VERSION=0.1.0 \
  -t grauwolf-builder:0.1.0 .
```

It contains the Rust toolchain with `wasm32-unknown-unknown`, the `grauwolf` CLI, the grauwolf
sources of exactly this version in `/opt/grauwolf/crates`, **all dependencies vendored** in
`/opt/grauwolf/vendor` (`$CARGO_HOME/config.toml` replaces crates.io with them and sets
`net.offline`) and a **pre-built target directory** `/opt/grauwolf/target`: the dependencies of a
generated service are already compiled. The image is big (several GiB) and is pulled once per node.

An agent is built in two steps, neither needs a network:

```bash
# 1. Generate the crate and its Dockerfile in the builder image (tool code is compiled here).
docker run --rm -v "$PWD:/work" -w /work grauwolf-builder:0.1.0 \
  grauwolf spec generate spec.json --dockerfile --out agent --builder-image grauwolf-builder:0.1.0
# 2. Build the agent image; only the agent crate is compiled.
docker build --network none agent
```

`--dockerfile` ([chapter 11](11-code-generation.md#a-dockerfile-for-the-builder-image---dockerfile))
writes a `Dockerfile` that starts `FROM` the builder image, builds with `cargo build --release
--offline --features service` and copies the binary to a distroless runtime image (Debian slim with
`git` for specs with git tools). The files the spec reads at run time are copied to `app/` and
become `/app`. Without `--builder-image` the name is
`ghcr.io/agentsw0rk/grauwolf-builder:<version of the CLI>`, so use a CLI and a builder image of the
same version: the pre-built dependencies only fit the grauwolf sources of that version. The
`Dockerfile` enforces it (the builder sets `GRAUWOLF_VERSION`; a mismatch fails the first build step
with a message that names both versions; `--allow-builder-mismatch` leaves the guard out). A tool
that drives the build can read both with `grauwolf version --json`:
`{"version": "0.1.0", "builder_image": "ghcr.io/agentsw0rk/grauwolf-builder:0.1.0"}`. Keep the
generated directory out of any other build context: it holds the crate, the `Dockerfile` and `app/`.

### What the workflow publishes

On every push to `main` and every pull request, the workflow builds the CLI image and the agent
image for the `echo` spec (both targets) for `linux/amd64`, runs `grauwolf --version`, starts the
agent and checks `/health` and the agent card. A second job builds the builder image **once per
architecture on a native runner** (`ubuntu-24.04` for amd64, `ubuntu-24.04-arm` for arm64, no
emulation), generates the `echo` agent and the `calculator` agent (tool code as sources) in it,
builds them with `docker build --network none` (this proves the build is offline, passes the
[version guard](11-code-generation.md#a-dockerfile-for-the-builder-image---dockerfile) and prints the
durations), starts them and checks `/health`.

On a tag `v<version>` (the tag must equal the version in `Cargo.toml`) the workflow additionally
pushes to `ghcr.io/agentsw0rk`, authenticated with the `GITHUB_TOKEN` (`packages: write`): the CLI
image `grauwolf`, the tool program `grauwolf-tools` and the builder image `grauwolf-builder`, each
built natively for `linux/amd64` and `linux/arm64`. Every architecture is pushed on its own and
identified by its digest; a last job merges the two digests into one multi-arch tag
(`docker buildx imagetools create`): `:<version>`, and `:latest` for the CLI and the tools image
(not for the builder, which must always match the CLI that generates). All images carry the labels
`org.opencontainers.image.source`, `.version` and `.revision`. Agent images depend on your own spec
and are not published; build them with the Dockerfiles above.

The packages are **private**. To pull them, log in with a personal access token that has
`read:packages`:

```bash
echo "$TOKEN" | docker login ghcr.io -u <github-user> --password-stdin
docker pull ghcr.io/agentsw0rk/grauwolf-builder:<version>
```

A Kubernetes cluster that builds or runs these images (AgentWeaver's, for example) needs an **image
pull secret**: `kubectl create secret docker-registry ghcr --docker-server=ghcr.io
--docker-username=<user> --docker-password=<token>`, referenced from the pod or service account.
Native arm64 runners are free for public repositories; for a private repository they may need a
GitHub plan with arm64 runners (larger runners), otherwise the `arm64` jobs wait for a runner that
never comes; they therefore run only on tags and manual runs, pushes and pull requests build amd64. Cutting a release is described in the
[developer guide](../developer-guide/02-development-setup.md#210-cutting-a-release).

## Approvals: a human decides

Some tools should not run without consent – `write_file`, for example, or anything that costs
money. With `--confirm-tools` you make tools **require approval** (several separated by commas).

### On the console

```bash
grauwolf spec run docs/user-guide/examples/09-notebook.json "Delete everything from shopping.md except bread." \
  --confirm-tools write_file
```

Before every call, grauwolf asks on stderr:

```text
Run tool write_file with {"content":"# Shopping\n\n- Bread\n","path":"shopping.md"}? [y/N]
```

`y` or `yes` runs the tool (`j` and `ja` work too); anything else declines, and the model receives
`The user refused the call of write_file.` as the tool result. This works with `spec run`,
`spec chat` and `spec resume`. With `spec mcp` the option is forbidden, because stdin carries the
MCP protocol there.

### In the service

```bash
grauwolf spec serve docs/user-guide/examples/09-notebook.json --confirm-tools write_file
```

Approvals need a back channel, so they are only available via `/run/stream`. Before the call, an
`approval` event arrives, and the run waits:

```text
event: approval
data: {"id":"8c1f0a…","tool":"write_file","args":{"path":"vacation.md","content":"…"}}
```

You decide with `POST /approvals/{id}`:

```bash
# approve
curl -X POST http://127.0.0.1:8080/approvals/8c1f0a… \
  -H 'Content-Type: application/json' -d '{ "approved": true }'

# decline, with a reason for the model
curl -X POST http://127.0.0.1:8080/approvals/8c1f0a… \
  -H 'Content-Type: application/json' \
  -d '{ "approved": false, "reason": "Please do not create new files." }'
```

Answer: `204 No Content`. Then the stream continues.

- Without `reason` the model receives `Denied by the user`.
- If no decision arrives within 5 minutes, the call is declined
  (`No approval given within 300 s`).
- If the request ran with a `user_id`, the decision must carry the same `user_id`; otherwise – and
  for unknown IDs – the service answers with `404`. With a user token, the service takes the
  `user_id` from the token.
- `/run`, A2A and MCP have no back channel: tools that require approval are always declined there,
  and the model learns `tool 'write_file' needs an approval that nobody can give here`.

Pending approvals live in the service's memory. If you run several instances behind a load
balancer, the decision must reach the same instance (sticky sessions).

## Integrating into Claude Desktop

`grauwolf spec mcp` provides the agent as an MCP server over stdin/stdout – exactly what desktop
programs like Claude Desktop expect. All messages go to stderr; stdout belongs to the protocol. An
entry in the Claude Desktop configuration looks roughly like this:

```json
{
  "mcpServers": {
    "notebook": {
      "command": "/home/you/.cargo/bin/grauwolf",
      "args": [
        "spec", "mcp", "/home/you/agents/09-notebook.json",
        "--cache", "/home/you/.grauwolf/tools"
      ]
    }
  }
}
```

Here too, besides the tool there are the agent's resources (spec, knowledge) and its prompt; the
tools directory is read again before every call, and automatic memory applies in the shared area.

Use absolute paths: Claude Desktop determines the working directory, and default folders like
`.grauwolf/tools` are relative to it. Paths *in* the spec (`root`, `sources`, …) are, as always,
relative to the spec file. Credentials like `ANTHROPIC_API_KEY` must be in the process's
environment.

## Observing

### Events as JSON lines

`--events` writes every event of the run as a JSON line to stderr – ideal for `jq` or a logging
system. The answer stays on stdout:

```bash
grauwolf spec run docs/user-guide/examples/09-notebook.json "Which notes are there?" --events 2> events.jsonl
```

`events.jsonl`, roughly:

```text
{"event":"agent_starting","input":"Which notes are there?"}
{"event":"node_starting","path":["call_llm"],"input":"Which notes are there?"}
{"event":"tool_call_starting","call":{"id":"call_1","tool":"list_directory","args":{"path":"."}}}
{"event":"tool_call_completed","result":{"id":"call_1","tool":"list_directory","content":"shopping.md"}}
{"event":"agent_completed","output":"There is one note: shopping.md.","usage":{"input_tokens":812,"output_tokens":46}}
```

`usage` appears when the provider reports token counts; `cost` additionally with `--prices`.

A generated service from `spec generate --service` has no flags; it writes the same lines (its own and its
sub-agents') to stderr when the environment says `GRAUWOLF_EVENTS=1` (or `true`), and in a cluster
`grauwolf-k8-ops agent deploy --events` sets that
([chapter 13](13-setting-up-a-cluster.md#events-in-the-pod-log)). Any other value is a start-up error that
names the variable.

### OpenTelemetry

With `--otel`, grauwolf exports every run as **spans** (time segments of a trace) over OTLP/HTTP –
to an OpenTelemetry Collector, Jaeger, Langfuse and others. Target and headers come from the
standard variables:

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318
export OTEL_SERVICE_NAME=notebook        # default: grauwolf
grauwolf spec serve docs/user-guide/examples/09-notebook.json --otel
```

Per run you see a span `invoke_agent <name>`, below it `node <path>` per node, `chat <model>` per
LLM call and `execute_tool <name>` per tool call. Sub-agents report their own `invoke_agent`
spans, as children of the call that runs them (for agents on another service, see "One trace across agents" below). Contents (answers, tool arguments and results) may contain personal data and are therefore
only recorded with `--otel-content`.

**One trace across agents.** When a traced agent calls another agent over A2A or MCP (HTTP), it
sends the W3C header `traceparent` (and `tracestate`, if it has one) with the request. The called
agent continues that trace: its `invoke_agent` span is a child of the span that made the call – the
`execute_tool <name>` span of the tool call, or the `node <path>` span of an `agent` node. Caller and
callee are then **one trace** in Jaeger or Tempo, as long as both export to the same collector.
Nothing changes for an agent without `--otel`: it sends no header, and a header it receives is
ignored. A missing or malformed `traceparent` starts a new trace, as before. The header only links
spans and grants nothing, so the service accepts it from every caller that passed authentication.
`grauwolf.caller` stays on the span, so you still see who called.

`--otel` is available with `spec run`, `spec resume`, `spec serve` and `spec mcp`; `spec chat`
rejects it.

**Generated services.** A service from `spec generate --service` has no flags; it exports the same
spans when the environment says so: `GRAUWOLF_OTEL=1` (or `true`), and `GRAUWOLF_OTEL_CONTENT=1` for
contents. Target and service name come from the variables above. The service continues an incoming
`traceparent`, records `grauwolf.caller` and the spans of its sub-agents, and sends the spans still
pending on SIGTERM and Ctrl+C. Any other value of `GRAUWOLF_OTEL` is a start-up error that names the
variable.

## Checkpoints: resuming interrupted runs

Long runs can break off – network gone, computer restarted. With `--run-id`, grauwolf saves a
checkpoint **before every node** (history, position in the graph, storage). The example is a flow
of two steps ([`examples/09-draft.json`](examples/09-draft.json)):

```json
{
  "spec_version": 1,
  "name": "draft",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You write factual texts in English.",
  "graph": {
    "nodes": [
      { "id": "draft", "kind": "llm_request" },
      { "id": "shorten", "kind": "llm_request" }
    ],
    "edges": [
      { "from": "start", "to": "draft" },
      { "from": "draft", "to": "shorten", "when": { "kind": "on_assistant_message" } },
      { "from": "shorten", "to": "finish", "when": { "kind": "on_assistant_message" } }
    ]
  }
}
```

```bash
grauwolf spec run docs/user-guide/examples/09-draft.json "Write a paragraph about wolves." --run-id wolves-1
# … interruption, e.g. with Ctrl+C during "shorten" …
grauwolf spec resume docs/user-guide/examples/09-draft.json --run-id wolves-1
```

`spec resume` continues at the last checkpoint; whatever came before does not run again.

- Checkpoints live in `.grauwolf/runs/<run-id>.json` (`--runs-dir`) or in a database
  (`--runs-db sqlite://runs.db`). Run IDs consist of letters, digits, `-` and `_`.
- Tool calls between two checkpoints may run a second time after an interruption.
- `--run-id` cannot be combined with `--conversation`. With `--user` the run uses this user's
  memory; pass the same `--user` when resuming
  (`spec resume … --run-id run-1 --user anna`) – the user is not stored in the checkpoint; without
  `--user`, `spec resume` continues in the shared area.
- Otherwise checkpoint runs behave like ordinary runs: `spec run --run-id` and `spec resume` load
  the tools directory (`tools_dir`), use the automatic memory (with `spec resume` the shared area)
  and ask for approvals on the console. Self-made tools ([chapter 10](10-self-made-tools.md)) are
  stored in the checkpoint and rebuilt on resume. `spec resume` reloads the tools directory – the
  tools at the time of resuming apply.

## Costs and budget

[Chapter 2](02-models.md) explains in detail what runs cost and how you limit them. Important in
operations:

- `--prices <file>` reads a price table (per million tokens, template:
  [`examples/09-operations/prices.json`](examples/09-operations/prices.json)); events and spans
  then report the costs.
- `--max-tokens <n>` and `--max-cost <amount>` end a run as soon as the limit is reached – in the
  service **per request**, with `spec chat` per line, with `spec mcp` per call. `--max-cost`
  needs `--prices`.

```bash
grauwolf spec serve docs/user-guide/examples/09-notebook.json \
  --prices docs/user-guide/examples/09-operations/prices.json --max-cost 0.05 --max-tokens 20000
```

## Key points

- `grauwolf spec serve <spec>` starts an HTTP service with `/run`, `/run/stream` (SSE),
  `/health`, A2A at `/a2a` including the agent card, and MCP at `/mcp`.
- Without security, local only. For access from outside, set `GRAUWOLF_SERVER_TOKEN` or create
  user tokens for `--users` with `grauwolf token`; conversations and memory then belong to the
  user.
- `--confirm-tools` makes tools require approval: on the console with `y` or `yes`, in the service via
  the SSE event `approval` and `POST /approvals/{id}`.
- `docker/cli.Dockerfile` and `docker/agent.Dockerfile` build the CLI and a generated agent service
  as container images (distroless or with git, non-root, port 8080); `docker/builder.Dockerfile`
  builds the offline builder image, and `spec generate --dockerfile` writes the `Dockerfile` that
  builds an agent in it without network; see "Container images".
- `grauwolf spec mcp` integrates the agent into Claude Desktop.
- `--events`, `--otel` and `--otel-content` make runs visible.
- `--run-id` and `spec resume` resume interrupted runs.

Continue with [chapter 10: Self-made tools](10-self-made-tools.md).
