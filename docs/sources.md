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
| NY Fed | SOFR, EFFR, OBFR, repo rates | Daily | Admitted under the NY Fed's Terms of Use, with notice ([ADM-0007](admissions/ADM-0007-nyfed-reference-rates.md)); see [public settlement anchors](#public-settlement-anchors-and-reference-data) |
| US Treasury | Yield curves, auction results | Daily | Admitted ([ADM-0006](admissions/ADM-0006-us-treasury-rates-and-auctions.md)) |
| USDA AMS, including mandatory livestock price reporting | Cash livestock and dairy prices from actual transactions | Several times a day | Admitted by purchase type ([ADM-0010](admissions/ADM-0010-usda-livestock-mandatory-reporting.md) to [ADM-0012](admissions/ADM-0012-usda-market-news-feeder-cattle-and-grain.md)); grain bids rejected as futures plus basis |
| EIA | Energy spot prices, inventories, production | Daily or weekly | Public domain, **but** some series (e.g. NYMEX futures prices) credit a commercial vendor and are excluded |
| CFTC Commitments of Traders | Positions by trader category for exchange futures | Weekly | Admitted ([ADM-0005](admissions/ADM-0005-cftc-commitments-of-traders.md)) |

## Public settlement anchors and reference data

Decided 2026-11-12 under the
[open reference prices plan](../.agents/plans/open-reference-prices.md).
Endpoints, formats and schedules are in
[specs/reference-sources.md](specs/reference-sources.md).

- **Anchors** names the futures families each source relates to, stated as
  economic fact.
- **Exclusions** are listed in each record.
- Records marked `exchange_referenced` come from transactions that may be
  privately priced against an exchange price
  ([ADR 0012](decisions/0012-open-terms-route-for-public-bodies-and-market-operators.md)).

### Admitted

| Source | Record | Anchors | History | Cadence |
|---|---|---|---|---|
| CFTC Commitments of Traders (legacy, disaggregated, TFF, supplemental) | [ADM-0005](admissions/ADM-0005-cftc-commitments-of-traders.md) | Positioning in every US futures and options market the CFTC reports. Not a settlement anchor | Legacy from 1986-01-15; disaggregated and TFF from 2006-06-13; supplemental from 2006-01-03 | Weekly: Tuesday data, released Friday about 3:30 p.m. ET |
| US Treasury par, real, bill and long-term rates | [ADM-0006](admissions/ADM-0006-us-treasury-rates-and-auctions.md) | Treasury futures are physically delivered, so these are carry and fair-value inputs for rates futures rather than settlement prices | Par curve from 1990; bills from 2002; real curve from 2003; long-term from 2000 | Daily, usually by 6:00 p.m. ET |
| US Treasury auction results (Fiscal Data) | [ADM-0006](admissions/ADM-0006-us-treasury-rates-and-auctions.md) | Auction yields and prices of new issues. The 13-week bill high rate is the index rate of Treasury floating rate notes | From 1979-11-15 | Per auction |
| NY Fed SOFR, BGCR, TGCR, SOFR averages and index | [ADM-0007](admissions/ADM-0007-nyfed-reference-rates.md) | SOFR is the rate that one-month and three-month SOFR futures settle against, averaged or compounded over the contract period | SOFR, BGCR and TGCR from 2018-04-02; averages and index from 2020-03-02 | Daily, about 8:00 a.m. ET |
| NY Fed EFFR, OBFR | [ADM-0007](admissions/ADM-0007-nyfed-reference-rates.md) | EFFR is the rate that 30-day federal funds futures settle against, as a monthly average | EFFR from 2000-07-03 (API) and from 1954 via H.15; OBFR from 2016-03-01 | Daily, about 9:00 a.m. ET |
| Federal Reserve Board H.15 | [ADM-0008](admissions/ADM-0008-federal-reserve-board-h15.md) | Long EFFR and Treasury constant-maturity history; commercial paper, prime and discount rates as rate inputs | EFFR from 1954; bills from 1954; CMTs from 1962; commercial paper from 1997 | Daily |
| Federal Reserve Board H.10 noon buying rates | [ADM-0009](admissions/ADM-0009-federal-reserve-board-h10.md) | Spot FX references for currency futures fair values. Most currency futures are physically delivered | From 1971 for the major currencies | Weekly release of daily rates (Mondays) |
| USDA Livestock Mandatory Reporting | [ADM-0010](admissions/ADM-0010-usda-livestock-mandatory-reporting.md) | Negotiated and formula barrow and gilt prices, from which the cash index that lean hog futures settle against is computed (formula prices flagged). The negotiated pork cutout, from which the cash index that pork cutout futures settle against is computed. Negotiated fed cattle prices, a fair-value input for live cattle futures, which are physically delivered | From 2001-04-01 | Several times a day; weekly and monthly summaries |
| USDA NDPSR and announced FMMO prices | [ADM-0011](admissions/ADM-0011-usda-dairy-ndpsr-and-class-prices.md) | The announced Class III and Class IV prices that Class III and Class IV milk futures settle against. The NDPSR averages that cash-settled butter, cheese, dry whey and nonfat dry milk futures settle against. All flagged `exchange_referenced` | As held by the DataMart | NDPSR weekly (Wednesday, four-week revisions); class prices monthly |
| USDA feeder cattle auction, direct and video reports | [ADM-0012](admissions/ADM-0012-usda-market-news-feeder-cattle-and-grain.md) | USDA and State market news feeder steer sales, from which the cash index that feeder cattle futures settle against is computed | Latest issue only on ams.usda.gov; archive channel pending | Daily and weekly |
| ERCOT DAM and RTM settlement point prices, LMPs, AS clearing prices | [ADM-0013](admissions/ADM-0013-ercot-prices.md) | ERCOT hub and load-zone settlement point prices, which ERCOT power futures and swaps settle against | Hub and zone prices from 2010; node-level prices only within the 5-to-31-day listing window | Day-ahead daily by 13:30 CPT; real time every 15 minutes and per SCED run |

### Rejected or under review

| Source | Record | Status | Deciding clause, in short |
|---|---|---|---|
| USDA grain bids and basis | [ADM-0012](admissions/ADM-0012-usda-market-news-feeder-cattle-and-grain.md) | Rejected | Quoted as futures plus basis |
| CAISO (NP15 and SP15 hub prices are settlement references for western power contracts) | [ADM-0014](admissions/ADM-0014-caiso-prices.md) | Rejected | API terms grant no rights in CAISO Data |
| PJM (Western Hub prices are settlement references for eastern power contracts) | [ADM-0015](admissions/ADM-0015-pjm-prices.md) | Rejected | Non-members may not republish |
| MISO | [ADM-0016](admissions/ADM-0016-miso-prices.md) | Rejected | Terms forbid publishing or distributing content |
| NYISO | [ADM-0017](admissions/ADM-0017-nyiso-prices.md) | Under review | Rights reserved, no redistribution grant |
| ISO New England | [ADM-0018](admissions/ADM-0018-iso-ne-prices.md) | Rejected | Duplication or non-personal use "may violate copyright" |
| SPP | [ADM-0019](admissions/ADM-0019-spp-prices.md) | Rejected | Commercial use needs written authorization |

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
