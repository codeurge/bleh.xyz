# 0021: Outcome markets use the spot price rule (`8 - szDecimals`)

- Status: Accepted
- Date: 2026-11-15
- Deciders: Repository owner (agent working directly for the owner)
- Amends: [0019](0019-backfill-registry-from-admitted-fills.md) (registry
  method `hl-backfill-registry/v2`) and assumption 9 of the
  [node format spec](../specs/hyperliquid-node-format.md)

## Context

Every instrument has a price scale, the number of decimals its prices are
stored with. A price with more decimals than the scale cannot be stored
exactly, so such coins are left out instead of being rounded.

The node reader assumed a fixed price scale of 4 for HIP-4 outcome markets
(`OUTCOME_PRICE_SCALE`), marked as unverified. The first registry run
(ADR 0019) found that every one of the 289 outcome coins seen had prices
with five decimals. All of them were left out, so the backfill carried no
outcome markets at all.

The venue's documentation gives no separate tick rule for outcome markets.
It describes the outcome trading API as "similar to spot" and outcomes as
sharing "most implementation details with spot trading". For spot it
documents prices of at most five significant figures and at most
`MAX_DECIMALS - szDecimals` decimals, where `MAX_DECIMALS` is 8 for spot
and 6 for perpetuals:

- [HIP-4](https://hyperliquid.gitbook.io/hyperliquid-docs/hyperliquid-improvement-proposals-hips/hip-4-outcome-markets)
- [Asset IDs](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/asset-ids)
- [Tick and lot size](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/tick-and-lot-size)

Outcome prices are probabilities between 0 and 1. With five significant
figures, a price such as 0.12345 needs five decimals and 0.012345 needs
six. Both fit the spot rule.

## Decision

1. **The price scale of a HIP-4 outcome market is `8 - szDecimals`**, the
   documented spot rule. It is implemented as
   `hl_node_reader::assumptions::outcome_price_scale`, and used both for
   live registration from metadata and for the derived backfill registry.
   It replaces `OUTCOME_PRICE_SCALE`.
2. **The registry method becomes `hl-backfill-registry/v2`.** Its only
   change from v1 is the outcome price scale. Perpetual coins get the same
   specs as under v1. Every price observed for an outcome coin must still
   fit the scale, and a coin with a price that does not fit is reported and
   left out, as before.
3. **v1 stays reproducible** from the signed image built at commit
   `a0fd554` (`ghcr.io/codeurge/bleh.xyz/hl-backfill:a0fd554…`). Its
   recorded result: perpetuals registered and every outcome coin left out.
4. The assumption stays marked **to verify** in the node format spec until
   checked against the venue's `outcomeMeta` metadata from the project's
   own node.

## Consequences

- Outcome markets enter the backfill. Their prices are stored exactly,
  with up to eight decimals for whole-number sizes.
- The synthetic fixtures' outcome market (`#9001`) moves to scale 8 with
  the same probabilities. The node files are unchanged, and only the
  fixed-point expected values gain trailing zeros.
- If the venue's real rule turns out narrower, for example a fixed tick of
  0.001, every price stored under this rule is still exact. Only the scale
  metadata is wider than needed. If it turns out wider, coins with prices
  that don't fit are reported by the registry check instead of being
  written wrong.
- No archive has been signed with outcome records yet, so no published
  data changes.

## Alternatives considered

- **Keep the fixed scale of 4.** This leaves every outcome market out of
  the history, and it contradicts the observed data.
- **A fixed scale of 5, from the observed maximum.** It fits the sample.
  But prices below 0.1 with five significant figures need six decimals,
  so a later day could fail.
- **Derive the scale from the observed decimals alone.** The scale could
  then differ between periods for the same coin. It also departs from how
  perpetual scales are derived (from the venue's rule), so a later node's
  metadata would conflict more often.
