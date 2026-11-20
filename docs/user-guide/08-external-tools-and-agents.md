# 8 External tools and other agents

In [chapter 3](03-tools.md) you wrote tools yourself. But many useful tools already exist ready-made
– as **MCP servers**. And sometimes the best tool for a subtask is a whole other agent. This chapter
shows both:

- `mcp_servers` integrates tools from third-party MCP servers, started locally or on the network.
- `agent_tools` turns other agents into tools – your own specs on your machine or remote agents
  that speak the A2A protocol.

To the model, both look like any other tool. So your flow needs a tool loop; the examples use the
`single_run` template from [chapter 4](04-flows.md) for this.

## Starting MCP servers locally

The **Model Context Protocol** (MCP) is an open standard through which programs offer tools to an
AI agent – for files, Git, databases, web pages and much more. A local MCP server is a program that
grauwolf starts when the agent starts and talks to over standard input and output (stdio):

```json
{
  "spec_version": 1,
  "name": "file_guide",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You answer questions about files in the shared directory. Look things up with the tools instead of guessing.",
  "mcp_servers": {
    "files": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "."],
      "tools": ["list_directory", "read_text_file"]
    }
  },
  "strategy": { "kind": "single_run" }
}
```

The file: [`examples/08-mcp-files.json`](examples/08-mcp-files.json). For this example you need
Node.js (`npx`).

| Field | Meaning |
|---|---|
| key (`files`) | name of the server, lowercase (`[a-z][a-z0-9_]*`) |
| `command` | program that starts the server |
| `args` | arguments for the program (default: none) |
| `tools` | the server's tools that your agent may use (required) |

The server runs in the current working directory – so `"."` in `args` here means the directory in
which you call `grauwolf`, not the spec's folder.

```bash
grauwolf spec run docs/user-guide/examples/08-mcp-files.json "Which chapters does the user guide have?"
```

Roughly:

```text
The folder docs/user-guide contains the chapters 01-getting-started.md to 11-code-generation.md,
plus README.md and the examples folder.
```

**Why declare tools?** A server often offers many more tools than your agent needs. grauwolf offers
the model only the listed ones. It also lets `spec validate` check tool names without starting the
server. Which tools a server has is in its documentation. If a declared tool is missing at startup,
grauwolf aborts and names the available ones, roughly like this:

```text
MCP server 'files' does not offer tool 'read_file' (available: read_text_file, list_directory, …)
```

**Security:** your own tools from [chapter 3](03-tools.md) run in an isolated sandbox. MCP servers,
on the other hand, run as normal programs with your permissions. Use only servers you trust, and
give them access only to what they need (here: one directory).

### Giving a local server the agent's identity: `workload_api`

A stdio server is a child process of the agent. It does **not** get the agent's Workload API
settings by default: `SPIFFE_ENDPOINT_SOCKET` and `SPIFFE_FETCH_TIMEOUT` are removed from the
environment it inherits. A server that logs in to an internal service with SPIFFE can be given
them (ADR-0110):

```json
"mcp_servers": {
  "internal": { "command": "internal-mcp", "tools": ["search"], "workload_api": true }
}
```

- Only for `command` servers; with `url` it is a validation error (`WorkloadApiWithoutStdio`).
- Without `SPIFFE_ENDPOINT_SOCKET` in the agent's environment the agent does not start; the error
  names the server (`MCP server 'internal': \`workload_api\` needs a workload identity, but
  SPIFFE_ENDPOINT_SOCKET is not set`).
- **The child gets the agent's identity.** SPIRE's Kubernetes attestor does not tell processes of
  a pod apart, so everything the agent may do with its SVIDs, the child may do too. A separate
  identity for the child needs the SPIRE Unix attestor (user ID or binary path selectors): cluster
  configuration, not done by grauwolf. Use `workload_api` only for servers you trust as much as the
  agent.

## MCP servers on the network

Many MCP servers run as a service on the internet. Instead of `command` you then specify `url`;
grauwolf talks to them over the "Streamable HTTP" transport:

```json
"mcp_servers": {
  "deepwiki": {
    "url": "https://mcp.deepwiki.com/mcp",
    "tools": ["read_wiki_structure", "ask_wiki_question"]
  }
}
```

The complete file: [`examples/08-mcp-http.json`](examples/08-mcp-http.json).

```bash
grauwolf spec run docs/user-guide/examples/08-mcp-http.json \
  "How is the repository tokio-rs/axum structured?"
```

The answer roughly summarizes the repository's wiki structure.

**Older servers (HTTP+SSE):** some servers only speak the older transport of protocol version
2024-11-05, in which the client opens an event stream with `GET` and sends its messages by `POST` to
an address the server announces. Choose it explicitly with `transport` (default:
`streamable_http`; `transport` only works together with `url`):

