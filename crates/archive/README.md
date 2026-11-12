# archive

Raw archive, partition writer, replay and manifests for the bleh.xyz mesh.

**Status:** implemented against synthetic fixtures; the node reader's
filter and parser are wired in by the integrator. Runbook:
[docs/operations/archive.md](../../docs/operations/archive.md).

## Responsibility

- **Capture** (`capture`): two hourly-rotated tiers fed by raw node lines:
  the private tier (verbatim, zstd with long-distance matching and optional
  dictionaries) and the filtered public root (the injected `LineFilter`,
  pinned single-frame zstd). Uploads go to S3-compatible storage
  write-once (`store`).
- **Partitions** (`partition`): deterministic daily Parquet per
  [archive-layout-v0](../../docs/specs/archive-layout-v0.md) for every
  record kind, read back and checked against the canonical content hash;
  static Iceberg v2 metadata (`partition::iceberg`).
- **Replay** (`replay`): the only path from filtered raw to partitions,
  through the injected `RawParser`.
- **Manifests** (`manifest`, `ipfs`, `torrent`, `digest`): per-file
  SHA-256, content hash, CIDv1 (UnixFS, raw leaves) and BitTorrent v2
  pieces root; the day's v2 torrent and magnet; secp256k1 signatures and
  the hash chain per [manifest-v0](../../docs/specs/manifest-v0.md).
- **Anchors** (`anchor`): OpenTimestamps (calendars, upgrade, Bitcoin
  header checks) and an Ethereum L2 anchor transaction behind the
  `AnchorSubmitter` trait (dry-run and JSON-RPC implementations).
- **Publishing and verification** (`publish`, `verify`): uploads, Kubo
  pinning (`IpfsPinner`), and mirror verification from a directory or URL.
- **Binary** `archive` (`cli`): `capture`, `partition`, `replay`,
  `iceberg`, `manifest`, `anchor`, `publish`, `verify`.

Every network call goes through `http::HttpClient`, so tests run offline
with `http::MockHttp`.

## Rules

- This crate is owned by one build lane. Other lanes do not edit it.
- Shared types come from `mesh-types`. Changes to `mesh-types` or the
  workspace `Cargo.toml` go through the integrator.
- See [ADR 0007](../../docs/decisions/0007-code-layout.md).
