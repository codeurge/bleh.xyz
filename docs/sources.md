# Source register

Research snapshot: 2026-11-11. Figures come from news and aggregator sites
unless marked otherwise. Check them against primary sources before relying on
them. Each source's fields are admitted under the
[admission policy](admission-policy.md).

## Tier A: open venues, read from our own nodes

These venues do their own price discovery and record trades and orders as
public chain state. Reading through our own node avoids any API terms.

### Hyperliquid (core perpetuals, HIP-3 and HIP-4 markets)

- **What:** Order-book perpetuals. HIP-3 lets third parties deploy markets on
  equity indices, commodities, rates, FX and single stocks. HIP-4 adds
  outcome markets.
- **Scope:** every market on the node, recorded and published from day one;
  new markets register automatically from node metadata (owner decision,
  2026-11-11). Admitted under
  [ADM-0001](admissions/ADM-0001-hyperliquid-trades-orders.md). Each market
  carries a liquidity grade (spread, depth, trades per minute), so thin
  markets are labelled as tapes, not benchmarks.
- **Reported listings beyond index, metals and oil (to confirm from node
  metadata):** trade.xyz reportedly lists NATGAS, TTF, copper, platinum,
  palladium, CORN, WHEAT, EUR, JPY and DXY; Kinetiq reportedly lists a
  USBOND perpetual, the only tick-level duration exposure found so far.
- **Scale (secondary sources):**
  - About 315 HIP-3 markets and about $4.08B open interest by late October 2026
  - The US large-cap index perpetual (trade.xyz) launched 2026-03-18 under a
    license from the index provider, with about $425M open interest
  - Gold at about $308M open interest
  - Oil and precious metals were over 67% of HIP-3 volume in Q1 2026, and the
    WTI contract reportedly peaked at $1.7B daily volume
  - trade.xyz holds roughly 90–94% of HIP-3 open interest, a concentration risk
- **Data available from a non-validating node:** per-block fills, order
  statuses and raw book diffs (`--write-fills`, `--write-order-statuses`,
  `--write-raw-book-diffs`, with `--batch-by-block`). Fills arrive one per
  user, so a book-matched trade is two fills sharing a `tid`, one with
  `crossed: true`. An L4 snapshot is requested from the node's local info
  server (`--serve-info`, `fileSnapshot` with `l4Snapshots`); state
  snapshots on disk (`periodic_abci_states`) are written every 10,000
  blocks. The snapshot plus the order-status stream gives a real-time book
  with order IDs.
- **Reference code:** `hyperliquid-dex/order_book_server` shows one way to
  serve an L4 book from node output. It has no LICENSE file and its README
  says it is a standalone project not written by the Hyperliquid core team.
  The mesh's reader is written from the documented schemas and copies no
  code from it.
- **Oracle anchoring (HIP-3):**
  - The HIP-3 mark is the median of up to two deployer-supplied marks and a
    local mark (median of best bid, best ask and last trade). Each update
    moves the mark at most 1%. Deployers are expected to update the oracle
    about every 3 seconds.
  - Hyperliquid uses the mark for margining, liquidations, TP/SL triggers and
    unrealized PnL.
  - `haltTrading` cancels all orders and settles positions at the current
    mark price.
  - trade.xyz bounds its mark within ±(1/max leverage) of a reference price
    in all sessions: the last externally derived fair price while the
    reference market is open, and the last external oracle price (for
    example the Friday close) while it is closed. At 20× maximum leverage
    that is ±5%. trade.xyz does not name its price sources; a third-party
    tracker says its oil oracles blend the front two CME contracts.
- **Core perpetuals:** mark and funding use Hyperliquid's validator oracle,
  described as a weighted median of centralized-exchange spot prices (to
  verify against current docs).
- **What the mesh publishes:** book-matched trades and order-level book
  events. These are formed by the venue's own matching. Within the mark band,
  traders set prices freely; the band and liquidations at mark are outside
  prices acting on the market. Records from these markets carry
  `oracle_anchored_venue`. Oracle, mark, funding, liquidation and settlement
  fields are never published.
- **To verify:** how ADL fills and HIP-4 outcome markets appear in node
  output; node file retention and pruning; each HIP-3 deployer's documented
  oracle method.

### Next venues (Phases 2–3)

Second and third venues give a real consolidated tape per exposure. Each is
read from infrastructure the project runs and gets its own admission record:

| Venue | Exposure | How we read it |
|---|---|---|
| Solana tokenized stocks (xStocks, Ondo) | Single stocks and index ETFs | Own RPC node with Geyser; decode AMM swaps |
| Uniswap v4 tokenized securities; PAXG and XAUT pools | Stocks; gold spot | Own Ethereum node; decode pool swaps |
| Derive | Crypto options and perpetuals | Own node for its L2; options trades feed crypto surfaces |
| dYdX v4 | Crypto perpetuals | Own full node |
| Lighter | Perpetuals | Decode the rollup data it posts in Ethereum blobs |
| Polymarket | FOMC and macro event odds | Own Polygon node |

