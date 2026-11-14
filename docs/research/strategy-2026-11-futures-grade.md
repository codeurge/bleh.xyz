# Strategy review: futures-grade prices without exchange data

- Date: 2026-11-14
- Status: Research note. Nothing here is a decision. Items marked
  **Owner decision** or **Admission needed** need a record in
  [docs/decisions/](../decisions/) or [docs/admissions/](../admissions/)
  first ([AGENTS.md](../../AGENTS.md) rules 2 and 4).
- Scope: how close an open, clean method can get to the futures prices
  for gold, silver, crude oil, natural gas, the US large-cap and US tech
  index exposures, rates, euro FX and crypto. Also what the one
  ~$200/month server should run, the project's status and risks, the
  best zero-cost moves, and a 90-day plan.
- Clean-lineage note: this document contains **no prices**. It was
  written without reading, quoting or comparing against exchange data,
  exchange-data vendors, broker screens or oracle networks. Figures are
  volumes and open interest at the order-of-magnitude level, from the
  sources in § 11, for planning only.

## 1. Executive summary

1. A clean price that is "really close" to the exchange's is achievable for gold, silver, WTI and Brent, and the US large-cap and tech index exposures while the exchange is open (design target: front months within about 5–30 bps). Nobody can *prove* tick-level closeness cleanly, because no public-domain copy of those exchange prices exists to measure against.
2. Weekends and the exchange's daily break are where the project can beat the exchange, not just match it. trade.xyz's oracle then runs off its own order book, so open prices exist when the exchange publishes none. This should be the headline product.
3. Every liquid open market for these exposures is on one HIP-3 deployer, trade.xyz. Felix, Ventuals, HyENA, Kinetiq and dreamcash have wound down or delisted. This is the largest structural risk.
4. Funding fields must stay excluded, because they are computed from the oracle. Method v2 replaces them with venue-bias estimates from open data and an exact roll-blend model: trade.xyz moves its oil reference over business days 5–10 in 20% daily steps.
5. EIA spot and futures prices, US Mint, FRED gold and IMF prices all come from vendors or benchmark administrators, so all are inadmissible. Part 43 swap prints are the only clean forward anchors for oil and gas, and DTCC's redistribution terms are unverified.
6. The $200 box should run the Hyperliquid node and the origin. It is the only admissible live path to more than 90% of the clean liquidity for every target exposure, including oracle-free Hyperliquid spot gold once that is admitted.
7. Everything else stays off the box. The home-archive server keeps the archive, pollers and backfill. Free ARM relays carry the fan-out. GitHub Actions becomes a second capture path. Vercel and R2 serve the site and the data.
8. $200 is tight. Only Cherry's Ryzen 7950X on annual billing (about $198, out of stock) or a Contabo 24-core (about €199, not yet checked for node suitability) fits. Budget $230–300 for month-to-month billing.
9. Before spending anything: make the home server safe (a failing cache NVMe, a single copy of the manifest key), settle the DTCC terms, and put the year of Hyperliquid gold, oil and index tape on the site.
10. In 90 days: history on the site, the node live, `synthetic-months@2` with uncertainty bands, an honest "How close" page and a weekend board built from clean measures only, applications to AWS, Cloudflare and NLnet, then Show HN.

## 2. What "really close" can mean, cleanly

A clean futures-equivalent price has three sources of error, and only
some of them can be measured without exchange data.

1. **Proxy error:** the gap between the open venue's price and what that
   venue tracks. On a HIP-3 perpetual during reference hours this gap is
   small. Funding and the mark band tie the perpetual to the deployer's
   reference, and arbitrage ties the reference to the exchange. The gap
   is largest when the reference market is closed, which is also when
   the open price is most valuable.
2. **Conversion error:** errors from three sources:
   - carry: financing, the lease rate for metals, dividends for indices;
   - the roll: which month a venue tracks, and how it blends two months
     during a roll window;
   - resolution: Hyperliquid prices have at most five significant
     figures, so an exposure quoted above 10,000 moves in whole units.
3. **Measurement error:** without exchange prices, the absolute error
   can only be bounded indirectly (§ 4.5).

The claim the project can make and defend:

| Session | Gold, silver | WTI, Brent | US large-cap, US tech index | Natural gas | Rates, FX |
|---|---|---|---|---|---|
| Reference market open (Sunday 18:00 to Friday 17:00 New York time, with a daily one-hour break) | Close: deep HIP-3 books, plus tokenized-gold spot as a level anchor | Close outside roll windows; wider during rolls | Close: deep HIP-3 books; dividend and financing estimates limit the back months | Indicative only (thin venue); the curve from swap prints is better than the front month | Daily: strong (public rates). Intraday: swap prints only |
| Daily break, holidays, weekends | **Unique:** the venue keeps trading, and its mark is built from its own book inside published bounds. No exchange publishes a price then | **Unique**, as for gold | **Unique**, as for gold | Thin | None |
| Expiry and settlement | No public-domain settlement exists; only an open reference close | Same | Same | Same | **Exact checks possible** for SOFR and fed funds months, and for USDA-settled agricultural months |

So the goal is met **as a close, labelled, independent price** for gold,
silver, crude and the two US index exposures during reference hours. It
is **exceeded** on weekends, when the open venues are the only price
there is. It is **not** met, and cannot be met cleanly, as a claim of
*measured* tick-level equality with the exchange's own price.

## 3. Exposure map

Figures are approximate orders of magnitude from a research read on
2026-11-14 (§ 11). They come from the venue's public info API (read for
research only; it is not an admitted source), Castle Labs, Loris and
DefiLlama. They are not data. They must be re-measured from admitted
sources (the node, the buckets) before anything relies on them.

### 3.1 Gold and silver

