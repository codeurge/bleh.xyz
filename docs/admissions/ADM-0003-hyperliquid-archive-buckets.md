# ADM-0003: Hyperliquid public archive buckets — historical fills, L1 data and book snapshots

- Status: Requested
- Requested: 2026-11-11 by maintainers
- Reviewers: To be assigned (two maintainers plus counsel)
- Decided:

## Source

Hyperliquid publishes historical data in AWS requester-pays S3 buckets. Its
docs say uploads happen roughly monthly, with no guarantee of timeliness or
completeness.

## Fields requested

| Location | Content | Admit? |
|---|---|---|
| `hl-mainnet-node-data/node_fills_by_block` (and older `node_fills`, `node_trades`) | Fills: price, size, side, time, market | Requested; owner addresses excluded |
| `hl-mainnet-node-data/replica_cmds`, `explorer_blocks` | L1 transactions and blocks | Requested, for order-level replay research |
| `hyperliquid-archive/market_data` | Hourly L2 book snapshots | Requested |
| `hyperliquid-archive/asset_ctxs` | Asset contexts with oracle, mark and funding | **Exclude** (same reasoning as ADM-0001) |

## Policy checks (to complete)

1. Original price formation: same venue as ADM-0001.
2. No contract on the path: requester-pays access goes through an AWS
   account. Check whether AWS's or Hyperliquid's terms add data
   restrictions.
3. Clean upstream: confirm that archived fills and snapshots contain no
   oracle inputs.
4. Exclusions: owner addresses; `asset_ctxs`.

## Legal basis

To be completed by counsel.

## Risks and review triggers

- Terms added to the buckets.
- Archived data found to differ in content from live node output.
