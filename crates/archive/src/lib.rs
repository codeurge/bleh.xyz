//! Raw archive, partition writer, replay and manifests for the bleh.xyz mesh.
//!
//! Skeleton. Captures filtered raw node output, writes deterministic daily Parquet partitions in the archive layout, replays raw files into partitions, and builds, signs, anchors and verifies daily manifests.
//!
//! Owned by one build lane; see `docs/decisions/0007-code-layout.md`.
