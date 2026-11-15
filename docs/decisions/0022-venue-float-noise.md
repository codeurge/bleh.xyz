# 0022: Round Hyperliquid decimals with floating-point noise to 15 significant digits

- Status: Accepted
- Date: 2026-11-15
- Deciders: Repository owner (agent working directly for the owner)
- Amends: [0019](0019-backfill-registry-from-admitted-fills.md) (registry
  method `hl-backfill-registry/v3`) and adds assumption 10 to the
  [node format spec](../specs/hyperliquid-node-format.md)

## Context

Canonical data never uses floating point (`mesh_types::decimal`). A venue
price or size is parsed into the instrument's fixed scale, and a string
with more significant decimals than the scale is rejected rather than
rounded.

The registry run over 2025-07-27 to 2026-11-14 (method v2: 11,416
objects, 3.73 billion fill objects) left out 29 perpetuals, including SUI,
ARB, OP and APE. Each had a size string with ten decimals. The archived
by-block fills show the cause. A liquidation fill on 2025-10-10 at 21:17
UTC carries `"sz":"2946657.1000000001"` and
`"closedPnl":"-1945471.4171330004"`. These are binary floating-point
values printed with 17 significant digits; the intended size is
2,946,657.1. A 64-bit float holds only 15 to 17 significant decimal
digits, and it holds 15 exactly, so the trailing `…0001` is noise from the
venue's arithmetic, not information. The same noise could appear in live
node output.

## Decision

1. **Assumption 10 of the Hyperliquid reader.** A venue decimal string
   with more than 15 significant digits is rounded, half up, to 15
   significant digits, and trailing fractional zeros are dropped. The
   rounding is decimal arithmetic on the string, with no floating point
   (`hl_node_reader::assumptions::venue_decimal`). Strings with 15 or
   fewer significant digits are not touched.
2. It is applied wherever Hyperliquid text becomes a fixed-point price or
   size:
   - the live normalizer;
   - L4 snapshots;
   - the backfill parsers for `node_fills` and `node_trades`;
   - the replay matcher;
   - the derived registry.
   A value that still has more decimals than the instrument's scale after
   rounding is rejected as before.
3. **The registry method becomes `hl-backfill-registry/v3`.** Its only
   change from v2 is that prices and sizes are rounded this way before
   their decimals are counted. Its report counts the rounded strings
   (`float_noise_rounded`). v2 stays reproducible from the image built at
   `27d51a4`.
4. This is not a general rounding rule. Venue values with up to 15
   significant digits that do not fit the scale (for example after a
   `szDecimals` change) are still reported and left out.

## Consequences

- The 29 perpetuals enter the backfill with their venue scales. Each one
  had a size with float noise in at least one fill.
- Records of those fills carry the intended value (2946657.1), not a
  value that no valid order could have.
- Exactness otherwise holds: nothing with 15 or fewer significant digits
  changes, and Hyperliquid's own rules (prices of at most five
  significant figures; sizes at `szDecimals`) keep real values far below
  15 digits.
- `closedPnl` and other non-admitted fields are not parsed, so the rule
  does not touch them.

## Alternatives considered

- **Leave the coins out.** This loses the full history of 29 major
  markets because of single fills.
- **Round to the instrument's scale.** That is general rounding, which
  would hide real scale changes and break the exactness rule.
- **Shortest round-trip of the parsed double.** Strings such as
  `0.30000000000000004` are already the shortest form of their double, so
  that rule would leave them unchanged; rounding to 15 significant digits
  covers both cases.
- **Drop only the affected fills.** Liquidations are trades and part of
  the tape; dropping them biases the history at exactly the moments it
  matters most.
