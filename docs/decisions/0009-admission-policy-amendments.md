# 0009: Admission policy amendments — open buckets and venue-matched prices

- Status: Accepted
- Date: 2026-11-11
- Deciders: Repository owner

## Context

Hyperliquid publishes history in openly listed requester-pays buckets, which
the original rule 2 routes (own node, mandated dissemination, public-domain
data) did not cover. HIP-3 markets also carry deployer-supplied oracle and
mark prices next to genuine book-matched trades, and the original rule 3 did
not say precisely which fields that rules out.

## Decision

- **Rule 2** adds a fourth route: a bucket or archive the venue operator
  publishes openly. Its terms are rechecked at each import.
- **Rule 3** becomes an engineering rule: published price fields come from
  venue matching, never from deployer- or oracle-supplied prices. Markets
  whose trading is tethered to an external reference carry the
  `oracle_anchored_venue` flag, and their trades are published.
- Admission is a maintainer's engineering review of field lineage.

## Consequences

- ADM-0003 (Hyperliquid public buckets) is admitted under the new route, so
  backfill runs in parallel with Phase 1.
- Every HIP-3 market's trades and books are published, and oracle, mark and
  funding fields are excluded by construction in `mesh-types`.
