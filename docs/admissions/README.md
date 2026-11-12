# Admission register

One record per admission decision, using [TEMPLATE.md](TEMPLATE.md). The
rules are in [admission-policy.md](../admission-policy.md), and the reason
admission is per field is in [ADR 0003](../decisions/0003-field-level-admission.md).

Statuses: `Requested`, `Under review`, `Admitted`, `Rejected`, `Revoked`.

Ingest code may merge only for fields covered by an `Admitted` record.
Every observation carries its admission ID.

| ID | Source | Fields | Status |
|---|---|---|---|
| [ADM-0001](ADM-0001-hyperliquid-trades-orders.md) | Hyperliquid node: trades, fills, order statuses, book diffs | Trade and order fields only; oracle, mark and funding excluded | Requested |
| [ADM-0002](ADM-0002-cftc-part43-swaps.md) | CFTC Part 43 public swap dissemination | Price, size, time, product, flags | Requested |

No source is admitted yet. The remaining Phase 0 candidates are listed in
[docs/sources.md](../sources.md) and tracked in
[.agents/plans/phase-0-foundation.md](../../.agents/plans/phase-0-foundation.md).
