//! Hyperliquid node output reader for the bleh.xyz mesh.
//!
//! Skeleton. Tails the node's per-block output files, parses them into mesh-types records for every market in the node's metadata, enforces the field allowlist (excluded fields are dropped at parse time), assigns sequence numbers and hands one BlockBatch per instrument, kind and block to the bus.
//!
//! Owned by one build lane; see `docs/decisions/0007-code-layout.md`.
