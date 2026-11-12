# hl-node-reader

Hyperliquid node output reader for the bleh.xyz mesh.

**Status:** skeleton.

## Responsibility

Tails the node's per-block output files, parses them into mesh-types records for every market in the node's metadata, enforces the field allowlist (excluded fields are dropped at parse time), assigns sequence numbers and hands one BlockBatch per instrument, kind and block to the bus.

## Rules

- This crate is owned by one build lane. Other lanes do not edit it.
- Shared types come from `mesh-types`. Changes to `mesh-types` or the
  workspace `Cargo.toml` go through the integrator.
- See [ADR 0007](../../docs/decisions/0007-code-layout.md).
