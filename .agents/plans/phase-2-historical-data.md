# Phase 2: historical data — Plan

Status: Active (backfill and archive design now; live recording starts with
the first node)
Last updated: 2026-11-12

## Goal

Once deployed, everything the mesh publishes live is also history that
anyone can query, download and verify:

- a **two-tier raw archive**: unfiltered raw (private, contains owner
  addresses, kept as long as storage allows) and filtered raw (admitted
  fields only, the public reproducibility root), from the first node's first
  day (owner decision, 2026-11-11)
- **normalized partitions**: Hive-partitioned Parquet with a catalog,
  rebuilt from filtered raw by replay, queried directly from a CDN
- **signed, anchored manifests**: hash-chained daily manifests, timestamped
  on Bitcoin and anchored on an Ethereum L2
- **distribution** over HTTPS, BitTorrent, IPFS and Arweave, with public
  mirrors, from the first recorded day
- **compatibility layers** so existing tools connect unchanged
- **backfill** from Hyperliquid's public buckets back to the earliest
  available date, including order-level history rebuilt from `replica_cmds`
- a **second venue**, admitted and recording

## Backfill sources (ADM-0003, Admitted)

Hyperliquid publishes history in AWS requester-pays buckets. The requester
pays transfer costs, and uploads are not guaranteed timely or complete.

| Bucket and path | Contents | Use |
|---|---|---|
| `hl-mainnet-node-data/node_fills_by_block` (older: `node_fills`, `node_trades`) | Fills from a non-validating node | Trade history |
| `hl-mainnet-node-data/replica_cmds`, `explorer_blocks` | L1 transactions and blocks | Order-level replay through an emulated matcher |
| `hyperliquid-archive/market_data` | L2 snapshots, one file per coin per hour (frequency inside to verify) | Book history and replay checkpoints |
| `hyperliquid-archive/asset_ctxs`, `hl-mainnet-node-data/misc_events_by_block` | Oracle, mark, funding, ledger events | **Excluded** |

Coverage dates come from bucket listings, not secondary sources.

## Integration status (2026-11-12)

The archive, history API and backfill tooling are merged and wired: the
`archive` binary carries the node reader's filters and the origin's parser,
live capture runs in the origin, and `hl-backfill` uses the reader and the
archive's Parquet writer. Replay of synthetic days is byte-identical across
runs and matches the live stream (crates/origin/tests/e2e.rs); backfilled
partitions read back through the history API (tools/hl-backfill/tests).
Nothing runs on real data yet, so no criterion is ticked.

## Acceptance criteria

- [ ] Every filtered raw file since the first node's first day is stored and
      listed in a daily manifest; unfiltered raw is private, zstd
      long-range with trained dictionaries, and access-logged.
- [ ] Daily manifests are hash-chained and signed, and list per file: size,
      content hash, IPFS CID, and the day's BitTorrent v2 infohash and
      magnet link.
- [ ] Every manifest has a Bitcoin OpenTimestamps proof and an L2 anchor,
      both checked by the verifier, along with signature and hash chain.
- [ ] Every recorded day is retrievable via HTTPS, BitTorrent and IPFS,
      verifiable against the signed manifest; manifests are also on Arweave.
- [ ] Normalized partitions rebuilt from filtered raw have the same canonical
      content hash; inside the pinned build container they are
      byte-identical. CI checks both on a synthetic fixture day.
- [ ] The catalog is queryable directly with DuckDB (httpfs and WASM),
      Polars and ClickHouse `s3()`; bars, analytics and volume profiles are
      precomputed; book at time T works from checkpoints.
- [ ] The small API serves JSON queries with documented limits, query
      timeouts and memory caps.
- [ ] Compatibility layers are published: Databento-shaped API over DBN,
      websocket replay at N× speed, NautilusTrader adapter, TradingView UDF
      datafeed, CCXT-style REST shim, MCP server.
- [ ] Trade history and book snapshots are backfilled to the earliest
      available date; order-level replay results are published for every
      day it succeeds on. Backfilled data carries ADM-0003 and the
      `backfill` (or `replayed`) flag.
- [ ] A public coverage map shows recorded, backfilled and missing hours per
      market. Gaps are flagged, never filled in.
- [ ] Gateways and keys are discoverable from the on-chain registry; SDKs
      rotate gateways and fall back to torrent web seeds.
- [ ] One command re-originates the pipeline on a clean host from
      reproducible builds.
- [ ] A second venue is admitted and recording.

## Dependencies

- Phase 1: node-reader, book-builder, bar-builder, orderflow-analytics,
  block-signing, signing-keys, cost-and-retention-model.
