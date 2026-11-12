# Runbook: swap pollers

`swap-pollers` captures CFTC Part 43 and SEC Regulation SBSR public swap
dissemination from DTCC Data Repository (U.S.) (admissions
[ADM-0002](../admissions/ADM-0002-cftc-part43-swaps.md) and
[ADM-0004](../admissions/ADM-0004-sec-sbsr-public-dissemination.md)). It
keeps every fetched file verbatim, then emits `swap_print` records as daily
JSON lines and on a channel for the gateway. Formats and the field mapping
are in [swap-repositories.md](../specs/swap-repositories.md).

## Why capture is time-critical

- Slice files are listed for only about 24 hours. Whether they can be
  fetched after they drop off the listing is unverified, so treat a gap of
  more than a few hours as data at risk.
- End-of-day files stay online for 366 days, and the rule only requires a
  repository to keep a year (17 CFR 43.3(c)(1)). Anything not captured
  within a year may be gone for good.
- Raw files are the record of truth: they are written to disk (and
  optionally S3) **before** parsing, so a parser bug never loses data.

## Running

```bash
cargo build --release -p swap-pollers

# Continuous capture (defaults shown).
target/release/swap-pollers \
  --raw-dir data/swap/raw \
  --state-dir data/swap/state \
  --out-dir data/swap/records \
  --listen 127.0.0.1:9187

# One cycle (slices, then end-of-day files), then exit. Non-zero exit if a
# slice file failed.
target/release/swap-pollers --once

# One-off archive of every end-of-day file still online (up to 366 days).
target/release/swap-pollers --once --cumulative-days 366

# Offline: parse a raw file (zip or CSV) to JSON lines on stdout. Uses the
# sidecar next to it when present; pass --bucket if the name does not say.
target/release/swap-pollers --parse data/swap/raw/dtcc-ddr/cftc/slices/2026-11-12/CFTC_SLICE_RATES_2026_11_12_2.zip
```

Stop with `SIGINT` or `SIGTERM`; the current file finishes its checkpoint
step on the next cycle boundary.

## Configuration

Every option has an environment variable `SWAP_POLLERS_<NAME>`.

| Option | Default | Meaning |
|---|---|---|
| `--raw-dir` | `data/swap/raw` | Verbatim raw files and sidecars |
| `--state-dir` | `data/swap/state` | `checkpoint.json` and `seen/` (dedupe) |
| `--out-dir` | `data/swap/records` | Daily JSON lines |
| `--no-jsonl` | off | Gateway channel only |
| `--listen` | `127.0.0.1:9187` | `/metrics` and `/health` |
| `--buckets` | all eight | `CFTC_IR,CFTC_CR,CFTC_EQ,CFTC_FX,CFTC_CO,SEC_IR,SEC_CR,SEC_EQ`; Canadian buckets are refused (not admitted) |
| `--slice-interval` | 60 s (min 5) | Slice listing poll |
| `--cumulative-interval` | 3600 s (min 60) | End-of-day listing poll |
| `--cumulative-days` | 7 (max 366, 0 off) | Days of end-of-day files to capture |
| `--dedupe-days` | 3 | Days of emitted dissemination IDs remembered |
| `--dtcc-api-base` | `https://pddata.dtcc.com/ppd/api` | Bucket-name lookup |
| `--dtcc-data-base` | resolved | Skip the lookup (for mirrors and tests) |
| `--user-agent` | `swap-pollers/<version>` | Request header |
| `--min-request-interval-ms` | 250 | Spacing between request starts |
| `--max-retries` | 5 | Retries on network errors, `408`, `429`, `5xx` |
| `--request-timeout` | 600 s | Per request; end-of-day equity files are large |
| `--s3-bucket`, `--s3-prefix`, `--s3-endpoint`, `--s3-region` | off, `swap-raw` | Mirror raw files to S3-compatible storage; credentials from `AWS_*` variables |
| `--log-json` | off | JSON logs; level from `RUST_LOG` (default `info`) |

## What happens each cycle

1. Resolve the data bucket name from the PPD API (unless
   `--dtcc-data-base` is set), and again after a `403`/`404`.
2. `GET dashboard/Slice.json` with `If-None-Match`/`If-Modified-Since`.
   On `304`, done.
3. For each configured bucket, every listed file not in the checkpoint,
   oldest `sliceId` first:
   1. stream to `<raw>/dtcc-ddr/<cftc|sec>/slices/<date>/<file>.part`,
      fsync, rename; write `<file>.meta.json` (URL, fetch time, size,
      SHA-256, validators, listing entry); mirror both to S3 if configured;
   2. parse, dedupe by dissemination ID, append JSON lines, send to the
      gateway channel;
   3. flush JSON lines, then the dedupe keys, then write the checkpoint.
