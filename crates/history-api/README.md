# history-api

Historical query API for the bleh.xyz mesh.

**Status:** skeleton.

## Responsibility

Thin HTTP service over the normalized Parquet partitions: trades, bars, book at time T and book-event ranges, with pagination, size limits and per-client rate limits.

## Rules

- This crate is owned by one build lane. Other lanes do not edit it.
- Shared types come from `mesh-types`. Changes to `mesh-types` or the
  workspace `Cargo.toml` go through the integrator.
- See [ADR 0007](../../docs/decisions/0007-code-layout.md).
