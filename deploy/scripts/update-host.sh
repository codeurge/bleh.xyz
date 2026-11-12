#!/usr/bin/env bash
# Move this host to an exact commit and its published images, with
# automatic rollback if the role does not come back healthy.
#
#   sudo deploy/scripts/update-host.sh PROFILE SHA [REGISTRY]
#
# PROFILE   node-box | home-archive | relay | origin
# SHA       full commit SHA; images ${REGISTRY}/<service>:${SHA} must exist
#           (built and signed by the deploy workflow)
# REGISTRY  default ghcr.io/codeurge/bleh.xyz
#
# Steps: fetch and check out SHA (detached), point .env at the images,
# pull, restart the profile, wait for health, else restore the previous
# commit and tag and restart again. Called by the deploy workflow's promote
# job over SSH, or by hand (the home-archive server behind NAT).
set -euo pipefail

profile=${1:?profile}
sha=${2:?sha}
registry=${3:-ghcr.io/codeurge/bleh.xyz}
repo=/opt/bleh/src
env_file=$repo/deploy/compose/.env
[[ "$sha" =~ ^[0-9a-f]{40}$ ]] || { echo "SHA must be a full commit SHA" >&2; exit 64; }

get_env() { sed -n "s/^$1=//p" "$env_file" | tail -n 1; }
set_env() { sed -i "s|^$1=.*|$1=$2|" "$env_file"; grep -q "^$1=" "$env_file" || echo "$1=$2" >> "$env_file"; }

prev_sha=$(git -C "$repo" rev-parse HEAD)
prev_tag=$(get_env BLEH_TAG)
prev_registry=$(get_env BLEH_REGISTRY)

healthy() {
  local ip url
  ip=$(get_env BLEH_OVERLAY_IP); ip=${ip:-127.0.0.1}
  case "$profile" in
    # The reader's health (the origin gateway runs inside it once wired).
    node-box|origin) url="http://${ip}:9184/health" ;;
    relay) url="http://${ip}:8080/health" ;;
    home-archive) url="http://${ip}:8081/health" ;;
  esac
  for _ in $(seq 1 60); do
    curl -fsS --max-time 3 "$url" >/dev/null 2>&1 && return 0
    sleep 5
  done
  return 1
}

switch_to() { # switch_to SHA REGISTRY TAG
  git -C "$repo" fetch --quiet origin
  git -C "$repo" checkout --quiet --detach "$1"
  set_env BLEH_REGISTRY "$2"
  set_env BLEH_TAG "$3"
  "$repo/deploy/up.sh" --no-build "$profile" pull
  "$repo/deploy/up.sh" --no-build "$profile" up
}

echo "update-host: $profile ${prev_sha:0:12} -> ${sha:0:12}"
if switch_to "$sha" "$registry" "$sha" && healthy; then
  echo "update-host: healthy at ${sha:0:12}"
  exit 0
fi

echo "update-host: not healthy; rolling back to ${prev_sha:0:12} (${prev_registry}:${prev_tag})" >&2
switch_to "$prev_sha" "$prev_registry" "$prev_tag" || true
healthy && echo "update-host: rolled back" >&2
exit 1
