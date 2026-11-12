# Phase 1: live data — Plan

Status: Active
Last updated: 2026-11-11

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
   - Status: Ready
   - Owner: Repository owner
   - Dependencies: None (vendor figures first; refine with node-runbook)
   - PR: Not opened
   - Next action: The infrastructure lane writes `docs/operations/costs.md`
     and `docs/operations/hosting.md` for the budget-first tiers in
     ADR 0010. Refine with measured node disk growth once the node runs.

3. **hosting-and-node-provisioning**
   - Status: Ready
   - Owner: Repository owner
   - Dependencies: cost-and-retention-model
   - PR: Not opened
   - Next action: Launch per ADR 0010: rent one dedicated node box (16+
     modern cores, 128 GB RAM, NVMe; provider's policy must allow blockchain
     nodes) and set up the owner's home server as the archive, seed and
     processing node behind a Cloudflare Tunnel; free relays in front. Write
     `docs/operations/node.md`. Second origin and Tokyo follow usage.

4. **infrastructure-as-code**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: hosting-and-node-provisioning
   - PR: Not opened
   - Next action: Staging and production on both providers in code, with a
     deploy pipeline from tagged reproducible builds and rollback.

5. **secrets-handling**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: hosting-and-node-provisioning
   - PR: Not opened
   - Next action: Storage credentials, CDN tokens and signing access in a
     secrets manager; nothing in the repository or images; documented
     rotation.

6. **signing-keys**
   - Status: Ready
   - Owner: Repository owner
   - Dependencies: secrets-handling
   - PR: Not opened
   - Next action: Generate the secp256k1 block-signing keys (one per
     ingest path) and the manifest key in a KMS or hardware key; document
     custody, rotation, publication in the registry and revocation.

7. **time-sync**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: hosting-and-node-provisioning
   - PR: Not opened
   - Next action: chrony (or equivalent) on every host; export clock
     offset; alert above 5 ms.

8. **tls-cdn-front**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: infrastructure-as-code
   - PR: Not opened
   - Next action: TLS and a CDN/DDoS front for gateways, the small API and
     the archive, passing websockets.

9. **observability**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: infrastructure-as-code
   - PR: Not opened
   - Next action: Metrics for block lag, `seq` gaps, path agreement, book
     reconciliation, publish latency, clock offset and clients; the
     external availability probe; alerts and a public status page.

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
    - Next action: Run both nodes with exactly the flags in § Source;
      measure disk growth per day; document in `docs/operations/node.md`.

12. **node-disk-and-pruning**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: node-runbook
    - PR: Not opened
    - Next action: Prune node output and `periodic_abci_states` once
      captured, with the node repository's pruner or our own job; alert on
      disk use.

13. **node-upgrade-runbook**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: node-runbook, node-reader
    - PR: Not opened
    - Next action: Handle `hl-visor` upgrades one node at a time; detect
      schema drift (unknown fields or shapes stop that kind and alert);
      mark announced upgrades on the status page.

14. **synthetic-fixtures**
    - Status: Ready
    - Owner: Unassigned
    - Dependencies: Phase 0 data-model-schema
    - PR: Not opened
    - Next action: Synthetic `--batch-by-block` files (fills, order
      statuses, book diffs, L4 snapshots) under `fixtures/synthetic/`:
      two-fill trades, partial fills, cancels, modifies, liquidation,
      settlement and HIP-4 settlement fills (to be dropped), new-market
      metadata, gaps and block boundaries.

15. **node-reader**
    - Status: Ready
    - Owner: Unassigned
    - Dependencies: synthetic-fixtures
    - PR: Not opened
    - Next action: Clean-room reader: tail by-block files with inotify, pair
      fills into book-matched trades, drop excluded fields and fills, assign
      `seq`, emit per-instrument block batches, write unfiltered and
      filtered raw, and dedupe the two paths with a mismatch alarm.

16. **market-auto-registration**
    - Status: Ready
    - Owner: Unassigned
    - Dependencies: Phase 0 registry-schema, node-reader
    - PR: Not opened
    - Next action: Register every market from node metadata (venue ID,
      deployer, tick and lot size) with a neutral name and
      `oracle_anchored_venue` where it applies; open a registry pull request
      for each new market.

### Lane: book, bars and analytics

