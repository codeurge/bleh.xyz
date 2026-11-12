#!/usr/bin/env bash
# External availability probe (docs/requirements.md, "Availability"):
# connect, subscribe to one instrument, and require a block heartbeat within
# 5 seconds. Run once a minute from a host outside both providers
# (deploy/systemd/bleh-probe.timer). Results are written in Prometheus text
# format for node_exporter's textfile collector.
#
#   ws-probe.sh [--once-stdout]
#
# Environment (defaults in brackets):
#   PROBE_URL         websocket URL [wss://live.bleh.xyz/v0/ws]
#   PROBE_TARGET      target label [live]
#   PROBE_INSTRUMENT  instrument to subscribe to [hyperliquid:perp:BTC]
#   PROBE_KIND        channel kind [trade]
#   PROBE_TIMEOUT     seconds allowed from connect to first block [5]
#   PROBE_CLIENT      websocat | mesh-client [websocat]
#   TEXTFILE_DIR      node_exporter textfile directory
#                     [/var/lib/node_exporter/textfile]
#
# With PROBE_CLIENT=mesh-client the reference client does the work
# (`mesh-client probe`, provided by the gateway/client lane) and prints one
# JSON line {"ok":bool,"first_block_seconds":f,"block_time_ms":n}.
set -uo pipefail

url=${PROBE_URL:-wss://live.bleh.xyz/v0/ws}
target=${PROBE_TARGET:-live}
instrument=${PROBE_INSTRUMENT:-hyperliquid:perp:BTC}
kind=${PROBE_KIND:-trade}
timeout_s=${PROBE_TIMEOUT:-5}
client=${PROBE_CLIENT:-websocat}
textfile_dir=${TEXTFILE_DIR:-/var/lib/node_exporter/textfile}
stdout_only=0
[[ "${1:-}" == "--once-stdout" ]] && stdout_only=1

now_ms() { date +%s%3N; }

ok=0
first_block_s=NaN
block_age_s=NaN
error=""
start=$(now_ms)

probe_websocat() {
  command -v websocat >/dev/null || { error="websocat not installed"; return; }
  command -v jq >/dev/null || { error="jq not installed"; return; }
  local sub result type line arrived bt
  sub=$(jq -cn --arg i "$instrument" --arg k "$kind" \
    '{op:"subscribe", id:"probe", channels:[{instrument:$i, kind:$k}], commitments:false}')
  # stdin stays open (sleep) so websocat keeps the connection; timeout caps
  # the whole exchange. The loop prints "<arrival_ms> <message>" for the
  # first block or error message and stops.
  result=$(
    { printf '%s\n' "$sub"; sleep "$timeout_s"; } \
      | timeout "$timeout_s" websocat --text --protocol bleh.v0.json "$url" 2>/dev/null \
      | while IFS= read -r line; do
          type=$(jq -r '.type // empty' <<<"$line" 2>/dev/null)
          if [[ "$type" == block || "$type" == error ]]; then
            printf '%s %s\n' "$(now_ms)" "$line"
            break
          fi
        done
  )
  if [[ -z "$result" ]]; then
    error="no block within ${timeout_s}s"
    return
  fi
  arrived=${result%% *}
  line=${result#* }
  type=$(jq -r '.type // empty' <<<"$line")
  if [[ "$type" == error ]]; then
    error="server error: $(jq -r '.code // "unknown"' <<<"$line")"
    return
  fi
  ok=1
  first_block_s=$(awk -v a="$start" -v b="$arrived" 'BEGIN{printf "%.3f", (b-a)/1000}')
  bt=$(jq -r '.block_time_ms // empty' <<<"$line")
  if [[ -n "$bt" ]]; then
    block_age_s=$(awk -v a="$bt" -v b="$arrived" 'BEGIN{printf "%.3f", (b-a)/1000}')
  fi
}

probe_mesh_client() {
  command -v mesh-client >/dev/null || { error="mesh-client not installed"; return; }
  local out
  if out=$(mesh-client probe --url "$url" --instrument "$instrument" --kind "$kind" \
      --timeout "${timeout_s}s" 2>/dev/null); then
    ok=$(jq -r 'if .ok then 1 else 0 end' <<<"$out")
    first_block_s=$(jq -r '.first_block_seconds // "NaN"' <<<"$out")
    local bt
    bt=$(jq -r '.block_time_ms // empty' <<<"$out")
    if [[ -n "$bt" ]]; then
      block_age_s=$(awk -v a="$bt" -v b="$(now_ms)" 'BEGIN{printf "%.3f", (b-a)/1000}')
    fi
  else
    error="mesh-client probe failed"
  fi
}

case "$client" in
  websocat) probe_websocat ;;
  mesh-client) probe_mesh_client ;;
  *) error="unknown PROBE_CLIENT $client" ;;
esac

duration_s=$(awk -v a="$start" -v b="$(now_ms)" 'BEGIN{printf "%.3f", (b-a)/1000}')
labels="target=\"${target}\""
metrics=$(cat <<EOF
# HELP bleh_probe_success 1 if a block heartbeat arrived within the timeout.
# TYPE bleh_probe_success gauge
bleh_probe_success{${labels}} ${ok}
# HELP bleh_probe_first_block_seconds Seconds from connect to the first block heartbeat.
# TYPE bleh_probe_first_block_seconds gauge
bleh_probe_first_block_seconds{${labels}} ${first_block_s}
# HELP bleh_probe_block_age_seconds Wall clock minus block_time of the first heartbeat.
# TYPE bleh_probe_block_age_seconds gauge
bleh_probe_block_age_seconds{${labels}} ${block_age_s}
# HELP bleh_probe_duration_seconds Total probe run time.
# TYPE bleh_probe_duration_seconds gauge
bleh_probe_duration_seconds{${labels}} ${duration_s}
# HELP bleh_probe_last_run_timestamp_seconds Unix time of the last probe run.
# TYPE bleh_probe_last_run_timestamp_seconds gauge
bleh_probe_last_run_timestamp_seconds{${labels}} $(date +%s)
EOF
)

if ((stdout_only)); then
  printf '%s\n' "$metrics"
else
  mkdir -p "$textfile_dir"
  tmp=$(mktemp "${textfile_dir}/.bleh_probe_${target}.XXXXXX")
  printf '%s\n' "$metrics" > "$tmp"
  chmod 0644 "$tmp"
  mv -f "$tmp" "${textfile_dir}/bleh_probe_${target}.prom"
fi

if ((ok)); then
  echo "probe ${target}: ok, first block after ${first_block_s}s, block age ${block_age_s}s"
else
  echo "probe ${target}: FAILED: ${error}" >&2
fi
exit 0
