# ADM-0007: Federal Reserve Bank of New York — reference rates

- Status: Admitted
- Requested: 2026-11-12 by the admissions lane (open reference prices plan, task 1)
- Reviewers: Repository owner (engineering review of field lineage)
- Decided: 2026-11-12 by the repository owner (owner decision, 2026-11-12)

## Source

The Federal Reserve Bank of New York (NY Fed) administers and publishes six
reference rates each business day:

| Rate | Short name | Published | API history from |
|---|---|---|---|
| Secured Overnight Financing Rate | SOFR | About 8:00 a.m. ET | 2018-04-02 |
| Broad General Collateral Rate | BGCR | About 8:00 a.m. ET | 2018-04-02 |
| Tri-Party General Collateral Rate | TGCR | About 8:00 a.m. ET | 2018-04-02 |
| SOFR Averages (30, 90 and 180 days) and SOFR Index | — | Shortly after SOFR | 2020-03-02 |
| Effective Federal Funds Rate | EFFR | About 9:00 a.m. ET | 2000-07-03 |
| Overnight Bank Funding Rate | OBFR | About 9:00 a.m. ET | 2016-03-01 |

The NY Fed is one of the twelve Reserve Banks. It is not a federal agency,
so its content is not public-domain government data. It is published under
the NY Fed's Terms of Use, so this admission uses the open-terms route of
rule 2
([ADR 0012](../decisions/0012-open-terms-route-for-public-bodies-and-market-operators.md)).

We read the public Markets Data API (markets.newyorkfed.org), which needs
no account. Endpoints are in
[reference-sources.md](../specs/reference-sources.md#federal-reserve-bank-of-new-york-reference-rates).

### How the rates are formed

Sources: the NY Fed's
[reference rates pages](https://www.newyorkfed.org/markets/reference-rates)
and
[Additional Information about Reference Rates](https://www.newyorkfed.org/markets/reference-rates/additional-information-about-reference-rates),
read 2026-11-12. Every rate is a volume-weighted median of overnight
transactions.

- **TGCR.** Tri-party general collateral repo transactions. The data are
  collected from BNY and from the Treasury's Office of Financial Research
  (OFR).
- **BGCR.** The TGCR data plus GCF Repo transactions, obtained from the
  OFR.
- **SOFR.** The BGCR data plus bilateral Treasury repo cleared through
  FICC's DVP service, obtained from the OFR.
- **Data collection before 2022-01-24.** Before that date, transaction data
  were collected under Board authority and "from DTCC Solutions LLC ...
  under a commercial agreement".
- **EFFR.** Overnight federal funds transactions reported on the FR 2420
  Report of Selected Money Market Rates. Before 2016-03-01 it was a
  volume-weighted mean of brokered trades.
- **OBFR.** The EFFR transactions plus certain Eurodollar and selected
  deposit transactions from the FR 2420.
- **SOFR Averages and Index.** Compounded from published SOFR values.
- **Revisions.** A same-day revision may occur at about 2:30 p.m. ET if the
  change exceeds one basis point.

## Fields

| Field | Meaning | Admit? |
|---|---|---|
| `effectiveDate`, `type` | Value date and rate | **Admitted** |
| `percentRate` | The reference rate | **Admitted** |
| `percentPercentile1`, `percentPercentile25`, `percentPercentile75`, `percentPercentile99` | Volume-weighted percentiles | **Admitted** |
| `volumeInBillions` | Underlying volume | **Admitted** |
| `revisionIndicator` | Revision marker | **Admitted** |
| `targetRateFrom`, `targetRateTo` (EFFR) | FOMC target range | **Admitted** |
| `average30day`, `average90day`, `average180day`, `index` (SOFRAI) | SOFR Averages and Index | **Admitted** |
| Other Markets Data API datasets (operations, primary dealer statistics, SOMA holdings and others) | Not requested | Not covered by this record |

## Policy checks

Under [admission-policy.md](../admission-policy.md):

1. **Original price formation.** Each rate is computed by the administrator
   from transactions reported to it, or collected for it, under Federal
   Reserve and OFR authority. The inputs are cash repo and unsecured
   funding trades. No exchange price is an input.
2. **No contract on the path.** The open-terms route applies
   ([ADR 0012](../decisions/0012-open-terms-route-for-public-bodies-and-market-operators.md)).
   The NY Fed
   [Terms of Use](https://www.newyorkfed.org/privacy/termsofuse) were read
   2026-11-12, and the Markets Data API documentation points to them.
   - **Licence grant:** "The New York Fed grants you a non-exclusive
     license, subject to the Terms, to use, copy, and distribute Content for
     your personal or business purposes." Access may be "manually or through
     an automated process or device".
   - **Conditions:**
     - keep source identifiers;
     - label modifications;
     - do not imply endorsement;
     - "If you distribute the Content, you must make the Content available
       with the same permissions, conditions, and restrictions set forth in
       these Terms."
   - **Reference rates notice.** Use or distribution of reference rate data
     must include this notice: "The [NAME OF DATA or CONTENT] is subject to
     the Terms of Use posted at newyorkfed.org. The New York Fed is not
     responsible for publication of the [DATA NAME] by [NAME OF PUBLISHER],
     does not [sanction] or [endorse] any particular republication, and has
     no liability for your use."
   - **SOFR and BGCR disclaimer.** The terms add a disclaimer for SOFR and
     BGCR data about the DTCC Solutions LLC data licence.
   - These are attribution and notice conditions on a grant that allows
     business use. None limits recipients, forbids commercial use or
     charges a fee.
3. **Venue-matched fields only.** The fields are the administrator's
   published statistics. No exchange feed, oracle or vendor price is used.
4. **Field-level exclusions.** None among the requested fields.

## Obligations carried with the data

- Every record carries the reference-rates notice above, completed with the
  rate name and "bleh.xyz" as publisher, in the registry entry and the
  archive manifest. SOFR and BGCR records also carry the DTCC Solutions
  disclaimer from the terms.
- Downstream users receive these records under the NY Fed's terms, which
  pass through. CC0 covers only the project's own contribution
  ([ADR 0012](../decisions/0012-open-terms-route-for-public-bodies-and-market-operators.md),
  decision 5).
- Outputs keep neutral names. Where a rate's name or acronym appears in a
  product description, the non-affiliation disclaimer in the terms goes with
  it: "[User] is not affiliated with the New York Fed. The New York Fed
  does not sanction, endorse, or recommend any products or services offered
  by [user]."
- No use of New York Fed marks beyond identifying the source.

## Source basis

Publication by the administrator under its Terms of Use, read 2026-11-12:

- <https://www.newyorkfed.org/privacy/termsofuse>
- <https://markets.newyorkfed.org/static/docs/markets-api.html>
- <https://www.newyorkfed.org/markets/reference-rates/additional-information-about-reference-rates>

## Risks and review triggers

- The NY Fed changes its Terms of Use, especially the reference-rates
  section or the licence grant.
- The API becomes a permissioned-access website. The terms say restrictions
  on permissioned sites supersede the licence.
- Methodology or input changes, which the NY Fed may make without notice.

## Decision

Admitted on 2026-11-12 by the repository owner for the fields above, read
from the public Markets Data API, with the required notice carried on every
record.
