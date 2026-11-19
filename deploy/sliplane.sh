#!/usr/bin/env bash
# Deploys OpenTrail to Sliplane through its API (https://ctrl.sliplane.io): a managed Postgres 18 and
# the app. The Deploy on Sliplane button (sliplane.yaml) does the same from a browser.
#
#   SLIPLANE_TOKEN=api_rw_org_... ./deploy/sliplane.sh            first install, or update to :latest
#   SLIPLANE_TOKEN=... OPENTRAIL_TAG=0.2.0 ./deploy/sliplane.sh     update to a release
#
# The token needs read and write access to projects, servers, services and postgres, and
# secrets:read so the database password can be read once. Nothing is written to disk.
#
# Optional:
#   SLIPLANE_SERVER_ID   the server to run on (default: the only server, or the first one)
#   SLIPLANE_PROJECT     project name (default: OpenTrail)
#   SLIPLANE_REGION      ger, fin, us-east, ... for the database (default: ger)
#   POSTGRES_INSTANCE    starter, base, ... (default: starter). Without a payment method, Sliplane
#                        offers starter and base free for 48 hours.
#   DATABASE_URL         bring your own Postgres 18 instead of creating one. Per-project SQL logins
#                        need a user with CREATEROLE; Settings, System says whether it has one.
#   RESEND_API_KEY, RESEND_FROM   pin email to Resend (otherwise the setup wizard asks for it)
#
# Afterwards, open the printed URL and enter the setup code from the service's logs.

set -euo pipefail

API=https://ctrl.sliplane.io/v0
IMAGE=${OPENTRAIL_IMAGE:-ghcr.io/nielsbosma/opentrail}
TAG=${OPENTRAIL_TAG:-latest}
PROJECT=${SLIPLANE_PROJECT:-OpenTrail}
REGION=${SLIPLANE_REGION:-ger}
APP_NAME=opentrail

for tool in curl jq openssl; do
  command -v "$tool" >/dev/null || { echo "This script needs $tool." >&2; exit 1; }
done
: "${SLIPLANE_TOKEN:?Set SLIPLANE_TOKEN to an API token from the Sliplane dashboard (team settings).}"

api() {
  local method=$1 path=$2 body=${3:-}
  local args=(-sS -X "$method" -H "Authorization: Bearer $SLIPLANE_TOKEN" -H "Accept: application/json" -w '\n%{http_code}')
  [[ -n "$body" ]] && args+=(-H "Content-Type: application/json" --data-binary @-)
  local out code
  out=$(printf '%s' "$body" | curl "${args[@]}" "$API$path")
  code=${out##*$'\n'}
  out=${out%$'\n'*}
  if [[ "$code" -ge 400 ]]; then
    echo "Sliplane refused $method $path ($code): $(jq -r '.message // .' <<<"$out" 2>/dev/null || echo "$out")" >&2
    exit 1
  fi
  printf '%s' "$out"
}

step() { printf '\n==> %s\n' "$*" >&2; }

step "Checking the token"
api GET /me >/dev/null && echo "The token works." >&2

step "Project $PROJECT"
project_id=$(api GET /projects | jq -r --arg n "$PROJECT" '[.[] | select(.name == $n)][0].id // empty')
if [[ -z "$project_id" ]]; then
  project_id=$(api POST /projects "$(jq -n --arg n "$PROJECT" '{name: $n}')" | jq -r .id)
  echo "Created $project_id" >&2
fi

services=$(api GET "/projects/$project_id/services")
app_id=$(jq -r --arg n "$APP_NAME" '[.[] | select(.name == $n)][0].id // empty' <<<"$services")
if [[ -n "$app_id" ]]; then
  step "Updating $APP_NAME to $IMAGE:$TAG"
  api POST "/projects/$project_id/services/$app_id/deploy" "$(jq -n --arg t "$TAG" '{tag: $t}')" >/dev/null
  domain=$(api GET "/projects/$project_id/services/$app_id" | jq -r '.network.managedDomain // empty')
  echo "Deploying. Migrations run on start." >&2
  echo "https://${domain:-<your domain>}"
  exit 0
fi

step "Server"
server_id=${SLIPLANE_SERVER_ID:-$(api GET /servers | jq -r '.[0].id // empty')}
[[ -n "$server_id" ]] || { echo "Create a server in Sliplane first, or set SLIPLANE_SERVER_ID." >&2; exit 1; }
echo "Using $server_id" >&2

if [[ -z "${DATABASE_URL:-}" ]]; then
  step "Postgres 18 (${POSTGRES_INSTANCE:-starter}, $REGION)"
  # The same user the Deploy on Sliplane button creates: not a superuser, but it has CREATEROLE.
  pg_id=$(api POST /postgres "$(jq -n --arg r "$REGION" --arg i "${POSTGRES_INSTANCE:-starter}" \
    '{name: "OpenTrail", instanceType: $i, region: $r, version: 18, databaseName: "opentrail", databaseUser: "app"}')" | jq -r .id)
  for _ in $(seq 1 90); do
    pg=$(api GET "/postgres/$pg_id")
    [[ $(jq -r .status <<<"$pg") == running ]] && break
    sleep 10
  done
  DATABASE_URL=$(jq -r '.connection.uri // empty' <<<"$pg")
  [[ -n "$DATABASE_URL" ]] || { echo "The database is not running yet, or the token lacks secrets:read. Re-run with DATABASE_URL set." >&2; exit 1; }
  case "$DATABASE_URL" in *sslmode=*) ;; *\?*) DATABASE_URL="$DATABASE_URL&sslmode=require" ;; *) DATABASE_URL="$DATABASE_URL?sslmode=require" ;; esac
