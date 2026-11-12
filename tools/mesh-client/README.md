# mesh-client

Reference client for the bleh.xyz gateway.

**Status:** implemented and tested.

## Responsibility

Connects (rotating across gateway URLs), subscribes, keeps a correct L4 book from snapshot plus deltas (or an L2 book from snapshot plus `levels` for `l2` channels), detects gaps and requests re-snapshots, verifies commitment signatures, full-stream roots and sampled inclusion proofs, and prints or records the stream as JSON lines. `mesh-client probe` checks one channel and prints one JSON line for monitoring; `mesh-client keys address` prints the address of a signing-key file. Runbook: [docs/operations/client.md](../../docs/operations/client.md).

## Rules

- This crate is owned by one build lane. Other lanes do not edit it.
- Shared types come from `mesh-types`. Changes to `mesh-types` or the
  workspace `Cargo.toml` go through the integrator.
- See [ADR 0007](../../docs/decisions/0007-code-layout.md).
