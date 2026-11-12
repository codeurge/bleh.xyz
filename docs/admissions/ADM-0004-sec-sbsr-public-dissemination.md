# ADM-0004: SEC Regulation SBSR public dissemination

- Status: Admitted
- Requested: 2026-11-11 by maintainers
- Reviewers: Repository owner (engineering review of field lineage)
- Decided: 2026-11-12 by the repository owner (owner decision, 2026-11-12)

Built as a parallel lane in Phases 1–2, alongside
[ADM-0002](ADM-0002-cftc-part43-swaps.md) (owner decision, 2026-11-11).

## Source

Security-based swap data repositories (for example DTCC and ICE Trade Vault)
publicly disseminate security-based swap transactions under SEC Regulation
SBSR (17 CFR 242.900–909): equity-index swaps and single-name credit
default swaps, among others. Rule 900(cc) defines public dissemination as
making data available over the Internet in a widely accessible,
machine-readable form, and Rule 900(tt) defines "widely accessible" as
available on a non-fee basis.

## Fields requested

| Field | Meaning | Admit? |
|---|---|---|
| Execution time, dissemination time | Timing | Requested |
| Asset class, product, underlier or reference entity, tenor, currency | Product identity | Requested |
| Price, spread, notional (as disseminated, including caps) | Pricing and size | Requested |
| Cancel, correct and condition flags | Dissemination flags | Requested |

## Policy checks (to complete)

1. Original price formation: prices agreed between counterparties and
   reported to the repository.
2. No contract on the path: dissemination on a non-fee basis by regulation.
   Check each repository's site and API for conditions on access.
3. Venue-matched fields only: the disseminated fields are transaction
   reports.
4. Exclusions: none expected.

## Source basis

Public dissemination under Regulation SBSR,
<https://www.law.cornell.edu/cfr/text/17/242.900>, read from each
repository's public site or API. How long each repository keeps data
online: to verify. Capture starts early so nothing ages out.

## Risks and review triggers

- A repository adds conditions to its site or API.
- Rule changes to Regulation SBSR.

## Decision

Admitted on 2026-11-12 by the repository owner for the fields above, read from
the repositories' free public dissemination. Repositories operated by an
exchange group are not used, so every print comes from an independent
repository.
