# Test git server with SPIFFE: live check of the git tools

The smart-HTTP test server of `grauwolf-git` (`serve_folder`, feature `test-git-server`) in its
mTLS mode ([ADR-0110](../../docs/adr/0110-workload-identity-for-tools-and-credentials.md), stage 1) as a
container, so that an agent can clone and push with its SPIFFE identity
([`builtin_tools.git.auth`](../../docs/user-guide/03-tools.md)). **For tests only**: the image is not part of the
release images, and nothing here is applied for you.

| File | What it is |
|---|---|
| `../../docker/test-git-server.Dockerfile` | Builds the example (debian-slim, uid 65532). |
| `test-git-server.yaml` | Service account, `ClusterSPIFFEID`, mapping ConfigMap, Deployment (restricted Pod Security Standard: non-root, read-only root file system, no capabilities, `RuntimeDefault` seccomp, `/tmp` as `emptyDir`) and Service on port 8443. |
| `mapping.json` | The same mapping as in the ConfigMap, for running the example locally. |

## Mapping

`{ "<spiffe id or prefix/*>": { "user": "<name>", "push": true|false } }`: an exact ID wins over a
prefix, the longest prefix over a shorter one, prefixes match whole path segments. Callers that are
not mapped get `403`, read-only callers get `403` on push. The file is read again at every request.

## Run it in a cluster

```sh
docker build -f docker/test-git-server.Dockerfile -t "$IMAGE" . && docker push "$IMAGE"
export NAMESPACE=git-test TRUST_DOMAIN=agentswork.de IMAGE=...
kubectl create namespace "$NAMESPACE"
kubectl label namespace "$NAMESPACE" pod-security.kubernetes.io/enforce=restricted
envsubst < examples/test-git-server/test-git-server.yaml | kubectl apply -f -
kubectl -n "$NAMESPACE" logs deploy/test-git-server   # "spiffe: serving as spiffe://…/sa/test-git-server"
```

The agent side is the spec of your agent (see the user guide, chapter 3). The server serves its
repository as `/tools.git` (the `url:` line of its log). With `grauwolf-k8-ops agent deploy --spec`
the spec folder is mounted read-only, but `deploy` mounts a directory of the agent's data volume at a
relative `root`, here `/app/spec/work`, so the spec needs no special path:

```json
"builtin_tools": {
  "tools": ["git_checkout", "git_commit", "git_push"], "root": "work",
  "git": {
    "repo_url": "https://test-git-server.git-test.svc:8443/tools.git",
    "push_policy": "any",
    "auth": { "test-git-server.git-test.svc": { "spiffe": { "id": "spiffe://agentswork.de/ns/git-test/sa/test-git-server" } } }
  }
}
```

The agent's pods need a registration entry (`ClusterSPIFFEID`) and the CSI socket as in
`examples/spire-openbao`; an agent in the namespace `agents` with the service account `writer` may
push, any other service account of `agents` may only read, everyone else gets `403`.
With `grauwolf-k8-ops`-style namespaces, create the server in its own namespace and let the
`ClusterSPIFFEID` of the agents select theirs.

## Run it locally

```sh
SPIFFE_ENDPOINT_SOCKET=unix:///run/spire/agent.sock \
  cargo run -p grauwolf-git --features test-git-server --example serve_folder -- \
  ./some-folder "" 8443 --spiffe examples/test-git-server/mapping.json
```

Without `--spiffe` the example serves plain HTTP on `127.0.0.1` as before (now also writable).
