# book

Order-level books, bars and order-flow analytics for the bleh.xyz mesh.

**Status:** implemented (Phase 1 book-builder, bar-builder,
orderflow-analytics and liquidity-grade; Phase 2 bar rollups).

## Responsibility

- L4 order books from a snapshot plus book events, with L2 levels, top of
  book, typed inconsistency errors, reconciliation against fresh snapshots
  (diff report and resync) and periodic checkpoints for "book at time T".
- Deterministic time bars (1s, 1m) with 5m, 15m, 1h and 1d rollups, and
  tick, volume and range bars.
- Footprint, delta and CVD, volume profile with POC and value area, order
  lifetimes and cancel/replace ratios, queue positions, sweep detection,
  L3 heatmap tiles and a liquidity grade, each with `method@version` and an
  inputs hash.
- A streaming `Processor` (block by block, for the gateway) and a batch
  replay through the same code (for the archive), with identical outputs.

Methods and the runbook: [docs/operations/book.md](../../docs/operations/book.md).

## Rules

- This crate is owned by one build lane. Other lanes do not edit it.
- Shared types come from `mesh-types`. Changes to `mesh-types` or the
  workspace `Cargo.toml` go through the integrator.
- See [ADR 0007](../../docs/decisions/0007-code-layout.md).

## Checks

```bash
cargo fmt --check
cargo clippy -p book --all-targets -- -D warnings
cargo test -p book
cargo bench -p book
```
