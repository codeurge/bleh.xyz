# ADM-0018: ISO New England — ISO Express and web services LMPs and reserve prices

- Status: Rejected
- Requested: 2026-11-12 by the admissions lane (open reference prices plan, task 1)
- Reviewers: Repository owner (engineering review of field lineage)
- Decided: 2026-11-12 by the repository owner (owner decision, 2026-11-12)

## Source

ISO New England (ISO-NE) publishes day-ahead and real-time LMPs, including
the Internal Hub and the load zones, plus reserve and regulation clearing
prices. It uses two channels:

- ISO Express CSV reports;
- the ISO-NE web services API, which requires an ISO Express account and
  HTTP Basic authentication.

Its Information Policy (Tariff Attachment D, § 3.0(a)) classes clearing
prices and LMPs as "Public Market Information" that "may be made available
to the public at large. (Fees may be applicable to cover process and
handling expenses.)"

## Fields

| Field | Admit? |
|---|---|
| DA and RT LMPs and components, hub and zone prices | **Rejected** |
| Reserve, regulation and operating reserve clearing prices | **Rejected** |

## Policy checks

Under [admission-policy.md](../admission-policy.md):

1. **Original price formation.** Passes. The prices are ISO-NE's market
   clearing.
2. **No contract on the path.** Fails, including under the open-terms route
   ([ADR 0012](../decisions/0012-open-terms-route-for-public-bodies-and-market-operators.md)).
   The source is the
   [ISO-NE Terms and Conditions](https://www.iso-ne.com/legal-privacy), read
   2026-11-12.
   - **Assent:** "By using this website, you signify your assent to these
     Terms and Conditions."
   - **Copyright and use:** Content is defined to include "all information
     and data on this website". The terms then say: "You are also hereby put
     on notice that the Content is protected by copyright under United
     States laws. Any duplication of the Content or non-personal use may
     violate copyright, trademark, and other laws."
   - There is no redistribution grant. Non-personal use is warned against.
     The web services account sits under the same terms.
3. **Venue-matched fields only.** Would pass.
4. **Exclusions.** All requested fields.

The deciding clause is "Any duplication of the Content or non-personal use
may violate copyright", read with the assent clause.

## Source basis

The ISO-NE pages above and the
[Information Policy](https://www.iso-ne.com/static-assets/documents/regulatory/tariff/attach_d/attachment_d.pdf),
read 2026-11-12. The current Market Rule 1 posting provisions were not
retrieved; they do not change the outcome.

## Risks and review triggers

- ISO-NE publishes terms that grant redistribution of public market
  information.

## Decision

Rejected on 2026-11-12 by the repository owner. ISO-NE's terms grant no
redistribution and warn that duplication or non-personal use may infringe.
