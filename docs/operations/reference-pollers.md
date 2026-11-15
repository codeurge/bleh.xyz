# Runbook: reference pollers

`reference-pollers` captures the admitted public reference sources
(admissions ADM-0005 to ADM-0013, ADM-0021 and ADM-0022) live and from
history, and emits
`reference_value` and `positioning` records
([observation-v0](../specs/observation-v0.md) §§ 2.12, 2.14). Endpoints,
formats and schedules are in
[reference-sources.md](../specs/reference-sources.md); the readers are
written clean-room from it.

## Sources

| Source (`--sources` name) | Admission | Channel read | Live | Backfill from | Records |
|---|---|---|---|---|---|
| `cot` | [ADM-0005](../admissions/ADM-0005-cftc-commitments-of-traders.md) | cftc.gov weekly and annual files | Fridays, after 3:30 p.m. ET; annual file of the year first (it gives the weekly files their header) | 1986-01-15 (legacy futures), 1995 (legacy combined), 2006 (disaggregated, TFF, supplemental) | `positioning` per market, family, form and category; combined records `exchange_referenced` |
| `treasury-rates` | [ADM-0006](../admissions/ADM-0006-us-treasury-rates-and-auctions.md) | home.treasury.gov XML feeds | Weekdays after 6 p.m. ET | 1990 (par curve), 2002 (bills), 2003 (real curve), 2000 (20-year CMT) | par and real par curves, bill rates (discount and coupon equivalent, with maturities), 20-year CMT |
| `treasury-auctions` | ADM-0006 | Fiscal Data `auctions_query` | After auctions close | 1979-11-15 | high, low and median yields, rates and prices, coupon, by security type and term |
| `nyfed` | [ADM-0007](../admissions/ADM-0007-nyfed-reference-rates.md) | Markets Data API | Weekdays 8:00 to 16:00 ET every 10 minutes (catches the 2:30 p.m. revisions) | 2000-07-03 (EFFR), 2016-03-01 (OBFR), 2018-04-02 (SOFR, BGCR, TGCR), 2020-03-02 (SOFR averages and index) | rates, percentiles, FOMC target range, SOFR averages and index |
| `frb-h15` | [ADM-0008](../admissions/ADM-0008-federal-reserve-board-h15.md) | Whole-release SDMX zip | Weekdays after 4:15 p.m. ET | 1954 (whole file) | admitted business-daily series by stem |
| `frb-h10` | [ADM-0009](../admissions/ADM-0009-federal-reserve-board-h10.md) | Whole-release SDMX zip | Mondays | 1971 (whole file) | certified noon buying rates (23 currencies) |
| `usda-lmr` | [ADM-0010](../admissions/ADM-0010-usda-livestock-mandatory-reporting.md) | LMPR DataMart API | Weekdays, every 30 minutes | 2001-04-02 | LM_HG201 negotiated and (flagged) formula swine prices; LM_CT100 negotiated cattle |
| `usda-dairy` | [ADM-0011](../admissions/ADM-0011-usda-dairy-ndpsr-and-class-prices.md) | LMPR DataMart API | Weekdays, hourly 15:00 to 23:00 UTC | first date the DataMart returns (requests start 2000) | NDPSR prices and announced class and component prices, all `exchange_referenced` |
| `usda-feeder` | [ADM-0012](../admissions/ADM-0012-usda-market-news-feeder-cattle-and-grain.md) | `ams.usda.gov/mnreports/sj_ls850.txt` | Weekdays, every few hours | none: the files hold only the latest issue and the archive channel (MyMarketNews) is not admitted | weekly summary sale prices |
| `ercot` | [ADM-0013](../admissions/ADM-0013-ercot-prices.md) | www.ercot.com listings and downloads (never the API) | Every 5 minutes | 2010 (hub and load-zone workbooks, NP4-180-ER and NP6-785-ER) | day-ahead and real-time SPPs and AS clearing prices per interval |
| `eia` | [ADM-0021](../admissions/ADM-0021-eia-energy-statistics.md) | Bulk files `PET.zip` and `NG.zip` (never the API) | Wednesday and Thursday 19:30 to 23:30 UTC every 30 minutes (the 3 p.m. ET bulk build after the Weekly Petroleum Status Report and the storage report); otherwise daily | 1974 (crude first purchase prices; other series later) | allowlisted EIA survey prices: weekly retail gasoline and diesel, residential heating oil and propane; monthly refiner, first purchase, imported crude and natural gas prices, `exchange_referenced` |
| `bls` | [ADM-0022](../admissions/ADM-0022-bls-consumer-price-indexes.md) | `download.bls.gov/pub/time.series` `cu`, `cw`, `su`, `ap` (never the API) | Weekdays 12:15 to 16:00 UTC every 30 minutes (the 8:30 a.m. ET CPI release); otherwise every 12 hours | 1913 (CPI-U and CPI-W all items), 1999 (C-CPI-U), 1980 (average prices) | CPI-U, CPI-W and C-CPI-U index levels and average prices, monthly, for the areas in `--bls-areas` |

