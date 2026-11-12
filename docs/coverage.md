# Expected coverage

These are estimates from the [source register](sources.md). They change as
venues list new markets. Requirement IDs refer to
[requirements.md](requirements.md). Phase 1 publishes every Hyperliquid
market on the node (owner decision, 2026-11-11), each with a liquidity
grade; "tape" below means a market thin enough to be labelled a tape, not a
benchmark. Listings marked "reported" are confirmed from node metadata at
launch.

| Asset class | Trades, book, order flow (R1–R3) | Bars and history (R4–R5) | Curves, surfaces and reference close (R6, R8, R10) |
|---|---|---|---|
| Equity indices | **Strong.** US large-cap index and other index perpetuals (HIP-3); tokenized stocks and ETFs on Solana and Uniswap v4 (Phases 2–3) | Good from venue launch (2026), via backfill | Good; equity-index swaps (SBSR) |
| Gold, silver | **Strong.** HIP-3 perpetuals; PAXG and XAUT pools (Phases 2–3) | Good | Good |
| Copper, platinum, palladium | Tape (reported trade.xyz listings) | Partial | Partial |
| Crude oil | **Strong** for WTI and Brent | Good | Good; Part 43 swaps and commodity options |
| Natural gas | Tape: NATGAS and TTF (reported) | Partial | Part 43 swaps and options |
| Gasoline, heating oil, power | None on venues | — | Part 43 refined-product and power swaps; EIA |
| Crypto | **Complete.** Here the exchange futures follow the open markets | Complete | Complete; option surfaces from Derive and HIP-4 ladders (Phase 3) |
| Interest rates, Treasuries | Tape: USBOND perpetual (Kinetiq, reported), the only tick-level duration exposure | Daily | **Strong.** Part 43 SOFR and OIS swaps, swaptions and caps (Phases 1–2 lane); NY Fed, Treasury (Phase 3) |
| Credit | None on venues | — | Single-name CDS (SBSR) |
| FX | Tape: EUR, JPY, DXY (reported) | Partial | Part 43 NDFs and FX options; daily official reference rates |
| Grains | Tape: CORN, WHEAT (reported) | Daily USDA cash prices | USDA |
| Livestock | None | USDA mandatory reporting, several times a day | USDA |
| Event odds | HIP-4 outcome markets; Polymarket (Phases 2–3) | Good | — |
| Options (R6) | Derive options trades (Phases 2–3) | — | Crypto surfaces (Derive, HIP-4); index and commodity surfaces from swaptions and commodity options (Phase 3) |

## Biggest gaps

1. **Listed options on indices and commodities.** On-chain options volume is
   under 1% of on-chain perpetual volume, and the main venues are
   crypto-only. Surfaces come from Part 43 swaptions and commodity options
   instead.
2. **Tick-level rates.** One thin perpetual; swap prints give curves, not a
   futures-like tape.
3. **Agriculture depth.** Grain perpetuals exist but are thin; livestock has
   no venue.
4. **Order-level history before recording.** Trades and book snapshots go
   back to the start of Hyperliquid's public archive; order-level history
   before our own recording comes from replaying `replica_cmds` through an
   emulated matcher.
5. **Venue concentration.** One HIP-3 deployer holds most of the open
   interest. The Phase 2–3 venues reduce this.
