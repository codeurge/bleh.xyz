# mesh-client

Reference client for the bleh.xyz gateway.

**Status:** skeleton.

## Responsibility

Connects to a gateway, subscribes, keeps a correct local book from snapshot plus deltas, detects gaps and requests re-snapshots, and optionally verifies block commitments and inclusion proofs.

## Rules

- This crate is owned by one build lane. Other lanes do not edit it.
- Shared types come from `mesh-types`. Changes to `mesh-types` or the
  workspace `Cargo.toml` go through the integrator.
- See [ADR 0007](../../docs/decisions/0007-code-layout.md).
