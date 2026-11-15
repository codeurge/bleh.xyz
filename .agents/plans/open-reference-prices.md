# Open reference prices — Plan

Status: Active
Last updated: 2026-11-14

## Goal

Get as close as clean lineage allows to a full alternative for exchange
futures data, live and historical:

- **Settlement anchors.** Admit the public-domain and free-by-law sources
  that many cash-settled futures settle against (agricultural cash prices,
  dairy prices, grid operators' electricity prices, Treasury yields and
  auction results, overnight reference rates) plus futures positioning
  (Commitments of Traders), and publish them live and with full history.
- **Synthetic contract months.** A versioned method and engine that produce
  a fair-value series for each contract month of an exposure (equity index,
  rates, energy, metals) from independent-venue spot prices, public rates,
  rulebook calendars, carry estimates and, where available, swap-print
  forward points, under neutral names with liquidity and quality flags.
- **Bring your own feed.** A client-side adapter that lets a user merge a
  feed they license themselves with bleh.xyz data on their own machine. That
  data never enters the repository, the archive or the network.

## Acceptance criteria

- Every new source has an admission record with a decision and field list;
  ingest code exists only for `Admitted` fields.
- The synthetic method is specified as `v1`, reproducible from published
  inputs, with an ADR, and its engine passes tests on synthetic fixtures.
- The bring-your-own-feed boundary is recorded in an ADR, enforced in code
  (no path from user-supplied data to signing, publishing or the archive),
  and tested on synthetic fixtures only.

## Reserved numbers

| Lane | Numbers |
|---|---|
| Admissions | ADM-0005 to ADM-0019; ADR 0012 if the admission policy needs an amendment |
| Synthetic contract months | ADR 0013; method spec `docs/specs/method-synthetic-months-v1.md`; crate `crates/synthetic` |
| Bring your own feed | ADR 0014 |
| Web snapshots | ADR 0016; spec `docs/specs/web-snapshot-v0.md` |
| Open sources survey | ADM-0020 to ADM-0022; survey `docs/research/open-sources-survey-2026-11.md` |

## Tasks

1. **reference-admissions**
   - Status: Done
   - Owner: admissions lane
   - Dependencies: None
   - PR: Not opened (owner pushes to main)
   - Next action: None. ADR 0012 and ADM-0005 to ADM-0019 decided (federal, Federal Reserve, USDA and ERCOT admitted with field exclusions; five grid operators and grain bids rejected; NYISO under review); access spec in `docs/specs/reference-sources.md`

2. **synthetic-months-method-and-engine**
   - Status: Done (merged to main 2026-11-12)
   - Owner: synthetic lane
   - Dependencies: None (rates inputs are fixtures until task 4 lands)
   - PR: Not opened (owner pushes to main)
   - Done: ADR 0013 (Accepted), method spec `synthetic-months@1`, the
     `reference_value` and `synthetic_month` kinds, `crates/synthetic`
     (equity index, crude and natural gas, gold and silver, SOFR months;
     Treasury futures specified for v2)
   - Next action: wire the engine into the origin behind a configuration
     flag, and replace the synthetic registry entries with real venue
     markets once task 4 emits `reference_value` records

3. **bring-your-own-feed**
   - Status: Done (merged to main 2026-11-12)
   - Owner: client lane
   - Dependencies: None (`synthetic_month` and `exposure` mapping targets
     activate once task 2 publishes those series)
   - PR: Not opened (owner pushes to main)
   - Done: ADR 0014 (Accepted); admission policy section "User-supplied
     data stays local"; `mesh_client::byo` and `mesh-client byo merge` /
     `check-map` with CSV, JSON-lines and DBN-file connectors and the
     vendor live client behind `databento-live`; boundary enforced by
     `UserSupplied`, `LocalPath` and `LoopbackAddr` with compile-fail and
     source-scan tests; synthetic-only tests; user guide
     `docs/operations/byo-feed.md`
   - Next action: wire the synthetic-month target to the series task 2
     publishes

