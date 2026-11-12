//! Reference client for the bleh.xyz gateway.
//!
//! Skeleton. Connects to a gateway, subscribes, keeps a correct local book from snapshot plus deltas, detects gaps and requests re-snapshots, and optionally verifies block commitments and inclusion proofs.
//!
//! Owned by one build lane; see `docs/decisions/0007-code-layout.md`.
