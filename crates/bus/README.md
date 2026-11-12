# bus

Internal bus client for the bleh.xyz mesh.

**Status:** skeleton.

## Responsibility

Publishes and consumes BlockBatch, record and block-commitment messages on NATS JetStream, one subject per instrument and record kind, with a bounded replay window.

## Rules

- This crate is owned by one build lane. Other lanes do not edit it.
- Shared types come from `mesh-types`. Changes to `mesh-types` or the
  workspace `Cargo.toml` go through the integrator.
- See [ADR 0007](../../docs/decisions/0007-code-layout.md).
