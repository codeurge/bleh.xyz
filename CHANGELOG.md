# Changelog

All notable changes to this project are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Releases will use
[Semantic Versioning](https://semver.org/) for code. Methods carry their own
versions (see [AGENTS.md](AGENTS.md)).

## Unreleased

### Added

- `archive partition-dir`: partitions daily record files
  (`date=YYYY-MM-DD/<file>`) once per settled day, with SHA-256 markers and
  write-once partitions; compose runs it as `swap-partition` so swap prints
  reach the history API. Self-hosted servers can keep the normalized
  archive in a local directory (`LOCAL_NORMALIZED_DIR`) and serve it with
  the history API; the swap raw mirror can be turned off
  (`SWAP_RAW_MIRROR_BUCKET` empty).
- Repository foundation: agent and contributor conventions, licenses
  (Apache-2.0 for code, CC0 1.0 for data and methods), security and conduct
  policies, decision records, admission register, tracked plans, CI and
  data-leak guardrails.
- Plans for Phase 1 (live data) and Phase 2 (historical data), with the
  roadmap reordered so live and historical venue data come first.
- ADR 0006 (proposed): service architecture for live and historical data.
- Admission request ADM-0003: Hyperliquid public archive buckets for
  backfill.
- Independent review applied, with owner decisions: every Hyperliquid
  market published from day one; ADM-0001 and ADM-0003 admitted; two-tier
  raw archive; backfill, Part 43 and SBSR lanes in Phases 1–2; per-block
  secp256k1 signing; active-active ingest; static-first history over CDN,
  BitTorrent, IPFS and Arweave with on-chain manifest anchoring;
  compatibility layers; open oracle in Phase 4.
- ADR 0008: the project is named bleh.xyz (supersedes 0005).
- ADR 0009: admission policy amendments (open buckets, venue-matched prices).
- Code foundation (ADR 0007): Cargo workspace with one crate per build lane;
  `mesh-types` (fixed-point prices, per-block sequencing, provenance, field
  allowlist, canonical hashing, Keccak Merkle trees, secp256k1 block
  commitments and manifests, Arrow schemas); `hl-synth` synthetic node
  fixtures; specs for observations, node format, manifests, archive layout
  and gateway v0; hardened checks with a self-test; Rust and cargo-deny CI.
- Brand: the tongue-out "e" wordmark, palette and type in `brand/`, with the
  first concept round and landing-page mocks in `brand/concepts/`.
- Brand system: guidelines, marketing site and manifesto (`brand/site/`), a
  30-second film in landscape and vertical plus a logo sting
  (`brand/video/`), and a social kit with launch copy (`brand/social/`).
- ADR 0010: budget-first hosting, about $150 a month (one rented node box,
  a self-hosted archive, seed and processing server, free relays, R2).
- `hl-backfill process` runs by-block fills through the live node reader
  (filter, parser, sequencer) and writes deterministic Parquet partitions
  of the `hyperliquid-backfill` archive with the archive crate's writer,
  readable by the history API; JSON lines remain behind `--format jsonl`.
- `swap-pollers` emits `modify`, `terminate` and `revive` prints and every
  non-trade `new` message with its `event_type`, which were skipped before
  (consumers counting volume select `event_type` `trade` or absent); CFTC
  records carry `strike`, `option_premium`, `option_type`, `option_style`
  and `first_exercise_date` (the strike is no longer the `price`); monetary
  spreads are `spread_amount`; commodity quantities are `quantity`.
- The `origin` service (`crates/origin`, ADR 0007 amendment): node files
  (one or two nodes, active-active) → reader → book builder (books,
  checkpoints, bars, analytics, venue coverage) → block signer → embedded
  gateway on `:8080` (`/v0/ws`, `/health`, `/freshness`, `/metrics`,
  `/proof`, `/signers`), optional JetStream side tap, and live capture of
  the private and filtered raw tiers with the `.captured-through`
  watermark. L4 snapshots read late are rolled forward over recent book
  events; reset books are published as resyncs for gateways. Runbook:
  `docs/operations/origin.md`.
- End-to-end tests on synthetic data: node files → origin → localhost
  websocket → reference client (books, signatures, roots, derived records),
  then capture → replay → Parquet → history API matching the live trades,
  bars and books, byte-identical across runs; backfill objects →
  `hl-backfill process` → partitions read by the history API.
- Observation kinds `activity_bar`, `delta_bar`, `profile_summary`,
  `order_activity`, `queue_positions`, `sweep`, `heatmap_tile`,
  `liquidity_grade` and `coverage`, the `replayed` flag, and swap print
  lifecycle actions, event type, option terms, `spread_amount` and
  commodity quantities, with allowlist, JSON Schema and Arrow schemas
  (ADR 0011). `Registry::replace` and `Registry::remove`.
- `archive capture run --follow`: live capture with sealing, write-once
  upload, the `.captured-through` watermark and a periodically rewritten
  `--metrics-out`; manifests now publish `_manifests/latest.json`.
- `mesh-client probe` (one JSON line for availability probes) and
  `mesh-client keys address`; every gateway flag can be set as a
  `BLEH_GATEWAY_*` variable; gateway L2 level deltas for `depth: l2`.
- Deploy: the node box runs the origin image; the deploy workflow lives in
  `.github/workflows/deploy.yml` (manual promote); root `.dockerignore`.
- ADR 0011: data model and gateway amendments for the integrated origin.
- Bring your own feed (ADR 0014): `mesh-client byo merge` merges a
  feed the user licenses themselves (CSV, JSON lines, DBN files, or the
  vendor's live client behind the off-by-default `databento-live` feature)
  with bleh.xyz prices on the user's machine only, through a user-written
  mapping file, and writes a side-by-side stream, an optional loopback
  websocket and local tracking statistics (basis, tracking error,
  lead/lag), every line tagged `user_supplied`. User data cannot reach
  signing, publishing or the archive; `mesh-client byo check-map` validates
  a mapping. The admission policy gains "User-supplied data stays local".
  Guide: `docs/operations/byo-feed.md`.
- Synthetic contract months (ADR 0013): the method
  `synthetic-months@1` (`docs/specs/method-synthetic-months-v1.md`, CC0)
  and `crates/synthetic` (library and `synthetic` binary), which value
  each contract month of equity index, crude oil, natural gas, gold,
  silver and one- and three-month overnight-rate exposures from venue
  spot composites, public rates, rulebook calendars, carry inputs and
  swap-print anchors, with quality flags and inherited liquidity grades.
  Tested on synthetic fixtures only (`fixtures/synthetic/synthetic-months/`).
- Observation v0 kinds `reference_value` (published rates, yields and
  reference prices, for future reference pollers) and `synthetic_month`,
  with allowlist, JSON Schema and Arrow schemas; existing canonical
  encodings unchanged.
- Public settlement anchors and reference data:
  - ADR 0012 adds an open-terms route to admission rule 2 and the
    `exchange_referenced` flag.
  - Records ADM-0005 to ADM-0019 cover CFTC Commitments of Traders, US
    Treasury rates and auctions, NY Fed reference rates, Federal Reserve
    H.15 and H.10, USDA livestock, dairy and feeder cattle, and seven grid
    operators.
  - Admitted: ERCOT, plus the federal, Federal Reserve and USDA sources,
    with exclusions per field.
  - Rejected: grain bids, CAISO, PJM, MISO, ISO-NE and SPP. NYISO stays
    under review.
  - Access details for the poller lane are in
    `docs/specs/reference-sources.md`.
- Reference pollers (`crates/reference-pollers`, plan task 4): live
  polling and yearly, resumable, idempotent history backfill for every
  admitted reference source (COT, Treasury rates and auctions, NY Fed,
  H.15, H.10, USDA LMR, dairy and feeder cattle, ERCOT), with raw capture,
  conditional requests, backoff, uniform revision handling, JSON lines,
  partitions through the archive writer, a series catalogue with notices,
  metrics and alerts, compose and systemd deployment and a runbook
  (`docs/operations/reference-pollers.md`). Synthetic fixtures only.
- ADR 0015: observation v0 gains flag bit 7
  `exchange_referenced`, the non-derived `positioning` kind (Commitments of
  Traders) and optional `period_start_ms` / `period_end_ms` on
  `reference_value`; manifest v0 gains an optional, signed `notices` list,
  filled by `archive manifest build` from `_notices.json`. Existing
  canonical encodings and manifests are unchanged.
- The synthetic engine reads the pollers' rates directly: curve preset
  `CurveConfig::reference_pollers()` and `synthetic --curve
  reference-pollers` (SOFR, Treasury bills, Treasury par curve), with the
  mapping in the method spec (§ 2.1, informative).

### Changed

- Workflow: the repository owner may push directly to `main`; other
  contributors use pull requests.
- Derived records are positioned at the block that completed them
  (`seq = (block, i)`) and signed with it; `ts_source_ms` stays the time of
  the last input (observation-v0 § 2.8).
- The gateway `block` heartbeat carries `venue`; channel caps answer
  `limit_exceeded`; `depth: l2` subscribers get `levels` messages instead
  of L4 book events. Clients of the earlier v0 draft must update.
- Partition paths write `#` as `%23` (`hyperliquid~outcome~%239001`).
- The `archive` binary is built by `crates/origin` with the node reader's
  filters and the origin's parser wired in; replay needs
  `BLEH_ARCHIVE_REGISTRY`.
- Shared dependencies are declared once in `[workspace.dependencies]`;
  `duckdb` has default features off.
- Order-level replay output from `hl-backfill` carries `replayed` as well
  as `backfill`.
- The node format spec records the reader's fill/modify classification,
  the filtered-raw format and the spot exclusion; the gateway protocol
  document matches the implementation.

## 0.0.0 — 2026-11-11

### Added

- Initial design: requirements, source register, admission policy,
  architecture, coverage and roadmap.
