#!/usr/bin/env bash
# Daily archive pipeline on the home-archive server, driving the archive
# CLI (docs/operations/archive.md, "Daily pipeline") through one-off
# containers: replay, manifests, anchors, publish, verify.
#
#   archive-pipeline.sh daily   [YYYY-MM-DD]   (default: yesterday, UTC)
#   archive-pipeline.sh anchors [DAYS]         (upgrade anchors for the last DAYS days, default 3)
#   archive-pipeline.sh records ARCHIVE        (sign and publish settled days of a
#                                               records archive, ADR 0017)
#   archive-pipeline.sh records-anchors ARCHIVE  (upgrade every pending Bitcoin
#                                               proof and republish those days)
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
#   ARCHIVE_RUN          command that runs the archive CLI in the archive-job
#                        container [$up --no-build $ARCHIVE_PROFILE archive]
#   ARCHIVE_KUBO         Kubo RPC API passed to publish [http://ipfs:5001]; the
#                        archive-job container also sets BLEH_ARCHIVE_KUBO_API
#
# Records archives (records mode; partition-dir writes them, ADR 0017):
#   LOCAL_NORMALIZED_DIR host directory holding <archive>/ roots, mounted in
#                        archive-job at /srv/normalized [/srv/normalized]
#   RECORDS_LAG_DAYS     a day is signed once it is this many days old, so every
#                        record file that can hold it is partitioned [14]
#   RECORDS_NOTICES      host path of the pollers' _notices.json, copied to the
#                        archive root before signing (empty: none) []
#   RECORDS_THROUGH      last day that may be signed (YYYY-MM-DD), for an archive
#                        a batch job is still filling: a signed day is sealed,
#                        so a day still being written must wait (empty: no cap) []
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
normalized_dir=${LOCAL_NORMALIZED_DIR:-/srv/normalized}
kubo=${ARCHIVE_KUBO-http://ipfs:5001}
archives=(hyperliquid-raw hyperliquid-normalized)

log() { printf 'archive-pipeline: %s\n' "$*" >&2; }
# Run the archive CLI in a one-off container (service archive-job).
archive() {
  if [[ -n "${ARCHIVE_RUN:-}" ]]; then
    # shellcheck disable=SC2086 # ARCHIVE_RUN is a command line.
    $ARCHIVE_RUN "$@" --log-format json
  else
    "$up" --no-build "$profile" archive "$@" --log-format json
  fi
}
kubo_args=()
[[ -n "$kubo" ]] && kubo_args=(--kubo "$kubo")

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
        --upload "${kubo_args[@]+"${kubo_args[@]}"}" "${extra[@]+"${extra[@]}"}"
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
        step republish "$a" archive publish --root "/work/$a" --archive "$a" --date "$day" --upload "${kubo_args[@]+"${kubo_args[@]}"}"
      done
    done
    write_metrics bleh_archive_anchors.prom
    ;;
  records)
    # A records archive is partitioned in place by partition-dir under
    # $normalized_dir/<archive>; sign each settled day after the last
    # manifest, oldest first (a chain's dates only increase).
    a=${2:?usage: archive-pipeline.sh records ARCHIVE}
    host_root="$normalized_dir/$a"
    root="/srv/normalized/$a"
    [[ -d "$host_root" ]] || { log "no archive at $host_root"; exit 1; }
    if [[ -n "${RECORDS_NOTICES:-}" && -s "$RECORDS_NOTICES" ]] \
        && ! cmp -s "$RECORDS_NOTICES" "$host_root/_notices.json"; then
      cp "$RECORDS_NOTICES" "$host_root/_notices.json"
      log "notices updated from $RECORDS_NOTICES"
    fi
    last=$(find "$host_root/_manifests" -mindepth 1 -maxdepth 1 -type d -name '????-??-??' 2>/dev/null \
      | sed 's#.*/##' | sort | tail -n 1)
    cutoff=$(date -u -d "${RECORDS_LAG_DAYS:-14} days ago" +%F)
    if [[ -n "${RECORDS_THROUGH:-}" ]]; then
      [[ "$RECORDS_THROUGH" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] \
        || { log "RECORDS_THROUGH is not a date: $RECORDS_THROUGH"; exit 1; }
      [[ "$RECORDS_THROUGH" < "$cutoff" ]] && cutoff=$RECORDS_THROUGH
      log "$a: signing through $cutoff"
    fi
    mapfile -t days < <(find "$host_root" -path "$host_root/_*" -prune -o -type d -name 'date=*' -print \
      | sed 's#.*/date=##' | sort -u)
    # publish_day DAY: upload, verify and mark a signed day. A day signed
    # but not yet published (an earlier run failed) is retried first.
    published_dir="$state_dir/published/$a"
    mkdir -p "$published_dir"
    publish_day() {
      step publish "$a" archive publish --root "$root" --archive "$a" --date "$1" --upload \
        "${kubo_args[@]+"${kubo_args[@]}"}" --iceberg-location "s3://${BLEH_PUBLIC_BUCKET:-bleh-archive}/$a" || return 1
      if [[ -n "${ARCHIVE_TRUSTED_SIGNER:-}" ]]; then
        step verify "$a" archive verify --url "$data_url/$a" --archive "$a" --date "$1" \
          --trusted "$ARCHIVE_TRUSTED_SIGNER" --bitcoin-api "$btc_api" || return 1
      fi
      : > "$published_dir/$1"
    }
    while read -r day; do
      [[ -z "$day" || -e "$published_dir/$day" ]] && continue
      log "$a day $day: signed earlier, publishing"
      publish_day "$day" || { write_metrics "bleh_archive_records_${a//-/_}.prom"; exit 1; }
    done < <(find "$host_root/_manifests" -mindepth 1 -maxdepth 1 -type d -name '????-??-??' 2>/dev/null \
      | sed 's#.*/##' | sort)
    signed=0
    for day in "${days[@]}"; do
      [[ -n "$last" && ! "$day" > "$last" ]] && continue
      [[ "$day" > "$cutoff" ]] && break
      log "$a day $day"
      step manifest "$a" archive manifest build --root "$root" --archive "$a" --date "$day" \
        --web-seed "$web_seed" --sign || break
      remember "bleh_archive_manifest_last_success_timestamp_seconds{archive=\"$a\"}" "$now"
      step ots-stamp "$a" archive anchor ots-stamp --root "$root" --date "$day"
      publish_day "$day" || break
      signed=$((signed + 1))
    done
    log "$a: $signed day(s) signed and published"
    ((ok_all)) && remember "bleh_archive_records_last_success_timestamp_seconds{archive=\"$a\"}" "$now"
    write_metrics "bleh_archive_records_${a//-/_}.prom"
    ;;

  records-anchors)
    # Upgrade every signed day whose Bitcoin proof is not confirmed yet,
    # oldest first, and republish the days that became confirmed. Picking
    # only the most recent days left most of a catch-up (many days signed in
    # one run) pending for good, and verify fails a day still pending 48
    # hours after signing (manifest-v0 § 6.3). A third argument (the old DAYS
    # window) is accepted and ignored.
    a=${2:?usage: archive-pipeline.sh records-anchors ARCHIVE}
    host_root="$normalized_dir/$a"
    root="/srv/normalized/$a"
    command -v jq >/dev/null || { log "records-anchors needs jq"; exit 1; }
    ots_status() { # ots_status DAY: the day's OpenTimestamps status ("" if none)
      jq -r '.opentimestamps.status // ""' "$host_root/_manifests/$1/manifest.anchors.json" 2>/dev/null
    }
    upgraded=0 still=0
    while read -r day; do
      [[ -z "$day" || "$(ots_status "$day")" == confirmed ]] && continue
      if step ots-upgrade "$a" archive anchor ots-upgrade --root "$root" --date "$day" --bitcoin-api "$btc_api"; then
        remember "bleh_archive_anchor_last_success_timestamp_seconds{archive=\"$a\",anchor=\"ots\"}" "$now"
      fi
      if [[ "$(ots_status "$day")" != confirmed ]]; then
        still=$((still + 1))
        continue
      fi
      # Republish so the upgraded proof reaches every channel (idempotent).
      step republish "$a" archive publish --root "$root" --archive "$a" --date "$day" --upload \
        "${kubo_args[@]+"${kubo_args[@]}"}" && upgraded=$((upgraded + 1))
    done < <(find "$host_root/_manifests" -mindepth 1 -maxdepth 1 -type d -name '????-??-??' 2>/dev/null \
      | sed 's#.*/##' | sort)
    log "$a: $upgraded day(s) confirmed and republished, $still still pending"
    write_metrics "bleh_archive_records_anchors_${a//-/_}.prom"
    ;;

  *) echo "usage: archive-pipeline.sh daily [DATE] | anchors [DAYS] | records ARCHIVE | records-anchors ARCHIVE" >&2; exit 64 ;;
esac

((ok_all)) || exit 1
