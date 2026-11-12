#!/usr/bin/env bash
# Daily archive pipeline on the home-archive server, driving the archive
# CLI (docs/operations/archive.md, "Daily pipeline") through one-off
# containers: replay, manifests, anchors, publish, verify.
#
#   archive-pipeline.sh daily   [YYYY-MM-DD]   (default: yesterday, UTC)
#   archive-pipeline.sh anchors [DAYS]         (upgrade anchors for the last DAYS days, default 3)
#
# Timers: bleh-archive-daily.timer (01:15 UTC, after hour 00 of the next day
# is sealed) and bleh-archive-anchors.timer (hourly). Results go to the
# textfile collector as bleh_archive_* metrics, which the alert rules read.
#
# Environment (from /etc/bleh/archive.env; defaults in brackets):
#   ARCHIVE_MIRROR_DIR   host path of the R2 mirror [/srv/archive]
#   ARCHIVE_SCRATCH_DIR  host path of the work area, mounted at /work [/srv/scratch/archive]
#   ARCHIVE_WEB_SEED     torrent web seed base [https://data.bleh.xyz/]
#   ARCHIVE_DATA_URL     public base for verification [https://data.bleh.xyz]
#   ARCHIVE_TRUSTED_SIGNER  manifest signer address to verify against (required for verify)
#   ARCHIVE_ICEBERG_LOCATION  [s3://bleh-archive/hyperliquid-normalized]
#   ARCHIVE_EVM_CHAIN_ID  L2 chain id; empty skips L2 anchoring (L2 not decided yet)
#   ARCHIVE_BITCOIN_API  [https://blockstream.info/api]
#   ARCHIVE_PROFILE      compose profile that runs the jobs [home-archive]
#
# Replay's instrument registry is deploy/compose/.env ARCHIVE_REGISTRY (a
# path inside the archive-job container); empty means the origin's registry
# from the mirror, $ARCHIVE_MIRROR_DIR/_registry/hyperliquid/registry.json,
# which registry-sync on the node box keeps in the public bucket.
#   TEXTFILE_DIR         [/var/lib/node_exporter/textfile]
set -uo pipefail

