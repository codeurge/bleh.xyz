# Roadmap

The first deployable milestone is **live and historical venue data from the
mesh** (Phases 1 and 2), with US swap dissemination alongside. More venues,
curves, surfaces, composites and the open oracle build on that foundation.

| Phase | Outcome when deployed | Plan |
|---|---|---|
| 0. Foundation | Governed repository, data-model and registry schemas, Hyperliquid admissions | [phase-0-foundation](../.agents/plans/phase-0-foundation.md) |
| 1. Live data | Signed, active-active live stream of every Hyperliquid market: trades, order-level books, bars and order-flow analytics; SDKs; Part 43 and SBSR pollers | [phase-1-live-data](../.agents/plans/phase-1-live-data.md) |
| 2. Historical data | Static, queryable archive over CDN, BitTorrent, IPFS and Arweave, anchored on-chain; backfill to the venue's earliest public data; compatibility layers; gateway discovery; second venue | [phase-2-historical-data](../.agents/plans/phase-2-historical-data.md) |
| 3. More venues, curves and surfaces | Third venue and consolidated tapes; intraday curves; option surfaces; agency data; signed pull-oracle | Not started |
| 4. Composites, settlement and open oracle | Composite exposures, dated-contract equivalents, open settlement, on-chain oracle | Not started |
| 5. Federation | Independent operators, gossip between nodes, contributor program | Not started |
| 6. Extend coverage | New asset classes and venues as they appear | Not started |

All Hyperliquid markets are recorded and published from day one (owner
decision, 2026-11-11).

Phases 0–2 run in parallel. Phase 0 gates Phases 1–2 only through the
data-model and registry schemas. The archive records from the day the first
node runs, and backfill from Hyperliquid's public buckets starts now, in
parallel with Phase 1.

## Phase 3: more venues, curves and surfaces

- Third venue and consolidated tapes per exposure, from the list in
  [sources.md](sources.md): Solana tokenized stocks, Uniswap v4 tokenized
  securities and PAXG/XAUT pools, Derive, dYdX v4, Lighter, Polymarket. Each
  is read from infrastructure the project runs.
- Intraday SOFR/OIS curve bootstrap from Part 43 prints and fixings
- Option surfaces: crypto from Derive trades and HIP-4 ladders; index and
  commodity from Part 43 swaptions and commodity options
- NY Fed, Treasury, USDA (including mandatory livestock reporting), EIA
  (excluding vendor-credited series) and Commitments of Traders ingest
- Signed pull-oracle for BTC, ETH, gold and WTI composites as soon as a
  second venue is live
- **Exit:** consolidated tapes for at least four exposures, a daily and
  intraday open rates curve, published surfaces, and the pull-oracle live.

## Phase 4: composites, settlement and open oracle

- Composite exposure methods with membership rules
- Dated-contract conversion model
- Open settlement method (v1), following IOSCO principles
- Basis measurement with a licensed validation partner (summary statistics)
- **Open oracle:** composite exposures published on-chain, verified by a
  contract from the signed block roots, with no exchange data in their
  lineage. A market that uses the oracle as an input is never a member of
  the composite it reads, so there is no circularity. The publication method
  (signed updates, update cadence, deviation thresholds) is to be designed.
- **Exit:** published, reproducible composites and settlements with measured
  basis, and the oracle live on at least one chain.

## Phase 5: federation

- Independent operators running the same adapters, gossip of signed records
  between nodes, gateways run by others
- Operator keys admitted to the on-chain registry built in Phase 2
- First-party contributor program
- **Exit:** at least three independent operators producing matching outputs.

There is no project token.

## Phase 6: extend coverage

- New asset classes and venues as on-chain markets grow, using the same
  admission, signing and archive machinery
- **Exit:** reviewed each year; no fixed end.

## Open questions

1. How do ADL fills and HIP-4 outcome markets appear in node output?
   (ADM-0001)
2. What is the snapshot frequency inside each `market_data` file, and how
   far back does each bucket go? (ADM-0003)
3. What formats, access methods and online retention does each swap
   repository offer? (ADM-0002, ADM-0004)
4. Which Ethereum L2 anchors manifests and hosts the registry?
5. Iceberg or DuckLake for the archive catalog?
6. Governance body and hosting funding.
