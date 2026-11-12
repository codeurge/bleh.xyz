# ADM-0003: Hyperliquid public archive buckets — historical fills, L1 data and book snapshots

- Status: Admitted
- Requested: 2026-11-11 by maintainers
- Reviewers: Repository owner (engineering review of field lineage)
- Decided: 2026-11-11 by the repository owner (owner decision, 2026-11-11)

## Source

Hyperliquid publishes historical data in AWS requester-pays S3 buckets. Its
docs say uploads happen roughly monthly, with no guarantee of timeliness or
completeness. The project reads them with its own AWS account and pays the
transfer cost. Buckets are copied as-is into the private raw tier, then
filtered into public filtered raw under the field rules below.

## Fields

| Location | Content | Admit? |
|---|---|---|
| `hl-mainnet-node-data/node_fills_by_block` (and older `node_fills`, `node_trades`) | Fills: price, size, side, time, market | **Admitted**, under ADM-0001's field rules: book-matched fills only; owner addresses, liquidation objects, account and fee fields excluded |
| `hl-mainnet-node-data/replica_cmds`, `explorer_blocks` | L1 transactions and blocks | **Admitted** for order-level replay through an emulated matcher, checkpointed against archived L2 snapshots; only ADM-0001 fields are published from replay output |
| `hyperliquid-archive/market_data` | L2 book snapshots, one file per coin per hour. Snapshot frequency inside each file: to verify | **Admitted** |
| `hl-mainnet-node-data/misc_events_by_block` | Funding, ledger and liquidation events | **Excluded.** Same reasons as ADM-0001 |
| `hyperliquid-archive/asset_ctxs` | Asset contexts with oracle, mark and funding | **Excluded.** Same reasons as ADM-0001 |

## Policy checks

Under [admission-policy.md](../admission-policy.md):

1. Original price formation: same venue as ADM-0001.
2. No contract on the path: the buckets are published openly by the venue
   operator. Requester-pays access needs an AWS account; this is the
   publisher's-open-bucket route in rule 2, adopted on 2026-11-11 (owner
   decision, 2026-11-11). The buckets' own terms are rechecked at each
   import.
3. Venue-matched fields only: archived fills go through the same filter as
   live fills. Replay output is filtered the same way.
4. Exclusions: owner addresses; liquidation, settlement and account fields;
   `misc_events_by_block`; `asset_ctxs`.

## Source basis

Hyperliquid's documentation of its historical data buckets, and listings of
the buckets themselves. Coverage dates are taken from bucket listings, not
secondary sources.

## Risks and review triggers

- Terms added to the buckets.
- Archived data differs in content or schema from live node output. Overlap
  days are compared against live recording.

## Decision

Admitted on 2026-11-11 by the repository owner for the locations and fields
above.