17. **book-builder**
    - Status: Ready
    - Owner: Unassigned
    - Dependencies: synthetic-fixtures
    - PR: Not opened
    - Next action: Order-level books from a `fileSnapshot` L4 snapshot plus
      per-block order statuses and diffs; derive L2 and top of book;
      reconcile at the checkpoint interval; emit checkpoints for the
      archive.

18. **bar-builder**
    - Status: Ready
    - Owner: Unassigned
    - Dependencies: Phase 0 data-model-schema
    - PR: Not opened
    - Next action: Deterministic 1-second and 1-minute time bars plus tick,
      volume and range bars, with buy and sell volume by aggressor side; no
      bar for intervals without trades.

19. **orderflow-analytics**
    - Status: Ready
    - Owner: Unassigned
    - Dependencies: book-builder, bar-builder
    - PR: Not opened
    - Next action: Live streams with `method@version` and input hash:
      footprint, delta and CVD, volume profile, L3 heatmap tiles, order
      lifetimes, queue position, cancel/replace ratios, sweep detection.
      Document each method.

20. **liquidity-grade**
    - Status: Ready
    - Owner: Unassigned
    - Dependencies: book-builder
    - PR: Not opened
    - Next action: Publish spread, depth and trades per minute per market,
      and the tape/benchmark label with its thresholds as a versioned
      method.

### Lane: bus, gateway, relay and client SDKs

21. **ring-and-side-tap**
    - Status: Ready
    - Owner: Unassigned
    - Dependencies: architecture-decision
    - PR: Not opened
    - Next action: In-process ring buffer from reader to gateway; NATS
      JetStream side tap for durability and replay; nothing archives from
      it.

22. **gateway**
    - Status: Ready
    - Owner: Unassigned
    - Dependencies: ring-and-side-tap, block-signing
    - PR: Not opened
    - Next action: Websocket gateway: subscribe by instrument and kind;
      snapshot tagged `(block, seq)`, contiguous deltas, per-block
      heartbeats with signed roots, forced re-snapshot on gaps, proofs on
      request; JSON and Arrow; per-IP caps; slow consumers drop to
      snapshot, then disconnect; `/health` and `/freshness`.

23. **relay**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: gateway
    - PR: Not opened
    - Next action: A relay binary anyone can run that re-serves the signed
      stream unchanged; document running one.

24. **protocol-docs**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: gateway
    - PR: Not opened
    - Next action: Document the protocol, signing and proofs in
      `docs/protocol/`.

25. **client-sdks**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: gateway
    - PR: Not opened
    - Next action: One Rust core that keeps a local book, verifies roots and
      proofs and recovers from gaps; Python via PyO3 and TypeScript via
      WASM; published packages.

### Lane: archive (Phase 1 part)

26. **block-signing**
    - Status: Ready
    - Owner: Unassigned
    - Dependencies: Phase 0 data-model-schema, signing-keys (test keys
      first)
    - PR: Not opened
    - Next action: Merkle root per block per stream over canonical record
      encodings, signed with secp256k1; proof generation; a test contract
      that verifies with `ecrecover`.

### Lane: Part 43 and SBSR pollers (parallel)

27. **admission-part43-sbsr**
    - Status: Ready
    - Owner: Unassigned
    - Dependencies: None
    - PR: Not opened
    - Next action: Complete ADM-0002 and ADM-0004: each repository's
      formats, access methods and online retention; which leg each
      commodity swap price is.

28. **swap-pollers**
    - Status: Ready (build); merge after admission-part43-sbsr
    - Owner: Unassigned
    - Dependencies: Phase 0 data-model-schema
    - PR: Not opened
    - Next action: Poll DTCC and ICE Trade Vault Part 43 and SBSR
      dissemination into `swap_print` observations (SOFR and OIS, FX NDFs
      and options, natural gas, power and refined products, swaptions and
      caps, commodity options, equity-index swaps, single-name CDS), with
      filtered raw capture. Capture is time-critical (43.3(c)(1)).

### Release

29. **staging-soak**
    - Status: Blocked
    - Owner: Unassigned
    - Dependencies: node-runbook, node-reader, gateway, observability,
      tls-cdn-front
    - PR: Not opened
    - Next action: 7 days in staging on live data from both nodes; verify
      every acceptance criterion, including a load test and an injected
      path mismatch.

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