Never used: the ERCOT API, MyMarketNews, treasurydirect.gov, FRED, the COT
Socrata API (channel B is for checks and is not implemented), the EIA and
BLS APIs (for checks only), BLS producer and import/export price indexes,
the excluded EIA families (spot, futures, Henry Hub, wholesale heating oil
and propane, NGL composite, wholesale electricity and gas hubs), and every
rejected or pending source (CAISO, PJM, MISO, ISO-NE, SPP, NYISO, USDA
grain prices).

## Why some capture is time-critical

- ERCOT lists real-time files for 7 days and day-ahead files for 31; node
  history older than that is only on the API archive, which is not
  admitted. An ERCOT outage longer than a few days loses node-level data
  for good (hub and load-zone history comes back from the yearly
  workbooks).
- The feeder cattle files hold one issue; a missed week is gone.
- Everything else keeps full history and can be backfilled.
- Raw bodies are written to disk **before** parsing, so a parser bug never
  loses data: fix the reader and replay with `parse`.

## Running

```bash
cargo build --release -p reference-pollers

# Continuous capture (defaults shown).
target/release/reference-pollers run \
  --raw-dir data/reference/raw \
  --state-dir data/reference/state \
  --out-dir data/reference/records \
  --listen 127.0.0.1:9188

# One cycle of each source, then exit (non-zero if a target failed).
target/release/reference-pollers once --sources nyfed,treasury-rates

# History into the reference-backfill archive, resumable.
target/release/reference-pollers backfill \
  --archive-root data/reference/archive/reference-backfill \
  --sources nyfed --from-year 2018

# Offline: replay a raw file (its sidecar names the source and parser).
target/release/reference-pollers parse data/reference/raw/nyfed/2026-03-16/nyfed_secured_live.230000.abcdef012345

# Registry entries (with notices) and the notices file for an archive.
target/release/reference-pollers catalog > catalog.json
target/release/reference-pollers notices > _notices.json
```

Deployed in the `home-archive` compose profile (service
`reference-pollers`; one-off `reference-backfill` in the `backfill`
profile), or with `bleh@reference-pollers` and
`bleh-reference-backfill.service`
([deploy/README.md](../../deploy/README.md)).

## Configuration

Every option has an environment variable `REFERENCE_POLLERS_<NAME>`.

