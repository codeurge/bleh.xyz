# Requirements

The requirements come from a reference consumer: a production futures
trading platform with order-flow charts, an order-book heatmap, options
analytics (GEX), backtests, paper trading, alerts, daily levels and an agent
API. It currently gets all of this from one exchange-licensed feed. If the mesh
meets its needs, it meets most consumers' needs.

## Requirements and targets

| ID | Requirement | Consumer use | Mesh target |
|---|---|---|---|
| R1 | Every trade, with aggressor side and venue timestamp | Footprint, delta, CVD, big trades, tape | Native for Tier A venues, plus published footprint, delta/CVD and sweep streams |
| R2 | Top-of-book quotes | Last price, alerts, paper fills | Native for Tier A venues; composite top of book |
| R3 | Order-by-order book (L3), with depth (L2) derived from it | Order-book heatmap, replay | Native for venues that expose it (e.g. own Hyperliquid node), with no embargo; published L3 heatmap tiles and replay at N× speed |
| R4 | Bars from 1 second to 1 day | Charts, reports, backtests | Per venue and per composite exposure; time, tick, volume and range bars and volume profiles |
| R5 | Long history | Backtests, studies | Archive from the node's first day, plus venue history imported back to the earliest available date |
| R6 | Options quotes, open interest, greeks | GEX, options flow | Crypto surfaces from Derive and HIP-4 ladders; index and commodity surfaces from Part 43 swaptions and commodity options (Phase 3) |
| R7 | Instrument registry, rolls, session calendars | Symbol identity everywhere | Open registry in git |
| R8 | Daily reference close | Daily change, levels | Open settlement method, recomputable |
| R9 | Updates at 10 per second; prices under 2 seconds old | Live charts, fills | Every block as it arrives; p99 of gateway send time minus node local write time ≤ 50 ms (lag behind block time reported); source, node, ingest and publish times on every record |
| R10 | Rate curves, macro data, positioning | Context, agent insights | Part 43 and SBSR swap prints from Phases 1–2; intraday curves and agency data from Phase 3 |

## Explicit non-goals

- **Exchange prices.** The mesh publishes prices for equivalent exposure,
  never the exchange's own prices.
- **Official settlements.** Contracts that settle on an exchange price still
  settle on it.
- **Execution-grade marks.** Anything that settles money for positions held at
  an exchange (prop-firm rules, margin, account flattening) should use
  broker-reported or licensed prices. Consumers can use the mesh alongside
  those.

## Quality requirements

- **Basis transparency.** For each composite exposure, publish the measured
  basis and tracking error to the reference market, with the method. A
  licensed consumer can measure this internally under its own license and
  publish summary statistics.
- **Lineage completeness.** No output field may lack a lineage chain back to
  admitted sources.
- **Determinism.** Given the archive, anyone can recompute partitions,
  composites, bars and settlements with the same canonical content hash.
  Byte-identical files are guaranteed inside the pinned build container
  (pinned writer, codec, row-group size, sort order and metadata).
- **Freshness.** Every record carries source time (`ts_source`), node time
  (`ts_node`), receive time (`ts_ingest`) and, live, publish time
  (`ts_publish`), so consumers can apply their own staleness rules.
- **Availability.** At least 99.9% per 30 days with active-active ingest
  (two nodes on two providers, from day one), measured by an external
  probe that connects once a minute, subscribes to one instrument and must
  receive a block heartbeat within 5 seconds. Announced node upgrades are
  excluded and listed on the status page.
- **Verifiability.** Every block's records are covered by a signed Merkle
  root per stream, so relays and mirrors need not be trusted. Every archived
  day is retrievable over HTTPS, BitTorrent and IPFS, and verifiable against
  a signed, hash-chained manifest anchored on-chain.
- **Reach.** Consumers use familiar tools: SDKs for Python, TypeScript and
  Rust, direct SQL over the archive (DuckDB, Polars, ClickHouse), and
  compatibility layers for existing market-data clients.
