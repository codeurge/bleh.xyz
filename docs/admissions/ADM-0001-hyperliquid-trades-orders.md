# ADM-0001: Hyperliquid node — trade and order fields (all markets)

- Status: Admitted
- Requested: 2026-11-11 by maintainers
- Reviewers: Repository owner (engineering review of field lineage)
- Decided: 2026-11-11 by the repository owner (owner decision, 2026-11-11)
- Amended: 2026-11-14: `subBuilders` and `priorityGas` named in the excluded account and fee fields, after both (and `cloid`) were seen in archived node output (ADM-0003 sample, 2026-11-14). Exclusion only; nothing newly admitted.

## Source

Hyperliquid's L1, read from non-validating nodes that the project runs (two,
on two providers). The node writes per-block fills, order statuses and raw
book diffs, and serves an L4 book snapshot on request (`--serve-info` with a
`fileSnapshot` request for `l4Snapshots`).

Scope: **every market on the node** — core perpetuals, every HIP-3
deployer's markets and HIP-4 outcome markets — recorded and published from
day one (owner decision, 2026-11-11). New markets register automatically
from node metadata. Each market carries a published liquidity grade
(spread, depth, trades per minute), so thin markets are labelled as tapes,
not benchmarks.

## Fields

| Field | Meaning | Admit? |
|---|---|---|
| Book-matched fills: `coin`, `px`, `sz`, `side`, `time`, `crossed`, `oid`, `tid`, `hash`, block number and time | Executions matched in the order book. A book-matched trade is one `tid` with exactly two fills, one per side, one of them `crossed: true` (the aggressor) | **Admitted** |
| Order statuses and raw book diffs: order ID, coin, price, size, side, status, time, block | Order-level book events | **Admitted** |
| L4 snapshot (orders only) | Book checkpoint from node state, requested without users | **Admitted** |
| Market metadata: coin, deployer, tick and lot size, max leverage | Registry facts for auto-registration | **Admitted** |
| Owner addresses (`user`, `side_info[].user`, `liquidatedUser`) | Account identifiers | **Excluded.** Identifies users |
| Fill `liquidation` object | Contains `markPx` and liquidation method | **Excluded.** Echoes the mark price |
| Liquidation, settlement and halt fills | HIP-3 `haltTrading` cancels orders and settles positions at the current mark price | **Excluded.** Priced by mark, not matched |
| HIP-4 settlement values | Validator- or deployer-supplied outcome settlement prices and the fills they produce | **Excluded.** Not matched in the book |
| ADL fills | Auto-deleveraging fills | **Excluded** if not book-matched. To verify how they appear in node output |
| `fee`, `feeToken`, `closedPnl`, `startPosition`, `dir`, `builder`, `builderFee`, `subBuilders`, `deployerFee`, `priorityGas`, `twapId`, `cloid` | Account, fee and order-plumbing details (`cloid` is the client order ID; `priorityGas` a per-order priority fee) | **Excluded.** Account-level, not market data |
| Oracle, mark, external and funding prices | Deployer- or validator-supplied reference prices | **Excluded.** Echo outside prices |
| Node output from `--write-hip3-oracle-updates` and `--write-misc-events` | Deployer oracle updates; funding, ledger and liquidation events | **Excluded.** These flags are never enabled |

## Policy checks

Under [admission-policy.md](../admission-policy.md):

1. Original price formation: fills, order statuses and book diffs are the
   venue's own matching and order events, in every market.
2. No contract on the path: data is read from our own nodes. The
   [hyperliquid-dex/node](https://github.com/hyperliquid-dex/node) repository
   (setup scripts and docs) is Apache-2.0. The node itself (`hl-visor`, which
   runs `hl-node`) is a closed binary downloaded from
   `binaries.hyperliquid.xyz` and GPG-verified. Our reader is written from
   the documented output schemas, not from other repositories' code.
3. Venue-matched fields only: only book-matched fills are admitted, and all
   oracle, mark, funding, liquidation and settlement fields are excluded.
   Outside prices still shape trading:
   - HIP-3 mark is a median of deployer-supplied marks and a local book mark,
     moves at most 1% per update, and drives margining and liquidations.
   - trade.xyz bounds its mark within ±(1/max leverage) of an external
     reference in all sessions; it does not name its reference sources.
   - Core perpetual mark and funding use Hyperliquid's validator oracle,
     which Hyperliquid describes as a weighted median of centralized-exchange
     spot prices (to verify against current docs).
   Every record from an oracle-anchored market carries
   `oracle_anchored_venue`. See [sources.md](../sources.md).
4. Exclusions: as listed in the table above.

## Source basis

Public chain state of Hyperliquid's L1, produced by non-validating nodes the
project runs, with output flags limited to `--write-fills`,
`--write-order-statuses`, `--write-raw-book-diffs`, `--batch-by-block`,
`--disable-output-file-buffering` and `--serve-info`. Schemas:
<https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/nodes/l1-data-schemas>
and the node README, reviewed 2026-11-11. HIP-4 output schemas: to verify in
node output.

## Risks and review triggers

- A node upgrade changes output schemas (`hl-visor` updates `hl-node`
  automatically). The reader rejects unknown shapes and alerts.
- The two nodes disagree on any `(block, event_index)`: integrity alarm.
- A market's trades are shown to be priced by an oracle, not matched in the
  book.
- A deployer changes its mark or oracle method.
- Concentration: one deployer holds most HIP-3 open interest.

## Decision

Admitted on 2026-11-11 by the repository owner for the admitted fields above,
for every Hyperliquid market.
