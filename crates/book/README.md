# book

Order-level book builder and bar builder for the bleh.xyz mesh.

**Status:** skeleton.

## Responsibility

Builds L4 order books from a snapshot plus book events, derives L2 levels and top of book, reconciles against fresh node snapshots, and builds deterministic bars (and later footprints and volume profiles) from trades.

## Rules

- This crate is owned by one build lane. Other lanes do not edit it.
- Shared types come from `mesh-types`. Changes to `mesh-types` or the
  workspace `Cargo.toml` go through the integrator.
- See [ADR 0007](../../docs/decisions/0007-code-layout.md).
