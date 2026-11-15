# Web snapshot v0

Small JSON files summarizing the latest poller records, for static sites
and quick consumers ([ADR 0016](../decisions/0016-web-snapshots.md)). Built
by `archive snapshot` ([operations](../operations/web-snapshots.md)).

Published at `https://data.bleh.xyz/web/v0/`. Every file is UTF-8 JSON with
`"format": "bleh-web-snapshot/v0"`. Fields that would be `null` are omitted.
Decimal values are strings exactly as in the records
([observation v0](observation-v0.md)); enum values use the records' names.

## Files

| Path | Contents |
|---|---|
| `latest.json` | Index: generation time, sources, files |
| `reference/<source>.json` | Reference series of one source |
| `positioning/<source>/<report>.json` | One report family's markets |
| `swaps/<source>.json` | Swap print counts and recent prints |

Readers start at `latest.json` and fetch only the files they need. Files
other than `latest.json` change only when their data changes; `latest.json`
is rewritten on every pass and uploaded last.

## `latest.json`

| Field | Meaning |
|---|---|
| `generated_ms` | When the snapshot was built (Unix ms) |
| `sources[]` | One entry per source, sorted by name |
| `sources[].source` | The records' `prov.venue` |
| `sources[].admission` | Admission ID covering the records |
| `sources[].notice` | Notice required by the admission, if any; show it with the values |
| `sources[].kinds` | Record kinds seen (`reference_value`, `positioning`, `swap_print`) |
| `sources[].records` | Records read |
| `sources[].series` | Reference series plus positioning markets |
| `sources[].latest_as_of` | Latest `as_of_date` (reference, positioning) or UTC day of the latest print |
| `sources[].last_record_ms` | Latest `prov.ts_source_ms` |
| `sources[].files` | This source's snapshot files |
| `files[]` | `path`, `bytes`, `sha256` of every other file |

## Common header

Every per-source file has `format`, `source`, `admission`, `notice` (if
any) and `last_record_ms`.

## `reference/<source>.json`

`series[]`, one per (instrument, measure, tenor, maturity), sorted by that
key:

| Field | Meaning |
|---|---|
| `instrument`, `measure`, `tenor`, `maturity_date`, `day_count`, `compounding`, `currency`, `unit` | From the latest record. `unit` is set on quantities (`measure` `stock`, `flow`, `capacity`, `ratio`; [ADR 0023](../decisions/0023-reference-quantities.md)) and absent otherwise; show it instead of `currency` |
| `flags` | The latest record's `prov.flags` (for example `exchange_referenced`) |
| `intraday` | True for series with delivery intervals |
| `latest` | `as_of_date`, `period_start_ms`, `period_end_ms`, `value`, `revision` |
| `points` | Daily series only: `[as_of_date, value]`, oldest first, at most 260 by default; the highest revision of each date |

The latest value is the one with the latest (`as_of_date`,
`period_start_ms`); a revision of the same date or interval replaces it.

## `positioning/<source>/<report>.json`

`report`, and `markets[]`, one per (contract market code, combined):

| Field | Meaning |
|---|---|
| `contract_market_code`, `combined`, `flags` | From the records |
| `as_of_date`, `open_interest` | The latest report |
| `categories` | Category name → `long`, `short`, `spreading` |
| `previous` | The report before it, same shape (`as_of_date`, `open_interest`, `categories`) |

## `swaps/<source>.json`

| Field | Meaning |
|---|---|
| `daily_counts[]` | `date` (UTC day of dissemination), `asset_class`, `prints`; the latest 14 days by default |
| `recent` | Asset class → the latest prints as full `swap_print` records, newest first, 50 by default |

Counts are derived by bleh.xyz from the disseminated prints and count every
print, including cancels and corrections.

## Caching

On `data.bleh.xyz`, `/web/` is cached at the edge for 60 seconds and in
browsers for 30. Objects also carry `Cache-Control: public, max-age=60`.
CORS allows `GET` and `HEAD` from any origin.

## Versioning

Adding fields is compatible. Removing or renaming a field, or changing a
meaning, publishes `web/v1/` beside `web/v0/`.