fi

step "OpenTrail ($IMAGE:$TAG)"
env_json=$(jq -n \
  --arg db "$DATABASE_URL" --arg secret "$(openssl rand -base64 48 | tr -d '\n')" \
  '[{key: "DATABASE_URL", value: $db, secret: true},
    {key: "APP_SECRET", value: $secret, secret: true},
    {key: "PORT", value: "3000", secret: false}]')
add_env() { env_json=$(jq --arg k "$1" --arg v "$2" --argjson s "$3" '. + [{key: $k, value: $v, secret: $s}]' <<<"$env_json"); }
[[ -n "${RESEND_API_KEY:-}" ]] && add_env OPENTRAIL_EMAIL_API_KEY "$RESEND_API_KEY" true
[[ -n "${RESEND_FROM:-}" ]] && add_env OPENTRAIL_EMAIL_FROM_ADDRESS "$RESEND_FROM" false

app=$(api POST "/projects/$project_id/services" "$(jq -n --arg s "$server_id" --arg n "$APP_NAME" --arg img "$IMAGE:$TAG" --argjson env "$env_json" '{
  name: $n, serverId: $s,
  deployment: {url: $img},
  network: {public: true, protocol: "http"},
  healthcheck: "/api/health",
  env: $env
}')")
app_id=$(jq -r .id <<<"$app")
domain=$(jq -r '.network.managedDomain // empty' <<<"$app")
if [[ -n "$domain" ]]; then
  api PUT "/projects/$project_id/services/$app_id/env/APP_URL" "$(jq -n --arg v "https://$domain" '{value: $v, secret: false}')" >/dev/null
  api POST "/projects/$project_id/services/$app_id/deploy" '{}' >/dev/null
fi

cat >&2 <<NEXT

OpenTrail is deploying. Next:
  1. Open https://${domain:-<the service domain>} once the deploy is green.
  2. Copy the setup code from the service's Logs in Sliplane and create the first admin.
  3. Enter your Resend API key and sender address, or skip email if everyone uses single sign-on.
  4. Check under Settings, System that the database user can create roles.
  5. With a custom domain, set APP_URL to it in the service's environment and redeploy.
NEXT
echo "https://${domain:-}"
