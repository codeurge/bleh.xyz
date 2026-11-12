# Phase 1: live data — Plan

Status: Active (build against synthetic data; ingest merges after ADM-0001)
Last updated: 2026-11-11

## Goal

Once deployed, anyone can connect to a public gateway and receive live data
for the starter markets, every record carrying provenance:

- trades with native aggressor side
- top of book and L2 depth
- order-level book events, with snapshot plus deltas
- 1-second and 1-minute bars

**Starter markets:** the HIP-3 markets tracking the S&P 500, gold, silver,
WTI and Brent, plus the BTC and ETH core perpetuals. Exact market IDs are
fixed in the registry (task 4).

## Source

A non-validating Hyperliquid node run by the project, writing per-block
fills, order statuses and raw book diffs, and periodic state from which an
L4 book snapshot is computed. Fields are limited to those requested in
[ADM-0001](../../docs/admissions/ADM-0001-hyperliquid-trades-orders.md).
Owner addresses and all oracle, mark, external and funding fields are
dropped at parse time.

Node facts to plan around (verify at provisioning):

- Official minimum for a non-validator: 16 cores, 128 GB RAM, 500 GB SSD,
  Ubuntu 24.04 only.
- Default settings produce about 100 GB of logs per day, and extra output
  flags raise that sharply (vendors report 1–2.6 TB/day with everything
  enabled). Enable only the streams we need, and rotate files into the raw
  archive (Phase 2).

## Acceptance criteria

- [ ] Gateway publishes each record within 250 ms (p99) of the node writing
      the block, measured in staging over 7 days.
- [ ] Rebuilt books match node L4 snapshots at every reconciliation
      checkpoint (at least every 10 minutes); mismatches alert and trigger a
      resync.
- [ ] Bars are deterministic: rebuilding from the same trades gives
      identical bars.
- [ ] Every published record carries instrument, venue, source and ingest
      timestamps, sequence and admission ID. A test fails the build if any
      non-allowlisted field reaches the output.
- [ ] Gateway availability is at least 99.5% over a 7-day staging soak, with
      public health and freshness endpoints.
- [ ] Protocol documentation and a reference client are published.
- [ ] ADM-0001 is `Admitted` before the node reader merges.

## Dependencies

- Phase 0: `data-model-schema`, `registry-schema`, `wire-format-decision`
  (binary encoding only).
- [ADR 0006](../../docs/decisions/0006-phase-1-2-service-architecture.md)
  accepted.
- ADM-0001 admitted (for merging the node reader and going live; everything
  else builds against synthetic data).

## Ordered work

1. **architecture-decision**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: None
   - PR: Not opened
   - Next action: Review ADR 0006 (language, bus, gateway protocol, hosting
     shape) and move it to Accepted or revise it.

2. **hosting-and-node-provisioning**
   - Status: Ready
   - Owner: Repository owner
   - Dependencies: architecture-decision
   - PR: Not opened
   - Next action: Choose a host that meets the node minimums with NVMe
     storage, low latency to Hyperliquid peers (the Tokyo region is common),
     and S3-compatible object storage nearby. Write
     `docs/operations/node.md` with the runbook.

3. **node-runbook**
   - Status: Blocked
   - Owner: Unassigned
   - Dependencies: hosting-and-node-provisioning
   - PR: Not opened
   - Next action: Run the non-validating node with output limited to fills,
     order statuses, raw book diffs and periodic state; measure disk growth
     per day; set rotation into the raw archive.

4. **registry-starter-markets**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: Phase 0 registry-schema
   - PR: Not opened
   - Next action: Add registry entries for the starter markets: venue IDs,
     tick and lot sizes, session behavior (24/7, with weekend bands noted),
     and the `oracle_anchored_venue` flag where it applies.

5. **synthetic-fixtures**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: Phase 0 data-model-schema
   - PR: Not opened
   - Next action: Generate synthetic node-format files (fills, order
     statuses, book diffs, snapshots) under `fixtures/synthetic/`, including
     edge cases: partial fills, cancels, modifies, gaps and reorg-free block
     boundaries.

6. **node-reader**
   - Status: Ready (build); merge Blocked on ADM-0001
   - Owner: Unassigned
   - Dependencies: synthetic-fixtures
   - PR: Not opened
   - Next action: Tail node output files, parse to normalized observations,
     enforce the field allowlist and drop excluded fields at parse time.

7. **book-builder**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: synthetic-fixtures
   - PR: Not opened
   - Next action: Build order-level books from an L4 snapshot plus the
     order-status stream; derive L2 and top of book; reconcile against fresh
     snapshots and resync on mismatch.

8. **bar-builder**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: Phase 0 data-model-schema
   - PR: Not opened
   - Next action: Deterministic 1-second and 1-minute bars from trades, with
     buy and sell volume split by aggressor side.

9. **internal-bus**
   - Status: Blocked
   - Owner: Unassigned
   - Dependencies: architecture-decision
   - PR: Not opened
   - Next action: Set up the internal bus with one subject per instrument
     and record kind, with replay from a bounded window for gateway restarts.

10. **gateway**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: internal-bus
    - PR: Not opened
    - Next action: Public websocket gateway: subscribe by instrument and
      kind; book snapshot plus deltas with sequence numbers; JSON and binary
      encodings; per-connection rate limits; `/health` and `/freshness`.

11. **observability**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: node-reader, gateway
    - PR: Not opened
    - Next action: Metrics for block lag, sequence gaps, book reconciliation
      results, publish latency and client counts, plus alerts and a public
      status page.

12. **protocol-docs-and-client**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: gateway
    - PR: Not opened
    - Next action: Document the gateway protocol in `docs/protocol/`; ship a
      minimal reference client that keeps a correct local book.

13. **staging-soak**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: node-runbook, gateway, observability, ADM-0001
    - PR: Not opened
    - Next action: Run 7 days in staging against live node data; verify the
      latency, reconciliation and availability criteria.

14. **production-launch**
    - Status: Blocked
    - Owner: Repository owner
    - Dependencies: staging-soak; Phase 0 public-name and
      contacts-and-licensing-review
    - PR: Not opened
    - Next action: Deploy the production gateway. Opening it to the public
      waits on the public name and counsel's confirmation of ADR 0002.

## Exit

All acceptance criteria met and the production gateway is live. Phase 2's
archive must already be recording by then.
