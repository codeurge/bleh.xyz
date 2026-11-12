# Phase 2: historical data — Plan

Status: Active (backfill and archive design now; live recording starts with
the first node)
Last updated: 2026-11-11

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
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: None
   - PR: Not opened
   - Next action: List the buckets for coverage dates; copy every admitted
     path as-is into the private raw tier, sharded by day on spot
     instances, with checksums and a copy log.

2. **backfill-fills**
   - Status: Blocked
   - Owner: Unassigned
   - Dependencies: bucket-copy, normalized-partitions
   - PR: Not opened
   - Next action: Filter archived fills to book-matched trades under
     ADM-0001's rules into filtered raw and partitions flagged `backfill`;
     compare with live-recorded overlap days.

3. **backfill-book-snapshots**
   - Status: Blocked
   - Owner: Unassigned
   - Dependencies: bucket-copy
   - PR: Not opened
   - Next action: Measure the snapshot frequency inside `market_data`
     files; import them as book history flagged `backfill`.

4. **l3-replay**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: bucket-copy (first days can start from a sample)
   - PR: Not opened
   - Next action: Rebuild order-level books before raw book diffs existed by
     replaying order actions from `replica_cmds` through an emulated
     matcher, checkpointed against archived L2 snapshots; publish matched
     days flagged `replayed` and a report of mismatches.

### Lane: archive

5. **filtered-raw-capture**
   - Status: Blocked
   - Owner: Unassigned
   - Dependencies: Phase 1 node-runbook, node-reader
   - PR: Not opened
   - Next action: Ship unfiltered and filtered raw to object storage from
     the first node's first day, zstd long-range with trained dictionaries
     for the raw tier.

6. **manifest-chain**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: Phase 0 data-model-schema, Phase 1 signing-keys
   - PR: Not opened
   - Next action: Specify the daily manifest (per file: size, content hash,
     IPFS CID, torrent infohash and magnet; previous manifest hash;
     signature) and build the verifier.

7. **manifest-anchoring**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: manifest-chain
   - PR: Not opened
   - Next action: Per day, an OpenTimestamps proof on Bitcoin and an anchor
     transaction on an Ethereum L2 (L2 TBD); store proofs next to the
     manifest and distribute them with it; extend the verifier to check
     signature, hash chain and both anchors.

8. **normalized-partitions**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: Phase 1 node-reader and book-builder (synthetic data is
     enough to start)
   - PR: Not opened
   - Next action: Deterministic Hive-partitioned Parquet writer (kind,
     venue, date; sorted by instrument and time; zstd) for trades, book
     events, book checkpoints, bars and analytics, with an Iceberg or
     DuckLake catalog.

9. **replay-tool**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: normalized-partitions
   - PR: Not opened
   - Next action: The only path from filtered raw to partitions, for live
     days, rebuilds and backfill; canonical content hash gate; pinned build
     container for byte identity; CI check on a fixture day.

10. **bar-rollups**
    - Status: Ready
    - Owner: Unassigned
    - Dependencies: Phase 1 bar-builder, orderflow-analytics
    - PR: Not opened
    - Next action: Precompute 5m, 15m, 1h and 1d bars and daily volume
      profiles; the daily boundary is UTC midnight, recorded in the
      registry.

11. **correction-and-versioning-protocol**
    - Status: Ready
    - Owner: Unassigned
    - Dependencies: manifest-chain
    - PR: Not opened
    - Next action: Document how a partition is corrected or withdrawn: new
      version, new manifest entry, old hashes kept listed, all distribution
      channels updated.

12. **torrent-and-ipfs-publishing**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: normalized-partitions, manifest-chain
    - PR: Not opened
    - Next action: Per day, create a BitTorrent v2 torrent with web seeds
      pointing at the HTTPS copy, add files to IPFS, and write magnets and
      CIDs into the manifest.

13. **seedbox-and-pinning**
    - Status: Blocked
    - Owner: Repository owner
    - Dependencies: torrent-and-ipfs-publishing
    - PR: Not opened
    - Next action: Run at least one always-on seeder and one IPFS pinning
      node; document how anyone can mirror.

14. **arweave-and-public-mirrors**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: manifest-chain, torrent-and-ipfs-publishing
    - PR: Not opened
    - Next action: Store each manifest (with proofs) on Arweave; publish
      partitions to Hugging Face datasets, the Internet Archive and Academic
      Torrents.

15. **mirror-verification**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: manifest-anchoring
    - PR: Not opened
    - Next action: A tool and doc so any mirror or user can verify downloads
      against the signed, anchored manifests.

### Lane: static history, small API and compatibility layers

16. **static-catalog**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: normalized-partitions, Phase 1 tls-cdn-front
    - PR: Not opened
    - Next action: Serve partitions and catalog from zero-egress storage
      behind the CDN; publish worked examples for DuckDB httpfs,
      DuckDB-WASM, Polars and ClickHouse `s3()`.

17. **small-api**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: static-catalog
    - PR: Not opened
    - Next action: HTTP service for small JSON queries (recent trades, bars,
      book at time T), with pagination, query timeouts, memory caps and
      per-client rate limits.

18. **replay-websocket**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: static-catalog, Phase 1 gateway
    - PR: Not opened
    - Next action: Replay any archived range over the live protocol at N×
      speed.

19. **dbn-compat-api**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: static-catalog, Phase 0 wire-format-decision
    - PR: Not opened
    - Next action: Databento-shaped live and historical API over DBN, with
      provenance and signatures alongside.

20. **client-adapters**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: small-api, Phase 1 client-sdks
    - PR: Not opened
    - Next action: NautilusTrader adapter, TradingView UDF datafeed and a
      CCXT-style REST shim, each with an example.

21. **mcp-server**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: small-api
    - PR: Not opened
    - Next action: MCP server exposing live snapshots, history queries,
      coverage and verification to agents.

22. **coverage-map**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: filtered-raw-capture, normalized-partitions
    - PR: Not opened
    - Next action: Per-market, per-hour coverage (recorded, backfilled,
      replayed, missing) as data and as a page; never fill gaps.

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
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: Phase 1 infrastructure-as-code, replay-tool
    - PR: Not opened
    - Next action: Reproducible builds plus a compose file (node → reader →
      gateway → archive) that brings up the whole pipeline on a clean host;
      test it from scratch.

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
