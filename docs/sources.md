# Source register

Research snapshot: 2026-11-11. Figures come from news and aggregator sites
unless marked otherwise. Check them against primary sources before relying on
them. Every source needs a legal review under the
[admission policy](admission-policy.md) before the project publishes data
from it.

## Tier A: open venues, read from our own nodes

These venues do their own price discovery and record trades and orders as
public chain state. Reading through our own node avoids any API terms.

### Hyperliquid (core perpetuals and HIP-3 builder-deployed markets)

- **What:** Order-book perpetuals. HIP-3 lets third parties deploy markets on
  equity indices, commodities and single stocks.
- **Scale (secondary sources):**
  - About 315 HIP-3 markets and about $4.08B open interest by late October 2026
  - The S&P 500 perpetual (trade.xyz) launched 2026-03-18 under an S&P Dow
    Jones Indices license, with about $425M open interest
  - Gold at about $308M open interest
  - Oil and precious metals were over 67% of HIP-3 volume in Q1 2026, and the
    WTI contract reportedly peaked at $1.7B daily volume
  - trade.xyz holds roughly 90–94% of HIP-3 open interest, a concentration risk
- **Data available from a non-validating node:** per-block trades, fills,
  order statuses and raw book diffs. An L4 snapshot computed from node state,
  with the order-status stream applied on top, gives a real-time book with
  order IDs. Hyperliquid publishes an open-source order-book server for this.
- **Lineage caution:** trade.xyz computes oracle, mark and "external" prices
  off-chain and pushes them about every 3 seconds. A third-party tracker says
  its oil oracles blend the front two CME contracts. **Trades and orders are
  admissible. Oracle, mark and funding fields are not**, until each deployer's
  method is shown to be clean.
- **Weekend behavior:** while CME is closed, trade.xyz caps how far mark and
  oracle prices can move from the last external price (about ±5% for xyz:CL,
  per an Arrakis analysis). Trades within that band are genuine price
  discovery, though they are still bounded by the last external price.
- **To verify:** the node's file formats and retention (much detail comes from
  vendor docs); each HIP-3 deployer's terms; whether index-licensed markets add
  any data terms.

### Other on-chain order books

- **Candidates:** dYdX v4 (own chain, public node), Derive (options and
  perpetuals on its own OP Stack L2, crypto only), and others.
- **To verify:** whether matching happens off-chain with only settlement
  on-chain. In that case trade data may only be available through an API with
  terms.

## Tier B: free by regulation, or public domain

| Source | Data | Timeliness | Status |
|---|---|---|---|
| CFTC Part 43 public swap dissemination (swap data repositories, e.g. DTCC) | Swap trades and prices: rates (incl. SOFR), commodities (WTI, natural gas), credit, FX | Near real time; block trades delayed and sizes capped | 17 CFR 43.3 requires publicly disseminated data to be free of charge. **To verify:** each repository's site and API terms and formats |
| NY Fed | SOFR, EFFR, OBFR, repo rates | Daily | Public; check attribution terms |
| US Treasury | Yield curves, auction results | Daily | Public domain |
| USDA AMS, including mandatory livestock price reporting | Cash livestock and grain prices from actual transactions | Several times a day | Public domain |
| EIA | Energy spot prices, inventories, production | Daily or weekly | Public domain, **but** some series (e.g. NYMEX futures prices) credit a commercial vendor and are excluded |
| CFTC Commitments of Traders | Positions by trader category for exchange futures | Weekly | Public domain |
| IEX delayed data (≥15 ms) | Equity and ETF quotes and trades from one exchange | 15 ms delay | Fee filings describe delayed data as free to redistribute; a 2025 SEC filing lists delayed data and DEEP+ as free. **To verify:** current Data Subscriber Agreement and attribution |

## Tier C: public, but under terms

Admit only after a review of each source's terms.

| Source | Use | Issue |
|---|---|---|
| Prediction markets (Kalshi, Polymarket) | FOMC and macro event odds | Kalshi's terms reportedly restrict commercial redistribution. Polymarket trades are on-chain, so they may move to Tier A if read from our own node |
| Centralized crypto exchange APIs | Crypto spot and perpetuals | API terms usually restrict redistribution |
| Tokenized gold (PAXG, XAUT) on-chain trades | Gold spot proxy | On-chain trades may qualify for Tier A; centralized venue data does not |

## Excluded

| Source | Reason |
|---|---|
| Exchange feeds and vendors reselling them | Exchange data contracts |
| Oracle networks (Pyth, Chainlink) and on-chain oracle, mark or funding fields | Inputs may include exchange data; Pyth's paid tier lists display rights without redistribution rights |
| FINRA end-of-day Treasury trade file | Free only for non-professional, non-commercial use |
| Broker screens or fills relayed by contributors | Exchange data under the broker's agreement |
| Scraped exchange websites | Website terms; exchange data |

## Sources consulted

- CoinDesk, 2026-03-10: https://www.coindesk.com/markets/2026/03/10/hyperliquid-s-permissionless-market-smashes-usd1-2-billion-in-open-positions-as-oil-and-equity-futures-boom
- Datawallet, HIP-3 explained: https://www.datawallet.com/crypto/hip-3-explained-hyperliquid-upgrade
- Boluo, HIP-3: https://www.boluo.im/crypto/what-is-hip3
- Coinperps, Hyperliquid statistics: https://www.coinperps.com/learn/hyperliquid-statistics
- S&P 500 perpetual launch: https://coinmarketman.com/blog/sp500-perpetual-futures-hyperliquid-hip3--en/
- Hyperliquid markets list: https://hyperliquidguide.com/markets
- HIP-3 market count: https://cryptoticker.io/en/hyperliquid-stock-commodity-perpetuals/
- HIP-3 open-interest concentration: https://cryptobriefing.com/hyperliquid-2-5b-open-interest-tokenized-equities/
- Hyperliquid L1 data schemas: https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/nodes/l1-data-schemas
- Hyperliquid order book server: https://uithub.com/hyperliquid-dex/order_book_server
- Dwellir data catalog: https://www.dwellir.com/docs/hyperliquid/data-catalog
- trade.xyz perp mechanics: https://docs.trade.xyz/perp-mechanics/overview
- RWA roll calendar: https://loris.tools/rwa/roll-calendar
- Arrakis on weekend bounds: https://x.com/ArrakisFinance/status/2032188566353428785
- 17 CFR 43.3: https://www.law.cornell.edu/cfr/text/17/43.3
- Part 43 dataset: https://catalog.data.gov/dataset/part-43-real-time-public-reporting-swaps
- FINRA Rule 6750: https://www.finra.org/rules-guidance/rulebooks/finra-rules/6750
- IEX 2025 SEC filing: https://www.sec.gov/files/rules/sro/iex/2025/34-104060.pdf
- IEX 2021 fee proposal: https://public-inspection.federalregister.gov/2021-25021.pdf
- Pyth pricing: https://pyth.network/pricing
- Blockscholes on on-chain options: https://www.blockscholes.com/research/the-renaissance-of-onchain-options
