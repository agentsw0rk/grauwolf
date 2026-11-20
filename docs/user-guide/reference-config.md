# Configuration reference: `grauwolf.toml`

`grauwolf.toml` is read from the directory of the spec. It holds two tables: the
**tool policy** (`[tool_packages]`), which decides which tool packages ([chapter 3](03-tools.md#trust-the-tool-policy-and-signatures))
may be used. The environment variable `GRAUWOLF_TOOL_POLICY` names another policy file in the same
format; it wins over `grauwolf.toml`. Without either, everything is allowed and nothing has to be
signed. The policy applies to every command that resolves tool packages (`spec run|chat|serve|mcp|
resume|build|generate`, `tools lock|pull`; `tools inspect` applies the allowlists only). A
file that cannot be read, unknown keys and wrong types are errors that name the file. A fully
commented example: [`examples/03-policy/grauwolf.toml`](examples/03-policy/grauwolf.toml).

## `[tool_packages]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `allowed_registries` | list of text | any | OCI registries (`ghcr.io`) or registry/repository prefixes (`ghcr.io/acme`; whole path parts) that may be used. An empty list allows none |
| `allowed_git_hosts` | list of text | any | Git hosts (`github.com`) or host/path prefixes (`github.com/acme`) that may be used. An empty list allows none |
| `require_signatures` | boolean | `false` | Every package must carry a valid signature, checked with `oci_keys` (OCI) or `git_allowed_signers` (git). Needs at least one of the two keys |
| `oci_keys` | list of paths | `[]` | PEM public keys (P-256) that verify the cosign-compatible signatures of OCI packages; files may hold several keys. Made by `grauwolf tools keygen` or cosign. Relative to the policy file |
| `git_allowed_signers` | path | – | File in the format of git's `gpg.ssh.allowedSignersFile` (`principals [namespaces="…"] keytype key`): who may sign commits and tags of git packages; principals are matched against the committer or tagger e-mail. Relative to the policy file |

Details and walkthrough: [Trust: the tool policy and signatures](03-tools.md#trust-the-tool-policy-and-signatures).

## `[credentials]`

Credential sources: variables that come from a file or from OpenBao instead of the environment
([chapter 12](12-credentials-from-openbao.md)). Read from the same `grauwolf.toml`, or from the file
named by `GRAUWOLF_CREDENTIALS` (which wins). Relative `file:` paths are relative to that file.

```toml
[credentials.openbao]
address = "https://openbao.internal:8200"
auth = { spiffe = { audience = "openbao", role = "grauwolf-agents", mount = "jwt" } }
refresh = "5m"

[credentials.variables]
ATLASSIAN_API_TOKEN = "openbao:kv/agents/reviewer#jira"
OPENAI_API_KEY      = "file:/run/secrets/openai/api-key"
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `openbao.address` | text | – | Base address of OpenBao or Vault. Required with an `openbao:` source |
| `openbao.auth.spiffe.audience` | text | – | Audience of the JWT-SVID; the role's `bound_audiences` |
| `openbao.auth.spiffe.role` | text | – | Role of the JWT auth method |
| `openbao.auth.spiffe.mount` | text | `jwt` | Mount of the JWT auth method |
| `openbao.refresh` | duration (`30s`, `5m`, `1h`) | `5m` | Interval after which all mapped values are read again |
| `variables.<NAME>` | text | – | `openbao:<mount>/<path>#<field>` (alias `vault:`) or `file:<path>`. `<NAME>` must be a variable of the [environment variable table](reference-cli.md#environment-variables), not one that is read from the process environment directly (`PATH`, `HOME`, `OTEL_EXPORTER_OTLP_*`, …); a variable that is mapped and also set is an error |

Unknown keys and wrong types are errors that name the file. The OpenBao login needs
`SPIFFE_ENDPOINT_SOCKET`.

## The identity service's policy file

Not part of `grauwolf.toml`: the JSON file that `grauwolf identity serve` reads from
`GRAUWOLF_IDENTITY_POLICY` ([chapter 14](14-acting-on-behalf-of-people.md#the-policy)). It names the
workloads that may request Txn-Tokens on behalf of people – the agents at the entry – and, in
`grants`, which agents may run on people's grants and which workloads may start such runs.

```json
{ "callers": {
    "spiffe://agentswork.de/ns/coordinator/sa/coordinator": { "purposes": ["review"] },
    "spiffe://agentswork.de/ns/frontends/*": {}
} }
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `callers.<SPIFFE ID>` | object | – | A workload that may request tokens. A key ending in `/*` is a prefix of whole path segments; the exact ID wins over a prefix, the longest prefix over shorter ones |
| `callers.<…>.purposes` | list of text | any | The purposes (`purp`) it may request; without the key any purpose is allowed |
| `grants` | object | none | Grants for unattended runs (ADR-0111 stage 3); without it no grant can be created or used |
| `grants.agents.<SPIFFE ID>` | object | – | An entry agent that may run on people's grants; `/*` prefixes as in `callers` |
| `grants.agents.<…>.purposes` | list of text | any | The purposes its grants may have; every purpose of a grant must be in the list |
| `grants.triggers` | list of SPIFFE IDs | `[]` | The workloads that may start grant runs (e.g. `spiffe://<td>/ns/schedules/sa/morning`, or `spiffe://<td>/ns/schedules/*`) |

```json
{ "callers": {},
  "grants": {
    "agents": { "spiffe://agentswork.de/ns/coordinator/sa/coordinator": { "purposes": ["review"] } },
    "triggers": ["spiffe://agentswork.de/ns/schedules/*"]
} }
```

Both sections are optional. `grants` is checked when a grant is created and again at every run of
it, so removing an agent or a trigger stops its existing grants at once
([chapter 14](14-acting-on-behalf-of-people.md#unattended-runs-grants-and-schedules)).

Unknown keys, invalid SPIFFE IDs and wrong types are errors: at start they stop the service, later
the old policy stays and the error is logged. The file is checked for changes every 2 seconds
(also a ConfigMap's symlink swap). Renewing a Txn-Token needs no entry: any workload of the token's
trust domain may renew.
