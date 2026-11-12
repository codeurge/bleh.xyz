# Phase 1: live data — Plan

Status: Active
Last updated: 2026-11-12

## Goal

Once deployed, anyone can connect to a public gateway or any relay and
receive a signed live stream of **every Hyperliquid market** (core
perpetuals, every HIP-3 deployer, HIP-4 outcome markets), every record
carrying provenance (owner decision, 2026-11-11):

- book-matched trades with native aggressor side
- top of book, L2 depth and order-level book events (snapshot plus deltas)
- time, tick, volume and range bars
- order-flow analytics: footprint, delta and CVD, volume profile, L3 heatmap
  tiles, order lifetimes, queue position, cancel/replace ratios, sweeps
- a liquidity grade per market

In parallel, Part 43 and SBSR swap dissemination is polled into the same
observation schema.

## Source

Two non-validating Hyperliquid nodes on two providers, admitted under
[ADM-0001](../../docs/admissions/ADM-0001-hyperliquid-trades-orders.md).
Output flags: `--write-fills`, `--write-order-statuses`,
`--write-raw-book-diffs`, `--batch-by-block`,
`--disable-output-file-buffering` and `--serve-info`. Never
`--write-hip3-oracle-updates` or `--write-misc-events`. L4 snapshots come
from the info server's `fileSnapshot` request (`l4Snapshots`, without
users). Only book-matched fills are published; owner addresses, oracle,
mark, funding, liquidation, settlement and fee fields are dropped at capture
into filtered raw.

Node facts to plan around (verify at provisioning):

- Official minimum for a non-validator: 16 cores, 128 GB RAM, 500 GB SSD,
  Ubuntu 24.04 only.
- Default settings produce about 100 GB of logs per day, and extra output
  flags raise that sharply (vendors report 1–2.6 TB/day with everything
  enabled). Only the flags above are enabled.
- State snapshots (`periodic_abci_states`) are written every 10,000 blocks.
- `hl-visor` downloads and runs new `hl-node` versions automatically, so
  output schemas can change without notice.

## Build lanes

| Lane | Plan |
|---|---|
| Infrastructure: active-active, multi-provider, CDN | This plan |
| Node-reader: all markets, inotify | This plan |
| Book, bars and analytics | This plan |
| Bus, gateway, relay and client SDKs | This plan (gateway discovery in Phase 2) |
| Archive: filtered raw, partitions, manifests, per-block signing, anchoring, torrents and IPFS | `block-signing` here; the rest in [Phase 2](phase-2-historical-data.md) |
| Static history, small API and compatibility layers | [Phase 2](phase-2-historical-data.md) |
| Parallel: Part 43 and SBSR pollers | This plan |
| Parallel: backfill | [Phase 2](phase-2-historical-data.md), starting now |

## Integration status (2026-11-12)

Every lane is merged and integrated. `crates/origin` runs node files →
reader → book builder → block signer → embedded gateway, with live raw
capture, in one process; `crates/origin/tests/e2e.rs` proves it end to end
on synthetic data (books equal the expected books, every signature and
root verifies, derived records arrive, and the archive replay reproduces
the live records). The acceptance criteria below need a live node and the
staging soak; none is ticked yet.

## Acceptance criteria

- [ ] p99 of (gateway send time − node local write time) is at most 50 ms
      over a 7-day staging soak. Lag behind `block_time` is reported, not
      gated.
- [ ] Availability is at least 99.9% over the soak with both nodes live,
      measured by the external probe in
      [requirements.md](../../docs/requirements.md) (announced node upgrades
      excluded), with public health and freshness endpoints and a status
      page.
- [ ] The two ingest paths agree on every `(block, event_index)`; an
      injected mismatch raises the integrity alarm.
- [ ] Every block's records are covered by a signed Merkle root per stream;
      the reference client and a test contract (`ecrecover`) verify roots
      and proofs.
- [ ] Every market on the node is registered automatically and published,
      with a liquidity grade.
- [ ] Rebuilt books match the node's L4 snapshot (from `fileSnapshot`) at
      every checkpoint (proposed every 10 minutes); mismatches alert and
      trigger a resync.
- [ ] Bars and analytics are deterministic (same canonical content hash on
      rebuild) and carry `method@version` and an input hash. Intervals
      without trades emit no bar; outages are published as `gap` records.
- [ ] Every published record carries instrument, venue, `ts_source`,
      `ts_node`, `ts_ingest`, `ts_publish`, `seq` and admission ID. A test
      fails the build if any non-allowlisted field or non-book-matched fill
      reaches the output.
- [ ] Gateway protocol is documented; SDKs for Rust, Python and TypeScript
      keep a correct local book, verify signatures and recover from gaps.
- [ ] Per-IP connection and subscription caps and the slow-consumer policy
      hold under a load test; public endpoints are behind TLS and a CDN/DDoS
      front, with published terms of use and a disclaimer.
- [ ] Part 43 and SBSR pollers capture every repository's public
      dissemination into filtered raw from their first day.
