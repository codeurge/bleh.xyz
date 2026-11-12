# Architecture

```
  sources                 nodes                    mesh                      consumers
  ───────                 ─────                    ────                      ─────────
  own chain nodes  ──▶  ingest adapters  ──▶  signed observations  ──▶  gateways (websocket)
  Part 43 feeds          (one per source)      gossip between nodes      archive (Parquet)
  agency data                                  composite engine          registry (git)
  contributors                                 settlement engine
```

## 1. Data model

Every record is an **observation**: a fact from one admitted source.

| Field | Meaning |
|---|---|
| `kind` | trade, quote, book_add, book_modify, book_cancel, bar, rate, reference |
| `instrument` | Mesh instrument ID from the registry |
| `venue` | Where it happened (venue ID, or a public-data source ID) |
| `ts_source` | Venue or source timestamp, in nanoseconds |
| `ts_ingest` | When the node received it |
| `seq` | Venue sequence number, where one exists |
| `price`, `size` | Fixed-point integers with a per-instrument scale |
| `side` | Aggressor side for trades (buy, sell, unknown), with a confidence flag if inferred |
| `order_id` | For order-level book events |
| `source_ref` | Pointer back to the raw record (block height and index, report ID) |
| `admission` | ID of the admission decision that covers this field set |
| `flags` | e.g. `oracle_anchored_venue`, `block_trade_delayed`, `size_capped` |
| `node`, `sig` | Publishing node's key and signature |

Derived records (composites, bars, conversions, settlements) add
`method@version` and a hash of their inputs.

### Wire format

**Proposal:** emit records in Databento's open-source DBN binary format under
the mesh's own dataset and publisher IDs, with provenance in a sidecar
channel. Existing tools that read DBN (including the reference consumer's
internal bus) could then switch with very little work. **To verify:** the DBN
license, and whether DBN's fixed record layouts can carry the provenance
fields or need an extension record. A JSON or Arrow representation is also
offered for simple clients.

## 2. Ingest nodes

- One adapter per source:
  - Hyperliquid node reader
  - Part 43 repository listeners
  - NY Fed, Treasury, USDA and EIA pollers
  - IEX delayed feed, if admitted
- Each adapter emits only its admitted fields, normalizes them to the data
  model, and signs.
- Several independent operators run the same adapter. Agreement between their
  outputs is the integrity check.

## 3. Venue tapes and books

- **Trades:** venue trade prints with native aggressor side.
- **Books:** order-by-order books rebuilt from venue events. L2 depth and top
  of book are derived from them, never ingested separately.
- **Bars:** built from trades. Bars are never accepted from outside, so every
  bar can be traced to its trades.

These give order flow, footprint and heatmaps **for each venue**, labeled as
that venue's. They are not exchange order flow, and the labels say so.

## 4. Composite exposures

An exposure is an economic risk, for example "S&P 500 front", "WTI front" or
"gold spot". Each has a published method:

- **Consolidated tape:** trades from all member venues, each tagged with its
  venue.
- **Composite price:** volume-weighted median across venues, with outlier
  rejection and staleness limits.
- **Membership rules:** minimum liquidity, maximum share for any one venue,
  and automatic exclusion when a venue diverges or goes quiet.

## 5. Dated-contract equivalents

Perpetuals never expire, but futures traders think in dated contracts. The
conversion model turns a perpetual price into a dated-contract equivalent:

```
dated_equiv(T) = perp_price × carry_factor(t, T)
carry_factor   = from financing (SOFR curve from Part 43 swaps and NY Fed),
                 the perpetual's funding rate history, and per-asset terms
                 (storage and convenience yield for commodities; dividend
                 yield for indices)
```

- Per-asset terms that can't be observed openly are estimated, and the
  estimate is published with its uncertainty.
- Roll schedules come from the registry.

## 6. Open settlement

- A daily reference close per exposure: a volume-weighted composite over a
  published window.
- The method follows the IOSCO Principles for Financial Benchmarks:
  governance, transparency, a method that is rarely changed and versioned
  when it is.
- Every settlement links to the archive slice it used, so anyone can
  recompute it.

## 7. Transport

- **Between nodes:** gossip (libp2p or NATS) of signed observations.
- **To consumers:** several independent websocket gateways. Clients verify
  signatures and may subscribe to more than one gateway.
- **Latency budget:** gateway fan-out within 100 ms of source, which meets
  10-per-second chart updates.

## 8. Archive

- Daily partitions in Parquet (with DBN dumps), each listed in a hash-chained
  manifest.
- Distributed by content address (IPFS or torrents) plus ordinary mirrors.
- Corrections are new versions. Old versions stay addressable.

## 9. Instrument registry

- A git repository of instruments, exposures, venue mappings, contract specs,
  roll schedules and session calendars.
- These are facts. Entries are written fresh and never copied from exchange
  documents.
- Each entry has a stable mesh ID. Venue and exchange symbols are mappings,
  not identities.

## 10. Governance

- Admission decisions, method versions and registry changes go through public
  review.
- Node operators publish their keys in the registry. Keys can be revoked.
