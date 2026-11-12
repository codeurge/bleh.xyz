# Phase 2: historical data — Plan

Status: Active (archive design now; recording starts with the Phase 1 node)
Last updated: 2026-11-11

## Goal

Once deployed, everything the mesh publishes live is also available as
history, permanently:

- a **raw archive** of node output, immutable, from the first day the node
  runs
- **normalized daily partitions** (trades, order-level book events, periodic
  book snapshots, bars) rebuilt deterministically from the raw archive
- a **historical API** for range queries
- **bulk downloads** of daily partitions
- a **backfill** from Hyperliquid's public archives, where admitted, so
  history starts before the mesh's own recording

## Backfill sources (to be admitted as ADM-0003)

Hyperliquid publishes history in AWS requester-pays buckets. The requester
pays transfer costs, and uploads are not guaranteed timely or complete.

| Bucket and path | Contents | Proposed use |
|---|---|---|
| `hl-mainnet-node-data/node_fills_by_block` (older: `node_fills`, `node_trades`) | Fills from a non-validating node | Trade history backfill |
| `hl-mainnet-node-data/replica_cmds`, `explorer_blocks` | Historical L1 transactions and blocks | Research: replay to rebuild order-level books |
| `hyperliquid-archive/market_data` | Hourly L2 book snapshots | Book-snapshot backfill |
| `hyperliquid-archive/asset_ctxs` | Asset contexts, including oracle, mark and funding | **Exclude** under ADM-0001's field rules |

Check coverage dates by listing the buckets. Secondary sources date them
differently.

## Acceptance criteria

- [ ] Every raw node file since go-live is in object storage, compressed,
      immutable and listed in a daily manifest.
- [ ] Daily manifests are hash-chained and signed; a verifier tool checks
      them.
- [ ] Normalized partitions rebuild byte-for-byte from the raw archive, and
      CI checks this on a fixture day.
- [ ] The historical API serves trades, bars (1s, 1m, 5m, 15m, 1h, 1d), a
      book snapshot at any time T, and book-event ranges, with documented
      limits.
- [ ] Bulk daily partitions are downloadable for every recorded day.
- [ ] A public coverage map shows recorded and missing hours per market.
      Gaps are flagged, never filled in.
- [ ] Backfilled data carries its own source and admission ID, and is
      flagged as backfill.

## Dependencies

- Phase 1: node-runbook (raw files), node-reader, book-builder, bar-builder.
- [ADR 0006](../../docs/decisions/0006-phase-1-2-service-architecture.md)
  accepted.
- ADM-0003 admitted before any backfill is published.

## Ordered work

1. **raw-archive**
   - Status: Blocked
   - Owner: Unassigned
   - Dependencies: Phase 1 node-runbook
   - PR: Not opened
   - Next action: Ship rotated node files to object storage (compressed,
     write-once), so recording starts the day the node goes live, even
     before the rest of this phase exists.

2. **manifest-chain**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: Phase 0 data-model-schema
   - PR: Not opened
   - Next action: Specify the daily manifest (file list, sizes, content
     hashes, previous manifest hash, signature) and build the verifier.

3. **normalized-partitions**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: Phase 1 node-reader and book-builder (synthetic data is
     enough to start)
   - PR: Not opened
   - Next action: Deterministic daily Parquet writer per market and kind
     (trades, book events, book snapshots every N seconds, 1s and 1m bars).
     Add DBN output if ADR 0004 is accepted.

4. **replay-tool**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: normalized-partitions
   - PR: Not opened
   - Next action: Rebuild normalized partitions from raw files for any day;
     add a CI determinism check on a fixture day.

5. **bar-rollups**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: Phase 1 bar-builder
   - PR: Not opened
   - Next action: Deterministic 5m, 15m, 1h and 1d bars from 1m bars; the
     daily boundary is UTC midnight, recorded in the registry.

6. **historical-api**
   - Status: Blocked
   - Owner: Unassigned
   - Dependencies: normalized-partitions, architecture-decision
   - PR: Not opened
   - Next action: HTTP API for range queries (trades, bars, book at time T,
     book events), with pagination, size limits and per-client rate limits,
     backed by the query engine chosen in ADR 0006.

7. **bulk-downloads**
   - Status: Blocked
   - Owner: Unassigned
   - Dependencies: normalized-partitions, manifest-chain
   - PR: Not opened
   - Next action: Publish daily partitions and manifests at a public HTTPS
     location. Content-addressed distribution comes in Phase 5.

8. **coverage-map**
   - Status: Blocked
   - Owner: Unassigned
   - Dependencies: raw-archive
   - PR: Not opened
   - Next action: Publish per-market, per-hour coverage as data and as a
     page; never fill gaps.

9. **admission-archive-buckets**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: None
   - PR: Not opened
   - Next action: Complete ADM-0003. Check the bucket terms, the
     requester-pays conditions, field exclusions (`asset_ctxs`), and whether
     archived fills match the live node's fields.

10. **backfill-fills**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: admission-archive-buckets, normalized-partitions
    - PR: Not opened
    - Next action: Import archived fills for the starter markets into
      backfill partitions with their own admission ID; check them against
      live-recorded days where the two overlap.

11. **backfill-book-snapshots**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: admission-archive-buckets
    - PR: Not opened
    - Next action: Import hourly L2 snapshots as low-resolution book history,
      clearly marked as such.

12. **replay-l1-research**
    - Status: Ready
    - Owner: Unassigned
    - Dependencies: admission-archive-buckets (for publishing only)
    - PR: Not opened
    - Next action: Test whether replaying `replica_cmds` through node
      software can rebuild full order-level history. If it can, the archive
      gets order-level depth back to the venue's start.

13. **cost-and-retention-model**
    - Status: Ready
    - Owner: Repository owner
    - Dependencies: Phase 1 node-runbook (measured growth)
    - PR: Not opened
    - Next action: Estimate storage growth (raw and normalized), egress for
      bulk downloads and requester-pays backfill costs; set the hosting
      budget. Raw is kept permanently.

14. **staging-and-production-deploy**
    - Status: Blocked
    - Owner: Repository owner
    - Dependencies: historical-api, bulk-downloads, coverage-map
    - PR: Not opened
    - Next action: Deploy to staging, verify every acceptance criterion
      against real recorded days, then deploy to production alongside the
      Phase 1 gateway.

## Exit

All acceptance criteria met, with the historical API and bulk downloads live.
