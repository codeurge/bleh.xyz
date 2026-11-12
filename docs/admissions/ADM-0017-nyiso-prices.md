# ADM-0017: NYISO — public LBMP and ancillary service price files

- Status: Under review
- Requested: 2026-11-12 by the admissions lane (open reference prices plan, task 1)
- Reviewers: Repository owner (engineering review of field lineage)
- Decided:

## Source

The New York Independent System Operator (NYISO) posts public CSV files on
mis.nyiso.com/public/. They need no login, registration or click-through.
The files are:

- day-ahead and real-time LBMPs by zone and generator (with marginal loss
  and congestion components);
- day-ahead and real-time ancillary service prices.

Monthly archives go back to November 1999.

A 2019 stakeholder draft of Market Services Tariff §§ 4.1–4.4 describes the
NYISO posting LBMPs on its OASIS. The effective tariff text was not
retrieved.

## Fields

| Field | Admit? |
|---|---|
| DA and RT zonal and generator LBMPs and components | **Under review** |
| DA and RT ancillary service prices | **Under review** |

## Policy checks

Under [admission-policy.md](../admission-policy.md):

1. **Original price formation.** Passes. The prices are NYISO's market
   clearing.
2. **No contract on the path.** Not yet met.
   - **What passes:** nothing must be accepted to obtain the files, and no
     clause restricts redistribution of data.
   - **What fails:** NYISO's only terms page is the
     [Legal Notice](https://www.nyiso.com/legal-notice) (read 2026-11-12),
     and it grants no permission. It says:
     - "The NYISO maintains this Web site for the benefit of its Market
       Participants and other authorized users."
     - "Access to this Web site does not confer any license or ownership
       interest in either the form or content of the Web site, including
       any confidential or proprietary information or intellectual property
       of any kind or nature, and the NYISO hereby expressly reserves such
       rights and property in its entirety."
   - **Images only:** its republication ban ("Downloading, republishing,
     retransmitting, reproducing, or other use of any image or video on
     this website as a stand-alone file is strictly prohibited") covers
     images and video, not data.
   - **Why it is not met:** NYISO is not a public body, so the public-domain
     route does not apply. The open-terms route requires terms that
     expressly allow redistribution
     ([ADR 0012](../decisions/0012-open-terms-route-for-public-bodies-and-market-operators.md)).
     A reservation of rights with no grant does not meet that test.
3. **Venue-matched fields only.** Would pass.
4. **Exclusions.** Not applicable while under review.

The deciding clause is the reservation sentence above ("...the NYISO hereby
expressly reserves such rights and property in its entirety"), together with
the absence of any redistribution grant.

## Source basis

- <https://www.nyiso.com/legal-notice> (read 2026-11-12)
- <https://mis.nyiso.com/public/> (index pages, read 2026-11-12; no terms
  shown)

## What would decide it

Either of these would let the record be admitted under ADR 0012:

- NYISO publishes, or confirms in writing, a permission to redistribute its
  public market data;
- NYISO revises its legal notice to grant redistribution.

If NYISO declines, the record becomes Rejected. No ingest code is written
while the record is under review.

## Risks and review triggers

- Any change to the NYISO Legal Notice.

## Decision

Under review as of 2026-11-12. NYISO's terms reserve all rights in site
content and grant no redistribution permission.
