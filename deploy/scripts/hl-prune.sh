#!/usr/bin/env bash
# Prune Hyperliquid node output on the node box (hourly, bleh-prune.timer).
#
# Budget: the node writes about 100 GB/day at default settings and more
# with the mesh's output flags (measure at provisioning; the dashboard shows
# bytes/day). Output is data until the archive has captured it, so hourly
# stream files are deleted only when BOTH hold:
#   - older than RETAIN_HOURS, and
#   - at or before the capture watermark written by the origin's live
#     capture (or a standalone `archive capture run --follow`):
#     $BLEH_RAW_DIR/.captured-through holds the last UTC hour
#     (YYYY-MM-DDTHH) sealed in every stream and uploaded, filtered and
#     unfiltered, to object storage. BLEH_RAW_DIR here is the host path
#     mounted as the origin's BLEH_RAW_DIR (compose .env BLEH_RAW_DIR).
# Emergency: if free space on the node volume falls below EMERGENCY_FREE_PCT
# the oldest stream hours are deleted regardless, the count is exported
# (bleh_hl_prune_emergency_deleted_files) and the hours become flagged gaps.
# A full disk stops the node, which loses more.
#
# Other directories (never captured by the mesh):
#   replica_cmds, node_twap_statuses   older than OTHER_RETAIN_HOURS
#   periodic_abci_states               all but the newest ABCI_KEEP files
#   node_logs, visor_child_stderr      older than LOG_RETAIN_DAYS
#   fileSnapshot output                older than SNAPSHOT_RETAIN_HOURS
#
# Environment (defaults):
#   HL_DATA_DIR [/srv/hl/hl]  HL_SNAPSHOT_DIR [/srv/hl/snapshots]
#   BLEH_RAW_DIR [/srv/bleh/raw]  RETAIN_HOURS [24]  OTHER_RETAIN_HOURS [24]
#   ABCI_KEEP [3]  LOG_RETAIN_DAYS [7]  SNAPSHOT_RETAIN_HOURS [6]
#   EMERGENCY_FREE_PCT [5]  DRY_RUN [0]
#   CAPTURE_REQUIRED [1]: 0 prunes stream files by age alone, for hosts
#   that run before archive capture exists (their hours are not archived)
#   TEXTFILE_DIR [/var/lib/node_exporter/textfile]
set -uo pipefail

data=${HL_DATA_DIR:-/srv/hl/hl}/data
snapshots=${HL_SNAPSHOT_DIR:-/srv/hl/snapshots}
raw=${BLEH_RAW_DIR:-/srv/bleh/raw}
retain_h=${RETAIN_HOURS:-24}
other_h=${OTHER_RETAIN_HOURS:-24}
abci_keep=${ABCI_KEEP:-3}
log_days=${LOG_RETAIN_DAYS:-7}
snap_h=${SNAPSHOT_RETAIN_HOURS:-6}
emergency_pct=${EMERGENCY_FREE_PCT:-5}
capture_required=${CAPTURE_REQUIRED:-1}
dry=${DRY_RUN:-0}
textfile_dir=${TEXTFILE_DIR:-/var/lib/node_exporter/textfile}

now=$(date -u +%s)
deleted_bytes=0
deleted_files=0
held=0
emergency=0
ok=1

log() { printf 'hl-prune: %s\n' "$*" >&2; }
remove() { # remove PATH...: delete files, counting bytes
  local f sz
  for f in "$@"; do
    [[ -e "$f" ]] || continue
    sz=$(du -sb "$f" 2>/dev/null | cut -f1)
    if [[ "$dry" == 1 ]]; then
      log "would delete $f ($sz bytes)"
    else
      rm -rf -- "$f" || { ok=0; continue; }
    fi
    deleted_bytes=$((deleted_bytes + ${sz:-0}))
    deleted_files=$((deleted_files + 1))
  done
}
free_pct() { df -P "$data" | awk 'NR==2 {gsub("%","",$5); print 100-$5}'; }

[[ -d "$data" ]] || { log "no node data at $data"; ok=0; }

# Capture watermark as epoch seconds of the END of the captured hour.
watermark=0
if [[ -s "$raw/.captured-through" ]]; then
  wm=$(head -n 1 "$raw/.captured-through")
  if t=$(date -u -d "${wm/T/ }:00:00" +%s 2>/dev/null); then
    watermark=$((t + 3600))
  else
    log "unreadable watermark '$wm'"
  fi
