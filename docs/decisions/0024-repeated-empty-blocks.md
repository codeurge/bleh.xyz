# 0024: The backfill skips a repeated Hyperliquid block that has no fills

- Status: Accepted
- Date: 2026-11-15
- Deciders: Repository owner (agent working directly for the owner)
- Amends: [0019](0019-backfill-registry-from-admitted-fills.md) (how
  `hl-backfill process` reads by-block fills); the records it writes do
  not change

## Context

`hl-backfill process` reads the node's by-block fills objects through the
live node reader, whose sequencer accepts only block numbers above the last
one sequenced. Any other line fails the whole hourly object.

Processing August 2025 failed on one object, the hour starting
2025-08-23 08:00 UTC. Its 36,633 lines have block numbers that only
increase except at one place: block 706112990 at 08:47:13 is followed by
706113001 at 08:48:17, and 706113001 appears again at 08:57:37 with a new
block time, then numbering continues from 706113002. Both copies of
706113001 have no fill events. The pause and the repeat fit a node that
stopped and, on restart, wrote its last block again. Failing the object
drops an hour of real trades from 2025-08-23.

## Decision

1. In the backfill's by-block parser, a line whose block number is at or
   below the last block sequenced **and** whose `events` list is empty is
   skipped. The report counts such lines as `repeated_empty_blocks`.
2. A repeated or lower block that carries fills still fails the object,
   as before. Choosing which copy of a non-empty block is right needs a
   person.
3. The live reader's sequencer is not changed.

## Consequences

- A skipped line produces no records and leaves the sequencer state
  unchanged, so `seq`, `gap_before` and every record of every other hour
  are exactly what they were. No method version changes.
- The 2025-08-23 partitions written without hour 08 by the failed run are
  deleted before that day is processed again. Write-once would otherwise
  reject the complete files. No day had been signed, because signing waits
  for months whose runs all exited cleanly
  ([archive runbook](../operations/archive.md#records-archives)).

## Alternatives considered

- **Skip the whole object.** This loses an hour of real trades for an
  empty line.
- **Treat any repeated block as a gap and continue.** This would silently
  drop or double fills if a non-empty block were ever written twice.
- **Change the live sequencer.** Live reading has its own restart handling;
  the backfill needs only this narrow case.
