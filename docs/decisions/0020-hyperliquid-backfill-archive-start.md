# 0020: The `hyperliquid-backfill` archive starts at 2025-07-27

- Status: Accepted
- Date: 2026-11-14
- Deciders: Repository owner (decision D2 in
  [hyperliquid-history-to-site](../../.agents/plans/hyperliquid-history-to-site.md))

## Context

Hyperliquid's archive buckets
([ADM-0003](../admissions/ADM-0003-hyperliquid-archive-buckets.md)) hold
three trade datasets
([buckets spec § 5](../specs/hyperliquid-archive-buckets.md)):

| Dataset | Period | Block numbers |
|---|---|---|
| `node_trades` | 2025-03-22 to 2025-06-21 | none |
| `node_fills` | 2025-05-25 to 2025-07-27 | none (bare `[user, fill]` lines) |
| `node_fills_by_block` | 2025-07-27 onward | in every line |

Records need `seq.block_number`. `process` resolves it for data without
block numbers from a clock of block times, and skips events outside the
clock rather than guessing ([backfill.md](../operations/backfill.md) § 5).
Before 2025-07-27 there are no batched lines to build a clock from, and the
exact sources of block times (`explorer_blocks`, 27.7 TB; `replica_cmds`,
89.3 TB) are far outside the project's budget. A first run on a 2025-03-22
hour confirmed it: 9,098 of 10,343 kept trades had no resolvable block.

The manifest chain only moves forward (`next.date > prev.date`,
[manifest-v0](../specs/manifest-v0.md) § 4), so whichever day is signed
first fixes the archive's start for good.

## Decision

1. **The `hyperliquid-backfill` archive starts at 2025-07-27**, the first
   day of `node_fills_by_block`, whose trades come through the node reader's
   live code path with exact block numbers.
2. **The early period (2025-03-22 to 2025-07-26) is kept complete in the
   private raw tier** and is not processed into `hyperliquid-backfill`.
3. It is published later as **a separate archive**,
   `hyperliquid-backfill-early`, with its own manifest chain, once a further
   ADR fixes how block-less trades are sequenced (a new method version; no
   `block_number` guessed; records flagged accordingly).

## Consequences

- The first signed day of `hyperliquid-backfill` is 2025-07-27; nothing
  earlier can join that chain.
- About 47 GB of raw (`node_trades` and `node_fills`) stays private until
  the early-period method exists. It is not discarded.
- `node_fills` on 2025-07-27 overlaps the first by-block day and is used
  for the overlap check, not for records.

## Alternatives considered

- **Hold all publishing until the early period is solved.** Rejected: it
  delays every day from 2025-07-27 for a period whose method is undecided.
- **Assign approximate block numbers from timestamps.** Rejected: the
  project never guesses `block_number`.
- **Buy `explorer_blocks` for the early period.** Rejected: thousands of
  dollars at requester-pays prices.
