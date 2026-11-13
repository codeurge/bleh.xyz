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
   the public bucket only.
2. Set in `.env`:

   ```bash
   WEB_SNAPSHOT_URL=s3://bleh-archive/web/v0
   S3_ENDPOINT=https://<account-id>.r2.cloudflarestorage.com
   WEB_SNAPSHOT_ACCESS_KEY_ID=<token access key>
   WEB_SNAPSHOT_SECRET_ACCESS_KEY=<token secret>
   ```

3. `docker compose --profile home-archive up -d web-snapshot`, then check
   `https://data.bleh.xyz/web/v0/latest.json`.

The edge module (`deploy/terraform/modules/cloudflare-edge`) already allows
cross-origin `GET` on the public bucket and caches `/web/` for 60 seconds.
Until `data.bleh.xyz` exists, the bucket's `r2.dev` URL works if public
access is turned on; point the site's `bleh-data` meta tag at it.

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
