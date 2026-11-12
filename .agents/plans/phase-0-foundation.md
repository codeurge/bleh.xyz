# Phase 0: foundation — Plan

Status: Active
Last updated: 2026-11-11

## Goal

Everything needed before ingest code is written: a governed repository, the
data model and wire format, admission decisions for the Phase 1 and 2
sources, and the registry schema.

## Acceptance criteria

- [x] Repository foundation: conventions, licenses, policies, ADRs, admission
      register, plans, CI
- [ ] ADR 0004 (wire format) decided
- [ ] Admission decisions for ADM-0001 and ADM-0002
- [ ] Admission requests filed for NY Fed, Treasury, USDA, EIA,
      Commitments of Traders, and IEX delayed data
- [ ] Data model specified as a machine-readable schema
- [ ] Registry schema and first entries (S&P 500, gold, silver, WTI, Brent
      exposures)
- [ ] Public name chosen (ADR), reporting address set, licensing confirmed
      by counsel

## Ordered work

1. **repo-foundation**
   - Status: Done
   - Owner: Maintainers
   - Dependencies: None
   - PR: https://github.com/codeurge/bleh.xyz/pull/1 (on `main` as f01a4eb)
   - Next action: None.

2. **branch-protection**
   - Status: Ready
   - Owner: Repository owner
   - Dependencies: repo-foundation
   - PR: Not applicable (repository settings)
   - Next action: Protect `main`: require a pull request, one approval, and
     passing `checks` CI.

3. **wire-format-decision**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: None
   - PR: Not opened
   - Next action: Check DBN licensing and extension records; move ADR 0004
     to Accepted or Rejected.

4. **admission-hyperliquid**
   - Status: Blocked
   - Owner: Unassigned
   - Dependencies: Counsel
   - PR: Not opened
   - Next action: Complete ADM-0001 policy checks: node software license,
     HIP-3 deployer terms, oracle-free trade fields.

5. **admission-part43**
   - Status: Blocked
   - Owner: Unassigned
   - Dependencies: Counsel
   - PR: Not opened
   - Next action: Complete ADM-0002: each swap repository's site and API
     terms, and the floating-leg question for commodity swaps.

6. **admission-requests-tier-b**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: None
   - PR: Not opened
   - Next action: File ADM records for NY Fed, Treasury, USDA (including
     mandatory livestock reporting), EIA (excluding vendor-credited series),
     Commitments of Traders, and IEX delayed data.

7. **data-model-schema**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: wire-format-decision for the binary encoding only
   - PR: Not opened
   - Next action: Turn the observation model in `docs/architecture.md` §1
     into a versioned schema (e.g. JSON Schema plus Arrow schema), with
     synthetic examples.

8. **registry-schema**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: None
   - PR: Not opened
   - Next action: Define instrument, exposure, venue-mapping, roll and
     session-calendar schemas, with entries written fresh from public facts.

9. **public-name**
   - Status: Ready
   - Owner: Repository owner
   - Dependencies: None
   - PR: Not opened
   - Next action: Shortlist names, run a trademark search, record the choice
     in a new ADR superseding 0005.

10. **contacts-and-licensing-review**
    - Status: Ready
    - Owner: Repository owner
    - Dependencies: None
    - PR: Not opened
    - Next action: Set the conduct and security reporting address; ask
      counsel to confirm ADR 0002 before any public release.

## Exit

All acceptance criteria met. Phase 1 (live data) and Phase 2 (historical
data) plans are already open and depend on `data-model-schema`,
`registry-schema` and `wire-format-decision`.
