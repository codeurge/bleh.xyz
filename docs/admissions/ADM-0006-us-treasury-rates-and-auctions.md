# ADM-0006: US Treasury — daily interest rate statistics and auction results

- Status: Admitted
- Requested: 2026-11-12 by the admissions lane (open reference prices plan, task 1)
- Reviewers: Repository owner (engineering review of field lineage)
- Decided: 2026-11-12 by the repository owner (owner decision, 2026-11-12)

## Source

The US Department of the Treasury publishes five daily interest rate series
on home.treasury.gov:

| Series | Available from |
|---|---|
| Daily Treasury Par Yield Curve Rates | 1990 |
| Daily Treasury Par Real Yield Curve Rates | 2003 |
| Daily Treasury Bill Rates | 2002 |
| Daily Treasury Long-Term Rates and Extrapolation Factors | 2000 |
| Daily Treasury Real Long-Term Rate Averages | 2000 |

The availability dates come from Treasury's
[Interest Rate XML Files](https://home.treasury.gov/developer-notice-xml-changes)
data availability table.

The Bureau of the Fiscal Service publishes Treasury auction results on
Fiscal Data:

- Treasury Securities Auctions Data, from 1979-11-15;
- Treasury Securities Upcoming Auctions.

We read the interest rate XML feeds and CSV downloads, and the Fiscal Data
API. Endpoints are in
[reference-sources.md](../specs/reference-sources.md#us-treasury).

### How the rates are formed

- **Par yield curve.** Treasury's
  [Yield Curve Methodology](https://home.treasury.gov/policy-issues/financing-the-government/interest-rate-statistics/treasury-yield-curve-methodology)
  (read 2026-11-12) gives these inputs: "indicative, bid-side market price
  quotations (not actual transactions) for the most recently auctioned
  securities obtained by the Federal Reserve Bank of New York at or near
  3:30 PM each trading day". The curve is derived with a monotone convex
  method, which replaced a quasi-cubic Hermite spline on 2021-12-06.
- **Bill rates.** These are "indicative closing market bid quotations on the
  most recently auctioned Treasury Bills in the over-the-counter market as
  obtained by the Federal Reserve Bank of New York".
- **Long-term rates.** These are averages of closing bid yields on long
  bonds.
- **Auction results.** These are the outcomes of Treasury's own auctions:
  rates and prices set by competitive bidding on Treasury's auction system.

## Fields

| Field | Meaning | Admit? |
|---|---|---|
| Par yield curve rates at every published tenor (1 Mo to 30 Yr, including 1.5 Mo, 4 Mo) | Treasury's official nominal curve | **Admitted** |
| Par real yield curve rates (5 to 30 Yr) | Treasury's official TIPS curve | **Admitted** |
| Bill rates: bank discount and coupon equivalent, by tenor; CUSIP and maturity of each input bill | Closing indicative bill quotes | **Admitted** |
| Long-term composite rate, 20-year CMT and extrapolation factor; real long-term average | Treasury averages | **Admitted** |
| Auction results: every field in Fiscal Data `auctions_query` (CUSIP, security type and term, dates, high, low and median rates, yields and prices, amounts tendered and accepted by bidder class, bid-to-cover, SOMA, FIMA, FRN index rate, TIPS reference CPI and index ratio, and the rest) | Treasury auction outcomes | **Admitted** |
| Upcoming auctions: every field in Fiscal Data `upcoming_auctions` | Auction calendar | **Admitted** |
| Auction announcement and result files on treasurydirect.gov (the `pdf_filenm_*` and `xml_filenm_*` fields name them) | Files on TreasuryDirect | **Not read.** The file-name fields are kept as metadata only (see check 2) |

## Policy checks

Under [admission-policy.md](../admission-policy.md):

1. **Original price formation.**
   - **Auction results** are prices formed in Treasury's own auctions.
   - **Rate series** are Treasury's statistics, computed from quotations
     reported to a public body (the New York Fed) for securities in the
     over-the-counter cash market. They are not re-published exchange
     prices. Treasury does not name the firms that supply the quotations.
2. **No contract on the path.**
   - **Rate series:** works of the US Government are not subject to
     copyright (17 U.S.C. § 105). home.treasury.gov carries no copyright
     claim or terms of use for these files (site policies read 2026-11-12).
   - **Fiscal Data:** the
     [API documentation](https://fiscaldata.treasury.gov/api-documentation/)
     (read 2026-11-12) says: "The data is offered free, without restriction,
     and available to copy, adapt, redistribute, or otherwise use for
     non-commercial or commercial purposes." It also says the API "does not
     require a user account or registration for a token".
   - **TreasuryDirect (not used):** the
     [TreasuryDirect website terms](https://www.treasurydirect.gov/legal-information/terms/)
     (§ 2) say: "You may view, download and print material on this site for
     non-commercial personal use only." That clause rules out
     treasurydirect.gov downloads as a channel. TreasuryDirect's
     [Web API terms](https://www.treasurydirect.gov/legal-information/developers/web-api-terms/)
     contain no redistribution limit, but Fiscal Data carries the same
     auction data under an express open licence, so it is the only auction
     channel.
3. **Venue-matched fields only.**
   - **Auction fields:** these are Treasury's own outcomes.
   - **Rate fields:** these are Treasury's published statistics. No
     exchange feed or commercial vendor is named as an input.
4. **Field-level exclusions.** TreasuryDirect-hosted files, as above.

## Source basis

Public-domain US Government data, and Fiscal Data's open licence, read on
2026-11-12:

- <https://home.treasury.gov/policy-issues/financing-the-government/interest-rate-statistics>
- <https://home.treasury.gov/developer-notice-xml-changes>
- <https://fiscaldata.treasury.gov/api-documentation/>
- <https://fiscaldata.treasury.gov/datasets/treasury-securities-auctions-data/>

## Risks and review triggers

- Treasury or the New York Fed discloses that the curve inputs come from an
  exchange feed or a commercial vendor's screen.
- Treasury changes the curve method or the inputs (it reserves the right to
  do so). The method version is recorded with each record.
- Fiscal Data changes its licence statement.

## Decision

Admitted on 2026-11-12 by the repository owner for the fields above:

- the five rate series, read from home.treasury.gov;
- auction results and upcoming auctions, read from the Fiscal Data API.

treasurydirect.gov is not used.
