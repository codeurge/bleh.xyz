#!/usr/bin/env bash
# Scheduled schema-drift check for Hyperliquid node output.
#
# hl-visor upgrades hl-node without notice, so the output format can change
# under the reader. The reader stops a kind on unknown fields at parse time
# (live detection); this job is the independent second check. It samples
# the newest lines of each stream the mesh reads, reduces them to the set of
# key paths, and compares that set with the key paths of the synthetic
# fixtures, which are generated from docs/specs/hyperliquid-node-format.md.
#
#   unknown key path (in node output, not in the spec)  -> drift
#   missing key path that the reader requires           -> drift
#
# It also records the sha256 of the running hl-node binary, so upgrades show
# on the dashboard and can be correlated with drift.
#
# Results go to node_exporter's textfile collector; alerts:
# BlehSchemaDriftCheckFailed, BlehSchemaDriftCheckStale, BlehNodeBinaryChanged.
#
# Environment (defaults):
#   HL_DATA_DIR      node data root containing data/ [/srv/hl/hl]
#   SPEC_FIXTURES    [/opt/bleh/src/fixtures/synthetic/hl/base]
#   KNOWN_EXTRA      extra accepted key paths, one per line
#                    [/opt/bleh/src/deploy/scripts/schema-known-extra.conf]
#   SAMPLE_LINES     lines sampled per stream [200]
#   HL_NODE_BINARY   path to hl-node on a native install [/srv/hl/hl-node];
#                    with the container, `docker exec` reads it instead
#   HL_CONTAINER     [bleh-hl-node-1]
#   TEXTFILE_DIR     [/var/lib/node_exporter/textfile]
#   STATE_DIR        [/var/lib/bleh]
set -uo pipefail

hl_data=${HL_DATA_DIR:-/srv/hl/hl}/data
fixtures=${SPEC_FIXTURES:-/opt/bleh/src/fixtures/synthetic/hl/base}
known_extra=${KNOWN_EXTRA:-/opt/bleh/src/deploy/scripts/schema-known-extra.conf}
sample=${SAMPLE_LINES:-200}
textfile_dir=${TEXTFILE_DIR:-/var/lib/node_exporter/textfile}
state_dir=${STATE_DIR:-/var/lib/bleh}
container=${HL_CONTAINER:-bleh-hl-node-1}
binary=${HL_NODE_BINARY:-/srv/hl/hl-node}

# Key paths the reader cannot work without (observation mapping in the spec).
required_fills="block_number block_time local_time events.[].[].coin events.[].[].px events.[].[].sz events.[].[].side events.[].[].time events.[].[].crossed events.[].[].tid events.[].[].oid"
required_order_statuses="block_number block_time local_time events.[].status events.[].time events.[].order.coin events.[].order.oid events.[].order.side events.[].order.limitPx events.[].order.sz events.[].order.timestamp"
required_raw_book_diffs="block_number block_time local_time events.[].coin events.[].oid events.[].side events.[].px events.[].raw_book_diff"

# Reduce JSON lines on stdin to sorted unique key paths, array indices as [].
key_paths() {
  jq -r 'paths(type != "object" and type != "array")
         | map(if type == "number" then "[]" else . end) | join(".")' 2>/dev/null \
    | sed -E 's/\.?\[\]/.[]/g; s/^\.//' | sort -u
}

stream_dir() { # the live node may use node_<s>_by_block or node_<s> (to verify)
  local s=$1 d
  for d in "$2/node_${s}_by_block" "$2/node_${s}"; do
    [[ -d "$d/hourly" ]] && { printf '%s\n' "$d"; return 0; }
  done
  return 1
}

newest_file() {
  find "$1/hourly" -type f -exec ls -t {} + 2>/dev/null | head -n 1
}

ok=1
metrics=()
report=()
mkdir -p "$state_dir" "$textfile_dir"

