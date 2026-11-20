#!/usr/bin/env bash
# Prepares OpenBao for grauwolf agents (ADR-0110): JWT auth method that trusts the JWT-SVIDs of
# SPIRE, a policy, a role bound to the agent's SPIFFE ID and a KV v2 secret.
#
#   . ./env.sh
#   export BAO_TOKEN=...            # a token that may configure auth methods, policies, mounts
#   export SECRET_VALUE=...         # the credential to store (read from the environment, never printed)
#   ./setup-openbao.sh
#
# Needs the `bao` CLI. Safe to run again (existing mounts are kept, the rest is overwritten).
set -euo pipefail
: "${TRUST_DOMAIN:?source env.sh first}"
: "${BAO_TOKEN:?set BAO_TOKEN}"
: "${SECRET_VALUE:?set SECRET_VALUE (the credential to store; it is never printed)}"

# The SPIFFE ID of the agent's pods: the ClusterSPIFFEID template of manifests/
# grauwolf-clusterspiffeid.yaml. The role accepts exactly this subject.
SPIFFE_ID="spiffe://${TRUST_DOMAIN}/ns/${NAMESPACE}/sa/${SERVICE_ACCOUNT}"

# 1. The JWT auth method with the signing keys of SPIRE.
bao auth enable -path="${JWT_MOUNT}" jwt 2>/dev/null || echo "auth/${JWT_MOUNT} exists"
bao write "auth/${JWT_MOUNT}/config" jwks_url="${JWKS_URL}" ${OIDC_ISSUER:+bound_issuer="${OIDC_ISSUER}"}

# 2. A KV v2 mount for the secrets.
bao secrets enable -path="${KV_MOUNT}" -version=2 kv 2>/dev/null || echo "${KV_MOUNT}/ exists"

# 3. The policy: read access to the secrets of the agents, nothing else.
bao policy write "${POLICY}" - <<POLICY_EOF
path "${KV_MOUNT}/data/agents/*" {
  capabilities = ["read"]
}
POLICY_EOF

# 4. The role: a JWT with this audience and exactly this subject gets the policy.
bao write "auth/${JWT_MOUNT}/role/${ROLE}" \
  role_type=jwt \
  user_claim=sub \
  bound_audiences="${AUDIENCE}" \
  bound_subject="${SPIFFE_ID}" \
  token_policies="${POLICY}" \
  token_ttl=1h

# 5. The secret (the value comes from the environment, not from the command line).
bao kv put -mount="${KV_MOUNT}" "${SECRET_PATH}" "${SECRET_FIELD}=${SECRET_VALUE}" >/dev/null

echo "OpenBao is ready: role ${ROLE} accepts ${SPIFFE_ID} (audience ${AUDIENCE}),"
echo "secret ${KV_MOUNT}/${SECRET_PATH}#${SECRET_FIELD} is stored."
