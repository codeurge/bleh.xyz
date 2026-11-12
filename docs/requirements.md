# Requirements

The requirements come from a reference consumer: a production futures
trading platform with order-flow charts, an order-book heatmap, options
analytics (GEX), backtests, paper trading, alerts, daily levels and an agent
API. It currently gets all of this from one exchange-licensed feed. If the mesh
meets its needs, it meets most consumers' needs.

## Requirements and targets

| ID | Requirement | Consumer use | Mesh target |
|---|---|---|---|
| R1 | Every trade, with aggressor side and venue timestamp | Footprint, delta, CVD, big trades, tape | Native for Tier A venues |
| R2 | Top-of-book quotes | Last price, alerts, paper fills | Native for Tier A venues; composite top of book |
| R3 | Order-by-order book (L3), with depth (L2) derived from it | Order-book heatmap, replay | Native for venues that expose it (e.g. own Hyperliquid node), with no embargo |
| R4 | Bars from 1 second to 1 day | Charts, reports, backtests | Per venue and per composite exposure |
| R5 | Long history | Backtests, studies | Permanent archive from day one; import venue history where it exists |
| R6 | Options quotes, open interest, greeks | GEX, options flow | Research problem; crypto only at first |
| R7 | Instrument registry, rolls, session calendars | Symbol identity everywhere | Open registry in git |
| R8 | Daily reference close | Daily change, levels | Open settlement method, recomputable |
| R9 | Updates at 10 per second; prices under 2 seconds old | Live charts, fills | Gateway fan-out within 100 ms of source; freshness on every record |
| R10 | Rate curves, macro data, positioning | Context, agent insights | From Tier B public data |

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
  licensed consumer can measure this internally under its own license. It
  should publish only summary statistics, after legal review.
- **Lineage completeness.** No output field may lack a lineage chain back to
  admitted sources.
- **Determinism.** Given the archive, anyone can recompute composites, bars
  and settlements bit-for-bit.
- **Freshness.** Every record carries source time, receive time and publish
  time, so consumers can apply their own staleness rules.