- [ ] Filtered raw capture (Phase 2) is running from the first node's first
      day.

## Dependencies

- Phase 0: `data-model-schema`, `registry-schema`.
- [ADR 0006](../../docs/decisions/0006-phase-1-2-service-architecture.md):
  Accepted.

## Ordered work

### Lane: infrastructure (active-active, multi-provider, CDN)

1. **architecture-decision**
   - Status: Done
   - Owner: Repository owner
   - Dependencies: None
   - PR: Not opened (on the review-corrections branch)
   - Next action: None. ADR 0006 is Accepted.

2. **cost-and-retention-model**
   - Status: In progress
   - Owner: Repository owner
   - Dependencies: None (vendor figures first; refine with node-runbook)
   - PR: Not opened
   - Next action: docs/operations/costs.md and hosting.md give the budget-first
     tiers of ADR 0010 (estimates). Refine with measured node disk growth
     once the node runs.

3. **hosting-and-node-provisioning**
   - Status: Ready
   - Owner: Repository owner
   - Dependencies: cost-and-retention-model
   - PR: Not opened
   - Next action: Rent the node box per ADR 0010 and docs/operations/node.md;
     bring up `deploy/up.sh node-box` (the origin service) and
     `home-archive`.

4. **infrastructure-as-code**
   - Status: In progress
   - Owner: Unassigned
   - Dependencies: hosting-and-node-provisioning
   - PR: Not opened
   - Next action: Built: compose profiles, Terraform stacks, systemd units,
     reproducible images and the manual-promote
     `.github/workflows/deploy.yml` (deploy/). Next: apply in staging and
     promote one build end to end.

5. **secrets-handling**
   - Status: In progress
   - Owner: Unassigned
   - Dependencies: hosting-and-node-provisioning
   - PR: Not opened
   - Next action: Secrets are files mounted read-only (deploy/compose/secrets,
     docs/operations/keys.md); nothing in images. Next: move the signing key
     into a KMS or hardware key and document its rotation drill.

6. **signing-keys**
   - Status: Ready
   - Owner: Repository owner
   - Dependencies: secrets-handling
   - PR: Not opened
   - Next action: Tooling exists (`mesh-client keys address`, `archive manifest
     keygen`, key rotation in docs/operations/bus.md). Generate the
     block-signing and manifest keys on the hosts and publish their
     addresses.

7. **time-sync**
   - Status: In progress
   - Owner: Unassigned
   - Dependencies: hosting-and-node-provisioning
   - PR: Not opened
   - Next action: host-bootstrap.sh configures chrony. Next: export the clock
     offset and alert above 5 ms on the live hosts.

8. **tls-cdn-front**
   - Status: In progress
   - Owner: Unassigned
   - Dependencies: infrastructure-as-code
   - PR: Not opened
   - Next action: Built: Cloudflare edge Terraform, Caddy relays with TLS.
     Next: apply and test websocket passthrough.

9. **observability**
   - Status: In progress
   - Owner: Unassigned
   - Dependencies: infrastructure-as-code
   - PR: Not opened
   - Next action: Built: Prometheus rules (latency, gaps, merge alarms,
     reconciliation, signing, capture), dashboards, the availability probe
     (`mesh-client probe`). Next: run on the hosts and publish a status
     page.

10. **public-endpoint-terms**
    - Status: Ready
    - Owner: Repository owner
    - Dependencies: None
    - PR: Not opened
    - Next action: Publish terms of use and a disclaimer: data is CC0,
      provided as is, not investment advice, with the published
      availability target.

### Lane: node-reader (all markets, inotify)

11. **node-runbook**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: hosting-and-node-provisioning
    - PR: Not opened
    - Next action: Drafted in docs/operations/node.md. Blocked on hosting: run
      the node with the flags in § Source and measure disk growth.

12. **node-disk-and-pruning**
    - Status: In progress
    - Owner: Unassigned
    - Dependencies: node-runbook
    - PR: Not opened
    - Next action: deploy/scripts/hl-prune.sh prunes up to the origin's
      `.captured-through` watermark (written by live capture,
      crates/origin). Next: verify on the node box.

13. **node-upgrade-runbook**
    - Status: In progress
    - Owner: Unassigned
    - Dependencies: node-runbook, node-reader
    - PR: Not opened
    - Next action: docs/operations/upgrades.md and schema-drift-check.sh exist;
      the reader rejects unknown fields. Next: rehearse a one-node-at-a-time
      upgrade.

14. **synthetic-fixtures**
    - Status: Done
    - Owner: Unassigned
    - Dependencies: Phase 0 data-model-schema
    - PR: Not opened
    - Next action: None. tools/hl-synth and fixtures/synthetic/hl (base and gap
      scenarios, L4 snapshot, expected books); `cargo test -p hl-synth`
      keeps them current.

