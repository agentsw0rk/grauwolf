#!/usr/bin/env bash
# Registers the three applications of delegation (ADR-0111 Q2) in Entra Local through its
# unauthenticated admin API (/admin/api) – only for internal tests – and prints what
# `grauwolf-k8-ops identity install` and `grauwolf login` need. Idempotent: an application is found
# by its display name and created only if missing.
#
#   ENTRA_URL   Entra Local (default https://entra.agentswork.de)
#   TENANT      its tenant (default 11111111-1111-1111-1111-111111111111)
#   API_URI     the application ID URI of the grauwolf identity API (default api://grauwolf-identity)
#   BAO_ADDR, BAO_TOKEN
#               when both are set, the client secret of "grauwolf identity service" is created (once)
#               and written to OpenBao at kv/identity/entra-client-secret (field `value`); it is never
#               printed. Without them no secret is created (it is only needed from stage 3 on).
#   KV_MOUNT    the KV v2 mount (default kv)
#
# Needs bash, curl and jq.
set -euo pipefail

ENTRA_URL=${ENTRA_URL:-https://entra.agentswork.de}
ENTRA_URL=${ENTRA_URL%/}
TENANT=${TENANT:-11111111-1111-1111-1111-111111111111}
API_URI=${API_URI:-api://grauwolf-identity}
SCOPE=access_as_user
KV_MOUNT=${KV_MOUNT:-kv}
SECRET_PATH=identity/entra-client-secret
ADMIN="$ENTRA_URL/admin/api"

for tool in curl jq; do
  command -v "$tool" >/dev/null || { echo "error: $tool is needed" >&2; exit 1; }
done

# admin METHOD PATH [JSON]: one call of the admin API; fails on an HTTP error.
admin() {
  local method=$1 path=$2 body=${3:-}
  if [ -n "$body" ]; then
    curl -fsS -X "$method" -H 'content-type: application/json' --data "$body" "$ADMIN$path"
  else
    curl -fsS -X "$method" "$ADMIN$path"
  fi
}

# app_json NAME: the application with exactly this display name, or nothing.
app_json() {
  local encoded
  encoded=$(jq -rn --arg v "$1" '$v|@uri')
  admin GET "/apps?search=$encoded&top=200" |
    jq -c --arg name "$1" '[.value[] | select(.displayName == $name)] | first // empty'
}

# ensure_app NAME CONFIDENTIAL [APP_ID_URI]: prints the application as JSON.
ensure_app() {
  local name=$1 confidential=$2 uri=${3:-} found body
  found=$(app_json "$name")
  if [ -n "$found" ]; then
    echo "application \"$name\": unchanged ($(jq -r .id <<<"$found"))" >&2
    printf '%s\n' "$found"
    return
  fi
  body=$(jq -cn --arg name "$name" --argjson confidential "$confidential" --arg uri "$uri" \
    '{displayName: $name, isConfidential: $confidential} + (if $uri == "" then {} else {appIdUri: $uri} end)')
  found=$(admin POST /apps "$body")
  echo "application \"$name\": created ($(jq -r .id <<<"$found"))" >&2
  printf '%s\n' "$found"
}

admin GET /health >/dev/null || { echo "error: $ADMIN/health does not answer" >&2; exit 1; }

# 1. The API people's tokens are for: the audience of their tokens, scope access_as_user.
api=$(ensure_app "grauwolf identity API" false "$API_URI")
api_id=$(jq -r .id <<<"$api")
if [ "$(jq -r .appIdUri <<<"$api")" != "$API_URI" ]; then
  admin PATCH "/apps/$api_id" "$(jq -cn --arg uri "$API_URI" '{appIdUri: $uri}')" >/dev/null
  echo "application \"grauwolf identity API\": appIdUri set to $API_URI" >&2
fi
if jq -e --arg s "$SCOPE" 'any(.exposedScopes[]?; .value == $s)' <<<"$api" >/dev/null; then
  echo "scope $SCOPE: unchanged" >&2
else
  admin POST "/apps/$api_id/scopes" \
    "$(jq -cn --arg s "$SCOPE" '{value: $s, adminConsentDisplayName: "Act on behalf of the signed-in person"}')" >/dev/null
  echo "scope $SCOPE: created" >&2
fi

# 2. The CLI: a public client; the device code flow needs no redirect URI.
cli=$(ensure_app "grauwolf CLI" false)
cli_id=$(jq -r .id <<<"$cli")

# 3. The identity service: a confidential client (its secret is needed from stage 3 on).
service=$(ensure_app "grauwolf identity service" true)
service_id=$(jq -r .id <<<"$service")
if [ -n "${BAO_ADDR:-}" ] && [ -n "${BAO_TOKEN:-}" ]; then
  stored=$(curl -sS -o /dev/null -w '%{http_code}' -H "X-Vault-Token: $BAO_TOKEN" \
    "${BAO_ADDR%/}/v1/$KV_MOUNT/metadata/$SECRET_PATH")
  if [ "$stored" = 200 ]; then
    echo "client secret: unchanged ($KV_MOUNT/$SECRET_PATH exists)" >&2
  else
    # The secret is shown only once: straight from the answer into OpenBao, never to the terminal.
    admin POST "/apps/$service_id/secrets" '{"displayName": "grauwolf identity service"}' |
      jq -c '{data: {value: .secretText}}' |
      curl -fsS -o /dev/null -X POST -H "X-Vault-Token: $BAO_TOKEN" \
        -H 'content-type: application/json' --data @- "${BAO_ADDR%/}/v1/$KV_MOUNT/data/$SECRET_PATH"
    echo "client secret: created and stored at $KV_MOUNT/$SECRET_PATH" >&2
  fi
else
  echo "client secret: not created (set BAO_ADDR and BAO_TOKEN to create it and store it in OpenBao)" >&2
fi

# Issuer and keys as the tenant's OpenID discovery names them.
discovery=$(curl -fsS "$ENTRA_URL/$TENANT/v2.0/.well-known/openid-configuration")
issuer=$(jq -r .issuer <<<"$discovery")
jwks=$(jq -r .jwks_uri <<<"$discovery")

cat <<OUT

grauwolf identity API      $api_id  ($API_URI, scope $SCOPE)
grauwolf CLI               $cli_id
grauwolf identity service  $service_id

grauwolf-k8-ops identity install --image <grauwolf CLI image> \\
  --tenant $TENANT \\
  --issuer $issuer \\
  --audience $api_id \\
  --audience $API_URI \\
  --jwks-url $jwks

grauwolf login --authority $ENTRA_URL --tenant $TENANT \\
  --client-id $cli_id --scope $API_URI/$SCOPE
OUT
