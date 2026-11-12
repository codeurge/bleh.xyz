# 0006: Service architecture for live and historical data (Phases 1–2)

- Status: Proposed
- Date: 2026-11-11
- Deciders: Maintainers

## Context

Phases 1 and 2 must turn one venue node's output into a public live stream
and a permanent, reproducible history. Requirements: p99 publish under
250 ms from block, deterministic rebuilds, order-level books, and costs a
small project can carry.

## Decision (proposed)

```
 node files ──▶ node-reader ──▶ internal bus ──▶ gateway (websocket) ──▶ clients
     │              │               │
     │              ├─ book-builder ┤
     │              └─ bar-builder ─┘
     ▼                              ▼
 raw archive (object storage)   partition writer ──▶ normalized Parquet ──▶ historical API
     └──────────── replay tool rebuilds partitions from raw ────────────┘      bulk downloads
```

- **Language: Rust** for node-reader, book-builder, bar-builder, gateway and
  partition writer. It has predictable latency, Hyperliquid's own order-book
  server is Rust, and the DBN libraries are Rust.
- **Internal bus: NATS JetStream.** One subject per instrument and record
  kind, with a bounded replay window so gateways can restart without gaps.
- **Gateway:** websocket with JSON and binary encodings; book snapshot plus
  sequenced deltas; health and freshness endpoints.
- **Storage:** S3-compatible object storage for the raw archive (write-once)
  and normalized daily Parquet partitions.
- **Historical queries:** DuckDB over Parquet behind a thin HTTP service.
  Revisit (for example ClickHouse) only if measured load needs it.
- **Determinism:** partitions are a pure function of raw files plus versioned
  code. The replay tool is the reference implementation.
- **Deployment:** containers with infrastructure as code. Node, readers and
  bus run in one region close to Hyperliquid peers; the archive is replicated
  to a second region.

## Consequences

- Recording raw files from day one makes every later improvement
  re-runnable over all history.
- One region is a single point of failure until Phase 5 federation. The
  replicated archive protects history but not live availability.
- DuckDB keeps operations simple, but concurrency limits must be measured.

## Alternatives considered

- Kafka instead of NATS: heavier to operate at this scale.
- A time-series database as the primary store: harder to make reproducible
  and to distribute openly than plain Parquet files.
- Hosted node or data providers: they add terms on the path and undermine
  admission.
