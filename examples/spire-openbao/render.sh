#!/usr/bin/env bash
# Prints the manifests with the settings of env.sh filled in, for `kubectl apply -f -`.
# Nothing is applied by this script.
#   . ./env.sh && ./render.sh | kubectl apply -f -
# An optional argument selects the manifests whose file name starts with it: ./render.sh 10
set -euo pipefail
cd "$(dirname "$0")"
: "${TRUST_DOMAIN:?source env.sh first}"
vars='${TRUST_DOMAIN} ${NAMESPACE} ${SERVICE_ACCOUNT} ${GRAUWOLF_IMAGE} ${OIDC_NAMESPACE} ${OIDC_SERVICE} ${BAO_ADDR} ${JWT_MOUNT} ${AUDIENCE} ${ROLE} ${KV_MOUNT} ${SECRET_PATH} ${SECRET_FIELD}'
export VARS="$(echo "$vars" | tr -d '${}')"
for file in manifests/${1:-}*.yaml; do
  echo "---"
  # Only the variables above are replaced: the Go templates of the ClusterSPIFFEID
  # (`{{ .TrustDomain }}`) stay as they are.
  if command -v envsubst >/dev/null; then
    envsubst "$vars" < "$file"
  else
    # Without envsubst (part of gettext): the same replacement of the listed variables.
    perl -pe 'BEGIN { %v = map { $_ => 1 } split / /, $ENV{VARS} } s/\$\{(\w+)\}/$v{$1} ? $ENV{$1} : $&/ge' < "$file"
  fi
done
