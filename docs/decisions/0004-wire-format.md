# 0004: DBN compatibility layer with provenance carried alongside

- Status: Accepted; planned, Phase 2 (compatibility layer). The historical
  endpoints are built; a live DBN encoding is not.
- Date: 2026-11-11 (revised 2026-11-13)
- Deciders: Maintainers

## Context

Many market-data tools already read Databento's DBN binary format, through
the Apache-2.0 [`dbn`](https://github.com/databento/dbn) crate and
Databento's client libraries. If the mesh serves DBN behind an API shaped
like Databento's, those consumers can switch with little work.

DBN's fixed record layouts cannot carry the mesh's provenance: the
admission ID, flags, the `derived` header, the full `(block_number,
event_index)` sequence, or the per-block signatures and Merkle proofs of
[observation-v0](../specs/observation-v0.md) § 7. They also hold sizes as
integers, so fractional sizes need a convention.

The first version of this record proposed DBN as a core wire format.
Phases 1 and 2 instead built JSON and CBOR on the gateway and Arrow and
Parquet in the archive ([ADR 0006](0006-phase-1-2-service-architecture.md),
[architecture](../architecture.md) § 1), and those carry every field.

## Decision

1. **Core encodings stay JSON and Arrow.** In Phases 1 and 2 the data
   model is defined by [observation-v0](../specs/observation-v0.md), its
   JSON Schema and its Arrow schema. The gateway speaks JSON and CBOR
   ([gateway-v0](../protocol/gateway-v0.md)); the archive is Parquet;
   the history API's native endpoints (`/v0/...`) return JSON. Canonical
   encodings, content hashes and signatures are defined over the JSON form
   only.
2. **DBN is a compatibility layer, planned for Phase 2.** It sits behind
   a Databento-shaped API and is derived from the core records, never the
   other way round. Nothing is archived, hashed or signed in DBN.
3. **Provenance and signatures are carried alongside, not inside.** DBN
   records keep their standard layouts; no extension record type is
   defined, so every existing DBN reader works unchanged. A consumer who
   needs provenance gets the same records with every field from the
   native endpoints (`/v0/...`), checks archive files against the signed
   manifests ([manifest-v0](../specs/manifest-v0.md)), and checks live
   blocks against the gateway's commitments and proofs. Per-response facts
   that DBN cannot hold travel in `X-Bleh-*` response headers (for
   example `X-Bleh-Size-Scales`).
4. **Mapping.** Venues get bleh-chosen dataset and publisher IDs from a
   private range (`BLEH.HL`, publisher 45001 for Hyperliquid), not
   assigned by Databento. The mapping of kinds to DBN schemas, the
   lossless default for sizes (`size_units=lots`) and the lossy points
   (block number modulo 2^32, no `event_index`) are documented in the
   [history API runbook](../operations/history-api.md#databento-shaped-historical-api-dbn).

### State on 2026-11-13

- Built: the historical endpoints in `crates/history-api`
  (`/databento/v0/timeseries.get_range` with `dbn`, `json` and `csv`
  encodings, and `metadata.list_datasets`, `metadata.list_schemas` and
  `metadata.list_publishers`), for the `trades`, `mbo`, `mbp-1`,
  `mbp-10` and `ohlcv-1s/1m/1h/1d` schemas, encoded with the `dbn`
  crate (DBN version 3).
- Not built: a live DBN encoding (`bleh.v0.dbn` on the gateway) and the
  remaining DBN schemas. Both are tracked by `dbn-compat-api` in the
  [Phase 2 plan](../../.agents/plans/phase-2-historical-data.md).

## Resolved from the first version

1. **Licences.** The `dbn` crate is Apache-2.0, compatible with this
   repository. The mesh writes its own data in the format; no Databento
   data or service is used.
2. **Extension record type.** Not pursued: provenance travels alongside
   (decision 3), so existing readers need no changes.
3. **No affiliation.** Dataset and publisher IDs are bleh-chosen, and the
   runbook states that no affiliation or endorsement is implied.

## Consequences

- Tools that read DBN can load mesh history without new code.
- DBN output is lossy where the runbook says so; anything exact uses the
  native endpoints or the archive.
- A change to the DBN mapping is a change to the history API, not to the
  data model or the wire format, and needs no new version of
  observation-v0.

## Alternatives considered

- **DBN as the core wire format:** existing readers, but no room for
  provenance and signatures, and integer sizes.
- **A DBN extension record type for provenance:** keeps everything in one
  stream, but existing DBN readers may reject or misread it.
- **A bespoke binary format:** full control, but no existing readers.
- **Arrow only:** good for analytics, weak for low-latency streaming.
