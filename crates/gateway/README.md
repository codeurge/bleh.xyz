# gateway

Public websocket gateway for the bleh.xyz mesh.

**Status:** skeleton.

## Responsibility

Serves subscriptions by instrument and kind over websocket in JSON and binary encodings: book snapshot plus contiguous deltas, per-block heartbeats and commitments, gap-driven re-snapshot, rate limits, health and freshness endpoints.

## Rules

- This crate is owned by one build lane. Other lanes do not edit it.
- Shared types come from `mesh-types`. Changes to `mesh-types` or the
  workspace `Cargo.toml` go through the integrator.
- See [ADR 0007](../../docs/decisions/0007-code-layout.md).
