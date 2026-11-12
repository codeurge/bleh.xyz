//! Internal bus client for the bleh.xyz mesh.
//!
//! Skeleton. Publishes and consumes BlockBatch, record and block-commitment messages on NATS JetStream, one subject per instrument and record kind, with a bounded replay window.
//!
//! Owned by one build lane; see `docs/decisions/0007-code-layout.md`.
