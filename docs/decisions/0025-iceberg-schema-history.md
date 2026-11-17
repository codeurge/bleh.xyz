# 0025: Iceberg tables keep a schema history

- Status: Accepted
- Date: 2026-11-16
- Deciders: Repository owner (agent working directly for the owner)
- Amends: [0023](0023-reference-quantities.md) § 4 ("the Iceberg metadata
  is rebuilt from scratch with the current schema") and
  [archive-layout-v0](../specs/archive-layout-v0.md) § 3.2–3.3; data
  files and records do not change

## Context

The archive rebuilds each kind's static Iceberg metadata from the signed
day manifests on every publish. Every metadata file except
`version-hint.text` is write-once, which works because the rebuild is a
pure function of the manifests.

ADR 0023 appended a nullable `unit` column to `reference_value` and said
the metadata would be rebuilt with the current schema. The table schema is
embedded in every data manifest (`<snapshot>-m0.avro`) and every
`v<N>.metadata.json`. So rebuilding with the new schema changed the bytes
of the 36 files already published for `reference-normalized`. From
2026-11-15 the daily publish stopped at the first of them with a
write-once conflict. Nothing was overwritten, but no new reference day was
published.

## Decision

1. Each kind's table has a **schema history**: version 0 is the schema as
   first published, and each later appended column starts a new version.
   The archive code lists the changes in order, each with the time it took
   effect. That time is when the change was committed: `reference_value.unit`
   took effect at 2026-11-15T06:15:32Z (commit f8df65c). The list is append
   only.
2. A snapshot uses the schema version in effect at its commit time, which
   is its manifest's `created_ns`. Its data manifest and metadata version
   carry that `schema-id`. `schemas` lists versions 0 to that one, and
   `current-schema-id` names it. This is ordinary Iceberg schema
   evolution: earlier snapshots keep the schema they were written with.
3. Readers are unaffected. Iceberg resolves columns by field ID, and a field
   ID missing from a data file reads as null, whichever schema version a
   manifest names. A snapshot committed after a change may list files
   written before it. Those files lack the new column, which reads as null
   as before.

## Consequences

- The rebuild stays a pure function of the manifests, and files published
  before a schema change regenerate byte for byte. Before deploying, the
  36 published `reference-normalized` files were rebuilt and compared with
  the published copies on skullkid and on `data.bleh.xyz`: all 36 sha256
  matched. The next snapshot (v13) carries schema 1 with `unit`.
- A future appended column needs one new entry in the list, with the
  commit time it takes effect. Forgetting it now fails loudly, with a
  write-once conflict at the next publish, rather than silently.
- A mirror that generates metadata for its own location gets the same
  history.

## Alternatives considered

- **Rewrite the published metadata with the current schema.** This
  breaks write-once and the promise that a published file never changes.
- **Version the metadata paths by schema.** Readers find a table through
  `metadata/version-hint.text` and `v<N>.metadata.json`, so the paths
  can't carry a schema fingerprint without moving the table.
- **Read each data file's Parquet footer to choose its schema.** This
  matches files exactly but makes every publish read every footer of the
  archive. Readers don't need it, because they resolve by field ID.
