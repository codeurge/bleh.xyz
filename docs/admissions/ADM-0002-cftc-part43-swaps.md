# ADM-0002: CFTC Part 43 public swap dissemination

- Status: Admitted
- Requested: 2026-11-11 by maintainers
- Reviewers: Repository owner (engineering review of field lineage)
- Decided: 2026-11-12 by the repository owner (owner decision, 2026-11-12)

Built as a parallel lane in Phases 1–2 (owner decision, 2026-11-11).

## Source

Swap data repositories (for example DTCC Data Repository (U.S.) and ICE
Trade Vault) publicly disseminate swap transaction and pricing data under
CFTC Part 43: SOFR and OIS swaps, FX NDFs and options, natural gas, power
and refined-product swaps, swaptions and caps, and commodity options.
17 CFR 43.3(c)(2) requires publicly disseminated data to be made available
free of charge. 17 CFR 43.3(c)(1) only requires a repository to keep data on
its website for at least one year after dissemination, so capture is
time-critical: anything not recorded within a year may be gone.

## Fields requested

| Field | Meaning | Admit? |
|---|---|---|
| Execution time, dissemination time | Timing | Requested |
| Asset class, product, underlier, tenor, notional currency | Product identity | Requested |
| Price, spread, rounded notional | Pricing and size as disseminated | Requested |
| Block, cap and cancel/correct flags | Dissemination flags | Requested |
| Option and swaption terms: strike, expiry, option type, premium | For swaptions, caps and commodity options | Requested |

## Policy checks (to complete)

1. Original price formation: swap prices are agreed between counterparties.
   Commodity swaps often float against an exchange settlement, but the
   disseminated **fixed price** is the price the counterparties agreed. The
   record says which leg each published price is.
2. No contract on the path: free by regulation. Check each repository's
   website and API for conditions on access.
3. Venue-matched fields only: the disseminated fields are transaction
   reports.
4. Exclusions: none expected.

## Source basis

Public dissemination under 17 CFR 43.3,
<https://www.law.cornell.edu/cfr/text/17/43.3>, read from each repository's
public site or API.

## Risks and review triggers

- A repository adds conditions to its site or API.
- Rule changes to Part 43.

## Decision

Admitted on 2026-11-12 by the repository owner for the fields above, read from
the repositories' free public dissemination. Repositories operated by an
exchange group are not used, so every print comes from an independent
repository.
