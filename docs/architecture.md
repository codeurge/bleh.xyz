# Architecture

```
  sources                 nodes                    mesh                      consumers
  ───────                 ─────                    ────                      ─────────
  own chain nodes  ──▶  ingest adapters  ──▶  block-signed records  ──▶  gateways and relays (websocket)
  Part 43 / SBSR         (one per source,      gossip between nodes      static archive (Parquet, torrents, IPFS)
  agency data             active-active)       analytics, composites     registry (git)
  contributors                                 settlement engine         open oracle (on-chain)
```

This is the full mesh. Phases 1–2 run one active-active pipeline per source,
described in [ADR 0006](decisions/0006-phase-1-2-service-architecture.md);
Phase 5 adds independent operators.

## 1. Data model

Every record is an **observation**: a fact from one admitted source.

| Field | Meaning |
|---|---|
| `kind` | trade, book_add, book_modify, book_cancel, book_snapshot, bar, analytics, heartbeat, gap, swap_print, rate, reference |
| `instrument` | Mesh instrument ID from the registry |
| `venue` | Where it happened (venue ID, or a public-data source ID) |
| `ts_source` | Venue or source time, as nanoseconds since the Unix epoch. Hyperliquid `block_time` has millisecond resolution, so the last six digits are zero |
| `ts_node` | When our node wrote the block (Hyperliquid `local_time`, milliseconds) |
| `ts_ingest` | When the reader received it |
| `ts_publish` | When the gateway sent it (live only; required by [requirements.md](requirements.md)) |
| `seq` | `(block_number, event_index)`. Hyperliquid has no venue sequence number; `event_index` is the record's position in its per-instrument block batch, in the order the data-model spec defines |
| `trade_id` | For trades: `(block_time, coin, tid)`. A `tid` alone is not unique |
| `price`, `size` | Fixed-point integers with a per-instrument scale, parsed from the venue's decimal strings |
| `side` | Aggressor side for trades. On Hyperliquid each trade arrives as two fills, one per user; the fill with `crossed: true` is the aggressor and gives the side |
| `order_id` | For order-level book events |
| `source_ref` | Pointer back to the filtered raw record (file, block, index) |
| `admission` | ID of the admission decision that covers this field set |
| `flags` | e.g. `oracle_anchored_venue`, `block_trade_delayed`, `size_capped`, `backfill`, `replayed` |

Derived records (bars, analytics, composites, conversions, settlements) add
`method@version` and a hash of their inputs.

### Signing

From Phase 1, each block is signed per stream (owner decision, 2026-11-11):

