# Settings of the whole setup, in one place. Source it before running render.sh and
# setup-openbao.sh:   . ./env.sh
#
# TRUST_DOMAIN must be the `trust_domain` of the SPIRE server and agent configuration. It shows
# up in every SPIFFE ID (spiffe://<trust domain>/ns/<namespace>/sa/<service account>), in the
# `bound_subject` of the OpenBao role and in the configuration of the OIDC discovery provider.
export TRUST_DOMAIN="${TRUST_DOMAIN:-agentswork.de}"

# Where grauwolf runs.
export NAMESPACE="${NAMESPACE:-agents}"
export SERVICE_ACCOUNT="${SERVICE_ACCOUNT:-grauwolf}"

# The grauwolf image comes from a private registry: set it, and create the pull secret
# `registry-agentswork` in $NAMESPACE (see README.md).
export GRAUWOLF_IMAGE="${GRAUWOLF_IMAGE:-registry.example.invalid/grauwolf:latest}"

# The OIDC discovery provider. It needs a SPIFFE ID of its own, so its namespace must not be in
# `ignoreNamespaces` of spire-controller-manager – that list usually contains spire-system, hence
# a namespace of its own.
export OIDC_NAMESPACE="${OIDC_NAMESPACE:-spire-oidc}"
export OIDC_SERVICE="${OIDC_SERVICE:-spire-oidc-discovery-provider.${OIDC_NAMESPACE}.svc}"

# OpenBao.
export BAO_ADDR="${BAO_ADDR:-http://openbao.openbao.svc:8200}"
export JWT_MOUNT="${JWT_MOUNT:-jwt}"
export AUDIENCE="${AUDIENCE:-openbao}"
export ROLE="${ROLE:-grauwolf-agents}"
export POLICY="${POLICY:-grauwolf-agents}"
export KV_MOUNT="${KV_MOUNT:-kv}"
export SECRET_PATH="${SECRET_PATH:-agents/reviewer}"
export SECRET_FIELD="${SECRET_FIELD:-jira}"
# Where OpenBao fetches the signing keys of the JWT-SVIDs (the discovery provider's /keys).
export JWKS_URL="${JWKS_URL:-http://${OIDC_SERVICE}/keys}"