for s in fills order_statuses raw_book_diffs; do
  live_dir=$(stream_dir "$s" "$hl_data") || { report+=("$s: no output directory under $hl_data"); ok=0; continue; }
  spec_dir=$(stream_dir "$s" "$fixtures") || { report+=("$s: no fixture directory under $fixtures"); ok=0; continue; }
  live_file=$(newest_file "$live_dir")
  [[ -n "$live_file" ]] || { report+=("$s: no files yet"); ok=0; continue; }

  live_keys=$(tail -n "$sample" "$live_file" | key_paths)
  spec_keys=$(find "$spec_dir/hourly" -type f -exec cat {} + | key_paths)
  if [[ -f "$known_extra" ]]; then
    spec_keys=$(printf '%s\n%s\n' "$spec_keys" "$(grep -v '^#' "$known_extra" | sed '/^$/d')" | sort -u)
  fi

  unknown=$(comm -23 <(printf '%s\n' "$live_keys") <(printf '%s\n' "$spec_keys") | sed '/^$/d')
  req_var="required_${s}"
  missing=""
  for k in ${!req_var}; do
    grep -qxF "$k" <<<"$live_keys" || missing+="$k "
  done
  n_unknown=$(sed '/^$/d' <<<"$unknown" | wc -l | tr -d ' ')
  n_missing=$(wc -w <<<"$missing" | tr -d ' ')
  metrics+=("bleh_schema_drift_unknown_keys{stream=\"$s\"} $n_unknown")
  metrics+=("bleh_schema_drift_missing_keys{stream=\"$s\"} $n_missing")
  if ((n_unknown > 0 || n_missing > 0)); then
    ok=0
    report+=("$s: unknown: $(tr '\n' ' ' <<<"$unknown")| missing: $missing| file: $live_file")
  fi
done

# hl-node binary identity.
sha=""
if [[ -x "$binary" ]]; then
  sha=$(sha256sum "$binary" | cut -c1-64)
elif command -v docker >/dev/null && docker inspect "$container" >/dev/null 2>&1; then
  sha=$(docker exec "$container" sh -c 'sha256sum /home/hluser/hl-node 2>/dev/null' | cut -c1-64)
fi
changed_at=$(cat "$state_dir/hl-node.changed" 2>/dev/null || echo 0)
if [[ -n "$sha" ]]; then
  if [[ "$sha" != "$(cat "$state_dir/hl-node.sha256" 2>/dev/null)" ]]; then
    printf '%s\n' "$sha" > "$state_dir/hl-node.sha256"
    changed_at=$(date +%s)
    printf '%s\n' "$changed_at" > "$state_dir/hl-node.changed"
    report+=("hl-node binary is now $sha")
  fi
  metrics+=("bleh_hl_node_binary_info{sha256=\"${sha:0:16}\"} 1")
fi

{
  echo "# HELP bleh_schema_drift_ok 1 if sampled node output matches the format spec."
  echo "# TYPE bleh_schema_drift_ok gauge"
  echo "bleh_schema_drift_ok $ok"
  echo "# TYPE bleh_schema_drift_unknown_keys gauge"
  echo "# TYPE bleh_schema_drift_missing_keys gauge"
  echo "# TYPE bleh_hl_node_binary_info gauge"
  printf '%s\n' "${metrics[@]+"${metrics[@]}"}"
  echo "# HELP bleh_hl_node_binary_changed_timestamp_seconds When the hl-node binary last changed."
  echo "# TYPE bleh_hl_node_binary_changed_timestamp_seconds gauge"
  echo "bleh_hl_node_binary_changed_timestamp_seconds $changed_at"
  echo "# TYPE bleh_schema_drift_last_run_timestamp_seconds gauge"
  echo "bleh_schema_drift_last_run_timestamp_seconds $(date +%s)"
} > "$textfile_dir/.bleh_schema_drift.prom.tmp"
mv -f "$textfile_dir/.bleh_schema_drift.prom.tmp" "$textfile_dir/bleh_schema_drift.prom"

if ((${#report[@]})); then printf 'schema-drift: %s\n' "${report[@]}" >&2; fi
((ok)) && echo "schema-drift: ok" || echo "schema-drift: DRIFT (see stderr)"
exit 0