4. **reference-pollers**
   - Status: Done (merged to main 2026-11-12)
   - Owner: reference-pollers lane
   - Dependencies: task 1 (only `Admitted` fields)
   - PR: Not opened (owner pushes to main)
   - Done: `crates/reference-pollers` with live polling and backfill for
     ADM-0005 to ADM-0013 (feeder cattle live only: no admitted history
     channel); ADR 0015 (Accepted): `exchange_referenced` bit 7,
     `positioning` kind, reference-value periods, manifest notices; the
     synthetic engine's reference-poller curve preset; compose, systemd,
     alerts and runbook `docs/operations/reference-pollers.md`; tests on
     synthetic fixtures only
   - Next action: volumes and tenorless rates (runbook open point); then a
     first real backfill into `reference-backfill`. Done since: the DTCC
     sentence of the NY Fed notice (2026-11-13), and the
     `reference-normalized` archive (ADR 0017), signed and published at
     `https://data.bleh.xyz/reference-normalized` from 2026-11-13

5. **web-snapshots**
   - Status: Done
   - Owner: owner session
   - Dependencies: task 4 (record files to read)
   - PR: Not opened (owner pushes to main)
   - Done: ADR 0016 (Accepted) and spec `web-snapshot-v0`; `archive
     snapshot` (incremental, bounded, uploads changed files to
     `BLEH_ARCHIVE_WEB_*`, `latest.json` last); compose `web-snapshot`
     service; edge cache rule for `/web/`; the site's reference data
     panel with source notices; runbook `docs/operations/web-snapshots.md`.
     Found while building it: the Federal Reserve readers emitted the
     SDMX no-data placeholder (-9999) as values; fixed.
   - Next action: none. Live since 2026-11-13: the home-archive server
     uploads to the public bucket and bleh.xyz shows the panel from
     `https://data.bleh.xyz/web/v0/`. Free-tier guidance and
     `scripts/r2-usage.sh` are in the runbook.

6. **open-sources-survey**
   - Status: In review
   - Owner: research lane
   - Dependencies: None
   - PR: Not opened (owner pushes to main)
   - Done: survey `docs/research/open-sources-survey-2026-11.md` (ranked
     candidates, rejections, next-5 queue, costs); ADM-0020 (Ethereum
     Uniswap pools), ADM-0021 (EIA) and ADM-0022 (BLS CPI) drafted with
     status `Requested`
   - Next action: owner decisions on ADM-0020 to ADM-0022 and on the
     survey's policy questions (non-US public bodies under ADR 0012,
     notices on derived records); then draft the foreign overnight-rate
     and inflation records (queue items 4 and 5). ADM-0021 and ADM-0022
     were admitted on 2026-11-15; their readers are task 7

7. **eia-bls-readers**
   - Status: In review
   - Owner: reference-pollers lane
   - Dependencies: task 4 (reference-pollers crate); ADM-0021 and ADM-0022
     `Admitted`
   - PR: branch `feat/eia-bls-readers` (the owner merges)
   - Done: sources `eia` (bulk `PET.zip`, `NG.zip`; explicit series
     allowlist; excluded families and vendor-credited series dropped;
     weekly retail and residential prices; monthly survey prices flagged
     `exchange_referenced`) and `bls` (`cu`, `cw`, `su`, `ap` data files;
     index levels and average prices; seasonal and C-CPI-U revisions;
     `--bls-areas`, default U.S. city average); notices; `--contact`
     User-Agent address; access specification sections; runbook section;
     compose variables; tests on synthetic fixtures
   - Next action: owner merge and deploy, then a first `eia` and `bls`
     backfill into `reference-backfill`. Open: EIA stocks, storage and
     supply are admitted but not emitted until a record kind holds a
     quantity (the same open point as task 4's volumes; needs an ADR)
