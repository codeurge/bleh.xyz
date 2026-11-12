# archive

Raw archive, partition writer, replay and manifests for the bleh.xyz mesh.

**Status:** skeleton.

## Responsibility

Captures filtered raw node output, writes deterministic daily Parquet partitions in the archive layout, replays raw files into partitions, and builds, signs, anchors and verifies daily manifests.

## Rules

- This crate is owned by one build lane. Other lanes do not edit it.
- Shared types come from `mesh-types`. Changes to `mesh-types` or the
  workspace `Cargo.toml` go through the integrator.
- See [ADR 0007](../../docs/decisions/0007-code-layout.md).
