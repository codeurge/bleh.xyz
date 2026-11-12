# gateway

Public websocket gateway and relay for the bleh.xyz mesh.

**Status:** implemented and tested.

## Responsibility

Websocket gateway implementing [gateway-v0](../../docs/protocol/gateway-v0.md) in JSON and CBOR: subscriptions with wildcards, snapshot plus contiguous deltas, per-block heartbeats and commitments, forced re-snapshots, slow-consumer policy, per-IP and rate limits; relay mode that verifies every commitment before re-serving; `/health`, `/freshness`, `/metrics`, `/proof`, `/signers`; a synthetic demo feed. Runbook: [docs/operations/gateway.md](../../docs/operations/gateway.md).

## Rules

- This crate is owned by one build lane. Other lanes do not edit it.
- Shared types come from `mesh-types`. Changes to `mesh-types` or the
  workspace `Cargo.toml` go through the integrator.
- See [ADR 0007](../../docs/decisions/0007-code-layout.md).
