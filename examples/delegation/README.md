# Delegation with Entra Local: acting on behalf of a person

The whole flow of [ADR-0111](../../docs/adr/0111-delegation-with-transaction-tokens.md) stage 1 in a
cluster: Alice signs in on the command line, calls the agent `coordinator`, which passes her
Txn-Token on to `reviewer`; in `reviewer` the run belongs to Alice. The background – concepts,
the runtime flow as diagrams, operations and the pitfalls of the live check – is in
[user guide chapter 14](../../docs/user-guide/14-acting-on-behalf-of-people.md), whose
[step-by-step setup](../../docs/user-guide/14-acting-on-behalf-of-people.md#step-by-step-setup) follows
the same steps.

What you need:

- a cluster set up as in [chapter 13](../../docs/user-guide/13-setting-up-a-cluster.md): SPIRE,
  OpenBao, `grauwolf-k8-ops oidc install` and `openbao setup` (run `openbao setup` again after an
  update of `grauwolf-k8-ops`: its policy now also covers the identity service);
- Entra Local (an emulator of a slice of Entra ID) at `https://entra.agentswork.de` with the tenant
  `11111111-1111-1111-1111-111111111111` and its seed users (`alice@entralocal.dev`);
- the grauwolf CLI image (it runs `grauwolf identity serve` and `grauwolf spec serve`) and the CLI
  on your machine; an image pull secret if the registry needs credentials (`--pull-secret`);
- **the issuer's host reachable from inside the cluster**: a pod must get the JWKS of
  `https://entra.agentswork.de`. If the name resolves to a public IP the pods cannot reach (no
  hairpin NAT), or Entra Local is served by the cluster's own ingress, set up split DNS and an egress
  rule for the identity service first – see the pitfall
  [503 "the identity service is not available"](../../docs/user-guide/14-acting-on-behalf-of-people.md#503-the-identity-service-is-not-available);
- a model backend the agents reach (below: Ollama at `http://ollama.ollama.svc:11434`).

The full checklist, with how to check each item, is in
[chapter 14, Prerequisites](../../docs/user-guide/14-acting-on-behalf-of-people.md#prerequisites).

Entra Local's admin API is unauthenticated: use it only for internal tests, and do not let the
ingress expose `/admin`.

## 1. Register the applications

```sh
export BAO_ADDR=http://127.0.0.1:8200 BAO_TOKEN=...   # optional: stores the service's secret
./entra-local-apps.sh
```

The script creates, by display name and only if missing:

| Application | Kind | What for |
|---|---|---|
| grauwolf identity API | application ID URI `api://grauwolf-identity`, scope `access_as_user` | the audience of people's tokens |
| grauwolf CLI | public client | `grauwolf login` (device code flow) |
| grauwolf identity service | confidential client with a secret | needed from stage 3 (grants) on |

The secret is shown by Entra Local only once; the script writes it straight to OpenBao
(`kv/identity/entra-client-secret`) and never to the terminal. Without `BAO_ADDR` and `BAO_TOKEN` it
creates no secret. At the end it prints the IDs and the two commands below with the values filled
in. Give the identity service **both** audiences of people's tokens: the **application ID** (a GUID) of "grauwolf identity API" (Entra Local's device code flow writes the resource application's ID into `aud`, as a v2.0 token of a real Entra ID does) and the application ID URI `api://grauwolf-identity` (what v1.0 tokens and the token generator of Entra Local's admin interface carry).

## 2. The identity service

```sh
grauwolf-k8-ops identity install --image registry.example.com/grauwolf:<version> \
  --pull-secret <namespace>/<name of the image pull secret> \
  --tenant 11111111-1111-1111-1111-111111111111 \
  --issuer <issuer printed by the script> \
  --audience <application ID of "grauwolf identity API"> \
  --audience api://grauwolf-identity \
  --jwks-url <jwks_uri printed by the script>
grauwolf-k8-ops identity status
```

Leave out `--pull-secret` for a public image. It makes namespace `identity` with the service `grauwolf-identity`
(`spiffe://<trust domain>/ns/identity/sa/grauwolf-identity`), a Transit key in OpenBao that signs the
Txn-Tokens, and an empty policy: no agent may act on behalf of people yet.

## 3. Two agents that take part

The specs are the examples of chapter 13: `coordinator` calls `reviewer` over mTLS.

```sh
SPECS=../../docs/user-guide/examples
grauwolf-k8-ops agent add coordinator
grauwolf-k8-ops agent add reviewer
grauwolf-k8-ops agent allow reviewer --from coordinator
grauwolf-k8-ops agent deploy reviewer --image registry.example.com/grauwolf:<version> \
  --spec $SPECS/13-reviewer.json --env OLLAMA_HOST=http://ollama.ollama.svc:11434 \
  --identity --events
grauwolf-k8-ops agent deploy coordinator --image registry.example.com/grauwolf:<version> \
  --spec $SPECS/13-coordinator.json --env OLLAMA_HOST=http://ollama.ollama.svc:11434 \
  --identity --events --host coordinator.example.com
grauwolf-k8-ops identity allow coordinator
```

`--identity` gives each agent `GRAUWOLF_IDENTITY_URL` and `GRAUWOLF_IDENTITY_SPIFFE_ID`. Only the
entry agent needs `identity allow`: `reviewer` only verifies and renews.

## 4. Sign in as Alice and call

```sh
grauwolf login --authority https://entra.agentswork.de \
  --tenant 11111111-1111-1111-1111-111111111111 \
  --client-id <application ID of "grauwolf CLI"> --scope api://grauwolf-identity/access_as_user
```

Open the printed address, enter the code and sign in as `alice@entralocal.dev`. The policy change
of `identity allow` reaches the identity service after about a minute (kubelet sync). The tokens are
cached in `~/.config/grauwolf/login.json` (mode 0600); `grauwolf login --status` shows who is signed
in. Then call the coordinator with Alice's token:

```sh
curl -sS https://coordinator.example.com/run \
  -H "Authorization: Bearer $(grauwolf login --print-token)" \
  -H 'content-type: application/json' -d '{"input": "Review: the sky is green."}'
grauwolf-k8-ops agent logs reviewer --events-only
```

Call through the Ingress, not through `kubectl proxy` (the API server removes the
`Authorization` header). In `reviewer`'s events `agent_starting` carries `on_behalf_of` with Alice's
`oid` and the same `txn` as in `coordinator`'s, and `caller` names the coordinator. Entra Local's
tokens carry no name, so `name` and `upn` are missing; a real Entra ID fills them.

## With a real Entra ID

Register the same three applications in the Azure portal (or with `az ad app create`):

- **grauwolf identity API**: "Expose an API", application ID URI (for example
  `api://grauwolf-identity`), scope `access_as_user`, `accessTokenAcceptedVersion` 2 in the manifest
  (so the tokens are v2.0 and `iss` is `https://login.microsoftonline.com/<tenant>/v2.0`);
- **grauwolf CLI**: "Allow public client flows" on, API permission on `access_as_user` (with admin
  consent, or each person consents at the first sign-in);
- **grauwolf identity service**: a client secret (stage 3; OBO and federated credentials are checked
  in a later stage).

Then `identity install --tenant <tenant> --issuer https://login.microsoftonline.com/<tenant>/v2.0
--audience <application ID of the API> --audience api://grauwolf-identity` (without `--jwks-url`, the keys come from the issuer's
OpenID discovery) and `grauwolf login --tenant <tenant> --client-id <CLI application ID> --scope
api://grauwolf-identity/access_as_user` (the authority defaults to
`https://login.microsoftonline.com`). The identity service's egress allows port 443 to all addresses
outside the cluster for the keys host, because a NetworkPolicy cannot match host names.
