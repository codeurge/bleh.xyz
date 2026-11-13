#!/usr/bin/env bash
# Month-to-date R2 usage against the free tier, from Cloudflare's GraphQL
# analytics (docs/operations/web-snapshots.md, "Staying in the free tier").
#
#   CLOUDFLARE_ACCOUNT_ID=... CLOUDFLARE_API_TOKEN=... scripts/r2-usage.sh
#
# The token needs Account Analytics Read only. Prints one line per meter and
# exits 1 when any meter reaches R2_USAGE_WARN_PCT (default 80) percent of
# its free allowance, so it can run from cron or a monitor.
#
# Free tier (https://developers.cloudflare.com/r2/pricing/): 10 GB-month
# Standard storage, 1,000,000 Class A and 10,000,000 Class B operations a
# month. Infrequent Access storage has no free tier. Storage here is the
# month's peak, which overstates the GB-month average.
set -euo pipefail

account=${CLOUDFLARE_ACCOUNT_ID:?set CLOUDFLARE_ACCOUNT_ID}
token=${CLOUDFLARE_API_TOKEN:?set CLOUDFLARE_API_TOKEN}
warn=${R2_USAGE_WARN_PCT:-80}

start=$(date -u +%Y-%m-01T00:00:00Z)
end=$(date -u +%Y-%m-%dT%H:%M:%SZ)

query='query($a: String!, $s: Time!, $e: Time!) {
  viewer { accounts(filter: {accountTag: $a}) {
    ops: r2OperationsAdaptiveGroups(limit: 1000, filter: {datetime_geq: $s, datetime_leq: $e}) {
      sum { requests } dimensions { actionType }
    }
    storage: r2StorageAdaptiveGroups(limit: 1000, filter: {datetime_geq: $s, datetime_leq: $e}) {
      max { payloadSize metadataSize } dimensions { bucketName }
    }
  } }
}'

body=$(jq -n --arg q "$query" --arg a "$account" --arg s "$start" --arg e "$end" \
  '{query: $q, variables: {a: $a, s: $s, e: $e}}')

resp=$(curl -fsS https://api.cloudflare.com/client/v4/graphql \
  -H "Authorization: Bearer ${token}" -H "Content-Type: application/json" \
  --data "$body")

if [[ $(jq '.errors // [] | length' <<<"$resp") -gt 0 ]]; then
  jq -r '.errors[].message' <<<"$resp" >&2
  exit 2
fi

# Operation classes from the R2 pricing page. Deletes and aborts are free;
# any action not listed is counted as Class A, the dearer class.
report=$(jq -r --argjson warn "$warn" '
  def class_b: ["HeadBucket","HeadObject","GetObject","UsageSummary","GetBucketEncryption",
                "GetBucketLocation","GetBucketCors","GetBucketLifecycleConfiguration"];
  def free: ["DeleteObject","DeleteBucket","AbortMultipartUpload"];
  .data.viewer.accounts[0] as $acc
  | ($acc.ops // []) as $ops
  | ([$ops[] | select(.dimensions.actionType as $t | (class_b | index($t)) != null) | .sum.requests] | add // 0) as $b
  | ([$ops[] | select(.dimensions.actionType as $t | (class_b | index($t)) == null and (free | index($t)) == null) | .sum.requests] | add // 0) as $a
  | ([($acc.storage // [])[] | (.max.payloadSize + .max.metadataSize)] | add // 0) as $bytes
  | [["storage_gb", ($bytes / 1e9), 10],
     ["class_a_ops", $a, 1000000],
     ["class_b_ops", $b, 10000000]]
  | (.[] | "\(.[0])\t\(.[1] | if . < 1 and . > 0 then (. * 1000 | round / 1000) else round end)\tof \(.[2])\t\((.[1] / .[2] * 1000 | round) / 10)%"),
    (if any(.[]; .[1] / .[2] * 100 >= $warn) then "OVER \($warn)% OF THE FREE TIER" else empty end)
' <<<"$resp")
printf '%s\n' "$report"
if grep -q '^OVER' <<<"$report"; then exit 1; fi
