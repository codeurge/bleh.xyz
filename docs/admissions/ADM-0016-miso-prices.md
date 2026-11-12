# ADM-0016: MISO — market reports and Data Exchange pricing

- Status: Rejected
- Requested: 2026-11-12 by the admissions lane (open reference prices plan, task 1)
- Reviewers: Repository owner (engineering review of field lineage)
- Decided: 2026-11-12 by the repository owner (owner decision, 2026-11-12)

## Source

The Midcontinent Independent System Operator (MISO) publishes day-ahead and
real-time LMPs, hub prices and ancillary service market clearing prices. It
uses two channels:

- market reports on misoenergy.org;
- the MISO Data Exchange "Pricing API".

## Fields

| Field | Admit? |
|---|---|
| DA and RT LMPs, hub prices, ancillary service MCPs | **Rejected** |

## Policy checks

Under [admission-policy.md](../admission-policy.md):

1. **Original price formation.** Passes. The prices are MISO's market
   clearing.
2. **No contract on the path.** Fails, including under the open-terms route
   ([ADR 0012](../decisions/0012-open-terms-route-for-public-bodies-and-market-operators.md)).
   The source is MISO's
   [Privacy Notice and Terms of Use](https://www.misoenergy.org/meet-miso/legal-and-privacy/),
   read 2026-11-12.
   - **Acceptance:** "Your use of this Website or the App constitutes your
     acceptance of the Terms and agreement to be bound by them."
   - **Redistribution:** "You are not permitted to modify, publish,
     transmit, participate in the transfer or sale of, reproduce, create
     derivative works of, distribute, publicly perform, publicly display or
     in any way exploit any of the materials or content on this Website or
     the App in whole or in part."
   - **Automated access:** "You agree not use any automated means,
     including, without limitation, agents, robots, scripts, or spiders, to
     access, monitor, or copy any part of this Website or the App."
   - **Data Exchange:** the
     [Data Exchange User Guide](https://cdn.misoenergy.org/MISO%20Data%20Exchange%20User%20Guide674561.pdf)
     says: "A MISO website account is required to create a Data Exchange
     account". That account binds the user to the same terms.
3. **Venue-matched fields only.** Would pass.
4. **Exclusions.** All requested fields.

The deciding clause is "You are not permitted to ... publish ... reproduce
... distribute ... any of the materials or content".

## Source basis

The MISO pages above, read 2026-11-12. The MISO tariff's posting clause was
not reached, because the tariff viewer requires JavaScript. It does not
change the outcome.

## Risks and review triggers

- MISO publishes terms that grant redistribution.

## Decision

Rejected on 2026-11-12 by the repository owner. MISO's terms forbid
publishing and distributing site content, and forbid automated access.