| Source | Kind | Liquidity reality | Hours | Admissible? | Cost to read | Role |
|---|---|---|---|---|---|---|
| trade.xyz `xyz:GOLD` (HIP-3) | Order-book perpetual, 25×, referenced to **spot** gold (no roll) | About $70M a day on weekdays and $17M on weekend days; about $280M open interest | 24/7. When the reference is closed, the oracle is a 30-minute moving average of the venue's own impact bid and ask, inside ±1/max-leverage bounds | **Admitted** (ADM-0001, flagged `oracle_anchored_venue`) | Node; history is in the copied buckets | Primary; tracks cash |
| trade.xyz `xyz:SILVER` | As above, spot-referenced | About $125M a day on weekdays; $150M open interest | 24/7 | Admitted | Node | Primary for silver |
| Hyperliquid core `PAXG` perpetual | Order-book perpetual priced off the validator oracle | About $4M a day; $23M open interest | 24/7 | Admitted | Node | Tape; cross-check |
| Hyperliquid **spot** XAUT0, XAUM (and an SLV token) | Order-book spot with **no oracle at all** | Small (XAUT0 about $2M a day in one undated snapshot) | 24/7 | **Not admitted:** spot is outside ADM-0001 ([node format](../specs/hyperliquid-node-format.md), "Spot is outside ADM-0001"). **Admission needed** | Same node and the same copied fills, so no extra cost | Oracle-free cash anchor for gold (§ 4.3 C) |
| PAXG and XAUT pools on Uniswap (Ethereum) | AMM swaps of redeemable gold tokens | Pools of roughly $0.3–8M. The tokens usually trade 0.5–2.5% below spot, with brief dislocations | 24/7 | Admissible from **our own Ethereum node**; not admitted yet | An Ethereum full node (about 2 TB SSD), or a policy amendment for verified third-party RPC | Second cash anchor, with a premium model |
| Lighter gold perpetual | Order book on a zk rollup; an oracle marks positions | About $25M a day | 24/7 for metals; equity markets 24/5 | To review: data comes through Ethereum blobs or its REST API (terms unchecked) | Blob decoding or the API | Later second venue |
| edgeX, Extended, GRVT, Paradex gold or silver | Order books | Under $20M a day each | 24/7 | To review | A reader for each | Low priority |
| Aster gold | Order book with hidden orders | High volume, low open interest | 24/7 | Weak, because some flow is hidden | — | Skip |
| Ostium, gTrade, Avantis | **Fills happen at the oracle price** | — | — | **Excluded:** no independent price formation | — | None |
| Tokenized gold on centralized exchanges | Order books | Large | 24/7 | Tier C (API terms) | — | None |

### 3.2 Crude oil and natural gas

| Source | Kind | Liquidity reality | Hours | Admissible? | Cost | Role |
|---|---|---|---|---|---|---|
| trade.xyz `xyz:CL` (WTI) | Order-book perpetual, 20×, referenced to a designated futures month. It rolls from business day 5 to business day 10 of each month in 20% daily steps | About $270M a day on weekdays and $70M on weekend days; $145M open interest | 24/7, book-driven when the reference is closed | Admitted | Node | Primary front-month tracker (`blend` during rolls) |
| trade.xyz `xyz:BRENTOIL` | As above, for Brent | About $195M a day; $290M open interest | 24/7 | Admitted | Node | Primary for Brent; a check on the WTI-Brent spread |
| trade.xyz `xyz:NATGAS` | As above, 10×, isolated margin only | About $18M a day; $20M open interest | 24/7 | Admitted | Node | Tape |
| CFTC Part 43 commodity swaps (DTCC) | Bilateral calendar-month swap prints, at the fixed price the counterparties agreed | Plentiful for natural gas and crude; block trades delayed and sizes capped | Near real time in business hours | **Admitted** (ADM-0002). Terms check pending (§ 6.3) | Already polled | Forward-curve anchors for every month beyond the front |
| EIA spot prices (WTI, Brent, Henry Hub, products) | Daily assessments | — | Daily | **Excluded:** republished "with permission" from Refinitiv (LSEG); see § 3.5 | — | None |
| EIA Weekly Petroleum Status Report (stocks, including Cushing) | EIA's own survey data, public domain | Weekly | — | Admissible under ADR 0012; **admission needed** | Free | Regime flag only, never a price |

### 3.3 US large-cap and US tech indices

| Source | Kind | Liquidity reality | Hours | Admissible? | Cost | Role |
|---|---|---|---|---|---|---|
| trade.xyz `xyz:SP500` | Order-book perpetual, 50×, on the licensed US large-cap index. The reference is the cash index in the cash session and futures-based quotes otherwise | About $395M a day on weekdays and $73M on weekend days; $495M open interest | 24/7 | Admitted. Output names stay neutral, because the index name is a licensed trademark | Node | Primary; tracks cash |
| trade.xyz `xyz:XYZ100` | Order-book perpetual, 30×, on the **deployer's own** tech-index basket, not the official index | About $420M a day on weekdays and $125M on weekend days; $305M open interest | 24/7 | Admitted | Node | Primary for US tech, with a caveat: the basket's tracking error is unmeasured |
| trade.xyz single US stocks and ETFs (about 87) | Order-book perpetuals | Varies | 24/7 | Admitted | Node | Cross-check; later, an open index replica (§ 4.4) |
| Hyperliquid spot SPY and QQQ tokens | Order-book spot of fund wrappers | Unknown | 24/7 | Not admitted (spot) | Node | Wrapper tracker once admitted |
| Solana xStocks (SPYx, QQQx), Ondo, Robinhood Chain tokens | AMM and solver trades of fund wrappers | Thin. On weekdays they follow the issuer's mint and redeem against the listed fund | 24/7 on DEXes | Admissible only from chain state, and a Solana RPC node is far beyond budget | Not affordable as our own node | Weak signal out of hours; later |
| Lighter US500 and US100 | Order books | Small | 24/5 | To review | — | Later |
| SEC SBSR equity-index swaps (DTCC) | Swap prints, mostly financing spreads rather than index levels | Moderate | Business hours | Admitted (ADM-0004) | Polled | Evidence on financing and dividends; research only |

### 3.4 Rates, FX and crypto

