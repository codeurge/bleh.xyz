# Admission register

One record per admission decision, using [TEMPLATE.md](TEMPLATE.md). The
rules are in [admission-policy.md](../admission-policy.md), and the reason
admission is per field is in [ADR 0003](../decisions/0003-field-level-admission.md).

Statuses: `Requested`, `Under review`, `Admitted`, `Rejected`, `Revoked`.

Ingest code may merge only for fields covered by an `Admitted` record.
Every observation carries its admission ID.

| ID | Source | Fields | Status |
|---|---|---|---|
| [ADM-0001](ADM-0001-hyperliquid-trades-orders.md) | Hyperliquid node, every market (core, HIP-3, HIP-4): book-matched fills, order statuses, book diffs, L4 snapshots | Venue-matched fields only; owner addresses, oracle, mark, funding, liquidation and settlement fields excluded | Admitted |
| [ADM-0002](ADM-0002-cftc-part43-swaps.md) | CFTC Part 43 public swap dissemination | Price, size, time, product, option terms, flags | Admitted (Phases 1–2 lane) |
| [ADM-0003](ADM-0003-hyperliquid-archive-buckets.md) | Hyperliquid public archive buckets | Historical fills, L1 data, L2 snapshots (one file per coin per hour); `asset_ctxs` and `misc_events_by_block` excluded | Admitted |
| [ADM-0004](ADM-0004-sec-sbsr-public-dissemination.md) | SEC Regulation SBSR public dissemination | Price, size, time, product, flags | Admitted (Phases 1–2 lane) |

Admission is a maintainer's engineering decision about field lineage. The
remaining candidates are listed in [docs/sources.md](../sources.md), with
their phases in [docs/roadmap.md](../roadmap.md).
