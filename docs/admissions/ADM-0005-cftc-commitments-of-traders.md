# ADM-0005: CFTC Commitments of Traders — every report type, full history

- Status: Admitted
- Requested: 2026-11-12 by the admissions lane (open reference prices plan, task 1)
- Reviewers: Repository owner (engineering review of field lineage)
- Decided: 2026-11-12 by the repository owner (owner decision, 2026-11-12)

## Source

The Commodity Futures Trading Commission (CFTC), a federal agency, publishes
the weekly Commitments of Traders (COT) reports. They break down each
Tuesday's open interest in US futures and options markets by trader
category. There are four report families:

| Report | Breakdown | History on cftc.gov |
|---|---|---|
| Legacy | Commercial, non-commercial, nonreportable; futures-only and futures-and-options combined; short and long formats | From 1986-01-15 (futures-only); combined from March 1995 |
| Disaggregated | Producer/merchant/processor/user, swap dealers, managed money, other reportables | From 2006-06-13 |
| Traders in Financial Futures (TFF) | Dealer/intermediary, asset manager/institutional, leveraged funds, other reportables | From 2006-06-13 |
| Supplemental (Commodity Index Traders) | Adds index traders, 13 agricultural markets, combined only | From 2006-01-03 |

We read the static files on cftc.gov: the weekly text files and the annual
historical compressed files. We also read the COT Public Reporting
Environment (publicreporting.cftc.gov), unauthenticated. Endpoints are in
[reference-sources.md](../specs/reference-sources.md#cftc-commitments-of-traders).

### How the CFTC compiles it

This summary follows the CFTC's
[About the COT Reports](https://www.cftc.gov/MarketReports/CommitmentsofTraders/index.htm)
and
[Explanatory Notes](https://www.cftc.gov/MarketReports/CommitmentsofTraders/ExplanatoryNotes/index.htm),
both read 2026-11-12:

- **Who reports.** "Clearing members, futures commission merchants, and
  foreign brokers (collectively called reporting firms) file daily reports
  with the Commission". The reports cover traders at or above the CFTC's
  reporting levels; this is the large-trader reporting programme,
  17 CFR Part 17. The CFTC's own summary says the reports are "based on
  position data supplied by reporting firms (FCMs, clearing members,
  foreign brokers and exchanges)".
- **Open interest.** Total open interest is the figure as reported to the
  Commission. Nonreportable positions are derived by subtracting reportable
  positions from total open interest.
- **Trader categories.** These come from traders' self-reported business
  purpose on CFTC Form 40, reviewed by CFTC staff.
- **Combined reports.** Futures-and-options-combined reports convert option
  positions to futures equivalents "using delta factors supplied by the
  exchanges".
- **Timing.** Data are as of Tuesday's close and released Friday at about
  3:30 p.m. ET.

## Fields

| Field | Meaning | Admit? |
|---|---|---|
| Report date, report week, market and exchange name, contract market name, CFTC contract market, market, region and commodity codes, commodity name, contract units | Identity and timing | **Admitted** |
| Open interest (all, old, other) | Total open contracts as reported to the CFTC | **Admitted** |
| Long, short and spreading positions by trader category (each report family), reportable and nonreportable totals | Positions | **Admitted** |
| Week-on-week changes, percent of open interest, number of traders per category | Derived by the CFTC | **Admitted** |
| Concentration ratios (largest 4 and 8 traders, gross and net) | Derived by the CFTC | **Admitted** |
| Futures-and-options-combined figures (all of the above for combined reports) | Option positions converted with exchange-supplied deltas | **Admitted with `exchange_referenced`** |
| Any price | Not published | Not applicable |

## Policy checks

Under [admission-policy.md](../admission-policy.md):

1. **Original price formation.** The reports contain no prices. They are
   regulatory statistics compiled by the CFTC from mandatory position
   reports. No field re-publishes a venue's prices.
2. **No contract on the path.** The data are public-domain government data.
   The CFTC [Web Policy](https://www.cftc.gov/WebPolicy/index.htm) (read
   2026-11-12) says: "Government information at the CFTC website is in the
   public domain. Public domain information may be freely distributed and
   copied, but it is requested that in any subsequent use the CFTC be given
   appropriate acknowledgement."
   - The Public Reporting Environment links to the same web policy and
     needs no account.
   - Application tokens for its Socrata API need a Socrata profile, whose
     terms were not reviewed, so pollers use no token. Unauthenticated
     requests are throttled by IP
     ([Socrata docs](https://dev.socrata.com/docs/app-tokens.html), read
     2026-11-12).
3. **Venue-matched fields only.** No price or trade fields are published.
   The size-like fields (open interest, positions) are the CFTC's own
   aggregates of reports filed under federal law. They are not an exchange
   feed or a vendor product.
   - The exchanges' role is limited to two things: reporting open interest
     to the regulator, and supplying the option deltas used in combined
     reports.
   - Combined-report records carry `exchange_referenced`
     ([ADR 0012](../decisions/0012-open-terms-route-for-public-bodies-and-market-operators.md)).
     Futures-only reports carry no flag.
4. **Field-level exclusions.** None. Outputs name markets descriptively. The
   CFTC's market and exchange names are kept as source metadata, as the
   naming rules allow for economic references.

## Source basis

Public-domain publication by a federal agency, read from cftc.gov and
publicreporting.cftc.gov on 2026-11-12:

- <https://www.cftc.gov/MarketReports/CommitmentsofTraders/index.htm>
- <https://www.cftc.gov/MarketReports/CommitmentsofTraders/HistoricalCompressed/index.htm>
- <https://publicreporting.cftc.gov/>

## Risks and review triggers

- The CFTC changes its web policy or adds conditions to the Public
  Reporting Environment.
- The CFTC changes the report methodology, categories or delta treatment.
- A report family starts publishing price fields. They would need their own
  review.

## Decision

Admitted on 2026-11-12 by the repository owner for every field above, for
all four report families, in futures-only and combined forms, for their
full history. Combined-report records carry `exchange_referenced`.
