# reference-pollers

Pollers and history backfill for the admitted public reference sources:
CFTC Commitments of Traders, Treasury rates and auctions, New York Fed
reference rates, Federal Reserve H.15 and H.10, USDA livestock, dairy and
feeder cattle prices, ERCOT prices (ADM-0005 to ADM-0013), EIA survey
prices (ADM-0021) and BLS consumer price indexes (ADM-0022). Emits
`reference_value` and `positioning` records. Owned by the reference-pollers
lane (open reference prices plan, task 4); see
`docs/decisions/0007-code-layout.md`.

Status: implemented and tested on synthetic fixtures only
(`fixtures/synthetic/reference/`); no test touches the network.

- Access specification (the readers' only input): `docs/specs/reference-sources.md`
- Running, configuration, metrics: `docs/operations/reference-pollers.md`
- Data model amendments: `docs/decisions/0015-v0-amendments-for-reference-pollers.md`

## Shape

Modelled on `crates/swap-pollers`: a binary with live polling (raw
capture, conditional requests, backoff, revision handling, JSON lines and
a gateway channel), plus a backfill that writes partitions through the
archive crate as `tools/hl-backfill` does.

| Module | Contents |
|---|---|
| `sources::*` | One reader per source: targets, schedule, parser with allowlists |
| `obs` | Observations to records: revisions, deterministic `seq`, flags, publication times |
| `revision` | Durable revision and dedupe state |
| `live`, `backfill` | Poll loop; yearly, resumable, idempotent backfill |
| `output`, `raw`, `http`, `metrics` | JSON lines and notices, raw capture, fetching, Prometheus |
| `catalog`, `source` | Registry entries with notices; source identities |

## Rules

- Only `Admitted` fields of `Admitted` records, through explicit
  allowlists; excluded fields are dropped before they become records, and
  tests check each source.
- No exchange data, no real data in the repository; fixtures are
  invented.
- Clean-room readers from documented formats only.