mode=${1:?usage: archive-pipeline.sh daily [DATE] | anchors [DAYS]}
mirror=${ARCHIVE_MIRROR_DIR:-/srv/archive}
scratch=${ARCHIVE_SCRATCH_DIR:-/srv/scratch/archive}
web_seed=${ARCHIVE_WEB_SEED:-https://data.bleh.xyz/}
data_url=${ARCHIVE_DATA_URL:-https://data.bleh.xyz}
iceberg=${ARCHIVE_ICEBERG_LOCATION:-s3://bleh-archive/hyperliquid-normalized}
chain_id=${ARCHIVE_EVM_CHAIN_ID:-}
btc_api=${ARCHIVE_BITCOIN_API:-https://blockstream.info/api}
profile=${ARCHIVE_PROFILE:-home-archive}
textfile_dir=${TEXTFILE_DIR:-/var/lib/node_exporter/textfile}
state_dir=${STATE_DIR:-/var/lib/bleh/archive-pipeline}
up=/opt/bleh/src/deploy/up.sh
archives=(hyperliquid-raw hyperliquid-normalized)

log() { printf 'archive-pipeline: %s\n' "$*" >&2; }
# Run the archive CLI in a one-off container (service archive-job).
archive() { "$up" --no-build "$profile" archive "$@" --log-format json; }

metrics=()
ok_all=1
step() { # step NAME ARCHIVE COMMAND...: run, record bleh_archive_step_ok
  local name=$1 a=$2; shift 2
  if "$@"; then
    metrics+=("bleh_archive_step_ok{step=\"$name\",archive=\"$a\"} 1")
    return 0
  fi
  log "step $name failed for $a"
  metrics+=("bleh_archive_step_ok{step=\"$name\",archive=\"$a\"} 0")
  ok_all=0
  return 1
}
# Last-success timestamps persist across runs, so an absent success keeps
# the old value (and the overdue alerts fire) instead of vanishing.
remember() { # remember SERIES VALUE
  mkdir -p "$state_dir"
  printf '%s %s\n' "$1" "$2" > "$state_dir/$(printf '%s' "$1" | tr -c 'A-Za-z0-9_' '_')"
}
write_metrics() { # write_metrics FILE
  mkdir -p "$textfile_dir"
  {
    echo "# HELP bleh_archive_step_ok 1 if the last run of a pipeline step succeeded."
    echo "# TYPE bleh_archive_step_ok gauge"
    printf '%s\n' "${metrics[@]+"${metrics[@]}"}"
  } > "$textfile_dir/.$1.tmp"
  mv -f "$textfile_dir/.$1.tmp" "$textfile_dir/$1"
  if [[ -d "$state_dir" ]]; then
    cat "$state_dir"/* > "$textfile_dir/.bleh_archive_last.prom.tmp" 2>/dev/null
    mv -f "$textfile_dir/.bleh_archive_last.prom.tmp" "$textfile_dir/bleh_archive_last.prom"
  fi
}
now=$(date -u +%s)

case "$mode" in
  daily)
    day=${2:-$(date -u -d yesterday +%F)}
    next=$(date -u -d "$day + 1 day" +%F)
    log "day $day"
    # Work on a copy of the day's filtered raw (plus hour 00 of the next
    # day, which replay reads for blocks just before midnight); the mirror
    # stays a read-only copy of R2.
    mkdir -p "$scratch/hyperliquid-raw"
    for s in "$mirror"/hyperliquid-raw/stream=*; do
      [[ -d "$s" ]] || continue
      st=${s##*/}
      mkdir -p "$scratch/hyperliquid-raw/$st/date=$day" "$scratch/hyperliquid-raw/$st/date=$next"
      cp -a "$s/date=$day/." "$scratch/hyperliquid-raw/$st/date=$day/" 2>/dev/null || true
      cp -a "$s/date=$next/00.jsonl.zst" "$scratch/hyperliquid-raw/$st/date=$next/" 2>/dev/null || true
    done
    if [[ -d "$mirror/hyperliquid-raw/_manifests" ]]; then
      mkdir -p "$scratch/hyperliquid-raw/_manifests"
      cp -an "$mirror/hyperliquid-raw/_manifests/." "$scratch/hyperliquid-raw/_manifests/"
    fi

    # Replay reads the origin's instrument registry (BLEH_ARCHIVE_REGISTRY in
    # the archive-job service; default: the copy registry-sync puts in the
    # public bucket, which archive-mirror brings here).
    registry_override=$(sed -n 's/^ARCHIVE_REGISTRY=//p' /opt/bleh/src/deploy/compose/.env 2>/dev/null | tail -n 1)
    if [[ -z "$registry_override" && ! -s "$mirror/_registry/hyperliquid/registry.json" ]]; then
      log "no registry at $mirror/_registry/hyperliquid/registry.json (registry-sync on the node box, archive-mirror here); replay will fail"
    fi
    step replay hyperliquid-normalized \
      archive replay --raw /work/hyperliquid-raw --date "$day" --out /work/hyperliquid-normalized
    for a in "${archives[@]}"; do
      step manifest "$a" archive manifest build --root "/work/$a" --archive "$a" --date "$day" \
        --web-seed "$web_seed" --sign || continue
      remember "bleh_archive_manifest_last_success_timestamp_seconds{archive=\"$a\"}" "$now"
      step ots-stamp "$a" archive anchor ots-stamp --root "/work/$a" --date "$day"
      if [[ -n "$chain_id" ]]; then
        step evm-submit "$a" archive anchor evm-submit --root "/work/$a" --date "$day" \
          --chain-id "$chain_id" --rpc "${BLEH_ARCHIVE_EVM_RPC:-}" --key-file /run/secrets/anchor_key
      fi
      extra=()
      [[ "$a" == hyperliquid-normalized ]] && extra=(--iceberg-location "$iceberg")
      step publish "$a" archive publish --root "/work/$a" --archive "$a" --date "$day" \
        --upload --kubo http://ipfs:5001 "${extra[@]+"${extra[@]}"}"
      if [[ -n "${ARCHIVE_TRUSTED_SIGNER:-}" ]]; then
        step verify "$a" archive verify --url "$data_url/$a" --archive "$a" --date "$day" \
          --trusted "$ARCHIVE_TRUSTED_SIGNER" --bitcoin-api "$btc_api"
      fi
    done
    ((ok_all)) && remember bleh_archive_daily_last_success_timestamp_seconds "$now"
    remember bleh_archive_daily_last_run_timestamp_seconds "$now"
    write_metrics bleh_archive_daily.prom
    ;;

  anchors)
    days=${2:-3}
    for i in $(seq 1 "$days"); do
      day=$(date -u -d "$i days ago" +%F)
      for a in "${archives[@]}"; do
        [[ -d "$scratch/$a/_manifests/$day" ]] || continue
        if step ots-upgrade "$a" archive anchor ots-upgrade --root "/work/$a" --date "$day" --bitcoin-api "$btc_api"; then
          remember "bleh_archive_anchor_last_success_timestamp_seconds{archive=\"$a\",anchor=\"ots\"}" "$now"
        fi
        if [[ -n "$chain_id" ]] && step evm-poll "$a" archive anchor evm-poll --root "/work/$a" --date "$day" \
            --chain-id "$chain_id" --rpc "${BLEH_ARCHIVE_EVM_RPC:-}" --key-file /run/secrets/anchor_key; then
          remember "bleh_archive_anchor_last_success_timestamp_seconds{archive=\"$a\",anchor=\"l2\"}" "$now"
        fi
        # Re-publish so upgraded proofs reach every channel (idempotent).
        step republish "$a" archive publish --root "/work/$a" --archive "$a" --date "$day" --upload --kubo http://ipfs:5001
      done
    done
    write_metrics bleh_archive_anchors.prom
    ;;
  *) echo "usage: archive-pipeline.sh daily [DATE] | anchors [DAYS]" >&2; exit 64 ;;
esac

((ok_all)) || exit 1
