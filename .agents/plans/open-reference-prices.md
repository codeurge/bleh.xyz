# Open reference prices — Plan

Status: Active
Last updated: 2026-11-12

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

## Tasks

1. **reference-admissions**
   - Status: In progress
   - Owner: admissions lane
   - Dependencies: None
   - PR: Not opened (owner pushes to main)
   - Next action: research each source's terms and lineage; write one record per source

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
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: task 1 (only `Admitted` fields)
   - PR: Not opened
   - Next action: pollers and history backfill for the admitted sources, emitting records the synthetic engine reads
