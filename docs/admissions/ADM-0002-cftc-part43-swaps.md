# ADM-0002: CFTC Part 43 public swap dissemination

- Status: Requested
- Requested: 2026-11-11 by maintainers
- Reviewers: To be assigned (two maintainers plus counsel)
- Decided:

## Source

Swap data repositories (for example DTCC Data Repository (U.S.)) publicly
disseminate swap transaction and pricing data under CFTC Part 43.
17 CFR 43.3 requires publicly disseminated data to be free of charge.

## Fields requested

| Field | Meaning | Admit? |
|---|---|---|
| Execution time, dissemination time | Timing | Requested |
| Asset class, product, underlier, tenor, notional currency | Product identity | Requested |
| Price, spread, rounded notional | Pricing and size as disseminated | Requested |
| Block, cap and cancel/correct flags | Dissemination flags | Requested |

## Policy checks (to complete)

1. Original price formation: swap prices are agreed between counterparties.
   Note that commodity swaps often float against an exchange settlement, but
   the disseminated **fixed price** is a market-agreed price. Counsel to
   confirm.
2. No contract on the path: free by regulation. Review each repository's
   website and API terms, which may still impose conditions on access.
3. Clean upstream: the disseminated fields are transaction reports.
4. Exclusions: none expected.

## Legal basis

17 CFR 43.3, <https://www.law.cornell.edu/cfr/text/17/43.3>. Repository
terms to be reviewed.

## Risks and review triggers

- A repository's terms restrict redistribution despite free access.
- Rule changes to Part 43.