15. **node-reader**
    - Status: Done
    - Owner: Unassigned
    - Dependencies: synthetic-fixtures
    - PR: Not opened
    - Next action: Implemented and tested on synthetic data
      (crates/hl-node-reader); live raw capture and the watermark run in the
      origin. Verify the spec § 11 assumptions on a live node.

16. **market-auto-registration**
    - Status: Done
    - Owner: Unassigned
    - Dependencies: Phase 0 registry-schema, node-reader
    - PR: Not opened
    - Next action: Implemented (hl_node_reader::markets,
      Registry::replace/remove). Open registry pull requests from
      `HL_REGISTRY_OUT` as markets appear.

### Lane: book, bars and analytics

17. **book-builder**
    - Status: Done
    - Owner: Unassigned
    - Dependencies: synthetic-fixtures
    - PR: Not opened
    - Next action: crates/book; the origin publishes checkpoints, rolls late L4
      snapshots forward and resyncs (crates/origin/src/derive.rs, tests on
      the gap scenario). Measure reconciliation against a live node's
      snapshots.

18. **bar-builder**
    - Status: Done
    - Owner: Unassigned
    - Dependencies: Phase 0 data-model-schema
    - PR: Not opened
    - Next action: None. bars@1, tick/volume/range bars; published live by the
      origin and archived by replay.

19. **orderflow-analytics**
    - Status: Done
    - Owner: Unassigned
    - Dependencies: book-builder, bar-builder
    - PR: Not opened
    - Next action: None. Every analytic is an observation kind (observation-v0 §
      2.10), published and signed per block by the origin and replayed
      identically into the archive (crates/origin/tests/e2e.rs).

20. **liquidity-grade**
    - Status: Done
    - Owner: Unassigned
    - Dependencies: book-builder
    - PR: Not opened
    - Next action: None. liquidity-grade@1 as the `liquidity_grade` kind.

### Lane: bus, gateway, relay and client SDKs

21. **ring-and-side-tap**
    - Status: Done
    - Owner: Unassigned
    - Dependencies: architecture-decision
    - PR: Not opened
    - Next action: None. crates/bus; the origin wires the JetStream tap when
      BLEH_NATS_URL is set (integration tests need a local nats-server).

22. **gateway**
    - Status: Done
    - Owner: Unassigned
    - Dependencies: ring-and-side-tap, block-signing
    - PR: Not opened
    - Next action: Embedded in the origin on :8080 (JSON and CBOR; Arrow
      encoding not built). Load-test the per-IP caps and slow-consumer
      policy in staging.

23. **relay**
    - Status: Done
    - Owner: Unassigned
    - Dependencies: gateway
    - PR: Not opened
    - Next action: None. `gateway relay` verifies every commitment before
      re-serving (docs/operations/gateway.md).

24. **protocol-docs**
    - Status: Done
    - Owner: Unassigned
    - Dependencies: gateway
    - PR: Not opened
    - Next action: None. docs/protocol/gateway-v0.md matches the implementation
      (ADR 0011).

25. **client-sdks**
    - Status: In progress
    - Owner: Unassigned
    - Dependencies: gateway
    - PR: Not opened
    - Next action: Rust reference client done (tools/mesh-client: books, L2,
      signatures, roots, proofs, failover, probe). Next: Python (PyO3) and
      TypeScript (WASM) bindings over it.

### Lane: archive (Phase 1 part)

26. **block-signing**
    - Status: In progress
    - Owner: Unassigned
    - Dependencies: Phase 0 data-model-schema, signing-keys (test keys
      first)
    - PR: Not opened
    - Next action: Per-block per-stream Merkle roots and secp256k1 signatures
      (crates/bus), signed by the origin and verified by the reference
      client end to end. Remaining: the Solidity test contract that verifies
      with `ecrecover`.

### Lane: Part 43 and SBSR pollers (parallel)

27. **admission-part43-sbsr**
    - Status: Done
    - Owner: Unassigned
    - Dependencies: None
    - PR: Not opened
    - Next action: None. ADM-0002 and ADM-0004 are Admitted.

28. **swap-pollers**
    - Status: Done
    - Owner: Unassigned
    - Dependencies: Phase 0 data-model-schema
    - PR: Not opened
    - Next action: DTCC Part 43 and SBSR polling with lifecycle actions, event
      types and option terms (crates/swap-pollers). ICE Trade Vault is
      excluded until admitted. Deploy on the home-archive server.

### Release

29. **staging-soak**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: node-runbook, node-reader, gateway, observability,
      tls-cdn-front
    - PR: Not opened
    - Next action: Blocked on hosting. Then 7 days in staging on live data from
      both nodes, checking every acceptance criterion, a load test and an
      injected path mismatch.

30. **production-launch**
    - Status: Blocked
    - Owner: Repository owner
    - Dependencies: staging-soak, public-endpoint-terms
    - PR: Not opened
    - Next action: Deploy production on both providers and open it to the
      public.

## Exit

All acceptance criteria met and the production gateway is live, with the
Phase 2 archive recording.
