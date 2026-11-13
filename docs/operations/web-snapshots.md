# Runbook: web snapshots

`archive snapshot` builds the [web snapshot](../specs/web-snapshot-v0.md)
files the static site reads ([ADR 0016](../decisions/0016-web-snapshots.md)):
latest values and short histories from the swap and reference pollers'
record files, each with its source's notice. It runs on the home-archive
server next to the pollers and only connects outward.

## Running

Compose runs it as `web-snapshot` in the `home-archive` profile:

```bash
docker compose --profile home-archive up -d web-snapshot
```

It reads `/var/lib/bleh/swap/records` and `/var/lib/bleh/reference/records`
(read-only), writes the files to the `web-snapshot` volume, and rebuilds
every `WEB_SNAPSHOT_INTERVAL_SECS` (default 300). Unchanged files are not
rewritten or uploaded.

By hand, against any record directories:

```bash
archive snapshot --records /srv/swap/records,/srv/reference/records --out /tmp/web
```

| Flag | Variable | Default |
|---|---|---|
| `--records` (repeatable) | `BLEH_SNAPSHOT_RECORDS` (comma-separated) | required |
| `--out` | `BLEH_SNAPSHOT_OUT` | required |
| `--points` | `BLEH_SNAPSHOT_POINTS` | 260 daily points per series |
| `--recent-swaps` | `BLEH_SNAPSHOT_RECENT_SWAPS` | 50 per source and asset class |
| `--swap-days` | `BLEH_SNAPSHOT_SWAP_DAYS` | 14 |
| `--follow` | `BLEH_SNAPSHOT_FOLLOW` | off (one pass) |
| `--interval-secs` | `BLEH_SNAPSHOT_INTERVAL_SECS` | 300 |

## Publishing to `data.bleh.xyz`

Uploading is off until `WEB_SNAPSHOT_URL` is set in `.env`:

1. In Cloudflare, create an R2 API token with **Object Read & Write** on
   the public bucket only. Its S3 access key ID is the token ID; the secret
   is the SHA-256 (hex) of the token value.
2. Set in `.env`. The `WEB_SNAPSHOT_*` store settings override `S3_*`, so
   other services can keep a local MinIO in `S3_*`:

   ```bash
   WEB_SNAPSHOT_URL=s3://bleh-archive/web/v0
   WEB_SNAPSHOT_ENDPOINT=https://<account-id>.r2.cloudflarestorage.com
   WEB_SNAPSHOT_REGION=auto
   WEB_SNAPSHOT_ALLOW_HTTP=false
   WEB_SNAPSHOT_PATH_STYLE=true
   WEB_SNAPSHOT_ACCESS_KEY_ID=<token ID>
   WEB_SNAPSHOT_SECRET_ACCESS_KEY=<sha256 of the token value>
   ```

3. `docker compose --profile home-archive up -d web-snapshot`, then check
   `https://data.bleh.xyz/web/v0/latest.json`.

The edge module (`deploy/terraform/modules/cloudflare-edge`) allows
cross-origin `GET` on the public bucket and caches `/web/` for 60 seconds.
Until `data.bleh.xyz` exists, the bucket's `r2.dev` URL works if public
access is turned on; point the site's `bleh-data` meta tag at it. `r2.dev`
is rate limited and not cached, so use it only for testing.

## Staying in the free tier

R2's free tier is 10 GB-month of Standard storage, 1 million Class A
(write and list) and 10 million Class B (read) operations a month; egress
is free. Infrequent Access storage has no free tier. Snapshots fit easily:

| Meter | Snapshot use | Bound |
|---|---|---|
| Storage | Under 10 MB | Fixed number of bounded files |
| Class A | Changed files plus `latest.json` each pass | About 130,000 a month at 300 s even if every file changed every pass |
| Class B | Edge cache refills only | The cache rule caches `/web/` for 60 s, and tiered caching sends refills through upper tiers, so reads scale with files and minutes, not visitors |

Keep it that way:

- Keep `WEB_SNAPSHOT_INTERVAL_SECS` at 60 or more.
- Keep the cache rule for `data.bleh.xyz`. Without it Cloudflare does not
  cache `.json`, and every page view becomes a Class B read.
- Every file the publisher rewrites in place (`latest.json`,
  `_manifests/index.json`, `_manifests/magnets.txt`, `manifest.anchors.json`,
  `manifest.json.ots`, `version-hint.text`) must match the short-TTL rule
  (`deploy/terraform/modules/cloudflare-edge`). Cached under the one-year
  rule, a stale index makes `archive verify` fail with "previous manifest
  not found". After changing the rule, purge the `_manifests/` prefix.
- Do not point the site at `r2.dev`, which bypasses the cache.
- Uploading settled archive days to the same bucket (`archive publish`)
  uses storage that grows every day and will leave the free tier; decide
  that separately.

Check month-to-date usage with `scripts/r2-usage.sh` (needs a token with
Account Analytics Read). It exits 1 at 80 percent of any allowance. A
Cloudflare budget alert at $1 of usage-based spend is the backstop.

## The site

`brand/site/index.html` names the snapshot root in
`<meta name="bleh-data" content="https://data.bleh.xyz/web/v0/">`. The
panel ("From the open sources") stays hidden unless `latest.json` and the
picked series load. The tiles are listed in `PICK` in
`brand/site/assets/site.js`; the notices of the sources shown appear under
the tiles, and every source's notice is in "Sources and notices". Keep it
that way: a source's notice must be shown with its values (ADR 0012).

## Checks

- Logs: `snapshot pass done` with `records`, `written` and `uploaded` each
  pass; `snapshot pass failed` on errors, retried next interval.
- Metrics (counters): `archive_web_snapshot_uploaded_files_total`,
  `archive_web_snapshot_bad_lines_total`, `archive_web_snapshot_errors_total`.
- Freshness: `latest.json` `generated_ms`, and per source `last_record_ms`
  and `latest_as_of`.

## Notes

- The first pass after a start reads every record file. Swap print files
  are large (gigabytes a day), so that pass takes a minute or two.
- A record file that shrinks (rewritten by a correction) makes the next
  pass forget everything and read every file again.
- Snapshots are not signed and are not the archive. Settled days are
  published through the signed pipeline ([archive.md](archive.md)).
