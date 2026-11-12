#!/usr/bin/env bash
# Measure TCP connect time from this host to Hyperliquid's gossip root
# peers, to compare candidate node locations ("verify by measuring" in the
# Phase 1 plan). Lower is better; the node streams blocks from these peers.
#
#   deploy/scripts/peer-latency.sh [Mainnet|Testnet]
set -euo pipefail
chain=${1:-Mainnet}
case "$chain" in
  Mainnet) url=https://api.hyperliquid.xyz/info ;;
  Testnet) url=https://api.hyperliquid-testnet.xyz/info ;;
  *) echo "chain must be Mainnet or Testnet" >&2; exit 64 ;;
esac

ips=$(curl -fsS --max-time 20 -X POST -H 'Content-Type: application/json' \
  --data '{"type":"gossipRootIps"}' "$url" | jq -r '.[]')
[[ -n "$ips" ]] || { echo "no root IPs returned" >&2; exit 1; }

printf '%-18s %10s %10s\n' peer best_ms median_ms
for ip in $ips; do
  times=()
  for _ in 1 2 3 4 5; do
    t=$(curl -s -o /dev/null --max-time 3 -w '%{time_connect}' "telnet://$ip:4001" </dev/null || true)
    [[ "$t" =~ ^[0-9.]+$ && "$t" != 0.000000 ]] && times+=("$(awk -v t="$t" 'BEGIN{printf "%.1f", t*1000}')")
  done
  if ((${#times[@]})); then
    sorted=$(printf '%s\n' "${times[@]}" | sort -n)
    best=$(head -n 1 <<<"$sorted")
    median=$(sed -n "$(( (${#times[@]} + 1) / 2 ))p" <<<"$sorted")
    printf '%-18s %10s %10s\n' "$ip" "$best" "$median"
  else
    printf '%-18s %10s %10s\n' "$ip" unreachable -
  fi
done | sort -k2,2n