- [ADR 0006](../../docs/decisions/0006-phase-1-2-service-architecture.md):
  Accepted.

## Ordered work

### Lane: backfill (parallel, starting now)

1. **bucket-copy**
   - Status: In progress
   - Owner: Unassigned
   - Dependencies: None
   - PR: Not opened
   - Next action: Built: `hl-backfill estimate` and `copy` with the ADM-0003
     denylist, checkpoints and retries (tools/hl-backfill). Run the copy
     with owner credentials.

2. **backfill-fills**
   - Status: In progress
   - Owner: Unassigned
   - Dependencies: bucket-copy, normalized-partitions
   - PR: Not opened
   - Next action: Built: `hl-backfill process` runs by-block fills through the
     node reader and writes Parquet with the archive writer;
     tests/history.rs checks partitions through the history API (ADM-0003,
     `backfill`). Run on the copied buckets and compare overlap days.

3. **backfill-book-snapshots**
   - Status: In progress
   - Owner: Unassigned
   - Dependencies: bucket-copy
   - PR: Not opened
   - Next action: Built: L2 snapshots parsed as `low_resolution` book history
     with the measured snapshot interval in the report. Run on the copied
     `market_data` files.

4. **l3-replay**
   - Status: In progress
   - Owner: Unassigned
   - Dependencies: bucket-copy (first days can start from a sample)
   - PR: Not opened
   - Next action: Prototype matcher in tools/hl-backfill/src/replay; output
     flagged `backfill` and `replayed`. Close the fidelity gaps listed in
     docs/operations/backfill.md § 7 on overlap days.

### Lane: archive

5. **filtered-raw-capture**
   - Status: In progress
   - Owner: Unassigned
   - Dependencies: Phase 1 node-runbook, node-reader
   - PR: Not opened
   - Next action: Built: live capture of both tiers in the origin
     (archive::follow: sealing, write-once upload, `.captured-through`).
     Blocked on the first node; L4 snapshot files are not captured yet.

6. **manifest-chain**
   - Status: Done
   - Owner: Unassigned
   - Dependencies: Phase 0 data-model-schema, Phase 1 signing-keys
   - PR: Not opened
   - Next action: None. docs/specs/manifest-v0.md; `archive manifest
     build/sign/verify`; `_manifests/latest.json` for monitoring.

7. **manifest-anchoring**
   - Status: Done
   - Owner: Unassigned
   - Dependencies: manifest-chain
   - PR: Not opened
   - Next action: OpenTimestamps and an Ethereum L2 anchor (`archive anchor`).
     Choose the L2 and its anchoring contract.

8. **normalized-partitions**
   - Status: Done
   - Owner: Unassigned
   - Dependencies: Phase 1 node-reader and book-builder (synthetic data is
     enough to start)
   - PR: Not opened
   - Next action: None. Deterministic Parquet for every record kind with static
     Iceberg metadata (crates/archive).

9. **replay-tool**
   - Status: Done
   - Owner: Unassigned
   - Dependencies: normalized-partitions
   - PR: Not opened
   - Next action: None. `archive replay` with the origin's parser; byte
     identity and content hashes checked in CI on synthetic days
     (crates/archive/tests, crates/origin/tests/e2e.rs).

10. **bar-rollups**
    - Status: Done
    - Owner: Unassigned
    - Dependencies: Phase 1 bar-builder, orderflow-analytics
    - PR: Not opened
    - Next action: None. The book builder rolls bars up to 1d and builds daily
      volume profiles, live and in replay.

11. **correction-and-versioning-protocol**
    - Status: Done
    - Owner: Unassigned
    - Dependencies: manifest-chain
    - PR: Not opened
    - Next action: None. Corrections write new part numbers in a new manifest
      (archive-layout-v0 § 6, `--correction`).

12. **torrent-and-ipfs-publishing**
    - Status: Done
    - Owner: Repository owner
    - Dependencies: normalized-partitions, manifest-chain
    - PR: Not opened
    - Next action: None. First run 2026-11-13: the `reference-normalized` archive
      (ADR 0017), 12 days signed, OpenTimestamps-stamped, published to
      `https://data.bleh.xyz/reference-normalized` with BitTorrent v2 torrents
      (`_manifests/magnets.txt`) and pinned on the home-archive IPFS node;
      every day passes `archive verify` over HTTPS.

13. **seedbox-and-pinning**
    - Status: In progress
    - Owner: Repository owner
    - Dependencies: torrent-and-ipfs-publishing
    - PR: Not opened
    - Next action: The home-archive IPFS node runs and pins every published
      day. Next: bring up the seeder (qBittorrent) for the torrents, schedule
      `archive-pipeline.sh records` and `records-anchors`, and document
      mirroring.