```json
"mcp_servers": {
  "legacy": {
    "url": "http://127.0.0.1:8931/sse",
    "transport": "sse",
    "tools": ["reverse"]
  }
}
```

The complete file: [`examples/08-mcp-sse.json`](examples/08-mcp-sse.json). grauwolf does not
guess the transport: if you point `streamable_http` at an old server (or `sse` at a new one), the
connection fails with the server's HTTP status. To try it without a server of your own, start the
example server of the repository (one tool, `reverse`):
`cargo run -p grauwolf-mcp --features test-sse-server --example serve_sse_mcp`. If an AgentWeaver `mcp-server` has the transport
`sse`, use `"transport": "sse"`.

If a server needs credentials, you specify **headers** under `headers`. Don't write the secret into
the spec, but as `${VARIABLE}`; grauwolf inserts the value of the environment variable at startup:

```json
"mcp_servers": {
  "docs": {
    "url": "https://mcp.example.com/mcp",
    "headers": { "Authorization": "Bearer ${DOCS_TOKEN}" },
    "tools": ["search"],
    "resources": true
  }
}
```

```bash
DOCS_TOKEN=secret grauwolf spec run my-spec.json "…"
```

If the variable is missing, startup aborts with `missing environment variable DOCS_TOKEN`.

**Resources:** besides tools, some servers also offer **resources** – readable content with an
address (URI), such as documents or records. With `"resources": true`, the agent gets two additional
tools for this, named after the server:

- `docs_list_resources` lists the resources with URI, name, description and type,
- `docs_read_resource` reads a resource (argument `uri`).

`resources` is available for local and remote servers.

### Securing the connection with SPIFFE

Inside a cluster that runs SPIRE you need no token for a server of your own. Give the entry an `auth.spiffe` instead of `headers`
(the two exclude each other for `Authorization`); grauwolf then proves its identity with a short-lived certificate:

```json
"mcp_servers": {
  "tickets": {
    "url": "https://tickets-mcp.tools.svc:8443/mcp",
    "tools": ["search"],
    "auth": { "spiffe": { "id": "spiffe://agentswork.de/ns/tickets/sa/tickets-mcp" } }
  },
  "wiki": {
    "url": "https://mcp.example.com/mcp",
    "tools": ["search"],
    "auth": { "spiffe": { "mode": "jwt", "audience": "wiki-mcp" } }
  }
}
```

`mtls` (the default) presents the certificate and accepts the server only if its SPIFFE ID is the given `id` (or one of `ids`, or any ID of `trust_domain`); the URL must be `https`. `"mode": "jwt"` sends a JWT-SVID for `audience` as `Authorization: Bearer`, for servers behind an ingress that ends TLS. `auth` needs `SPIFFE_ENDPOINT_SOCKET` and does not work with `command`.

The called side must check callers as well: a grauwolf agent needs `GRAUWOLF_SPIFFE=1` and a callers file (`GRAUWOLF_SPIFFE_USERS`) that lists your agent, in a cluster set up with `grauwolf-k8-ops` `agent allow <target> --from <your agent>`.

Two errors tell the causes apart:

```text
MCP over HTTP: TLS handshake with https://tickets-mcp.tools.svc:8443/mcp failed: the server is spiffe://agentswork.de/ns/tickets/sa/other, expected spiffe://agentswork.de/ns/tickets/sa/tickets-mcp; the call was not executed
MCP over HTTP: no connection to https://tickets-mcp.tools.svc:8443/mcp (refused or given up after 30 s); the call was not executed
```