4. The listing's validators are saved only if every file succeeded, so a
   failed file is retried next cycle.
5. Hourly, the same for `dashboard/Cumulative.json` (`.../eod/...`) for
   the last `--cumulative-days` days. End-of-day files are always archived.
   Their records, flagged `backfill`, are emitted when the day is inside
   the dedupe window (duplicates of live slices are dropped) or when no
   live slice covered that day; otherwise the file is archived only.

Delivery is at least once: a crash after JSON lines are flushed but before
the checkpoint can repeat records on restart (the dedupe store catches most;
the archive lane should still dedupe on `(venue, repository,
dissemination_id)`). A crash after download but before the checkpoint
reuses the raw file without fetching it again.

Files that cannot be parsed at all (malformed zip, required column missing)
are archived, counted in `swap_parse_errors_total{scope="file"}` and not
retried. Re-run them with `--parse` after a parser fix.

## Outputs

```text
<raw>/dtcc-ddr/<cftc|sec>/<slices|eod>/<YYYY-MM-DD>/<file>.zip
<raw>/dtcc-ddr/<cftc|sec>/<slices|eod>/<YYYY-MM-DD>/<file>.zip.meta.json
<out>/date=<YYYY-MM-DD>/swap_print.jsonl     # by UTC dissemination day
<state>/checkpoint.json
<state>/seen/seen-<YYYY-MM-DD>.bin
```

JSON lines use the canonical encoding of
[observation-v0](../specs/observation-v0.md) § 6.1, one `Record` per line
(`"kind":"swap_print"`). The archive lane partitions them.

## Metrics

| Metric | Labels | Watch for |
|---|---|---|
| `swap_fetches_total` | `target`, `outcome` (`ok`, `retry`, `error`) | Rising `error` or `retry` |
| `swap_fetch_bytes_total` | `target` | Bandwidth (the slice listing is about 3 MB per change) |
| `swap_files_total` | `bucket`, `kind`, `outcome` (`captured`, `archived_only`, `unparseable`, `error`) | Any `error` or `unparseable` |
| `swap_rows_total` | `bucket` | Volume |
| `swap_records_total` | `bucket`, `action` | A bucket going flat during market hours |
| `swap_records_skipped_total` | `bucket`, `reason` | Lifecycle and non-trade events (expected, large for equities) |
| `swap_parse_errors_total` | `bucket`, `scope` (`row`, `file`) | Any `file`; a jump in `row` |
| `swap_field_errors_total` | `bucket`, `field` | A format change in one column |
| `swap_unknown_columns_total` | `bucket`, `column` | **Any**: the layout changed; review the spec |
| `swap_duplicates_total` | `bucket` | Expected after end-of-day files |
| `swap_orphan_updates_total` | `bucket` | Corrections of prints not seen (expected right after start) |
| `swap_dissemination_fallbacks_total` | `source` | Non-zero means rows lack a dissemination timestamp |
| `swap_raw_persist_errors_total` | `store` (`local`, `object`) | **Any**: raw capture at risk |
| `swap_last_dissemination_timestamp_seconds` | `bucket` | |
| `swap_lag_seconds` | `bucket` | Capture lag behind dissemination; alert above 15 minutes for busy buckets |
| `swap_last_successful_poll_timestamp_seconds` | `listing` | Alert if older than 10 minutes |

`/health` returns `200` while a listing poll succeeded within five slice
intervals plus five minutes (or during that long after start-up) and `503`
otherwise.

## Risks and responses

| Situation | Response |
|---|---|
| `swap_unknown_columns_total` rises | Raw capture continues. Compare the new header with the spec, update `fields.rs` and the spec, replay affected raw files with `--parse` |
| Files fail repeatedly (`404`) | The slice may have been delisted; check the dashboard. If end-of-day files exist for the day, raise `--cumulative-days` to cover it |
| Bucket name changes | Handled automatically after a `403`/`404`; check logs for "re-resolving" |
| Long outage | Slices older than about 24 hours may be lost; end-of-day files cover the gap (`backfill` flag). Run `--once --cumulative-days N` |
| Local disk filling | End-of-day equity files are large; mirror to S3 and prune local raw after verifying the mirror |
| DTCC maintenance (Sat 22:00 to Sun 06:00 ET) | No new data; queued messages arrive afterwards |
| Terms of use change | A review trigger for ADM-0002 and ADM-0004; stop publishing and ask the owner. The terms are not yet verified (spec § 2.4) |
| A request to add ICE Trade Vault or another exchange-group repository | Not admitted; needs a new admission decision first |