14. **arweave-and-public-mirrors**
    - Status: Ready
    - Owner: Unassigned
    - Dependencies: manifest-chain, torrent-and-ipfs-publishing
    - PR: Not opened
    - Next action: Not started: store manifests on Arweave; publish to public
      dataset mirrors.

15. **mirror-verification**
    - Status: Done
    - Owner: Unassigned
    - Dependencies: manifest-anchoring
    - PR: Not opened
    - Next action: None. `archive verify` (signature, chain, anchors, files)
      from a directory or URL.

### Lane: static history, small API and compatibility layers

16. **static-catalog**
    - Status: Done
    - Owner: Unassigned
    - Dependencies: normalized-partitions, Phase 1 tls-cdn-front
    - PR: Not opened
    - Next action: None. Hive paths and static Iceberg metadata
      (archive-layout-v0 § 4) are served from R2 behind the CDN at
      `https://data.bleh.xyz/<archive>/`, first for `reference-normalized`;
      the edge cache rules give every rewritten file a one-minute TTL.

17. **small-api**
    - Status: Done
    - Owner: Unassigned
    - Dependencies: static-catalog
    - PR: Not opened
    - Next action: None. crates/history-api with limits, timeouts and memory
      caps; it serves the replayed archive in the end-to-end test.

18. **replay-websocket**
    - Status: Done
    - Owner: Unassigned
    - Dependencies: static-catalog, Phase 1 gateway
    - PR: Not opened
    - Next action: None. `/v0/replay` in the history API.

19. **dbn-compat-api**
    - Status: In progress
    - Owner: Unassigned
    - Dependencies: static-catalog, Phase 0 wire-format-decision
    - PR: Not opened
    - Next action: Historical Databento-shaped endpoints over DBN are built. A
      live DBN encoding (`bleh.v0.dbn`) is not.

20. **client-adapters**
    - Status: In progress
    - Owner: Unassigned
    - Dependencies: small-api, Phase 1 client-sdks
    - PR: Not opened
    - Next action: TradingView UDF and CCXT-style shims are built in the history
      API. NautilusTrader adapter not started.

21. **mcp-server**
    - Status: Done
    - Owner: Unassigned
    - Dependencies: small-api
    - PR: Not opened
    - Next action: None. `/mcp` in the history API.

22. **coverage-map**
    - Status: In progress
    - Owner: Unassigned
    - Dependencies: filtered-raw-capture, normalized-partitions
    - PR: Not opened
    - Next action: The `coverage` kind (observation-v0 § 2.11) is published live
      and archived, so quiet and unrecorded hours differ. Next: use it in
      the history API coverage endpoint and publish the page.

### Lane: bus, gateway, relay and client SDKs (Phase 2 part)

23. **gateway-discovery**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: Phase 1 gateway, Phase 1 signing-keys
    - PR: Not opened
    - Next action: On-chain registry of gateways, relays and keys (for
      example the Ethereum Attestation Service), ENS/IPNS names, a Tor onion
      service and Nostr-announced manifests; SDKs rotate gateways and fall
      back to torrent web seeds.

### Lane: infrastructure (Phase 2 part)

24. **one-command-reorigination**
    - Status: In progress
    - Owner: Unassigned
    - Dependencies: Phase 1 infrastructure-as-code, replay-tool
    - PR: Not opened
    - Next action: deploy/up.sh brings up each role, the node box as one origin
      service. Test from scratch on a clean host.

25. **code-mirrors**
    - Status: Ready
    - Owner: Repository owner
    - Dependencies: None
    - PR: Not opened
    - Next action: Mirror the repository to Codeberg and Radicle
      automatically.

### Second venue

26. **second-venue**
    - Status: Ready
    - Owner: Unassigned
    - Dependencies: Phase 0 data-model-schema
    - PR: Not opened
    - Next action: Pick the first of the Phase 2–3 venues in
      `docs/sources.md` (for example PAXG/XAUT pools or Solana tokenized
      stocks), file its admission record, and build its reader on our own
      node.

### Release

27. **staging-and-production-deploy**
    - Status: Blocked
    - Owner: Repository owner
    - Dependencies: static-catalog, small-api, torrent-and-ipfs-publishing,
      manifest-anchoring, coverage-map
    - PR: Not opened
    - Next action: Verify every acceptance criterion against real recorded
      and backfilled days in staging, then deploy to production alongside
      the Phase 1 gateway.

## Exit

All acceptance criteria met, with static history, distribution channels,
compatibility layers and backfill live.
