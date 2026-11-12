# ADM-0012: USDA AMS Market News — feeder cattle auctions, direct and video sales; grain reports

- Status: Admitted (feeder cattle fields only; grain price fields rejected,
  see Decision)
- Requested: 2026-11-12 by the admissions lane (open reference prices plan, task 1)
- Reviewers: Repository owner (engineering review of field lineage)
- Decided: 2026-11-12 by the repository owner (owner decision, 2026-11-12)

## Source

USDA AMS Livestock, Poultry and Grain Market News publishes voluntary
market reports. Market News reporters write them, some jointly with state
departments of agriculture as Federal-State Market News. Two families are
in scope here.

- **Feeder and stocker cattle.** These are auction reports, direct-trade
  reports, and internet and video auction reports. They record sales by
  class, frame, muscle grade and weight, with head counts, weight ranges
  and price ranges. They include the national summaries:
  - SJ_LS850, National Feeder and Stocker Cattle Summary, weekly;
  - LSDDFSS and LSPDFSS, the daily summaries.
- **Grain.** These are national and state grain reports: elevator,
  processor and export bids, terminal markets.

### Channels

- **MyMarketNews API** (marsapi.ams.usda.gov). It needs a personal API key
  issued after registration. mymarketnews.ams.usda.gov could not be reached
  from the review environment on 2026-11-12: connections were reset. Its
  registration terms were therefore not read, and this record does not
  cover that channel.
- **AMS report files** under <https://www.ams.usda.gov/mnreports/>. These
  are linked from the AMS Market News pages and need no account. This is
  the channel this record admits. Endpoints are in
  [reference-sources.md](../specs/reference-sources.md#usda-ams-market-news-feeder-cattle).

## Fields

| Field | Meaning | Admit? |
|---|---|---|
| Report identity and dates; auction or market name and location; receipts; head counts | Metadata and volumes | **Admitted** |
| Feeder and stocker cattle auction sales: class (steers, heifers, bulls), frame and muscle grade, weight range and average weight, price range and weighted average price | Prices formed at public auction | **Admitted** |
| Internet and video auction sales, including lots for later delivery, with the auction's delivery and slide terms | Prices formed at auction | **Admitted** |
| Direct-trade feeder sales quoted FOB, current or forward delivery | Negotiated cash trades | **Admitted**, except rows the report describes as basis, futures-priced or formula-priced |
| Any feeder row described as priced from futures, at a basis, or by formula | Futures-referenced | **Excluded** |
| Report narrative quoting futures moves | Exchange prices | **Excluded** |
| Grain cash bids, basis levels, and any price expressed as or derived from a futures price plus or minus basis | Priced off exchange futures | **Rejected** |
| Grain futures settlements quoted in reports | Exchange prices | **Rejected** |
| Grain volumes (for example export inspections and receipts) | Not prices | Not requested; would need their own review |

## Policy checks

Under [admission-policy.md](../admission-policy.md):

1. **Original price formation.**
   - **Feeder cattle:** auction and video prices are formed by open outcry
     or online bidding at the market. Direct trades are negotiated between
     buyer and seller.
   - **Grain:** Market News quotes country, processor and export bids as a
     basis to the nearby exchange futures contract, with the flat price
     equal to futures plus basis. These prices are therefore derived from
     exchange prices, and they fail rule 1 and rule 3.
2. **No contract on the path.** AMS reports are works of the US Government,
   so they are public domain under 17 U.S.C. § 105. The report files on
   ams.usda.gov need no account. AMS states for its mandatory data that
   "There are no restrictions on who may obtain this data or how it may be
   used" (<https://mpr.datamart.ams.usda.gov/>, read 2026-11-12). It
   directs users to MyMarketNews for the voluntary reports, which carry no
   contrary notice on the AMS pages read.
   - Federal-State reports are published by USDA AMS as Market News
     reports.
   - The MyMarketNews API is not used until its registration terms are
     read.
3. **Venue-matched fields only.** The admitted feeder prices are auction
   and cash-trade prices. Every price derived from futures is excluded.
4. **Field-level exclusions.** As listed above.

## Source basis

Public-domain publication by USDA AMS, read 2026-11-12:

- <https://www.ams.usda.gov/market-news/feeder-and-replacement-cattle-auctions>
- <https://www.ams.usda.gov/market-news/feeder-and-replacement-cattle-summary>
- <https://www.ams.usda.gov/market-news/direct-feeder-cattle-reports>
- <https://www.ams.usda.gov/market-news/feeder-cattle-internet-video-reports>
- <https://www.ams.usda.gov/market-news/national-grain-reports>

## Risks and review triggers

- The MyMarketNews registration terms are read. If they place no limit on
  redistribution, the API becomes an admitted channel through an update to
  this record.
- A state co-publisher claims rights in a Federal-State report.
- Direct-trade reports start pricing lots by futures-based formulas without
  marking them.

## Decision

- **Admitted** on 2026-11-12 by the repository owner: feeder and stocker
  cattle auction, video and direct-trade fields as listed, read from AMS
  report files on ams.usda.gov.
- **Rejected:** grain price fields, because they are futures plus basis.
- **Not covered:** the MyMarketNews API channel, whose terms could not be
  reached.
