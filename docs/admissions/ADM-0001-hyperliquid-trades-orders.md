# ADM-0001: Hyperliquid node — trade and order fields

- Status: Requested
- Requested: 2026-11-11 by maintainers
- Reviewers: To be assigned (two maintainers plus counsel)
- Decided:

## Source

Hyperliquid's L1, read from a non-validating node that the project runs. The
node writes per-block trades, fills, order statuses and raw book diffs, and
can compute an L4 book snapshot from node state. Covers core perpetuals and
HIP-3 builder-deployed markets.

## Fields requested

| Field | Meaning | Admit? |
|---|---|---|
| Trades and fills: price, size, aggressor side, time, block, market | Executions on the venue | Requested |
| Order statuses and book diffs: order ID, price, size, side, status, time | Order-level book events | Requested |
| L4 snapshot | Book checkpoint built from node state | Requested |
| Owner addresses | Account identifiers on orders and fills | **Exclude.** Not needed; reduces privacy risk |
| Oracle, mark, external and funding prices | Deployer-computed reference prices | **Exclude.** May be derived from exchange data (see [sources.md](../sources.md)) |

## Policy checks (to complete)

1. Original price formation: trades and orders are made on the venue.
   Confirm for each HIP-3 deployer's markets.
2. No contract on the path: data read from our own node. Confirm that node
   software licensing and any deployer terms carry no data restrictions.
3. Clean upstream: confirm that trade and order fields contain no oracle
   inputs. Note that oracle-anchored funding and weekend bands **influence**
   trading on these markets. Admission is for the trades themselves, which
   are flagged `oracle_anchored_venue`.
4. Exclusions: owner addresses; all oracle, mark, external and funding
   fields.

## Legal basis

To be completed by counsel.

## Risks and review triggers

- A deployer's terms add data restrictions.
- A market's trades are shown to be priced by an oracle, not matched in the
  order book.
- Concentration: one deployer holds most HIP-3 open interest.
