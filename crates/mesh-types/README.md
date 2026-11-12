# mesh-types

Shared data model for the bleh.xyz market-data mesh (`observation-v0`).

**Status:** implemented and tested. **Owner:** integrator. Lanes use this
crate but do not edit it; request changes through the integrator (see
[ADR 0007](../../docs/decisions/0007-code-layout.md)).

## Responsibility

One definition of every record, identifier, encoding, hash and signature
used anywhere in the mesh, so the live path and the archive agree byte for
byte. Specified in [observation-v0](../../docs/specs/observation-v0.md)
and [manifest-v0](../../docs/specs/manifest-v0.md).

## Contents

| Module | Key items |
|---|---|
| `decimal` | `Price`, `Size` (i64 mantissa + scale), `parse_exact`, `parse_with_scale`, `rescale`, `checked_add/sub` |
| `time` | `UnixMillis`, `UnixNanos`, RFC 3339 parse and format |
| `seq` | `Seq { block_number, event_index }`, `Seq::end_of_block` |
| `ids` | `InstrumentId` (any venue market, `path_segment` with `#` as `%23`, `venue_wide`), `VenueId`, `AdmissionId`, `MethodId`, `OrderId`, `TradeId` |
| `flags` | `Flag`, `Flags` |
| `provenance` | `Provenance` |
| `record` | `Trade`, `BookEvent` + `BookAction`, `BookSnapshot` + `SnapshotDepth`, `Bar` + `BarInterval`, `Record`, `RecordKind` |
| `derived` | `DerivedHeader`, `Footprint`, `VolumeProfile` |
| `analytics` | `ActivityBar`, `DeltaBar` (`SignedSize`), `ProfileSummary`, `OrderActivity`, `QueuePositions`, `Sweep`, `HeatmapTile`, `LiquidityGrade`, venue `Coverage` |
| `swap` | `SwapPrint` (Part 43 / SBSR: lifecycle actions, event type, option terms, quantities) |
| `reference` | `ReferenceValue` (published rates, yields, prices; `Compounding`, `DayCount`) |
| `synthetic` | `SyntheticMonth` (fair value of a contract month; `SyntheticQuality`, `CarrySource`) |
| `batch` | `BlockBatch` (bus unit), gap detection |
| `registry` | `InstrumentSpec::hyperliquid`, `Registry` (`insert`, `replace`, `remove`) |
| `allowlist` | `check_record`, `check_batch`, `EXCLUDED_SOURCE_FIELDS` |
| `hash` | `canonical_json`, `canonical_record`, `ContentHasher`, `content_hash`, `Sha256Digest` |
| `canonical` | Direct canonical encoders for trades, book events and snapshots (byte-identical to the generic path; used by `canonical_record`) |
| `merkle` | `H256`, `merkle_root`, `MerkleProof` (RFC 9162, Keccak-256) |
| `signing` | `SigningKey`, `EthAddress`, `EthSignature`, `BlockCommitment`, `StreamId` (secp256k1, EIP-191) |
| `manifest` | `Manifest`, `SignedManifest`, `verify_signed`, `verify_link`, `ManifestAnchors` |
| `arrow` | `schema(kind)` |

## Rules

- No floats in canonical data; no `unwrap()` outside tests; no `unsafe`.
- Changing a canonical encoding changes every hash and signature: it needs
  a new major version and a decision record.
- Dependencies: `serde`, `serde_json`, `sha2`, `sha3`, `k256`,
  `arrow-schema`, `thiserror`.
