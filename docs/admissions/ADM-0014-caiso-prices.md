# ADM-0014: CAISO — OASIS LMPs and ancillary service prices

- Status: Rejected
- Requested: 2026-11-12 by the admissions lane (open reference prices plan, task 1)
- Reviewers: Repository owner (engineering review of field lineage)
- Decided: 2026-11-12 by the repository owner (owner decision, 2026-11-12)

## Source

The California Independent System Operator (CAISO) publishes market results
on OASIS (oasis.caiso.com), as its FERC-approved tariff requires. Tariff
Section 6 says:

- day-ahead LMPs and ancillary service marginal prices are published by
  1:00 p.m. (§ 6.5.3.2.2);
- FMM and real-time dispatch LMPs are published every 15 and 5 minutes
  (§§ 6.5.5.2.2, 6.5.5.2.4).

Fields requested were day-ahead, FMM and real-time LMPs (including the
NP15, SP15 and ZP26 trading hubs) and ancillary service prices.

## Fields

| Field | Admit? |
|---|---|
| DA, FMM and RTD LMPs and components; hub prices | **Rejected** |
| DA and RT ancillary service marginal prices | **Rejected** |

## Policy checks

Under [admission-policy.md](../admission-policy.md):

1. **Original price formation.** Passes. The prices are CAISO's own market
   clearing.
2. **No contract on the path.** Fails, including under the open-terms route
   ([ADR 0012](../decisions/0012-open-terms-route-for-public-bodies-and-market-operators.md)).
   The terms at <https://www.caiso.com/privacy-terms-of-use> were read
   2026-11-12.
   - **Website terms:** "By using this Website ... you agree to adhere to
     these Terms of Use." And: "No material or information from this Website
     ... may be copied, reproduced, republished, uploaded, posted,
     transmitted or distributed except (i) as authorized in these Terms of
     Use, (ii) as expressly authorized in the materials and/or information
     contained on this Website, or (iii) as expressly authorized in writing
     by the California ISO."
   - **Public records clause:** materials "may be used by you provided that
     you keep intact all copyright, trademark and other proprietary notices
     and that you credit the California ISO". This permits use, not
     redistribution, so it is not an express grant to republish.
   - **API terms** (OASIS is reached through the CAISO API):
     - The licence is to "use the CAISO API solely in accordance with the
       Terms".
     - "CAISO owns all right, title and interest in and to the CAISO API and
       CAISO Data. ... These terms grant you no right, title or interest in
       any intellectual property owned or licensed by CAISO, including
       without limitation the CAISO API and any CAISO Data."
     - "You may not use or access the CAISO API for ... any other
       benchmarking or competitive purposes."
3. **Venue-matched fields only.** Would pass.
4. **Exclusions.** All requested fields, under check 2.

The deciding clauses are the API terms' "grant you no right ... in ... any
CAISO Data" and the website's redistribution ban, which has no applicable
authorization.

## Source basis

- <https://www.caiso.com/privacy-terms-of-use> (website and API terms, read
  2026-11-12)
- <https://www.caiso.com/documents/section-6-communications-as-of-may-1-2026.pdf>
  (tariff Section 6)
- The OASIS Acceptable Use Policy is behind the developer-site login and was
  not read.

## Risks and review triggers

- CAISO gives express written authorization for free redistribution, under
  clause (iii).
- CAISO changes its terms to grant redistribution.

## Decision

Rejected on 2026-11-12 by the repository owner. CAISO's terms grant no
right to redistribute CAISO Data, and they forbid competitive use of the
API.