else
  log "no capture watermark at $raw/.captured-through: stream files are kept"
fi

# Hour files: <stream>/hourly/<YYYYMMDD>/<H>, oldest first.
hour_files() {
  find "$data" -mindepth 4 -maxdepth 4 -path '*/hourly/*/*' -type f 2>/dev/null \
    | awk -F/ '{print $(NF-1), $NF, $0}' | sort -k1,1n -k2,2n | cut -d' ' -f3-
}
hour_end() { # hour_end FILE -> epoch seconds at the end of its UTC hour
  local d h
  h=${1##*/}; d=${1%/*}; d=${d##*/}
  date -u -d "${d:0:4}-${d:4:2}-${d:6:2} ${h}:00:00" +%s 2>/dev/null | awk '{print $1 + 3600}'
}

if [[ -d "$data" ]]; then
  while IFS= read -r f; do
    stream=$(cut -d/ -f1 <<<"${f#"$data"/}")
    end=$(hour_end "$f") || continue
    [[ -n "$end" ]] || continue
    age_h=$(( (now - end) / 3600 ))
    case "$stream" in
      node_fills*|node_order_statuses*|node_raw_book_diffs*)
        if ((age_h >= retain_h)); then
          if ((end <= watermark)) || [[ "$capture_required" == 0 ]]; then remove "$f"; else held=$((held + 1)); fi
        fi
        ;;
      *)
        ((age_h >= other_h)) && remove "$f"
        ;;
    esac
  done < <(hour_files)

  # replica_cmds/<start_time>/<date>/<height>: by modification time.
  while IFS= read -r -d '' f; do
    remove "$f"
  done < <(find "$data/replica_cmds" -type f -mmin "+$((other_h * 60))" -print0 2>/dev/null)
  find "$data/replica_cmds" -mindepth 1 -type d -empty -delete 2>/dev/null

  # periodic_abci_states: keep the newest ABCI_KEEP files.
  mapfile -t abci < <(find "$data/periodic_abci_states" -type f -name '*.rmp' -exec ls -t {} + 2>/dev/null)
  if ((${#abci[@]} > abci_keep)); then remove "${abci[@]:abci_keep}"; fi

  for d in node_logs visor_child_stderr; do
    find "$data/$d" -type f -mtime "+$log_days" -print0 2>/dev/null | xargs -0 -r rm -f --
  done
  find "$data" -mindepth 2 -type d -empty -delete 2>/dev/null
fi

if [[ -d "$snapshots" ]]; then
  find "$snapshots" -type f -mmin "+$((snap_h * 60))" -print0 2>/dev/null | xargs -0 -r rm -f --
fi

# Emergency: delete the oldest stream hours until free space recovers.
if [[ -d "$data" ]] && (( $(free_pct) < emergency_pct )); then
  log "free space below ${emergency_pct}%: deleting oldest stream hours regardless of capture"
  while IFS= read -r f; do
    (( $(free_pct) >= emergency_pct + 5 )) && break
    remove "$f"
    emergency=$((emergency + 1))
  done < <(hour_files)
fi

mkdir -p "$textfile_dir"
cat > "$textfile_dir/.bleh_hl_prune.prom.tmp" <<EOF
# HELP bleh_hl_prune_ok 1 if the last prune run completed without errors.
# TYPE bleh_hl_prune_ok gauge
bleh_hl_prune_ok $ok
# TYPE bleh_hl_prune_deleted_bytes gauge
bleh_hl_prune_deleted_bytes $deleted_bytes
# TYPE bleh_hl_prune_deleted_files gauge
bleh_hl_prune_deleted_files $deleted_files
# HELP bleh_hl_prune_uncaptured_files Stream files past retention kept because the archive has not captured them.
# TYPE bleh_hl_prune_uncaptured_files gauge
bleh_hl_prune_uncaptured_files $held
# HELP bleh_hl_prune_emergency_deleted_files Stream files deleted before capture to free disk.
# TYPE bleh_hl_prune_emergency_deleted_files gauge
bleh_hl_prune_emergency_deleted_files $emergency
# TYPE bleh_hl_prune_last_run_timestamp_seconds gauge
bleh_hl_prune_last_run_timestamp_seconds $now
EOF
mv -f "$textfile_dir/.bleh_hl_prune.prom.tmp" "$textfile_dir/bleh_hl_prune.prom"
log "deleted $deleted_files files ($deleted_bytes bytes), held $held for capture, emergency $emergency"
