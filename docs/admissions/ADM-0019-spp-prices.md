# ADM-0019: Southwest Power Pool — Marketplace LMPs and market clearing prices

- Status: Rejected
- Requested: 2026-11-12 by the admissions lane (open reference prices plan, task 1)
- Reviewers: Repository owner (engineering review of field lineage)
- Decided: 2026-11-12 by the repository owner (owner decision, 2026-11-12)

## Source

Southwest Power Pool (SPP) publishes the following through the file browser
on its portal (portal.spp.org):

- day-ahead and real-time balancing market LMPs by settlement location;
- day-ahead and real-time market clearing prices for regulation, spinning,
  supplemental and ramp products.

Anonymous downloads work. History goes back to 2013.

## Fields

| Field | Admit? |
|---|---|
| DA and RTBM LMPs and components by settlement location and hub | **Rejected** |
| DA and RTBM market clearing prices | **Rejected** |

## Policy checks

Under [admission-policy.md](../admission-policy.md):

1. **Original price formation.** Passes. The prices are SPP's market
   clearing.
2. **No contract on the path.** Fails, including under the open-terms route
   ([ADR 0012](../decisions/0012-open-terms-route-for-public-bodies-and-market-operators.md)).
   - The [SPP Terms and Conditions](https://www.spp.org/terms-conditions/)
     (read 2026-11-12) say: "Permission is implicitly granted to copy and
     distribute (via computer network or printed form) in whole or in part
     (with appropriate citation) EXCEPT when such materials will be used, in
     whole or in part, within a commercial publication ... Any commercial
     use of these materials requires prior, express written authorization
     from the author(s) or a duly authorized officer of SPP."
   - The portal's terms of use carry the same rule: "Commercial use of any
     information contained on the Portal requires express written
     authorization". This was read from the text embedded in the portal
     application, because the portal page renders client-side.
   - A ban on commercial use is a disqualifying condition. CC0
     redistribution would permit commercial use.
3. **Venue-matched fields only.** Would pass.
4. **Exclusions.** All requested fields.

The deciding clause is "Any commercial use of these materials requires
prior, express written authorization".

## Source basis

The SPP pages above, read 2026-11-12. The SPP tariff posting clause was not
retrieved; it does not change the outcome.

## Risks and review triggers

- SPP removes the commercial-use restriction.
- SPP grants written authorization for free redistribution, including
  commercial use.

## Decision

Rejected on 2026-11-12 by the repository owner. SPP's terms require written
authorization for any commercial use.
