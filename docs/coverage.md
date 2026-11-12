# Expected coverage

These are estimates from the [source register](sources.md). They change as
venues list new markets. Requirement IDs refer to
[requirements.md](requirements.md).

| Asset class | Trades, book, order flow (R1–R3) | Bars and history (R4–R5) | Curves and reference close (R8, R10) |
|---|---|---|---|
| Equity indices | **Strong.** S&P 500 perpetual and other index perpetuals on Hyperliquid; IEX delayed ETF data if admitted | Good from venue launch (2026) onward | Good |
| Gold, silver | **Strong** | Good | Good |
| Copper, platinum | Partial; depends on listings | Partial | Partial |
| Crude oil | **Strong** for WTI and Brent | Good | Good; Part 43 swap prints help |
| Natural gas | Partial | Partial | Part 43 swap prints |
| Gasoline, heating oil | Weak | Weak | Part 43 swap prints, EIA |
| Crypto | **Complete.** Here the exchange futures follow the open markets | Complete | Complete |
| Interest rates, Treasuries | Weak at the tick level | Daily | **Strong.** Part 43 swaps, NY Fed, Treasury |
| FX | Partial; listings need checking | Partial | Daily official reference rates |
| Grains | None | Daily USDA cash prices | USDA only |
| Livestock | None | USDA mandatory reporting, several times a day | USDA only |
| Options (R6) | Crypto only (e.g. Derive) | — | — |

## Biggest gaps

1. **Options on indices and commodities.** On-chain options volume is under
   1% of on-chain perpetual volume, and the main venues are crypto-only.
2. **Tick-level rates.** Swap prints give curves, not a futures-like tape.
3. **Agriculture.** No open venue trades grains or livestock at depth.
4. **Deep history.** History builds up from the day the mesh starts recording,
   except where venue archives go back further.
5. **Venue concentration.** One HIP-3 deployer holds most of the open
   interest. If it fails, much coverage goes with it.