The first means the server answered with another identity than `id`: fix `id`. The second means nothing is listening or a NetworkPolicy keeps you out. Details: [chapter 9](09-operations.md#agents-calling-agents-with-spiffe), [chapter 13](13-setting-up-a-cluster.md#letting-agents-call-each-other) and the reference ([`auth.spiffe`](reference-spec.md#authspiffe-in-agent_toolsid-and-mcp_serversid)).

### Typical errors

```text
…:/mcp_servers/files: MCP server needs exactly one of `command` (local) and `url` (remote)
…:/mcp_servers/files/transport: `transport` only applies to a remote MCP server with `url`
…:/mcp_servers/files/tools/2: tool name 'read_text_file' is used more than once (the other one is the … at …)
```

All tool names of a spec share one namespace: your own tools, MCP tools, sub-agents, search and
memory tools must not have the same name.

## Agents as tools: `agent_tools`

Instead of letting one agent do everything, you build a team: small, specialized agents, each with
its own spec, its own model and its own system prompt. A **coordinator** distributes the work by
calling the others like tools.

```json
{
  "spec_version": 1,
  "name": "coordinator",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You coordinate requests to customer service. You pass bug reports on to create_ticket and translations into German to translator. Afterwards, briefly summarize the results for the user.",
  "agent_tools": {
    "translator": {
      "spec": "08-team/translator.json",
      "description": "Translates an English text into German"
    },
    "create_ticket": {
      "spec": "08-team/create_ticket.json",
      "description": "Turns a bug report into a ticket with title, priority and tags"
    }
  },
  "strategy": { "kind": "single_run" }
}
```

The file: [`examples/08-coordinator.json`](examples/08-coordinator.json). The two sub-agents are in
[`examples/08-team/`](examples/08-team/):
[`translator.json`](examples/08-team/translator.json) is a simple text agent,
[`create_ticket.json`](examples/08-team/create_ticket.json) returns a `Ticket` as in
[chapter 5](05-structured-data.md).

| Field | Meaning |
|---|---|
| key (`translator`) | name of the tool, lowercase like every tool name |
| `spec` | spec file of the sub-agent, relative to this spec |
| `description` | what the sub-agent is for; required with `spec`, because the model decides based on it |

This is how a sub-agent behaves:

- **Arguments:** if its `input` is text, the tool gets an argument `request` with the task. If its
  `input` is a type of its own, that type's fields are the arguments.
- **Result:** text comes back unchanged, a type of its own as JSON text.
- **Own history:** every call starts fresh, with the model and system prompt of its spec. It does
  not see what the coordinator has discussed so far – so write complete tasks.
- **Errors** of the sub-agent are shown to the model as tool errors, and it can react to them.
- Sub-agents may themselves have tools, MCP servers, knowledge and further sub-agents; grauwolf
  loads everything at startup.

The specs of an agent and its local sub-agents (`agent_tools` with `spec`) form a tree, and one set
of rules holds for the whole tree in every command (`spec validate`, `spec build`, `tools lock`,
`spec run`, `spec serve`, `spec generate`):

- **The path rule.** A sub-agent's spec must be inside the directory of the root spec (the one you
  give the command): a path that leads out of it with `..`, or an absolute path, is an error
  (`sub-agent spec outside the agent's directory`). This way the whole tree can be copied into an
  image. Paths with `..` that stay inside the directory are fine.
- **Cycles** are errors, naming the chain: `sub-agents form a cycle: a.json → b.json → a.json`.
  `spec validate` reports them like a run does.
- **Diamonds.** If two agents name the same sub-agent spec, it is one sub-agent: read once, and at
  run time loaded once and shared by both parents – one set of MCP servers, one knowledge index.
- **One lockfile and one policy.** The `grauwolf.lock` and the tool policy (`grauwolf.toml`, or
  `GRAUWOLF_TOOL_POLICY`) next to the root spec cover every agent of the tree, so no sub-agent
  brings a laxer policy. The entries of a sub-agent's tool packages and git skills are in the root
  lockfile under `<spec path relative to the root's directory>#<reference>`, for example
  `agents/writer.json#ghcr.io/acme/tools/math:1.0`; entries of the root spec keep the plain
  reference. A `grauwolf.lock` next to a sub-agent's spec is not used (a warning says so).
  `spec build` and `tools lock` cover the sub-agents, too. An entry that no agent of the tree uses
  any more (say, after you removed a sub-agent) is stale: `--locked` fails and names it, without
  `--locked` the entry is dropped from the lockfile.
- A remote sub-agent (`url`) is a leaf: it has no spec files, no signature check.

`spec validate` checks the whole tree. Errors in it are shown with the sub-agent's file name:

```bash
grauwolf spec validate docs/user-guide/examples/08-coordinator.json
```

A run, roughly:

```bash
grauwolf spec run docs/user-guide/examples/08-coordinator.json \
  "When paying with PayPal, only a blank white page appears. Please also provide this in German for our service provider."
```

```text
I created a ticket: "Blank white page when paying with PayPal" (priority 1, tags: payment,
paypal, checkout). In German for the service provider: "Beim Bezahlen mit PayPal erscheint nur
eine weiße Seite."
```

With `--events` you can see when the coordinator calls which sub-agent.

## Remote agents via A2A

**A2A** (Agent-to-Agent) is an open protocol through which agents of different frameworks give each
other tasks. Every grauwolf agent that you start with `spec serve` is reachable through it
([chapter 9](09-operations.md)) – and conversely, your agent can use remote A2A agents as
sub-agents. Instead of `spec` you then specify `url`:

```json
{
  "spec_version": 1,
  "name": "requester",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You pass questions about the house rules on to caretaker and repeat its answer.",
  "agent_tools": {
    "caretaker": {
      "url": "http://127.0.0.1:8080",
      "headers": { "Authorization": "Bearer ${CARETAKER_TOKEN}" }
    }
  },
  "strategy": { "kind": "single_run" }
}
```

The file: [`examples/08-a2a.json`](examples/08-a2a.json).

