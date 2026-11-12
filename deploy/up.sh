#!/usr/bin/env bash
# One command to bring up a bleh.xyz host role with docker compose.
#
#   deploy/up.sh <profile> [command] [args...]
#
# Profiles (deploy/compose/compose.yaml; ADR 0010 for the production ones):
#   synthetic     laptop stack, no node (demo gateway, fixture replay, MinIO)
#   node-box      rented node box: Hyperliquid node, reader, gateway, capture
#   home-archive  self-hosted archive, seed and processing server
#   relay         public relay VM
#   origin        node-box + home-archive on one box that can run the node
#   node          alias for node-box
#
# Commands (default: up):
#   up            build images if needed, then start the profile detached
#   down          stop the profile (volumes kept)
#   ps | logs | config | pull | restart   passed to docker compose
#   backfill ARGS run a one-off hl-backfill job with ARGS
#
# Options (before the profile):
#   --no-build    do not build images (use BLEH_REGISTRY/BLEH_TAG from .env)
#
# First run creates deploy/compose/.env from env.example.conf and, for the
# synthetic profile only, throwaway test keys. Production keys are never
# generated here; see docs/operations/keys.md.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
compose_dir="$here/compose"
env_file="$compose_dir/.env"
secrets="$compose_dir/secrets"
build=1

die() { printf 'up.sh: %s\n' "$*" >&2; exit 1; }
note() { printf 'up.sh: %s\n' "$*" >&2; }

while [[ "${1:-}" == --* ]]; do
  case "$1" in
    --no-build) build=0; shift ;;
    -h|--help) sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown option $1" ;;
  esac
done

profile=${1:-}
[[ -n "$profile" ]] || { sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'; exit 64; }
shift
cmd=${1:-up}
[[ $# -gt 0 ]] && shift

case "$profile" in
  node) profile=node-box ;;
  synthetic|node-box|home-archive|relay|origin) ;;
  *) die "unknown profile '$profile'" ;;
esac

profiles=("$profile")
[[ "$profile" == origin ]] && profiles=(node-box home-archive)

command -v docker >/dev/null || die "docker is not installed"
docker compose version >/dev/null 2>&1 || die "docker compose v2 is required"

# ---------------------------------------------------------------- .env
umask 077
if [[ ! -f "$env_file" ]]; then
  cp "$compose_dir/env.example.conf" "$env_file"
  pw=$(openssl rand -hex 24)
  sed -i.bak "s/^S3_SECRET_ACCESS_KEY=__GENERATED__$/S3_SECRET_ACCESS_KEY=${pw}/" "$env_file"
  rm -f "$env_file.bak"
  note "created $env_file; review it before production use"
fi
chmod 600 "$env_file"

set_env() { # set_env KEY VALUE: replace or append in .env
  if grep -q "^$1=" "$env_file"; then
    sed -i.bak "s|^$1=.*|$1=$2|" "$env_file" && rm -f "$env_file.bak"
  else
    printf '%s=%s\n' "$1" "$2" >> "$env_file"
  fi
}
get_env() { sed -n "s/^$1=//p" "$env_file" | tail -n 1; }

set_env BLEH_PROFILE "${profiles[0]}"
if [[ "$profile" == home-archive || "$profile" == origin ]]; then
  if [[ "$(get_env PROMETHEUS_CONFIG)" == *prometheus-synthetic.yml ]]; then
    set_env PROMETHEUS_CONFIG ../observability/prometheus/prometheus.yml
  fi
