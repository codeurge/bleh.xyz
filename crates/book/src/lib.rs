//! Order-level book builder and bar builder for the bleh.xyz mesh.
//!
//! Skeleton. Builds L4 order books from a snapshot plus book events, derives L2 levels and top of book, reconciles against fresh node snapshots, and builds deterministic bars (and later footprints and volume profiles) from trades.
//!
//! Owned by one build lane; see `docs/decisions/0007-code-layout.md`.
