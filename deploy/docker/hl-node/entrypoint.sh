#!/usr/bin/env bash
# Entrypoint for the bleh hl-node image (also used by deploy/scripts/node-install.sh
# as the reference for the native install). Writes visor.json and
# override_gossip_config.json, checks the output flags against the set the
# mesh is admitted to run (ADM-0001, ADR 0006), then runs hl-visor.
#
# Environment:
#   HL_CHAIN               Mainnet (default) or Testnet
#   HL_NODE_FLAGS          output flags; default is exactly the mesh set
#   HL_ROOT_NODE_IPS       comma-separated gossip root IPs; if empty, the
#                          current list is fetched with gossipRootIps
#   HL_TRY_NEW_PEERS       true|false (default false, as in the node README)
#   HL_RESERVED_PEER_IPS   comma-separated reserved peer IPs (default none)
#   HL_ALLOW_FLAG_OVERRIDE set to 1 to run flags other than the mesh set
#                          (never in production)
set -euo pipefail

readonly MESH_FLAGS="--write-fills --write-order-statuses --write-raw-book-diffs --batch-by-block --disable-output-file-buffering --serve-info"
# Streams the admission excludes. They must never be written to disk.
readonly FORBIDDEN_FLAGS="--write-misc-events --write-hip3-oracle-updates --write-trades --write-system-and-core-writer-actions"

chain=${HL_CHAIN:-Mainnet}
flags=${HL_NODE_FLAGS:-$MESH_FLAGS}
home_dir=${HOME:-/home/hluser}
cd "$home_dir"

log() { printf 'hl-node-entrypoint: %s\n' "$*" >&2; }

case "$chain" in
  Mainnet) info_url="https://api.hyperliquid.xyz/info" ;;
  Testnet) info_url="https://api.hyperliquid-testnet.xyz/info" ;;
  *) log "HL_CHAIN must be Mainnet or Testnet, got '$chain'"; exit 64 ;;
esac

for f in $flags; do
  for bad in $FORBIDDEN_FLAGS; do
    if [[ "$f" == "$bad" ]]; then
      log "refusing to run with $f: that stream is excluded by ADM-0001"
      exit 64
    fi
  done
done
if [[ "${HL_ALLOW_FLAG_OVERRIDE:-0}" != 1 ]]; then
  sorted_want=$(tr ' ' '\n' <<<"$MESH_FLAGS" | sort | xargs)
  sorted_got=$(tr ' ' '\n' <<<"$flags" | sed '/^$/d' | sort | xargs)
  if [[ "$sorted_want" != "$sorted_got" ]]; then
    log "flags must be exactly: $MESH_FLAGS"
    log "got: $flags (set HL_ALLOW_FLAG_OVERRIDE=1 for local experiments only)"
    exit 64
  fi
fi

printf '{"chain": "%s"}\n' "$chain" > visor.json

# Gossip configuration. Mainnet needs at least one root IP.
to_ip_objects() { jq -Rc 'split(",") | map(select(length > 0) | gsub("\\s"; "")) | map({Ip: .})'; }
if [[ -n "${HL_ROOT_NODE_IPS:-}" ]]; then
  roots=$(printf '%s' "$HL_ROOT_NODE_IPS" | to_ip_objects)
else
  log "fetching gossip root IPs from $info_url"
  if fetched=$(curl -fsS --max-time 20 --retry 3 -X POST -H 'Content-Type: application/json' \
      --data '{"type":"gossipRootIps"}' "$info_url"); then
    roots=$(jq -c 'map({Ip: .})' <<<"$fetched")
  else
    roots="[]"
  fi
fi
if [[ "$roots" == "[]" ]]; then
  if [[ -s override_gossip_config.json ]]; then
    log "no root IPs available; keeping the existing override_gossip_config.json"
  elif [[ "$chain" == Mainnet ]]; then
    log "no gossip root IPs: set HL_ROOT_NODE_IPS"
    exit 69
  fi
else
  reserved=$(printf '%s' "${HL_RESERVED_PEER_IPS:-}" | to_ip_objects)
  jq -n --argjson roots "$roots" --argjson reserved "$reserved" \
     --arg chain "$chain" --argjson try "${HL_TRY_NEW_PEERS:-false}" \
     '{root_node_ips: $roots, try_new_peers: $try, chain: $chain, reserved_peer_ips: $reserved}' \
     > override_gossip_config.json
  log "gossip roots: $(jq -c '[.root_node_ips[].Ip]' override_gossip_config.json)"
fi

log "starting hl-visor run-non-validator $flags"
# shellcheck disable=SC2086 # flags are intentionally word-split
exec ./hl-visor run-non-validator $flags
