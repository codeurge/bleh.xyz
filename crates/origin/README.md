# origin

The bleh.xyz origin service: one process from Hyperliquid node files to the
public, signed live stream and the archive's raw tiers.

**Status:** implemented and tested end to end on synthetic data.
**Owner:** integrator ([ADR 0007](../../docs/decisions/0007-code-layout.md),
amendment of 2026-11-12). Runbook:
[docs/operations/origin.md](../../docs/operations/origin.md).

## Responsibility

- `origin` binary: reader pipeline (one or two node directories,
  active-active merge) → `derive::Deriver` (books, checkpoints, bars,
  analytics, coverage) → `bus::BlockPublisher` with `BlockSigner` →
  embedded gateway on `0.0.0.0:8080`, optional JetStream side tap, and live
  capture of the private and filtered raw tiers with the
  `.captured-through` watermark.
- `derive`: the one block-by-block driver of the book builder, used live
  and by archive replay, so derived records are identical on both paths.
- `hooks`: the archive's filters (`hl_node_reader::filter_line`) and parser
  (reader replay + deriver).
- `archive` binary: the archive command line with those hooks wired in.

## Tests

- Unit tests: coverage intervals, derived-record positions, snapshot
  roll-forward and resync on the gap scenario, configuration.
- `tests/e2e.rs`: hl-synth node files written block by block → origin →
  localhost websocket → reference client (books equal the expected books,
  every signature and recomputed root verifies, derived records arrive),
  live capture watermark, then `archive capture run` + `archive replay`
  twice (byte-identical partitions) and the history API returning the live
  trades, bars and books.

## Rules

- Shared types come from `mesh-types`; changes to this crate, `mesh-types`
  or the workspace `Cargo.toml` go through the integrator.
