# 0006: Service architecture for live and historical data (Phases 1–2)

- Status: Accepted
- Date: 2026-11-11
- Deciders: Repository owner, after independent review

## Context

Phases 1 and 2 turn Hyperliquid node output (every market) and US swap
dissemination into a public, signed live stream and a reproducible,
verifiable history. Targets: p99 of gateway send time minus node local
write time at most 50 ms, 99.9% availability, deterministic rebuilds,
order-level books, and costs a small project can carry.

## Decision

```
 node A (provider 1) ─▶ reader A ─┐
 node B (provider 2) ─▶ reader B ─┴─▶ dedupe on (block, event_index) ── mismatch ─▶ integrity alarm
                                          │
            ┌─────────────────────────────┼──────────────────────────────┐
            ▼                             ▼                              ▼
  unfiltered raw (private)     books, bars, analytics           filtered raw (public root)
                               block signing (Merkle roots)              │
                                          │                              ▼
                               in-process ring buffer          replay tool ─▶ Hive-partitioned Parquet
                                    │           │                              + catalog, manifests
                                    ▼           ▼                              │
                         gateway (websocket)  JetStream side tap               ▼
                                    │         (durability, replay)   CDN, BitTorrent, IPFS, Arweave
                                    ▼                                 small JSON API, compat layers
                          clients and relays
```

- **Language: Rust** for the readers, book, bar and analytics builders,
  gateway, replay tool and partition writer. Readers are clean-room
  implementations from documented schemas. SDKs for Python (PyO3),
  TypeScript (WASM) and Rust share one Rust core.
- **Active-active ingest:** two nodes on two providers from day one, primary
  near Hyperliquid peers (Tokyo; to verify at provisioning). Records are
  deduplicated on `(block, event_index)`; any mismatch is an integrity
  alarm.
- **Node output:** `--write-fills`, `--write-order-statuses`,
  `--write-raw-book-diffs`, `--batch-by-block`,
  `--disable-output-file-buffering` and `--serve-info`. Readers tail the
  by-block files with inotify. L4 snapshots come from the info server's
  `fileSnapshot` request.
- **Batches:** one batch per instrument per block, so fills, order statuses
  and book diffs stay in order. `seq` is `(block_number, event_index)`.
- **Signing:** one Merkle root per block per stream, signed with secp256k1
  (verifiable on-chain with `ecrecover`). Roots travel with each block;
  proofs are served on request. Daily manifests are signed too.
- **Hot path:** reader → in-process ring buffer → gateway. **NATS
  JetStream** is a side tap for durability and replay, never on the hot path
  and never a source for the archive.
- **Archive:** readers write unfiltered raw (private, contains owner
  addresses; zstd long-range with trained dictionaries) and filtered raw
  (admitted fields only, the public root). Normalized partitions are built
  from filtered raw by the replay tool only, the same code path for live
  days, rebuilds and backfill.
- **Gateway protocol:** a book snapshot tagged with `(block, seq)`, then
  contiguous deltas; a heartbeat with signed roots every block; a forced
  re-snapshot whenever a gap is detected. JSON and Arrow encodings. Per-IP
  connection and subscription caps and a slow-consumer policy (drop to
  snapshot, then disconnect). Health and freshness endpoints.
- **Latency:** p99 of `gateway_send_time − node local write time` ≤ 50 ms.
  Lag behind `block_time` is reported, not gated.
- **Historical access, static first:** Hive-partitioned Parquet sorted by
  instrument and time, zstd, with an Iceberg or DuckLake catalog, on
  zero-egress object storage behind a CDN. Users query it directly (DuckDB
  httpfs or WASM, Polars, ClickHouse `s3()`); the same files seed torrents
  and IPFS. A small HTTP service answers JSON queries only, with query
  timeouts and memory caps; bars and volume profiles are precomputed.
- **Compatibility layers (Phase 2):** a Databento-shaped live and historical
  API over DBN (ADR 0004), websocket replay at N× speed, a NautilusTrader
  adapter, a TradingView UDF datafeed, a CCXT-style REST shim and an MCP
  server for agents.
- **Determinism:** partitions are a pure function of filtered raw files plus
  versioned code. The gate is a canonical content hash of the records; byte
  identity is required only inside the pinned build container (pinned
  writer, codec, row-group size, sort order and metadata).
- **Deployment:** containers with infrastructure as code on two providers.
  One command (reproducible builds plus a compose file: node → reader →
  gateway → archive) re-originates the whole pipeline anywhere.

## Consequences

- Recording filtered raw from the first day makes every later improvement
  re-runnable over all history, and the public can rebuild every partition.
- Per-block signatures make relays, mirrors and the oracle contract
  untrusted-safe from Phase 1.
- Building partitions only by replay means the live path and the rebuild
  path cannot drift apart.
- Two providers remove the single-node failure; independent operators
  arrive with Phase 5.
- Static-first history scales with the CDN rather than with servers.

## Alternatives considered

- Kafka instead of NATS: heavier to operate at this scale.
- A bus on the hot path: adds a network hop to every record.
- Archiving from the bus: ties history to bus retention and live-path bugs.
- A query server as the main historical interface: costs grow with users;
  static files plus client-side engines do not.
- Hosted node or data providers: they add terms on the path.