- The records of one stream (for example one instrument's trades) in one
  block are the leaves of a Merkle tree. The root is signed with secp256k1,
  so a contract can verify it with `ecrecover`.
- The signed scope of a record is every field except `ts_ingest` and
  `ts_publish`, which differ between nodes and gateways.
- The gateway sends the signed root with each block; a Merkle proof for any
  record is available on request. Relays, mirrors and the Phase 4 oracle
  contract therefore need not be trusted.
- Daily manifests are signed as well (see § 9).

### Wire format

Phases 1–2 ship **JSON** (gateway) and **Arrow** (gateway binary encoding and
Parquet in the archive). A DBN compatibility layer, with a Databento-shaped
live and historical API, is planned for Phase 2
([ADR 0004](decisions/0004-wire-format.md)).

## 2. Ingest nodes

- One adapter per source:
  - Hyperliquid node reader, every market (Phases 1–2)
  - Part 43 and SBSR repository pollers (parallel lane in Phases 1–2)
  - Hyperliquid public archive importer and `replica_cmds` replay (backfill,
    started alongside Phase 1)
  - Readers for the next venues (Phases 2–3; see [sources.md](sources.md))
  - NY Fed, Treasury, USDA and EIA pollers (Phase 3)
- **Active-active from day one:** two nodes on two providers each feed a
  reader. Records are deduplicated on `(block, event_index)`; any mismatch
  between the two is an integrity alarm.
- Each adapter keeps only its admitted fields, filtered at capture, and
  normalizes them to the data model. New Hyperliquid markets register
  automatically from node metadata.
- From Phase 5, independent operators run the same adapters and sign their
  output. Agreement between their outputs is the integrity check.

## 3. Venue tapes, books and analytics

- **Trades:** book-matched trades, each built from a fill pair sharing one
  `tid`, with native aggressor side from `crossed`. Liquidation-at-mark,
  settlement and other non-matched fills are dropped.
- **Batches:** the reader tails `--batch-by-block` files with inotify and
  emits one batch per instrument per block, so fills, order statuses and book
  diffs for that block stay in order.
- **Books:** order-by-order books start from an L4 snapshot requested from
  the node (`--serve-info`, `fileSnapshot` with `l4Snapshots`, without
  users), then apply each block's order statuses and diffs. The builder
  reconciles against a fresh snapshot at a fixed checkpoint interval
  (proposed 10 minutes) and resyncs on mismatch. L2 depth and top of book are
  derived from the order book, never ingested separately.
- **Book at time T:** the archive stores a book checkpoint at the same fixed
  interval. A book at T is the last checkpoint before T plus the events up to
  T.
- **Bars:** time, tick, volume and range bars, built from trades only. An
  interval with no trades emits no bar. Missing data (node or reader outage)
  is published as a `gap` record and in the coverage map, never filled in.
- **Order-flow analytics:** live and archived streams, each with
  `method@version` and an input hash: footprint, delta and CVD, volume
  profile, L3 heatmap tiles, order lifetimes, queue position, cancel/replace
  ratios and sweep detection.
- **Liquidity grade:** each market's spread, depth and trades per minute,
  published so thin markets are labelled as tapes, not benchmarks.

These give order flow, footprint and heatmaps **for each venue**, labeled as
that venue's. They are not exchange order flow, and the labels say so.

## 4. Composite exposures

An exposure is an economic risk, for example "US large-cap index front",
"WTI front" or "gold spot". Each has a published method:

- **Consolidated tape:** trades from all member venues, each tagged with its
  venue.
- **Composite price:** volume-weighted median across venues, with outlier
  rejection and staleness limits.
- **Membership rules:** minimum liquidity grade, maximum share for any one
  venue, and automatic exclusion when a venue diverges or goes quiet.

## 5. Dated-contract equivalents

Perpetuals never expire, but futures traders think in dated contracts. The
conversion model turns a perpetual price into a dated-contract equivalent:

```
dated_equiv(T) = perp_price × carry_factor(t, T)
carry_factor   = from financing (SOFR curve from Part 43 swaps and NY Fed),
                 the observed basis between venue perpetuals and on-chain
                 spot where it exists, and per-asset terms (storage and
                 convenience yield for commodities; dividend yield for
                 indices)
```

- Per-asset terms that can't be observed openly are estimated, and the
  estimate is published with its uncertainty.
- Venue funding fields are not used: they are excluded under ADM-0001.
- Roll schedules come from the registry.

## 6. Open settlement

- A daily reference close per exposure: a volume-weighted composite over a
  published window.
- The method follows the IOSCO Principles for Financial Benchmarks:
  governance, transparency, a method that is rarely changed and versioned
  when it is.
- Every settlement links to the archive slice it used, so anyone can
  recompute it.

## 7. Open oracle

- **Pull oracle (Phase 3):** as soon as a second venue is live, a signed
  BTC, ETH, gold and WTI composite is available on request; consumers submit
  the signed update on-chain and a contract verifies it with `ecrecover`.
- **Full oracle (Phase 4):** every composite exposure and reference close
  published on-chain, with no exchange data in its lineage.
- **No circularity:** a market that uses the oracle as an input (for its
  mark, funding or settlement) is never a member of the composite the oracle
  publishes. Membership rules enforce this.
- The publication method (signed updates, update cadence, deviation
  thresholds, chains) is to be designed and recorded in an ADR.

## 8. Transport

- **Hot path:** reader → in-process ring buffer → gateway. NATS JetStream is
  a side tap for durability and replay, not on the hot path.
- **Gateway protocol:** a book snapshot tagged with `(block, seq)`, then
  contiguous deltas; a heartbeat with the signed block roots every block,
  including blocks with no events for that instrument; any detected gap
  forces a fresh snapshot. The full protocol is in `docs/protocol/`.
- **Relays:** anyone can relay the stream; clients verify the block
  signatures, so relays need not be trusted.
- **Discovery (Phase 2):** gateways, relays and keys are listed in an
  on-chain registry, with ENS/IPNS names, a Tor onion service and
  Nostr-announced manifests. SDKs rotate between gateways and fall back to
  torrent web seeds for history.
- **Latency:** p99 of `ts_publish − ts_node` (gateway send time minus node
  local write time) is at most 50 ms. Lag behind `block_time` is reported,
  not gated.
- **Phase 5:** gossip (libp2p or NATS) of signed records between independent
  operators' nodes.

## 9. Archive

- **Unfiltered raw:** node output and copied venue archives as written,
  including owner addresses. Private, never published, kept as long as
  storage allows (owner decision, 2026-11-11). Compressed with zstd
  long-range mode and trained dictionaries.
- **Filtered raw:** the admitted fields only, filtered at capture. This is
  the public, hashed root for reproducibility.
- **Normalized partitions:** Hive-partitioned Parquet (by kind, venue and
  date), sorted by instrument and time, zstd-compressed, with an Iceberg or
  DuckLake catalog. They are rebuilt from filtered raw by the replay tool
  only. Determinism is checked on a canonical content hash of the records;
  byte identity is checked only inside the pinned build container (pinned
  writer, codec, row-group size, sort order and metadata). Bars and volume
  profiles are precomputed.
- **Static-first access:** partitions sit on zero-egress object storage
  behind a CDN. Users query them directly with DuckDB (httpfs or WASM),
  Polars or ClickHouse `s3()`. A small API serves JSON queries only.
- **Manifests:** one per day, hash-chained and signed. Each lists, per file:
  size, content hash, IPFS CID, and the day's BitTorrent v2 infohash and
  magnet link. Each manifest hash is timestamped on Bitcoin with
  OpenTimestamps and anchored on an Ethereum L2 (L2 TBD); the proofs are
  stored and distributed with the manifest.
- **Distribution, from the first recorded day:** HTTPS, BitTorrent v2
  torrents with web seeds pointing at the HTTPS copy, and IPFS; manifests on
  Arweave; mirrors on Hugging Face datasets, the Internet Archive and
  Academic Torrents. The project runs at least one always-on seeder and one
  pinning node, and anyone can mirror and verify against the signed
  manifests.
- **Corrections** are new partition versions with a new manifest entry. Old
  hashes stay listed.

## 10. Instrument registry

- A git repository of instruments, exposures, venue mappings, contract specs,
  roll schedules, session calendars and liquidity grades.
- These are facts. Entries are written fresh and never copied from exchange
  documents. Hyperliquid markets are added automatically from node metadata.
- Each entry has a stable mesh ID. Venue and exchange symbols are mappings,
  not identities.

## 11. Governance

- Admission decisions, method versions and registry changes go through public
  review.
- Signing keys and gateways are published in the on-chain registry from
  Phase 2 (for example the Ethereum Attestation Service), and in Phase 5 it
  admits independent operators' keys. Keys can be revoked.
- Code is mirrored on Codeberg and Radicle. One command rebuilds the whole
  pipeline (node → reader → gateway → archive) from reproducible builds.
- There is no project token.
