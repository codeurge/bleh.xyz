# bus

In-process bus, block signer and JetStream side tap for the bleh.xyz mesh.

**Status:** implemented and tested.

## Responsibility

In-process broadcast ring from producers to the gateway (per-subscriber cursors, lag detection); the block signer stage (per block and stream Merkle roots over canonical encodings, secp256k1 commitments, key loading and rotation, inclusion-proof store); and the NATS JetStream side tap, one subject per instrument and kind, with a bounded replay window. Runbook: [docs/operations/bus.md](../../docs/operations/bus.md). On-chain verification of its commitments: [crates/contracts](../contracts/README.md).

## Rules

- This crate is owned by one build lane. Other lanes do not edit it.
- Shared types come from `mesh-types`. Changes to `mesh-types` or the
  workspace `Cargo.toml` go through the integrator.
- See [ADR 0007](../../docs/decisions/0007-code-layout.md).