fi
if [[ "$profile" == relay ]]; then
  # Trust X-Forwarded-For only from Cloudflare's published ranges.
  if ranges=$(curl -fsS --max-time 10 https://www.cloudflare.com/ips-v4 \
      && printf '\n' && curl -fsS --max-time 10 https://www.cloudflare.com/ips-v6); then
    set_env CADDY_TRUSTED_PROXIES "$(tr '\n' ' ' <<<"$ranges" | xargs)"
  else
    note "could not fetch Cloudflare ranges; CADDY_TRUSTED_PROXIES unchanged"
  fi
fi

# ---------------------------------------------------------------- secrets
mkdir -p "$secrets/alertmanager" "$secrets/origin-ca"
chmod 700 "$secrets"

gen_test_key() { # 32-byte secp256k1 secret as hex, for synthetic use only
  openssl ecparam -name secp256k1 -genkey -noout -outform DER 2>/dev/null \
    | tail -c +8 | head -c 32 | od -An -tx1 | tr -d ' \n'
}
need_secret() { # need_secret FILE DESCRIPTION
  [[ -s "$secrets/$1" ]] || die "missing $secrets/$1 ($2); see docs/operations/keys.md"
}
placeholder() { [[ -e "$secrets/$1" ]] || : > "$secrets/$1"; }

uses() { local p; for p in "${profiles[@]}"; do [[ "$p" == "$1" ]] && return 0; done; return 1; }

if uses synthetic; then
  [[ -s "$secrets/signing-key.hex" ]] || { gen_test_key > "$secrets/signing-key.hex"; note "generated a TEST block-signing key"; }
  [[ -s "$secrets/manifest-key.hex" ]] || { gen_test_key > "$secrets/manifest-key.hex"; note "generated a TEST manifest key"; }
fi
if uses node-box; then
  need_secret signing-key.hex "block-signing key for this ingest path"
  need_secret tunnel-token.txt "Cloudflare Tunnel token for origin.bleh.xyz"
  [[ -n "$(get_env HL_DATA_DIR)" ]] || note "HL_DATA_DIR is empty: node data goes to a Docker volume, not /srv/hl (run deploy/scripts/host-bootstrap.sh)"
  [[ "$(get_env S3_ENDPOINT)" != http://minio:9000 ]] || die "set S3_ENDPOINT and R2 credentials in $env_file for node-box"
fi
if uses home-archive; then
  need_secret manifest-key.hex "manifest signing key"
  need_secret tunnel-token.txt "Cloudflare Tunnel token for the home-archive hostnames"
  for f in page_webhook_url ticket_webhook_url deadman_webhook_url; do
    [[ -s "$secrets/alertmanager/$f" ]] || note "alert receiver $f not set; notifications for it will fail"
  done
  [[ "$(get_env S3_ENDPOINT)" != http://minio:9000 ]] || die "set S3_ENDPOINT and R2 credentials in $env_file for home-archive"
fi
if uses relay; then
  need_secret upstream-headers.txt "Cloudflare Access service-token headers for the origin"
  [[ -n "$(get_env BLEH_SIGNERS)" ]] || note "BLEH_SIGNERS is empty: the relay cannot verify upstream signatures"
  [[ -s "$secrets/origin-ca/origin.pem" ]] || note "no Origin CA certificate in secrets/origin-ca: live.$(get_env BLEH_DOMAIN) will not serve TLS"
fi
[[ -s "$secrets/grafana-admin-password.txt" ]] || { openssl rand -base64 24 > "$secrets/grafana-admin-password.txt"; note "generated the Grafana admin password in secrets/"; }
# Compose wants every declared secret file to exist; unused ones stay empty.
for f in signing-key.hex manifest-key.hex anchor-key.hex tunnel-token.txt upstream-headers.txt \
         alertmanager/page_webhook_url alertmanager/ticket_webhook_url alertmanager/deadman_webhook_url; do
  placeholder "$f"
done

# ---------------------------------------------------------------- images
targets_for() {
  case "$1" in
    synthetic) echo "hl-node-reader gateway archive history-api" ;;
    node-box) echo "hl-node hl-node-reader gateway archive" ;;
    home-archive) echo "archive history-api swap-pollers hl-backfill" ;;
    relay) echo "gateway history-api" ;;
  esac
}
registry=$(get_env BLEH_REGISTRY)
if ((build)) && [[ "$cmd" == up || "$cmd" == backfill ]] && [[ "${registry:-bleh}" == bleh ]]; then
  docker info >/dev/null 2>&1 || die "the Docker daemon is not running"
  targets=()
  for p in "${profiles[@]}"; do
    read -r -a t <<<"$(targets_for "$p")"
    targets+=("${t[@]}")
  done
  read -r -a targets <<<"$(printf '%s\n' "${targets[@]}" | sort -u | xargs)"
  "$here/docker/build.sh" --registry bleh --also-tag "$(get_env BLEH_TAG)" "${targets[@]}"
fi

# ---------------------------------------------------------------- compose
dc=(docker compose --project-directory "$compose_dir" -f "$compose_dir/compose.yaml" --env-file "$env_file")
for p in "${profiles[@]}"; do dc+=(--profile "$p"); done

case "$cmd" in
  up)
    "${dc[@]}" up -d --remove-orphans "$@"
    if uses synthetic; then
      port=$(get_env CADDY_HTTPS_PORT)
      cat <<EOF
bleh synthetic stack is up:
  gateway   wss://live.localhost:${port}/v0/ws   (relay: wss://relay.localhost:${port}/v0/ws)
  history   https://api.localhost:${port}/
  archive   https://data.localhost:${port}/       (MinIO console: http://127.0.0.1:9001)
  grafana   https://grafana.localhost:${port}/    (admin password: deploy/compose/secrets/grafana-admin-password.txt)
  metrics   http://127.0.0.1:9090
EOF
    else
      "${dc[@]}" ps
    fi
    ;;
  backfill)
    "${dc[@]}" --profile backfill run --rm hl-backfill "$@"
    ;;
  down|ps|logs|config|pull|restart)
    "${dc[@]}" "$cmd" "$@"
    ;;
  *) die "unknown command '$cmd'" ;;
esac
