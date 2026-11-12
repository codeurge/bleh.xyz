# Roadmap

## Phase 0: specification and policy

- Data model and wire format (decide on DBN compatibility)
- Admission policy and the first admission reviews with counsel: Hyperliquid
  trade and order fields, Part 43, NY Fed, Treasury, USDA, EIA,
  Commitments of Traders, IEX delayed data
- Registry schema and first entries
- **Exit:** written admission decisions for the Phase 1 and Phase 2 sources.

## Phase 1: Tier A ingest

- Run a non-validating Hyperliquid node and build the adapter: trades, fills,
  order statuses, book diffs, L4 snapshots
- Rebuild order-by-order books; derive depth, top of book and bars
- Daily archive with manifest; replay tooling
- **Exit:** a live tape and book for S&P 500, gold, silver, WTI and Brent
  markets, plus a replayable archive.

## Phase 2: Tier B ingest

- Part 43 swap listeners, and pollers for NY Fed, Treasury, USDA, EIA and
  Commitments of Traders
- SOFR curve built from swaps and fixings
- **Exit:** a daily open rates curve and cash-market reference prices.

## Phase 3: composites and settlement

- Composite exposure methods with membership rules
- Dated-contract conversion model
- Open settlement method (v1), following IOSCO principles
- Basis measurement with a licensed validation partner, publishing summary
  statistics only after legal review
- **Exit:** published, reproducible composites and settlements with measured
  basis.

## Phase 4: federation

- Signed observations, gossip between nodes, several independent gateways
- Operator onboarding and key registry
- First-party contributor program for off-exchange quotes and trades
- **Exit:** at least three independent operators producing matching outputs.

## Phase 5: options research

- Survey on-chain options venues as they grow
- Model-based approaches using open interest that can be published openly
- **Exit:** a decision on whether an open options surface is feasible.

## Open questions

1. Is DBN's license compatible, and can it carry provenance?
2. Do HIP-3 deployers' terms say anything about data? Are any oracle methods
   clean?
3. What are the exact terms of each swap repository's public site and API?
4. Are IEX's current delayed-data terms really free redistribution?
5. Which on-chain order books match on-chain and which off-chain?
6. Project name, license, and governance body.
7. Who funds node and archive hosting?