| Option | Default | Meaning |
|---|---|---|
| `--sources` | all | Comma list of the names above (empty means all) |
| `--raw-dir` | `data/reference/raw` | Verbatim bodies and sidecars |
| `--state-dir` | `data/reference/state` | Per source: `state.json` (validators, last body hashes, fetched documents), `revisions/`, `backfill/` |
| `--out-dir` | `data/reference/records` | Daily JSON lines and `_notices.json` |
| `--no-jsonl` | off | Channel only (embedding process) |
| `--listen` | `127.0.0.1:9188` | `/metrics`, `/health` |
| `--user-agent` | `reference-pollers/<version>` | Generic; never a person's name or address |
| `--contact` | `data@bleh.xyz` | Project role address appended to the User-Agent as `(<contact>)`, as BLS asks of bulk clients; never a personal address; empty for none |
| `--min-request-interval-ms` | 500 | Spacing between request starts (all sources share one client) |
| `--max-retries` | 5 | Retries on network errors, `408`, `429`, `5xx` (exponential backoff, `Retry-After` honoured) |
| `--request-timeout` | 300 s | Per request |
| `--base-url name=url` | official hosts | Mirrors and tests (`cftc`, `treasury`, `fiscaldata`, `nyfed`, `frb`, `datamart`, `ams`, `ercot`, `eia`, `bls`) |
| `--ercot-products` | `12331,12329,12301,24898` | Report types polled live; `12328` (bus LMPs), `12300` (SCED LMPs) and `24891` (SCED AS prices) are admitted but heavy |
| `--bls-areas` | `0000` | BLS area codes emitted (comma list), or `all`. Every area is admitted; the default keeps the U.S. city average, which bounds the web snapshot (`reference/bls.json`). `all` adds about 5,700 series and the area partition files to the backfill |
| `--log-json` | off | JSON logs; level from `RUST_LOG` |

## What happens each cycle

1. The reader lists its targets (URLs) for today.
2. Each target is fetched with `If-None-Match` / `If-Modified-Since`; a
   `304`, or a body identical to the last one, ends there.
3. A changed body is written verbatim to
   `raw/<source>/<date>/<target>.<HHMMSS>.<sha>` with a `.meta.json`
   sidecar, then parsed. Follow-up targets (Fiscal Data pages, ERCOT
   documents) are queued; ERCOT documents are fetched once and remembered
   for 90 days.
4. Values dated before the source's live window (14 to 400 days) are
   dropped; older history is the backfill's.
5. Each value goes through the revision store: an unchanged value is
   skipped; a changed value for a date already emitted is emitted again
   with the next `revision`. This handles NY Fed same-day revisions, H.15
   revisions, NDPSR four-week revisions, LMR corrections and ERCOT price
   corrections the same way.
6. Records are appended to the JSON lines and sent to the channel; then
   the JSON lines, revision store and state are flushed, in that order.
   Delivery is at least once: dedupe on `(instrument, kind, seq)`.
7. A cycle with failures is retried after one minute, doubling up to the
   source's interval; a clean cycle waits for the schedule (often inside
   the release window, rarely outside it).

## Records

- Instruments: `<venue>:ref:<series>` (series names in the catalogue) and
  `cftc-cot:positioning:<contract market code>`. EIA and BLS use the
  agencies' own series IDs: `eia:ref:PET.EMM_EPMR_PTE_NUS_DPG.W`,
  `bls:ref:CUUR0000SA0`. Weekly EIA values are dated the date the bulk
  file gives (the survey Monday for prices); monthly values of both are
  dated the first of the month.
- `prov.seq` is `(T * 100 + revision, slot)`: `T` is the value's date
  (midnight UTC, seconds) or its interval start; the slot orders a curve's
  tenors or a market's categories. It depends only on the data, so live,
  backfill and replay agree and a revision never collides with what it
  revises.
