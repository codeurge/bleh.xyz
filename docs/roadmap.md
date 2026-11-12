# Roadmap

The first deployable milestone is **live and historical venue data from the
mesh** (Phases 1 and 2). Public-data curves, composites and federation follow
once that foundation is running.

| Phase | Outcome when deployed | Plan |
|---|---|---|
| 0. Foundation | Governed repository, schema, registry, first admissions | [phase-0-foundation](../.agents/plans/phase-0-foundation.md) |
| 1. Live data | Public gateway streaming trades, books and bars for the starter markets | [phase-1-live-data](../.agents/plans/phase-1-live-data.md) |
| 2. Historical data | Permanent archive, historical API and bulk downloads; backfill from venue archives | [phase-2-historical-data](../.agents/plans/phase-2-historical-data.md) |
| 3. Public data (Tier B) | Rates curve, cash-market prices, positioning | Not started |
| 4. Composites and settlement | Composite exposures, dated-contract equivalents, open settlement | Not started |
| 5. Federation | Independent signed nodes, several gateways, contributor program | Not started |
| 6. Options research | Decision on an open options surface | Not started |

Phases 1 and 2 run partly in parallel. The archive (Phase 2) starts recording
the day the Phase 1 node goes live, so history begins as early as possible.

## Phase 3: public data (Tier B)

- Part 43 swap listeners; NY Fed, Treasury, USDA, EIA and Commitments of
  Traders ingest
- SOFR curve from swaps and fixings
- **Exit:** a daily open rates curve and cash-market reference prices.

## Phase 4: composites and settlement

- Composite exposure methods with membership rules
- Dated-contract conversion model
- Open settlement method (v1), following IOSCO principles
- Basis measurement with a licensed validation partner (summary statistics
  only, after legal review)
- **Exit:** published, reproducible composites and settlements with measured
  basis.

## Phase 5: federation

- Signed observations, gossip between nodes, several independent gateways
- Content-addressed archive distribution (IPFS or torrents)
- Operator onboarding, key registry, first-party contributor program
- **Exit:** at least three independent operators producing matching outputs.

## Phase 6: options research

- Survey on-chain options venues as they grow
- Model-based approaches using open interest that can be published openly
- **Exit:** a decision on whether an open options surface is feasible.

## Open questions

1. Is DBN's license compatible, and can it carry provenance? (ADR 0004)
2. Do HIP-3 deployers' terms say anything about data? Are any oracle methods
   clean? (ADM-0001)
3. What are the terms of Hyperliquid's public archive buckets? (ADM-0003)
4. What are the exact terms of each swap repository's public site and API?
   (ADM-0002)
5. Are IEX's current delayed-data terms really free redistribution?
6. Project name, governance body, and hosting funding.
