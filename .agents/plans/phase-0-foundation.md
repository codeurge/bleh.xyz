# Phase 0: foundation — Plan

Status: Active
Last updated: 2026-11-11

## Goal

A governed repository, the data-model and registry schemas, the code layout,
and admission decisions for the Hyperliquid sources. Phase 0 gates Phases 1–2
only through `data-model-schema` and `registry-schema`; everything else runs
in parallel.

## Acceptance criteria

- [x] Repository foundation: conventions, licenses, policies, ADRs, admission
      register, plans, CI
- [x] ADM-0001 (Hyperliquid node, every market) and ADM-0003 (Hyperliquid
      public archive) admitted (owner decision, 2026-11-11)
- [ ] Code layout decided (ADR 0007)
- [ ] Data model specified as a machine-readable schema in `docs/specs/`
- [ ] Registry schema, with automatic registration of Hyperliquid markets
      from node metadata, neutral names and a liquidity-grade field
- [x] Project name: bleh.xyz (ADR 0008)
- [ ] Reporting address set

ADR 0004 (DBN) is planned for Phase 2 as a compatibility layer, not a
Phase 0 decision. ADM-0002 and the new ADM-0004 (SBSR) belong to the Part 43
and SBSR lane in the Phase 1 plan; the agency (Tier B) records move to
Phase 3.

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
   - Next action: Protect `main`: require a pull request and passing CI for
     everyone except the owner, who stays on the bypass list and may
     self-merge while there is one maintainer (owner decision, 2026-11-11).
     Add a one-approval rule once there are three maintainers.

3. **code-layout-decision**
   - Status: In progress
   - Owner: Maintainers
   - Dependencies: None
   - PR: Not opened
   - Next action: Review ADR 0007 (workspace under `crates/`, `tools/`,
     `fixtures/synthetic/`, `docs/specs/`, `docs/protocol/`,
     `docs/operations/`) and accept it.

4. **admission-hyperliquid**
   - Status: Done
   - Owner: Repository owner
   - Dependencies: None
   - PR: Not opened (on the review-corrections branch)
   - Next action: None. ADM-0001 and ADM-0003 are Admitted. Follow-ups: how
     ADL fills appear in node output; snapshot frequency inside
     `market_data` files.

5. **data-model-schema**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: code-layout-decision
   - PR: Not opened
   - Next action: Turn the observation model in `docs/architecture.md` §1
     into a versioned JSON Schema and Arrow schema under `docs/specs/`:
     `seq = (block_number, event_index)` and the event order within a
     per-instrument block batch, `trade_id = (block_time, coin, tid)`,
     `ts_source`/`ts_node`/`ts_ingest`/`ts_publish`, `gap`, `heartbeat`,
     `analytics` and `swap_print` kinds, and the per-block signed scope and
     Merkle leaf encoding. Include synthetic examples.

6. **registry-schema**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: None
   - PR: Not opened
   - Next action: Define instrument, exposure, venue-mapping, roll,
     session-calendar and liquidity-grade schemas, with entries written
     fresh from public facts, neutral output names (for example "US
     large-cap index front"), and a mapping from node metadata for automatic
     registration.

7. **wire-format-decision**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: None
   - PR: Not opened
   - Next action: Update ADR 0004 to "planned, Phase 2": DBN as a
     compatibility layer behind a Databento-shaped API, with provenance and
     signatures carried alongside. Phases 1–2 core encodings stay JSON and
     Arrow.

8. **project-name**
   - Status: Done
   - Owner: Repository owner
   - Dependencies: None
   - PR: Not applicable
   - Next action: None. The name is bleh.xyz, on the owner's domain (ADR 0008).

9. **reporting-address**
   - Status: Ready
   - Owner: Repository owner
   - Dependencies: None
   - PR: Not opened
   - Next action: Set the conduct and security reporting address and enable
     private vulnerability reporting.

## Moved elsewhere

- **admission-part43** (ADM-0002) and the SBSR record (ADM-0004): Part 43
  and SBSR lane in the [Phase 1 plan](phase-1-live-data.md).
- **admission-requests-tier-b**: records for NY Fed, Treasury, USDA
  (including mandatory livestock reporting), EIA (excluding vendor-credited
  series) and Commitments of Traders, in Phase 3. IEX is excluded.

## Exit

All acceptance criteria met.