- Live records: `ts_source_ms` is the publication time the source states
  (USDA `published_date`, the EIA series' `last_updated`), else
  `Last-Modified` (the CPI release time on BLS files), else the fetch time;
  `ts_ingest_ns` is the fetch time.
- Backfill records: flag `backfill`; `ts_source_ms` is an estimate from
  the documented release schedule (`obs::backfill_published_ms`) and
  `ts_ingest_ns` equals it, so the archive is a pure function of the files.
- `exchange_referenced` on COT combined records, swine formula and combined
  prices, every dairy price, and the EIA monthly survey prices (ADR 0012,
  ADM-0021 check 3). EIA weekly retail and residential prices and every BLS
  value carry no flag.
- ERCOT intervals carry `period_start_ms` and `period_end_ms` in UTC,
  converted from Central prevailing time; the hour repeated when daylight
  time ends is the one flagged `Y`.
- Values are carried exactly as published (decimal text, no floats).

What is admitted but not emitted, because the record kinds have no place
for it: volumes (head counts, NDPSR sales, NY Fed volumes, auction amounts
and bid-to-cover, and the EIA weekly and monthly supply, stocks, Cushing
stocks, refinery utilisation and natural gas storage series, which are
allowlisted and counted as `unsupported_volume`), BLS footnote codes,
BLS annual averages (`M13`) and semiannual values (`S01` to `S03`), rates without a tenor (bank prime, long-term composites
and averages, the extrapolation factor), CUSIPs, and the COT changes,
percentages, trader counts and concentration ratios. Announced dairy
reports 2989, 3345, 3351, 3354, 3355 and 3357, LMR reports other than 2511
and 2466, and PDF issues of the feeder cattle summary are not read until
their layouts are in the access specification.

## Notices

ADR 0012 requires notices to travel in the registry entry and the
manifest:

- `catalog` prints every series family with its notice (the New York Fed
  notice completed per rate; H.15 EFFR carries it too) and method periods.
- `_notices.json` (written to the output directory and to every backfill
  archive root) is read by `archive manifest build`, so every manifest of
  that archive carries the notices, signed ([manifest-v0](../specs/manifest-v0.md)
  § 2.3). When the archive lane partitions `records/` into an archive,
  copy `_notices.json` to that archive's root.

The New York Fed notice for SOFR, BGCR and the SOFR averages and index
includes the DTCC Solutions LLC disclaimer, copied verbatim from the Terms
of Use (2026-11-13). If the terms change it, update
`NYFED_DTCC_DISCLAIMER` and record the date here.

The EIA notice (`EIA_NOTICE`) names EIA, says where each value's
publication date is (`ts_source_ms`) and that EIA endorses nothing. The BLS
notice (`BLS_NOTICE`) cites BLS, says where the retrieval date is and
carries the BLS.gov disclaimer verbatim (terms read 2026-11-14).

## EIA and BLS

**Allowlists.**

- `eia` reads only the series IDs in `ALLOWLIST`
  (`crates/reference-pollers/src/sources/eia.rs`), each in one admitted
  family. Everything else in `PET.zip` and `NG.zip` is dropped
  (`out_of_scope`), and the excluded families (spot, futures, Henry Hub,
  `_PWR_` wholesale and resale) are dropped first (`excluded`) even if
  someone adds them to the list; a test fails if the two overlap.
- Adding a series: confirm on EIA's data table (`eia.gov/dnav/...`) that it
  comes from one of the admitted surveys and credits no third party, add
  it with its family, and note it in the access specification.
- `bls` reads only the `cu`, `cw`, `su` and `ap` directories; a series of
  any other survey in a file is dropped as `excluded`.

**Alerts specific to these sources** (`ref_unknown_fields_total`, alert
`BlehReferenceUnknownFields`):

| `field` | Meaning | Action |
|---|---|---|
| `vendor_credit:<series>` | An allowlisted EIA series now credits a vendor or exchange (ADM-0021 review trigger). It is no longer emitted | Tell the owner; remove the series from the allowlist and review ADM-0021 |
| `units:<series>` | An allowlisted EIA price is no longer in dollars | Check the table; fix or remove the entry |
| `frequency:<series>`, `series_shape:<series>`, `bulk_line_shape` | The bulk file layout changed | Read the raw file with `parse`, update the reader and the specification |
| `column:<name>` | A BLS data file has an undocumented column | Check the layout file (`cu.txt`); the documented columns are still read |

A bulk file with no series lines at all is a parse error
(`BlehReferenceParseErrors`).

**Volumes.** EIA's weekly and monthly supply, stocks (Cushing included)
and storage series are admitted and allowlisted but not emitted: no
record kind holds a quantity yet (counted as `unsupported_volume`). Raw
files keep them, so emitting them later is a replay with `parse`.

**User-Agent.** BLS refuses generic clients. The pollers append
`--contact` (`data@bleh.xyz`, a project role address) to the User-Agent.
A run of `403` from `download.bls.gov` usually means the contact was
removed or the address is being refused; keep it a role address, never a
person's.

**Disk.** Each changed EIA bulk file is kept whole in `raw/eia/`: about
55 MB for `PET.zip` and 4 MB for `NG.zip`, and EIA rebuilds them twice a
day, so expect up to about 30 GB a year. BLS adds about 105 MB a month
(`cu` and `cw` current files are about 49 MB each). Prune old raw
directories of these two sources if space is short; the records and the
backfill do not need them.

**Revisions.** BLS seasonally adjusted series are revised five years back
each February, and C-CPI-U values from initial to final over a year: live
polls keep 1,900 days of revision state for `bls` and emit each changed
value as the next revision. EIA monthly prices keep 400 days.

**First run.** A fresh state emits the whole live window at once: 400 days
of EIA prices, and 1,900 days of the selected BLS series (about 1,400
monthly U.S. city average series).

**Areas.** `--bls-areas` decides which BLS areas are emitted (default
`0000`, the U.S. city average). `all` is admitted but adds about 5,700
series; the web snapshot keeps every series of a source in one file
(`reference/bls.json`), which then grows to tens of megabytes.

## Backfill

- One unit per source and calendar year, from the source's earliest
  admitted date (`--from-year` and `--to-year` narrow it).
- Files that hold many years (H.15, H.10, the COT 2006–2016 files, the EIA
  bulk files, the BLS data files) are fetched once per run. BLS years from
  1997 (1995 for average prices) come from the `.data.0.Current` files and
  earlier years from the partition files that cover the selected areas;
  series discontinued since 1997 appear only in the partition files and are
  not backfilled for those years.
- Partitions are written through the archive crate's writer (write-once,
  content-hash gate). A partition is written only when no later unit can
  add to it; records published after their year ends (a 30 December COT
  report released in January) wait in `state/<source>/backfill/carry.jsonl`
  until the next unit.
