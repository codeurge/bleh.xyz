# 0019: Backfill instrument registry derived from the admitted fills

- Status: Accepted
- Date: 2026-11-14
- Deciders: Repository owner (decision D1 in
  [hyperliquid-history-to-site](../../.agents/plans/hyperliquid-history-to-site.md))

## Context

`hl-backfill process` normalizes Hyperliquid trades only for coins in an
instrument registry (`--registry`) and skips, and counts, events for any
other coin. A registry entry needs the market class, the venue symbol, the
size scale (`szDecimals`) and the price scale derived from it
([registry-v0](../specs/registry-v0.md);
`crates/mesh-types/src/registry.rs`).

Live registration reads those facts from the project's own node's info
server, which [ADM-0001](../admissions/ADM-0001-hyperliquid-trades-orders.md)
admits. The backfill runs without a node, so it has no admitted metadata
source. Hyperliquid's public info API is not admitted, and admitting it
would put a terms-governed service on the data path that ADM-0001 keeps
contract-free. Delisted coins also matter: history refers to markets the
venue no longer lists.

## Decision

1. **The backfill registry is derived from the admitted fills themselves**,
   method `hl-backfill-registry/v1`, implemented as `hl-backfill registry`:
   - Objects are read from the raw tier and passed through the same filter
     as `process`, so only admitted, book-matched events count. Liquidation,
     settlement and other mark-priced fills never shape the registry.
   - The class comes from the symbol: plain names are core perpetuals,
     `dex:NAME` HIP-3 perpetuals (namespace, `oracle_anchored_venue`), `#N`
     outcome markets. Spot (`@N`, pairs) is outside ADM-0001 and dropped.
   - The size scale is the largest number of size decimals seen for the coin
     in the selection (trailing zeros ignored). The price scale follows the
     venue's rule (`6 - size scale` for perpetuals, the outcome scale for
     outcome markets).
   - Every observed price must fit the price scale. A coin where one does
     not is reported and **not registered**, so `process` counts its events
     as `unknown_coin` rather than writing them at a wrong scale.
   - Specs carry admission `ADM-0003`, the backfill's admission.
2. **Delisted coins are covered** because they traded in the period.
3. **A node's metadata supersedes** the derived registry when a node runs:
   registered specs are kept and conflicts reported, as registry-v0 § 3
   already provides.
4. **Method versioning (AGENTS.md rule 5).** Any change to these rules is
   `hl-backfill-registry/v2` or later; the report names the method, and the
   published registry is regenerated from the raw tier with the new version.

## Consequences

- No new source and no admission change are needed for the backfill.
- Observed size decimals can be fewer than the venue's `szDecimals`, never
  more. Every size and price in the period is still represented exactly;
  only the scale metadata may differ from the venue's listing.
- A coin whose `szDecimals` changed within a selection can fail the price
  check. Narrowing the date range, or a later method version that splits
  the period, resolves it.
- First run on archived samples (2026-11-14): all 378 perpetuals seen
  (including 159 HIP-3 markets) registered cleanly. All 289 outcome coins
  seen were reported, not registered: their prices have five decimals, while
  the node reader assumes four for outcome markets
  (`OUTCOME_PRICE_SCALE`, an unverified assumption). Outcome markets stay
  out of the backfill until that assumption is corrected in the reader and
  the registry method together. [ADR 0021](0021-outcome-market-price-scale.md)
  does that: outcome markets take the spot rule (`8 - szDecimals`), method
  `hl-backfill-registry/v2`.

## Alternatives considered

- **A narrow admission for the public info API's metadata endpoints.**
  Rejected for now: it adds a terms-governed service to the data path, and
  the fills already carry every fact the backfill needs.
- **A hand-maintained list.** Rejected: it does not scale to hundreds of
  HIP-3 and delisted markets, and it cannot be reproduced from the archive.