- `url` is the agent's base address (grauwolf then loads `/.well-known/agent-card.json`) or directly
  the address of its **agent card** – the agent's self-description.
- `headers` works as with MCP, `${VAR}` included.
- `description` is optional here; if omitted, grauwolf takes the description from the agent card.
- Every call is a new, separate message without a remembered conversation history. If the remote
  agent asks a follow-up question, it comes back as the result, and the model can respond to it.
- If the agent card offers streaming (`capabilities.streaming`, always the case for grauwolf
  services), grauwolf fetches the answer piece by piece. Long tasks then do not abort after ten
  minutes at the read timeout: it applies only between two signs of life from the agent.

To try it out, start the knowledge agent from [chapter 7](07-conversations-and-knowledge.md) as a
service and call it in a second terminal:

```bash
GRAUWOLF_SERVER_TOKEN=secret grauwolf spec serve docs/user-guide/examples/07-knowledge.json
CARETAKER_TOKEN=secret grauwolf spec run docs/user-guide/examples/08-a2a.json \
  "May I leave my bike in the stairwell?"
```

The answer is roughly: `No, bicycles belong in the bicycle cellar – the stairwell is an escape
route.`

### Securing the connection with SPIFFE

For an agent of your own cluster, `auth.spiffe` replaces the bearer token. The URL is the target's mTLS port over `https`; with a target behind an ingress that ends TLS, use `jwt`:

```json
"agent_tools": {
  "reviewer": {
    "url": "https://reviewer.reviewer.svc:8443",
    "auth": { "spiffe": { "id": "spiffe://agentswork.de/ns/reviewer/sa/reviewer" } }
  },
  "archivist": {
    "url": "https://archivist.example.com",
    "auth": { "spiffe": { "mode": "jwt", "audience": "archivist" } }
  }
}
```

The called agent needs `GRAUWOLF_SPIFFE=1` and a callers file that names your agent (`GRAUWOLF_SPIFFE_USERS`); in a cluster `grauwolf-k8-ops agent allow reviewer --from <your agent>` does both the file and the NetworkPolicy. Mind that the agent card of the target is fetched while your agent starts, so a target that cannot be reached stops your agent at start.

A wrong `id` and an unreachable port look different:

```text
A2A over HTTP: TLS handshake with https://reviewer.reviewer.svc:8443/.well-known/agent-card.json failed: the server is spiffe://agentswork.de/ns/reviewer/sa/reviewer, expected spiffe://agentswork.de/ns/intruder/sa/intruder; the call was not executed
A2A over HTTP: no connection to https://reviewer.reviewer.svc:8443/.well-known/agent-card.json (refused or given up after 30 s); the call was not executed
```

The first names the identity the server really has: set `id` to it. The second means the port is not reachable (agent not deployed, or a NetworkPolicy keeps the caller out). See [chapter 9](09-operations.md#agents-calling-agents-with-spiffe), [chapter 13](13-setting-up-a-cluster.md#letting-agents-call-each-other) and the reference ([`auth.spiffe`](reference-spec.md#authspiffe-in-agent_toolsid-and-mcp_serversid)).

### Typical errors

```text
…:/agent_tools/caretaker: sub-agent needs exactly one of `spec` (local) and `url` (remote, A2A)
…:/agent_tools/translator/description: local sub-agent needs a `description`
…:/agent_tools/Checker: 'Checker' is not a valid ID: allowed are lowercase letters, digits and '_', starting with a letter
```

## What do I use when?

| You want … | Use |
|---|---|
| a ready-made tool (files, Git, web, database) | an MCP server, locally via `command` |
| a service on the network with an MCP interface | an MCP server via `url` with `headers` |
| the same service or an agent inside your cluster, without a shared secret | `auth.spiffe` on the `url` entry |
| a subtask with its own prompt, model or tools | a local sub-agent via `spec` |
| an agent from another team or framework | a remote sub-agent via `url` (A2A) |
| only part of the flow with a different model or different tools, but in the same history | a subgraph ([chapter 6](06-subgraphs-and-planners.md)) |

## Key points

- `mcp_servers` names either `command` (+ `args`) or `url` (+ `headers` with `${VAR}`, and
  `transport`: `streamable_http` or `sse`) per server, and always the `tools` used; `resources: true` adds `<server>_list_resources` and
  `<server>_read_resource`.
- MCP servers run outside the sandbox with your permissions.
- `agent_tools` turns other agents into tools: locally with `spec` and a required `description`,
  remotely with `url` (A2A) and optional `headers`.
- Sub-agents work with their own, fresh history; argument `request` for text input.
- Inside a cluster with SPIRE, `auth.spiffe` on a `url` entry replaces the token: `mtls` with `id`, or `jwt` with `audience`.
- All tool names share one namespace.

Continue with [chapter 9: Operations](09-operations.md).