To verify per venue: whether matching happens on-chain or only settlement,
and which fields are matched rather than oracle-priced.

## Tier B: free by regulation, or public domain

| Source | Data | Timeliness | Status |
|---|---|---|---|
| CFTC Part 43 public swap dissemination (DTCC, ICE Trade Vault) | SOFR and OIS swaps, FX NDFs and options, natural gas, power and refined-product swaps, swaptions and caps, commodity options | Near real time; block trades delayed and sizes capped | Free of charge under 17 CFR 43.3(c)(2); repositories need only keep a year online (43.3(c)(1)), so capture is time-critical. Phases 1–2 lane ([ADM-0002](admissions/ADM-0002-cftc-part43-swaps.md)) |
| SEC Regulation SBSR public dissemination (DTCC, ICE Trade Vault) | Equity-index swaps, single-name CDS | Near real time, with dissemination delays for some trades | Non-fee basis under 17 CFR 242.900(cc) and (tt). Phases 1–2 lane ([ADM-0004](admissions/ADM-0004-sec-sbsr-public-dissemination.md)) |
| NY Fed | SOFR, EFFR, OBFR, repo rates | Daily | Public; check attribution terms |
| US Treasury | Yield curves, auction results | Daily | Public domain |
| USDA AMS, including mandatory livestock price reporting | Cash livestock and grain prices from actual transactions | Several times a day | Public domain |
| EIA | Energy spot prices, inventories, production | Daily or weekly | Public domain, **but** some series (e.g. NYMEX futures prices) credit a commercial vendor and are excluded |
| CFTC Commitments of Traders | Positions by trader category for exchange futures | Weekly | Public domain |

## Tier C: public, but under terms

Data reached only through API terms. The on-chain equivalents above replace
these where they exist.

| Source | Use | Issue |
|---|---|---|
| Kalshi | FOMC and macro event odds | Terms reportedly restrict commercial redistribution; Polymarket covers the same events on-chain |
| Centralized crypto exchange APIs | Crypto spot and perpetuals | API terms usually restrict redistribution |
| Tokenized gold on centralized venues | Gold spot proxy | Covered on-chain by PAXG and XAUT pools |

## Excluded

| Source | Reason |
|---|---|
| Exchange feeds and vendors reselling them | Exchange data contracts |
| Oracle networks (Pyth, Chainlink) and on-chain oracle, mark or funding fields | Inputs may include exchange data; Pyth's paid tier lists display rights without redistribution rights |
| FINRA end-of-day Treasury trade file | Free only for non-professional, non-commercial use |
| Broker screens or fills relayed by contributors | Exchange data under the broker's agreement |
| Scraped exchange websites | Website terms; exchange data |
| Ostium | Oracle-priced trades, not matched; a July 2026 oracle-signer exploit was reported |
| IEX delayed data | An exchange feed distributed under IEX's data agreements, and not needed: index exposure comes from venue markets (owner decision, 2026-11-11). A 2025 SEC filing may have redefined "delayed" as 15 minutes (unconfirmed) |

## Sources consulted

- CoinDesk, 2026-03-10: https://www.coindesk.com/markets/2026/03/10/hyperliquid-s-permissionless-market-smashes-usd1-2-billion-in-open-positions-as-oil-and-equity-futures-boom
- Datawallet, HIP-3 explained: https://www.datawallet.com/crypto/hip-3-explained-hyperliquid-upgrade
- Boluo, HIP-3: https://www.boluo.im/crypto/what-is-hip3
- Coinperps, Hyperliquid statistics: https://www.coinperps.com/learn/hyperliquid-statistics
- Index perpetual launch: https://coinmarketman.com/blog/sp500-perpetual-futures-hyperliquid-hip3--en/
- Hyperliquid markets list: https://hyperliquidguide.com/markets
- HIP-3 market count: https://cryptoticker.io/en/hyperliquid-stock-commodity-perpetuals/
- HIP-3 open-interest concentration: https://cryptobriefing.com/hyperliquid-2-5b-open-interest-tokenized-equities/
- Hyperliquid L1 data schemas: https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/nodes/l1-data-schemas
- Hyperliquid order book server (no license): https://github.com/hyperliquid-dex/order_book_server
- Hyperliquid node (README, flags, info server): https://github.com/hyperliquid-dex/node
- HIP-3 deployer actions: https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/hip-3-deployer-actions.md
- HIP-3 overview (halt and settlement): https://hyperliquid.gitbook.io/hyperliquid-docs/hyperliquid-improvement-proposals-hips/hip-3-builder-deployed-perpetuals
- Mark price uses: https://hyperliquid.gitbook.io/hyperliquid-docs/trading/robust-price-indices
- trade.xyz discovery bounds: https://docs.trade.xyz/perpetuals/mechanics/discovery-bounds.md
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
