//! Historical query API for the bleh.xyz mesh.
//!
//! Skeleton. Thin HTTP service over the normalized Parquet partitions: trades, bars, book at time T and book-event ranges, with pagination, size limits and per-client rate limits.
//!
//! Owned by one build lane; see `docs/decisions/0007-code-layout.md`.
