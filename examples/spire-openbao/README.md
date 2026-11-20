# SPIRE and OpenBao with grauwolf: live check

Manifests and a script to try [credentials from OpenBao with SPIFFE](../../docs/user-guide/12-credentials-from-openbao.md)
on a cluster that runs SPIRE (with spire-controller-manager and the SPIFFE CSI driver) and OpenBao.
**Nothing here is applied for you**: `render.sh` only prints manifests, `setup-openbao.sh` only
talks to OpenBao with the token you give it.

| File | What it is |
|---|---|
| `env.sh` | All settings in one place. `TRUST_DOMAIN` (default `agentswork.de`) must equal `trust_domain` in the SPIRE server and agent configuration. |
| `manifests/10-oidc-discovery-provider.yaml` | SPIRE OIDC discovery provider (serves the JWKS that OpenBao verifies JWT-SVIDs with) and its `ClusterSPIFFEID`, in a namespace of its own (`OIDC_NAMESPACE`, default `spire-oidc`): spire-controller-manager gives no SPIFFE ID to pods in its `ignoreNamespaces`, which usually include spire-system. |
| `manifests/20-grauwolf-clusterspiffeid.yaml` | The `ClusterSPIFFEID` for pods labelled `app.kubernetes.io/name: grauwolf`; its template uses `{{ .TrustDomain }}`. |
| `manifests/30-grauwolf-job.yaml` | Service account, `[credentials]` configuration and a Job that runs `grauwolf credentials check` and prints only `resolved <VAR> from openbao (N bytes)`. |
| `render.sh` | Fills the `${…}` placeholders of the manifests from `env.sh` (with `envsubst`, or a built-in fallback). |
| `setup-openbao.sh` | `bao` commands: JWT auth with the `jwks_url`, KV v2, policy, role (`bound_audiences`, `bound_subject`, `user_claim=sub`), the secret. |

## Order

```sh
. ./env.sh                                   # TRUST_DOMAIN, namespaces, addresses, image
./render.sh | less                           # look at what would be applied

# 1. SPIRE side: the discovery provider and the registration of the grauwolf pods
./render.sh 10 | kubectl apply -f -
./render.sh 20 | kubectl apply -f -

# 2. OpenBao side
export BAO_TOKEN=...  SECRET_VALUE=...       # never printed
./setup-openbao.sh

# 3. The check (create the pull secret first, see below)
./render.sh 30 | kubectl apply -f -
kubectl -n "$NAMESPACE" logs job/grauwolf-credentials-check
# resolved ATLASSIAN_API_TOKEN from openbao (N bytes)
```

## The image

The grauwolf image comes from a **private registry**. Set `GRAUWOLF_IMAGE` in `env.sh` (the default
is a placeholder that does not resolve) and create the pull secret the Job refers to:

```sh
kubectl -n "$NAMESPACE" create secret docker-registry registry-agentswork \
  --docker-server=<registry> --docker-username=<user> --docker-password=<token>
```

The Job names it in `imagePullSecrets` as `registry-agentswork`.

## Trust domain

Everything that contains the trust domain takes it from `TRUST_DOMAIN`: the discovery provider's
`workload_api.trust_domain` (manifest `10-…`) and the `bound_subject` of the OpenBao role
(`setup-openbao.sh`). The `ClusterSPIFFEID` templates use `{{ .TrustDomain }}` and follow the SPIRE
server. If the SPIRE server uses `jwt_issuer`, set `OIDC_ISSUER` to the same value before running
`setup-openbao.sh`; it becomes `bound_issuer` of the auth method.

## Live check (2026-11-14)

The example was run on a real cluster: k3s 1.34, SPIRE 1.15.5 with trust domain `agentswork.de`,
OpenBao 2.7.2.

- The OIDC discovery provider runs in its own namespace (`spire-oidc`): the controller-manager
  ignores `spire-system`, so a pod there gets no registration entry.
- OpenBao accepted `jwks_url` over plain HTTP inside the cluster. `bound_issuer` was not set: the
  SPIRE server has no `jwt_issuer`.
- The image was built in the cluster (kaniko, temporary namespace) and pushed to a private registry.
- The Job ran under the restricted Pod Security Standard and printed
  `resolved ATLASSIAN_API_TOKEN from openbao (32 bytes)`.
- Counter-check: the same Job with the service account `default` got the SPIFFE ID
  `spiffe://agentswork.de/ns/agents/sa/default`; OpenBao refused it with
  `HTTP 400: error validating token: invalid subject (sub) claim`, and the error contained no value.
- A start-up race showed up: a pod that starts before its registration entry is synced gets
  `no identity issued` from the Workload API. grauwolf now waits for the identity
  (`SPIFFE_FETCH_TIMEOUT`, default 30 s).