- Re-running is safe: finished years are skipped, and a run from fresh
  state reproduces identical files (they are reported `already_present`).
- Output: one JSON line per unit on stdout (records, files written,
  already present, carried). Build manifests of `reference-backfill` with
  the archive CLI as for other archives.

## Metrics

| Metric | Labels | Watch for |
|---|---|---|
| `ref_last_success_timestamp_seconds` | `source` | Alerts `BlehReferencePollStale` (ERCOT, 30 minutes) and `BlehReferenceSourceStale` (36 hours) |
| `ref_polls_total` | `source`, `outcome` (`ok`, `partial`, `error`) | Rising `partial` or `error` |
| `ref_fetches_total` | `source`, `outcome` (`ok`, `retry`, `error`) | Retries (throttling) |
| `ref_targets_total` | `source`, `outcome` (`changed`, `not_modified`, `unchanged`, `error`) | |
| `ref_records_total` | `source`, `kind` | Volume |
| `ref_revisions_total` | `source` | Revisions emitted |
| `ref_duplicates_total` | `source` | Unchanged values skipped |
| `ref_rows_skipped_total` | `source`, `reason` (`excluded`, `missing`, `malformed`, `out_of_scope`, `unsupported_*`, `other_frequency`) | A jump in `malformed` |
| `ref_unknown_fields_total` | `source`, `field` | Any: `BlehReferenceUnknownFields`; layout changed, nothing undocumented emitted |
| `ref_parse_errors_total` | `source` | Any: `BlehReferenceParseErrors` |
| `ref_stale_reports_total` | `source` | Feeder cattle file older than ten days |
| `ref_latest_value_date_seconds` | `source` | Newest value date emitted |
| `ref_backfill_units_total` | `source`, `outcome` | Backfill progress |

`/health` is `503` with the stale sources listed when a source has not
completed a clean cycle within three of its outside-window intervals.

## Incidents

See [incident.md](incident.md#reference-pollers-stalled).
