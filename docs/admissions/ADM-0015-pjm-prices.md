# ADM-0015: PJM — Data Miner 2 LMPs and ancillary service prices

- Status: Rejected
- Requested: 2026-11-12 by the admissions lane (open reference prices plan, task 1)
- Reviewers: Repository owner (engineering review of field lineage)
- Decided: 2026-11-12 by the repository owner (owner decision, 2026-11-12)

## Source

PJM Interconnection publishes day-ahead and real-time LMPs, hub prices
(for example Western Hub) and ancillary service prices through Data Miner 2.
Operating Agreement Schedule 1, § 1.10.8(b) requires the Office of the
Interconnection to "post the Day-ahead Prices".

## Fields

| Field | Admit? |
|---|---|
| DA and RT LMPs, hub and zone prices, ancillary service clearing prices | **Rejected** |

## Policy checks

Under [admission-policy.md](../admission-policy.md):

1. **Original price formation.** Passes. The prices are PJM's market
   clearing.
2. **No contract on the path.** Fails, including under the open-terms route
   ([ADR 0012](../decisions/0012-open-terms-route-for-public-bodies-and-market-operators.md)).
   All pages below were read 2026-11-12.
   - The [Data Miner 2 page](https://www.pjm.com/markets-and-operations/etools/data-miner-2)
     says: "Information and data contained in Data Miner is for internal use
     only and redistribution of information and or data contained in or
     derived from Data Miner is strictly prohibited without an active PJM
     Membership. A minimum level of Associate Membership is required."
   - The
     [Data Miner terms](https://www.pjm.com/-/media/DotCom/etools/edatafeed/data-license-agreement-edata-feed-data-miner-2.pdf),
     § 1, say: "Members may republish data, however non-members are
     specifically prohibited from republishing data."
   - The [pjm.com legal page](https://www.pjm.com/about-pjm/legal.aspx)
     says: "Access to this website does not confer any license or ownership
     interest in either the form or content of the website".
   - Paid membership as a condition of redistribution is a disqualifying
     condition.
3. **Venue-matched fields only.** Would pass.
4. **Exclusions.** All requested fields.

The deciding clause is "non-members are specifically prohibited from
republishing data".

## Source basis

The PJM pages above, read 2026-11-12. The Associate Membership fee was not
checked, because any membership requirement disqualifies the source.

## Risks and review triggers

- PJM removes the non-member republication ban.

## Decision

Rejected on 2026-11-12 by the repository owner. Republication is reserved
to PJM members.
