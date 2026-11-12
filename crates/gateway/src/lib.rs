//! Public websocket gateway for the bleh.xyz mesh.
//!
//! Skeleton. Serves subscriptions by instrument and kind over websocket in JSON and binary encodings: book snapshot plus contiguous deltas, per-block heartbeats and commitments, gap-driven re-snapshot, rate limits, health and freshness endpoints.
//!
//! Owned by one build lane; see `docs/decisions/0007-code-layout.md`.
