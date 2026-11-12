# Changelog

All notable changes to this project are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Releases will use
[Semantic Versioning](https://semver.org/) for code. Methods carry their own
versions (see [AGENTS.md](AGENTS.md)).

## Unreleased

### Added

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

### Changed

- Workflow: the repository owner may push directly to `main`; other
  contributors use pull requests.

## 0.0.0 — 2026-11-11

### Added

- Initial design: requirements, source register, admission policy,
  architecture, coverage and roadmap.