| Exposure | Best clean sources | What they support |
|---|---|---|
| 10-year note and bond futures | Treasury par curve and auctions (ADM-0006, daily); Part 43 SOFR swaps (ADM-0002, intraday); Paragon's `para` 2Y/10Y/30Y rate perpetuals on HIP-3 (live, liquidity unchecked; Kinetiq's USBOND is delisted) | Daily values are strong; intraday values come from swap prints |
| SOFR and fed funds futures | NY Fed SOFR and EFFR (ADM-0007); Part 43 OIS swaps | **Exact** expiry checks possible |
| Euro FX | H.10 noon rates (ADM-0009, released weekly); the `xyz:EUR` perpetual (50×, liquidity unchecked); Part 43 FX in DTCC's FOREX bucket; a euro short rate from the ECB (not admitted) | Daily; intraday values are `tape` |
| BTC, ETH (including the exchange's dated futures) | Hyperliquid core perpetuals (the deepest open book); on-chain DEX pools as a cross-check | Spot is excellent; the dated-futures basis is only an estimate |

### 3.5 What this means

- **One venue, one deployer.** Every liquid open market for gold,
  silver, crude, gas and the two US indices is on trade.xyz, which had
  more than 95% of HIP-3 volume in mid-2026. Diversity today comes only
  from:
  - Hyperliquid spot and core markets;
  - tokenized gold on Ethereum;
  - small order-book venues such as Lighter and edgeX.
- **The weekend is the prize.** When the reference market is closed,
  trade.xyz's oracle follows its own book, so weekend trades are the
  venue's own price discovery.
  - A third-party study of six weekends (February–March 2026, 37
    symbols) found the last venue price before the open closer to the
    reopening price than Friday's close in about 78% of cases. For gaps
    under 25 bps it did no better than chance.
  - The study comes from a group close to the venue, and the project
    cannot reproduce it with clean data. Treat it as a hint, not as
    evidence.
- **During reference hours, the venue follows an outside price.**
  - trade.xyz relays external prices every few seconds. It does not name
    its sources; one vendor says its index oracle comes from an oracle
    network.
  - Funding pulls the perpetual toward that reference, and arbitrage
    bots hedge against the exchange. One third-party study found the
    index perpetual's mid a few bps from its oracle on average.
  - The trades are still matched on the venue's own book, so they are
    admitted. The anchoring is disclosed with `oracle_anchored_venue`.
- **Public-domain prices for these contracts do not exist.**
  - EIA republishes petroleum and Henry Hub spot prices "with
    permission" from Refinitiv (LSEG), and its futures tables (NYMEX)
    stop at April 2024.
  - US Mint pricing steps with the weekly London gold price.
  - FRED removed its gold series in January 2022, when ICE Benchmark
    Administration data left FRED.
  - IMF commodity prices need permission for commercial reuse.

  None is admissible. The clean anchors that remain are:
  - public rates: Treasury, NY Fed and the Board;
  - Part 43 and SBSR swap prints;
  - EIA's own survey data, which is fundamentals, not prices;
  - USDA transaction reports.

## 4. Method: a critique of synthetic-months@1 and a proposed @2

### 4.1 What v1 gets right

- Its inputs are clean only. Its names are neutral. Every approximation
  is listed in [§ 15 of the spec](../specs/method-synthetic-months-v1.md),
  the arithmetic reproduces bit for bit, and every record carries an
  inputs hash ([ADR 0013](../decisions/0013-synthetic-contract-months.md)).
- The tracker idea, with a `cash` or `front_month` spot node, is the
  right abstraction.
- Swap-print anchors for oil and gas are the only clean forward-curve
  observations that exist, and v1 already uses them.
- Grade inheritance (the second-best source decides) stops one thin
  venue from labelling a value a benchmark.

### 4.2 Gaps that matter for "really close"

| # | Gap in v1 | Effect | Fix in @2 |
|---|---|---|---|
| G1 | A source is either `cash` or `front_month`. trade.xyz moves its oil and gas reference over business days 5–10 in 20% daily steps | A drift of up to the calendar spread during every roll window | `tracks = blend`, with the deployer's published roll schedule recorded as a registry fact (a rule, like a contract calendar, never a price) |
| G2 | The venue observation is whichever is newer: the last trade or the top-of-book mid | A one-lot print or a thin top level moves the value, and the order-level book the node gives is ignored | Impact mid at a reference size from the order-level book, else a 30-second trade VWAP |
| G3 | No venue premium model. Funding fields are excluded (ADM-0001), correctly, because funding is computed against the oracle and would leak the reference price | The perpetuals' small, persistent premium goes straight into the value | Venue bias estimated from open data only (§ 4.3 C) |
| G4 | A fixed 100 bps dispersion threshold | Too loose for gold or an index in quiet hours, too tight in a crash | A robust scale (MAD) with a floor and a cap |
| G5 | No awareness of sessions: the engine does not know when the reference market is closed | Weekend and holiday values look like weekday values | A `reference_closed` mode and flag, from published session rules |
| G6 | No numeric uncertainty | Consumers cannot tell 3 bps from 60 bps | `uncertainty_bps`, with its components |
| G7 | Carry for gold (the lease rate) and for indices (dividends) is a configured constant | The error grows with time to expiry, so back months drift | Open estimators where they exist (§ 4.3 E); a wider published uncertainty where they do not |
| G8 | Treasury months are only specified, not built | No 10-year or bond months | Implement § 13 of the spec, with an intraday shift from Part 43 SOFR swap prints |
| G9 | No validation method | Nothing to show for "how close" | `method-validation@1` (§ 4.5) |

### 4.3 Proposed `synthetic-months@2`

**Owner decision:** this needs an ADR, a new method version and an
observation-v0 amendment for the new fields and flags. v1 stays
documented and reproducible.

**A. Tracker model per source (registry facts only).**

| `tracks` | Meaning | Example |
|---|---|---|
| `cash` | The cash underlying | `xyz:GOLD`, `xyz:SILVER`, `xyz:SP500` (the cash index in the cash session; the reference outside it is to be documented), oracle-free spot gold tokens |
| `front_month` with a roll rule | The front month, switching on one fixed day | Venues that roll in one step |
| `blend` with a schedule | A weighted mix of two months, with weights from the deployer's published roll calendar | `xyz:CL`, `xyz:BRENTOIL`, `xyz:NATGAS` (business days 5–10, 20% a day) |
| `wrapper` with a ratio | A fund or wrapper token: price = ratio × underlying | Spot SPY and QQQ tokens. The ratio is estimated from open venues only, never from a fund administrator's published value |

A `blend` source constrains a weighted pair of months, not one node.
Given the weights `w` and `1-w`, the engine solves for the front month,
taking the calendar spread from the curve (swap anchors where they
exist, carry otherwise). It adds the spread's uncertainty times `1-w` to
the value's uncertainty.

**B. Venue observation.** Use these in order:
- the mid of the volume-weighted bid and ask needed to fill a reference
  size on the order-level book (per exposure, for example ten times the
  median trade size);
- else the 30-second VWAP of book-matched trades;
- else the last trade, flagged `observation_trade`.

This is deterministic, uses admitted fields only, and resists one-lot
prints.

**C. Venue bias, without funding fields.**
- Each source `i` gets a bias `b_i`: an exponentially weighted mean of
  `ln(p_i) - ln(composite without i)`. The half-life is long, for
  example 24 hours of time in which both are fresh.
- The level is pinned by an **anchor class**:
  - for gold, sources with a physical redemption arbitrage
    (oracle-free spot gold tokens, PAXG and XAUT pools), each with its
    own premium term;
  - where no such source exists, the liquidity-weighted mean of all
    sources.
- Without an anchor class, the composite inherits the average venue
  premium, and the record says so (`level_unanchored`). For the indices,
  that premium is today's largest unmeasurable term.
- The perpetual mechanism itself limits the premium, because a lasting
  premium costs the long side funding every hour. @2 states that bound
  from the venue's *published funding formula*, never from funding
  values.

**D. Composite.**
1. Convert every source to the target (the cash equivalent, or a given
   month) using its tracker and the carry.
2. Subtract its bias.
3. Weight each source by a liquidity score, decayed with age (a
   half-life per exposure).
4. Reject sources more than 5 MAD from the weighted median (floor 2 bps,
   cap 200 bps).
5. Cap any one venue at 50% of the weight, unless it is the only fresh
   source (flag `venue_concentrated`; today it will almost always be
   set).
6. Publish the weighted median, with the weighted mean for diagnostics.

**E. Carry.**

| Term | v1 | @2 |
|---|---|---|
| Financing | SOFR OIS swap prints, then bills and par yields | The same, plus the curve's age and basis published as uncertainty. The spread the exchange's financing carries over SOFR is unobservable cleanly and is stated as such |
| Gold and silver lease | A constant | Implied from Part 43 precious-metal forward or swap prints **if** DTCC's COMMODITIES bucket carries them (to verify from captured prints). Otherwise the constant, with a wide published uncertainty. No gold venue tracks a futures month, so the lease cannot come from venues |
| Index dividends | A constant | The constant, with a published uncertainty. Research option for v3: sum declared dividends from SEC EDGAR filings (public domain) for the largest constituents, weighted by open-venue market values |
| Crude and gas | Swap anchors, else financing only | The same, plus the perpetual's blend weights (A) to place the front month. EIA weekly stocks only as a regime flag, never in the formula |

**F. Reference-closed mode.**
- A session calendar per exposure marks when the reference market is
  closed: weekends, holidays, the daily break. Like the contract
  calendars, it is built from published rules.
- Values computed then carry `reference_closed` and a wider
  uncertainty.
- trade.xyz holds its mark within ±1/max-leverage of the last external
  close: ±4% for gold, ±5% for oil. Values near that bound carry
  `venue_bounded`, because the market cannot express a larger move.

These are the mesh's most distinctive values: prices for gold, oil and
US indices at times when the exchange publishes none.

**G. Uncertainty.**
- `uncertainty_bps` is one standard deviation, combining four published
  components in quadrature:
  - **source dispersion:** the robust scale of the converted sources,
    divided by √n;
  - **venue bias:** the anchor class's own uncertainty;
  - **carry:** curve age and estimate width, times time to expiry;
  - **roll:** calendar-spread uncertainty times the blend weight.
- A `confidence` grade (A to D) follows fixed thresholds per family.

### 4.4 Recipes per exposure

The targets in the last column are design goals, not measurements.

| Exposure | Best clean recipe | Main remaining error | Target (front month, reference hours) |
|---|---|---|---|
| Gold | `xyz:GOLD` (cash) + carry. Anchor class: spot gold tokens (after admission) and PAXG/XAUT pools, each with a premium term | Lease estimate; token premiums | About 5–15 bps |
| Silver | `xyz:SILVER` (cash) + carry | One venue; lease estimate | 10–30 bps; `tape` while it is the only venue |
| WTI, Brent | `xyz:CL` and `xyz:BRENTOIL` as `blend` trackers; Part 43 calendar swaps for the spread and the back months | Roll windows; sparse swap prints | 10–30 bps outside rolls; wider within them |
| Natural gas | `xyz:NATGAS` (thin); Part 43 gas swaps (plentiful) | Thin venue | 30–100 bps; the curve is better than the front |
| US large-cap index | `xyz:SP500` (cash) + financing − dividends; spot SPY token as a check once admitted | Dividends and financing (about 5–15 bps per quarter of horizon); level unanchored | About 5–20 bps |
| US tech index | `xyz:XYZ100` (cash) + carry; spot QQQ token. Later, an **open index replica**: single-stock perpetuals weighted by shares outstanding from SEC EDGAR filings (public domain) | Basket tracking error, which cannot be measured cleanly; whole-unit price tick | 5–25 bps; labelled a basket, not the index |
| 10-year note, bond | § 13 of the spec: cheapest-to-deliver from the Treasury curve and auction data; intraday shift from Part 43 SOFR prints | Intraday drift in the swap spread; no delivery options | Daily good; intraday indicative |
| Euro FX | Covered interest parity from H.10, SOFR and an admitted euro rate; `xyz:EUR` as `tape` | H.10 is released only weekly | Daily only |
| SOFR, fed funds months | Method v1 § 12 | Convexity in back months | **Exact** at expiry (§ 4.5) |
| BTC, ETH dated futures | Hyperliquid core perpetuals + a carry estimate | The exchange futures' basis cannot be observed openly | Spot excellent; basis is an estimate |

### 4.5 Measuring accuracy without exchange data

**What can be validated cleanly, and should be published live:**

1. **Exact expiry checks, where settlement is a public formula on
   public data.** One- and three-month SOFR months settle on averaged
   and compounded SOFR. 30-day fed funds months settle on the EFFR
   average. Lean hogs, feeder cattle and dairy months settle on USDA
   indexes. Each expiring synthetic month is compared with that formula,
   computed from the admitted source. No exchange data is touched: the
   rulebook rule is a fact.
2. **Hold-out forward checks for oil and gas.** Predict each new Part 43
   calendar-month print from the curve built without it, and publish the
   residuals by tenor. The residuals include the approximation that a
   swap prices an average of futures over its month (method § 11.2).
3. **Leave-one-venue-out.** Predict each venue from the others, and
   publish the residuals per exposure and session. This measures
   internal consistency, not truth.
4. **Weekend-gap test.** Compare the last `reference_closed` value with
   the composite one hour after the reference market reopens, by which
   time the perpetuals' external references have refreshed. This uses
   only open data, and it measures how informative weekend trading was.
5. **No-arbitrage checks.** Calendar spreads stay within carry bounds
   (gold contango cannot exceed full financing). Discount factors are
   monotone. The implied carry is smooth. Any breach is flagged.
6. **Reproducibility.** Anyone can recompute the published values from
   the archive and get the same bits.

**What cannot be validated cleanly:** the absolute distance between a
composite and the exchange's own price for gold, crude, the indices and
crypto futures.
- No public-domain price for those contracts exists, so only a licensed
  party can measure that distance.
- Nothing derived from such a measurement may enter the project
  ([AGENTS.md](../../AGENTS.md) rule 1).
- The [bring-your-own-feed adapter](../operations/byo-feed.md) already
  lets each licensed user see the distance privately, on their own
  machine.

**Owner decision:** the roadmap's Phase 4 line "basis measurement with a
licensed validation partner (summary statistics)" conflicts with hard
rule 1 as written ("nothing derived from it"). Choose one:
- drop the line; or
- record in an ADR that third parties may publish their own measurements
  on their own sites. The project may link to them, but never ingests,
  archives or quotes their numbers.

## 5. The $200/month server

### 5.1 What a Hyperliquid node buys

- **Live order-level books and every matched fill** for all of these:
  - `xyz:GOLD`, `SILVER`, `CL`, `BRENTOIL`, `NATGAS`, `SP500`, `XYZ100`
    and `EUR`;
  - about 87 single stocks;
  - the core BTC, ETH and PAXG perpetuals;
  - Hyperliquid spot (XAUT0, XAUM, SPY, QQQ and SLV tokens), once spot is
    admitted.

  That is every target exposure with real liquidity, from one process,
  with no API terms on the path.
- **Without a node, there is no admissible live path.**
  - The public API gives 20-level L2 books and trades, rate-limited.
  - The only terms found are the app interface's Terms of Use, which
    cover the website, not the API, and grant no redistribution. That
    does not pass admission rule 2 as written.
  - The buckets are uploaded roughly monthly, with "no guarantee", so
    they give history only.
- **The node needs:** 16 vCPU, 128 GB RAM, at least 500 GB SSD and
  Ubuntu 24.04. It writes about 100 GB of logs a day at default
  settings. The README advises Tokyo for the lowest latency. The mesh's
  target is measured from node write to gateway send, so Europe is
  acceptable (ADR 0010).

### 5.2 Options compared

| Option | What it unlocks for gold, oil and the indices | What it misses | Verdict |
|---|---|---|---|
| **(a) Hyperliquid node + origin** | Live order-level books for every target exposure. Weekend discovery. A signed live stream. Phase 1 exits (all code is done and blocked only on hosting) | Single-venue concentration; no Ethereum or Solana venues | **Best.** It is the only admissible live source for the goal |
| (b) A lighter box running readers for other chains | Ethereum PAXG/XAUT pools; possibly Lighter (blob decoding, unproven) | No live oil or index price at all. The clean liquidity is one to two orders of magnitude smaller. A Solana RPC node (for xStocks) needs hundreds of GB of RAM and is not affordable on any plan | Diversification for later, not the core |
| (c) No rented box: the home server and free relays only | Archive, pollers, backfill, the synthetic engine on history | No live venue data: the home server cannot run the node (old CPUs, shared RAM) | Tier 0 today; the fallback if the box is dropped |

### 5.3 Recommendation

1. **The box runs only the chain-following hot path:**
   - `hl-node` with `--write-fills`, `--write-order-statuses`,
     `--write-raw-book-diffs`, `--batch-by-block` and `--serve-info`;
   - `origin`: reader, book builder, block signer, gateway and live raw
     capture;
   - the synthetic and composite engine, wired in behind its flag
     ([open reference prices](../../.agents/plans/open-reference-prices.md),
     task 2 follow-up), so that composites and synthetic months are
     signed in the same blocks as their inputs.
2. **Hardware:** 16 Zen 4 or Zen 5 cores, 128 GB, 2 × NVMe, from a
   provider whose policy allows nodes.
   - Cherry Servers Ryzen 9 7950X: $232.83 a month, or $197.91 billed
     annually; out of stock everywhere at the time of checking.
   - Contabo's 24-core Genoa (128 GB, 2 × 1 TB SSD): about €199. Out of
     stock; its node policy and disk type are unchecked.
   - Hetzner AX102 (€259 + €129 setup): its policy is hostile to
     blockchain workloads.
   - OVHcloud Advance-4: about €300.
   - Tokyo boxes that meet the spec cost $700 or more.

   **Owner decision:** rent month-to-month for the first month (about
   $233) to confirm the node keeps up on a week's soak. Switch to annual
   billing only then. Update ADR 0010's "about $150" target to the
   actual ceiling.
3. **Everything else stays off the box:**

| Host | Runs | Cost |
|---|---|---|
| Node box | Node, origin, gateway, synthetic and composite engine | ~$200–233 |
| Home-archive server | Archive pipeline (partitions, manifests, anchors), backfill, Part 43, SBSR and reference pollers, web snapshots, IPFS and the seeder, Prometheus and Grafana, the LAN history API (later a read-only tunnel, per D4) | Electricity only |
| 2 free ARM VMs (2 OCPU and 12 GB in total since mid-2026) | Gateway in relay mode (verifies every signature) and Caddy | $0 |
| GitHub Actions (free for public repositories) | A **second capture path** for DTCC slices, which stay listed for only about 24 hours, and for the reference pollers, writing to R2; a nightly `archive verify --url` of the public archive; the site build | $0 |
| Vercel | bleh.xyz, including the "How close" page reading JSON from `data.bleh.xyz` | $0 (Hobby; Cloudflare Pages as the fallback) |
| Cloudflare R2 | Public archive and web snapshots | A few $ a month |

4. **Later, and optional:** add an Ethereum full node on the home-archive
   server for the PAXG and XAUT pools. The server's old cores suit an
   I/O-bound node, but it needs a new SSD (a one-off cost; **owner
   decision**). The alternative is an ADR that admits public chain state
   read from third-party RPC and verified by a light client (**owner
   decision**, amends admission rule 2).

## 6. Status and risk review (2026-11-14)

### 6.1 In flight, blocked or half-done

| Plan, task | Status | What is actually missing |
|---|---|---|
| [History](../../.agents/plans/hyperliquid-history-to-site.md), `finish-trade-copy` | In progress | The 392 GB copy into the private raw tier is running; checkpoint verification follows |
| History, `verify-formats-on-samples` | Ready | One day of each dataset through `process --format jsonl` |
| History, `backfill-registry` (D1) | Shown as blocked on D1 | D1–D4 were approved on 2026-11-14 per the owner. The plan file still shows "Blocked" until its lane updates it |
| History, `ipfs-datastore-off-docker-img` | Ready | Must happen before any backfill day is pinned. The datastore sits on the failing cache NVMe |
| History, `process-from-2025-07-27`, `bars-for-backfill`, `sign-and-publish-backfill`, `site-multi-archive`, `coverage-page` | Ready or blocked behind processing | The path from raw to the site. The first Hyperliquid days on bleh.xyz are the most credible next artefact |
| History, `book-snapshots-next-month` | Blocked (size, credits) | The `market_data` listing; at most one month of credits |
| [Phase 1](../../.agents/plans/phase-1-live-data.md), `hosting-and-node-provisioning`, `node-runbook`, `staging-soak`, `production-launch` | Ready or blocked | All wait on the box. Nothing live exists |
| Phase 1, `signing-keys` | Ready | No block-signing key yet; the manifest key exists only on the home-archive server |
| Phase 1, `client-sdks` | In progress | The Rust client is done. The Python (PyO3) and TypeScript (WASM) bindings are not started; the site shows them as planned |
| Phase 1, `cost-and-retention-model`, `time-sync`, `observability`, `tls-cdn-front`, `node-disk-and-pruning`, `node-upgrade-runbook` | In progress | Each needs the first real week of node operation to finish |
| [Phase 2](../../.agents/plans/phase-2-historical-data.md), `seedbox-and-pinning` | In progress | IPFS pins, but the qBittorrent seeder is not running, so magnets have only the web seed |
| Phase 2, `arweave-and-public-mirrors`, `code-mirrors` | Ready, not started | Hugging Face, Internet Archive, Codeberg, Radicle |
| Phase 2, `coverage-map` | In progress | The `coverage` kind exists; no page shows it |
| Phase 2, `dbn-compat-api`, `client-adapters` | In progress | The live halves wait on the node |
| Phase 2, `second-venue` | Ready, not chosen | The biggest strategic gap for the goal (§ 3.5) |
| Phase 2, `gateway-discovery` | Blocked | Which L2 to use (roadmap open question 4) |
| [Phase 0](../../.agents/plans/phase-0-foundation.md), `branch-protection`, `reporting-address` | Ready | Minutes each; both visible to any reviewer |
| Open reference prices, task 2 follow-up | Not started | Wire `crates/synthetic` into the origin, and replace the synthetic registry entries with real venue markets |
| Records archives (ADR 0017) | Running | `reference-normalized` is public and signed (12 days). `swaps-normalized` is signed locally but not published to R2 |

### 6.2 Stale or inconsistent documents

- [roadmap.md](../roadmap.md) still lists NY Fed, Treasury, USDA and COT
  ingest under Phase 3. They were admitted and have been polled since
  2026-11-12.
- [coverage.md](../coverage.md) rates gold, oil and the indices
  **Strong**, but nothing is live, and every deployer except trade.xyz
  has left. It should say "expected at node launch, one deployer".
- [sources.md](../sources.md) plans an "own RPC node with Geyser" for
  Solana tokenized stocks, which is not a budget option. It also cites
  reported listings from deployers that have since wound down (Kinetiq's
  USBOND).
- [costs.md](../operations/costs.md) targets about $150 a month. The
  owner's ceiling is now one box at about $200, with everything else
  near zero.
- The roadmap's Phase 4 "licensed validation partner" line (§ 4.5).

### 6.3 Risks

| Risk | Likelihood | Impact | Mitigation (zero cost unless noted) |
|---|---|---|---|
| **The home-archive cache NVMe has bad blocks.** It holds `docker.img` (every container) and the IPFS datastore | High | Every home service stops; unpublished records are lost | Move `docker.img`, app data and IPFS to the array now. Replace the NVMe (one-off, about $50–100; **owner decision**). Keep the pollers' record files on the array |
| **One home server runs every poller,** and DTCC lists slices for only about 24 hours | Medium | Permanent gaps in swap history | A second capture path on GitHub Actions writing to R2, deduplicated by the partitioner |
| **DTCC's redistribution terms are unverified.** [swap-repositories § 2.4](../specs/swap-repositories.md) says an owner decision is needed before records are published, yet web snapshots have published recent prints since 2026-11-13. DTCC's site terms say some material "may be subject to separate agreements" | Medium | Credibility; a takedown request | **Owner decision, now:** verify the PPD terms or pause the `swaps/` snapshot files |
| **Commodity swap lineage.** Many commodity swaps float against an exchange settlement; the disseminated fixed price is bilateral but economically tied to it | Low–medium | Critics call the anchors "derived" | **Owner decision:** flag those prints `exchange_referenced`, as ADR 0012 does for reference records |
| **The manifest key exists on one host** (mode 600, on the failing server) | Medium | Losing the key breaks the archive's chain of trust | An encrypted offline backup per [keys.md](../operations/keys.md) (sops/age, two recipients). Publish the signer address in the repository, a DNS TXT record and on the site |
| **Venue concentration:** one deployer lists every liquid target market | High | A deployer halt (which settles at mark) removes almost all discovery for gold, oil and the indices | Admit Hyperliquid spot and other order-book venues (§ 3); publish the concentration on the site |
| **The "derived data" argument** against oracle-anchored venues | Low–medium | Reputational, or a letter | The policy already flags `oracle_anchored_venue` and excludes oracle, mark and funding fields. Publish a plain-language explainer. Weekend values are the venue's own |
| **trade.xyz oracle incidents** (a bad print moved one single-stock mark by about 19% on 2026-07-27) | Medium | Liquidations and fills at distorted prices | Outlier rejection in @2; flag the windows; exclude halt and settlement fills (already done) |
| **Hyperliquid's node requirements grow** beyond a $200 box | Medium | The node stops keeping up | Rent monthly at first; keep the node-behind alarm; Tier 0 remains the fallback |
| **AWS credits** ($100, capped at $50 a month) **end on 2027-05-14** | Certain | No more requester-pays copies | Spend them on the trade history (done) and on `market_data` for the main markets. Apply to AWS Open Data, so that in-region copies cost requests only |
| **R2 growth** beyond the free 10 GB | Certain | A few dollars a month | Within the owner's tolerance. Keep swaps and filtered raw on torrent and IPFS until storage is sponsored |
| **The Vercel Hobby plan** is for non-commercial use | Low | The site has to move | Cloudflare Pages is a free drop-in fallback |

## 7. Ranked zero-cost actions

Ranked by impact on credibility and on the owner's goal, per unit of
effort.

| # | Action | Effort | Plan or task it touches | Decision or admission |
|---|---|---|---|---|
| 1 | **Make the home-archive server safe.** Move `docker.img`, app data and IPFS off the failing NVMe. Back up the manifest key encrypted, in two places. Publish the signer address | 2–4 h | History task 7 `ipfs-datastore-off-docker-img`; Phase 1 `signing-keys` | NVMe replacement is a one-off (owner) |
| 2 | **Settle the DTCC terms.** Verify the PPD terms (or ask DTCC in writing); until then, stop publishing `swaps/` snapshot files | 1 h + decision | Swap pollers runbook; ADM-0002/0004 review trigger | **Owner decision** |
| 3 | **Ship the year of Hyperliquid gold, oil and index tape** (2025-07-27 onward) to the site: registry (D1), process, bars, sign, multi-archive picker, coverage page | 1–2 weeks | History tasks 2–10 | D1–D4 approved |
| 4 | **Admit Hyperliquid spot markets** (spot gold tokens and the fund tokens, all oracle-free), then backfill them from fills that are already copied | Record: 2 h. Code: 1–2 days, in a separate PR | Phase 2 `second-venue`; node-format spot rule | **Admission needed** (new ADM or an ADM-0001 amendment) |
| 5 | **Publish a clean "How close" page, v0, from history:** cross-venue dispersion, weekend-gap tests, exact SOFR and fed funds expiry checks, Part 43 hold-out residuals, liquidity grades and the concentration | 3–5 days | New task in open reference prices; History `coverage-page` | None for the page; § 4.5 metrics need the @2 ADR |
| 6 | **Apply to AWS Open Data Sponsorship and Cloudflare Project Alexandria;** prepare NLnet for its 2027-01-03 deadline | 1 day | costs.md "Free and sponsored" | Owner submits |
| 7 | **Write the ADR and spec for `synthetic-months@2`** (trackers, blend, bias, uncertainty, reference-closed mode) and an exposure-composite method; register the real trade.xyz markets with their published roll rules | 3–5 days | Open reference prices task 2 follow-up; ADR 0013 | **Owner decision** (ADR, observation-v0 amendment) |
| 8 | Second capture path on GitHub Actions for DTCC slices and reference pollers | 1 day | Phase 1 `swap-pollers` follow-up | None |
| 9 | Branch protection, the reporting address, code mirrors | 1 h | Phase 0 tasks 2 and 9; Phase 2 `code-mirrors` | None |
| 10 | Fix the stale documents (§ 6.2) and mark the Phase 4 validation-partner line | 1–2 h | roadmap, coverage, sources, costs | Phase 4 line: **owner decision** |
| 11 | Start the qBittorrent seeder | 1 h | Phase 2 `seedbox-and-pinning` | None |
| 12 | Python history client over the static archive (DuckDB over HTTPS), which makes the site's history sample real before the live SDKs | 2–3 days | Phase 1 `client-sdks` | None |
| 13 | Evaluate Lighter and edgeX data access and terms, and the Ethereum pool reader, for the second venue | 1 day of research | Phase 2 `second-venue` | Admissions; possibly an RPC policy ADR |

## 8. Shining without spending

- **An honest "How close" page.**
  - One card per exposure: the composite, its uncertainty band, flags,
    sources with their weights, liquidity grades and the concentration
    warning.
  - Below the cards, the validation board from § 4.5: SOFR and fed
    funds expiry checks (exact), swap hold-out residuals, leave-one-out
    errors and weekend-gap results.
  - State plainly what cannot be measured and why. That candour is the
    project's argument.
- **The weekend board.** "Gold, oil and US indices, live while the
  exchange is closed": the open price, how far it has moved since the
  reference closed, the venue's bound, and when the exchange reopens. No
  incumbent publishes this, and it is the most shareable thing the
  project can show.
- **A year of open HIP-3 tape**, with verification and SQL in the
  browser, is itself a launch: nobody else offers a signed CC0 archive of
  it.
- **Grants and sponsorships**, in order:
  1. AWS Open Data Sponsorship: rolling, about two weeks to a decision,
     needs an open license. It would host the archive in S3 and make
     in-region copies from Hyperliquid's buckets cost requests only.
     Processing `replica_cmds` would still need compute.
  2. Cloudflare Project Alexandria: a plan upgrade and credits.
  3. NLnet: next deadline 2027-01-03.
  4. Sovereign Tech Fund, once the project counts as infrastructure.
  5. GitHub Secure Open Source Fund: about $10k; the next session is
     unannounced.

  Hyper Foundation has no open general grants; its 2026 programme was
  closed. Oracle's free tier was halved in mid-2026.
- **Community.** Good first issues: the Python and TypeScript bindings,
  the "How close" page, readers for second venues (after their
  admissions), and the registry's roll and session rules. Relays and
  mirrors need no trust, because clients verify signatures, so anyone can
  run one and be listed.
- **Show HN.**
  - The rules: something people can try now, without sign-up, built by
    the poster; no landing pages; stay in the thread.
  - Post once the node is live and at least two weekends of live weekend
    data are on the board (about weeks 9–11).
  - A possible title: "Show HN: Open, signed gold, oil and index prices
    from on-chain order books (CC0)".
  - Lead with the weekend board, the verification in the browser and the
    candid limits. HN publishes no official advice on timing.

## 9. 90-day roadmap

| Weeks | Milestone | Exit check |
|---|---|---|
| 1 | Home server made safe (action 1); DTCC decision (2); trade copy verified; formats checked; spot admission record drafted (4) | NVMe off the critical path; key backed up; copy at 14,460 objects |
| 2–3 | Registry (D1); 2025-07-27 onward processed; bars; first `hyperliquid-backfill` days signed and on the site; grant applications sent (6) | `archive verify --url` passes for the first days; the site lists two archives |
| 3–4 | Rent the box month-to-month; node and origin on staging; relays up; the @2 ADR drafted (7); Actions capture path (8) | Node keeps up for 7 days (`staging-soak`) |
| 5–6 | Production launch on one origin; signed live stream; spot markets ingested live (after admission); "How close" page v0 from history (5) | External probe meets the target; first live weekend archived |
| 7–8 | `synthetic-months@2` and the composite engine live behind the flag for gold, silver, WTI, Brent and both indices; weekend board | Uncertainty bands published; first exact SOFR expiry check published |
| 9–11 | Two or more weekends of live weekend data; Python history client; mirrors and seeder; Show HN | The validation board shows every § 4.5 metric |
| 12–13 | Second-venue decision (Ethereum pools via our own node or verified RPC, or Lighter); `market_data` book snapshots for the main markets within credits | Second venue admitted, or the reason recorded |

## 10. Decisions and admissions needed

1. **DTCC redistribution:** verify the terms, or pause the swap snapshot
   files (§ 6.3).
2. **Admission: Hyperliquid spot markets**, as a new record or an
   amendment to ADM-0001. The admission merges before any code that uses
   it.
3. **ADR: `synthetic-months@2`**, an exposure-composite method and a
   validation method, with an observation-v0 amendment
   (`uncertainty_bps`, `confidence`, and the flags `reference_closed`,
   `venue_bounded`, `venue_concentrated`, `level_unanchored`,
   `observation_trade`, `carry_implied`).
4. **Roadmap Phase 4 "licensed validation partner":** drop it, or record
   in an ADR that it is link-only (§ 4.5).
5. **Hosting:** rent the box (which one, and how it is billed); update
   ADR 0010 and costs.md to the ~$200 ceiling.
6. **Chain state through third-party RPC verified by a light client**
   (amends admission rule 2), **or** a one-off SSD for our own Ethereum
   node on the home-archive server.
7. **`exchange_referenced` on commodity swap prints** that float against
   an exchange settlement.
8. **Admission: EIA Weekly Petroleum Status Report** (EIA's own
   fundamentals; prices stay excluded).
9. **Admission: a euro short rate from the ECB** for euro FX, under the
   ADR 0012 route, after its terms are checked.
10. **One-off replacement of the home-archive NVMe.**

## 11. Sources

All accessed 2026-11-14 unless marked. Secondary and aggregator figures
are approximate and were not reproduced from admitted data.

**Hyperliquid and trade.xyz**
- Node README (requirements, Tokyo advice, flags): https://github.com/hyperliquid-dex/node
- HIP-3 overview: https://hyperliquid.gitbook.io/hyperliquid-docs/hyperliquid-improvement-proposals-hips/hip-3-builder-deployed-perpetuals
- HIP-3 deployer actions: https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/hip-3-deployer-actions
- Funding: https://hyperliquid.gitbook.io/hyperliquid-docs/trading/funding
- Websocket subscriptions: https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions
- Rate limits: https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/rate-limits-and-user-limits
- Historical data: https://hyperliquid.gitbook.io/hyperliquid-docs/historical-data
- trade.xyz specification index: https://docs.trade.xyz/perpetuals/specifications-and-schedules/specification-index.md
- trade.xyz commodities and roll: https://docs.trade.xyz/perpetuals/markets/commodities.md
- trade.xyz US indices: https://docs.trade.xyz/perpetuals/markets/equity-indices/us.md
- trade.xyz oracle price: https://docs.trade.xyz/perpetuals/mechanics/oracle-price.md
- trade.xyz mark price: https://docs.trade.xyz/perpetuals/mechanics/mark-price.md
- trade.xyz discovery bounds: https://docs.trade.xyz/perpetuals/mechanics/discovery-bounds.md
- trade.xyz architecture: https://docs.trade.xyz/architecture.md
- trade.xyz funding change: https://docs.trade.xyz/perpetuals/changelog/funding-rate-formula-updates.md
- trade.xyz index licensing disclaimer: https://docs.trade.xyz/legal-and-disclaimers/s-and-p-500-disclaimers.md
- Hyperliquid app Terms of Use (interface only, last updated 2026-06-15): https://app.hyperliquid.xyz/terms
- Deployer wind-downs:
  - https://panews.io/articles/019ecf58-5c89-7115-b50c-3ba9359c11cd
  - https://cryptobriefing.com/ventuals-shutdown-hip3-markets/
- Deployer concentration: https://0xarchive.io/resources/research/addressing-hyperliquid-s-hip-3-deployer-bottleneck
- Weekend price discovery study: https://0xarchive.io/blog/hyperliquid-weekend-price-discovery-in-24-7-markets
- Index perpetual tracking and participants:
  - https://arrakis.finance/blog/sp-500
  - https://arrakis.finance/blog/who-is-trading-on-hip3
- HIP-3 statistics and oracle incident: https://www.coinperps.com/learn/hyperliquid-hip-3-statistics
- Dwellir node-feed catalogue: https://www.dwellir.com/docs/hyperliquid/data-catalog/node-feeds/fills

**Other venues and tokens**
- Perpetual venue rankings: https://castlelabs.io/research/perp-dex-rankings
- Gold perpetuals across venues: https://loris.tools/markets/perps/gold
- PAXG redemption: https://help.paxos.com/hc/en-us/articles/360041903332
- XAUT: https://rwa.xyz/assets/XAUT
- XAUt0: https://www.theblock.co/post/356727/xaut0-tether-gold-omnichain-ton
- Tokenized gold on Uniswap: https://cryptobriefing.com/uniswap-tokenized-gold-dex-volume/
- Tokenized gold as a price signal: https://www.coingecko.com/learn/tokenized-gold-price-signal
- Solana tokenized-stock volume:
  - https://www.kucoin.com/news/flash/solana-spot-dexs-record-5-8b-tokenized-stock-volume-in-q2-2026
  - https://solanacompass.com/news/raydium-routes-90-of-solanas-tokenized-stock-dex-volume-with-5b-processed-as-september-sets-onchain-ath
- Ondo Global Markets: https://defillama.com/protocol/ondo-global-markets
- Robinhood Chain stock tokens: https://www.kucoin.com/news/flash/robinhood-chain-launches-12m-in-tokenized-stocks-deposited-into-defi
- Lighter equity hours: https://www.theblock.co/post/384493/dominant-onchain-perps-exchange-lighter-rolls-out-24-5-equity-perps-trading
- Ostium order types (oracle-priced): https://ostium-labs.gitbook.io/ostium-docs/ostium-trading-engine/order-types
- RWA perpetuals market overview: https://coinmarketcap.com/events/rwa-perpetuals-state-of-the-market-august-2026/

**Public anchors and their exclusions**
- EIA price sources ("republished by EIA with permission"): https://www.eia.gov/todayinenergy/prices.php
- EIA spot table definitions: https://www.eia.gov/dnav/pet/TblDefs/pet_pri_spt_tbldef2.asp
- EIA natural gas price sources: https://www.eia.gov/dnav/ng/TblDefs/ng_pri_fut_tbldef2.asp
- EIA futures table (no data after 2024-04-05): https://www.eia.gov/dnav/pet/pet_pri_fut_s1_d.htm
- EIA copyright and reuse: https://www.eia.gov/about/copyrights_reuse.php
- EIA Weekly Petroleum Status Report: https://www.eia.gov/petroleum/supply/weekly/
- FRED removal of IBA data: https://news.research.stlouisfed.org/2022/01/ice-benchmark-administration-ltd-iba-data-to-be-removed-from-fred/
- US Mint product pricing: https://www.usmint.gov/resources/product-pricing
- IMF terms: https://www.imf.org/en/about/copyright-and-terms
- 17 CFR 43.3: https://www.law.cornell.edu/cfr/text/17/43.3
- DTCC PPD dashboard: https://pddata.dtcc.com/ppd/cftcdashboard
- DTCC site terms: https://www.dtcc.com/terms

**Hosting and programmes**
- Cherry Servers Ryzen 7950X: https://www.cherryservers.com/pricing/dedicated-servers/amd-ryzen-7950x
- Cherry Servers Tokyo: https://www.cherryservers.com/bare-metal-dedicated-servers/tokyo
- Hetzner AX102: https://www.hetzner.com/dedicated-rootserver/ax102/
- OVHcloud catalogue: https://eu.api.ovh.com/1.0/order/catalog/public/baremetalServers?ovhSubsidiary=IE
- Latitude.sh: https://www.latitude.sh/pricing
- Vultr bare metal: https://api.vultr.com/v2/plans-metal
- Contabo: https://contabo.com/en/dedicated-servers/
- Oracle Always Free: https://docs.oracle.com/en-us/iaas/Content/FreeTier/freetier_topic-Always_Free_Resources.htm
- AWS Open Data Sponsorship: https://aws.amazon.com/opendata/open-data-sponsorship-program/
- Cloudflare Project Alexandria: https://www.cloudflare.com/lp/project-alexandria
- NLnet calls: https://nlnet.nl/news/2026/20260803-phaseshift.html
- Sovereign Tech Fund: https://sovereign.tech/programs/fund
- GitHub Secure Open Source Fund: https://github.com/open-source/github-secure-open-source-fund
- Hyper Foundation grants (closed): https://cryptobriefing.com/hyper-foundation-10m-grants-usdh-migration/
- Show HN rules: https://news.ycombinator.com/showhn.html
