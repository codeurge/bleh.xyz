# 0017: Archives for poller records

- Status: Accepted
- Date: 2026-11-13
- Deciders: Repository owner

## Context

The swap and reference pollers write daily record files, and
`archive partition-dir` turns settled days into Parquet partitions
([archive.md](../operations/archive.md), "Daily record files"). Both
partitioners wrote into one directory with no archive name, so their days
had no manifest chain: nothing was signed, anchored, or published through
the archive pipeline, which handled only the Hyperliquid archives
([archive-layout-v0](../specs/archive-layout-v0.md) § 1).

Poller days also differ from node days. A reference record is dated by its
publication time, the first fetch of a source can carry earlier issues,
and the New York Fed revises rates for up to ten days. A day is complete
only once every record file that can hold it has been partitioned.

## Decision

1. **Two archives, each with its own manifest chain:**
   `reference-normalized` (reference values and positioning, live pollers)
   and `swaps-normalized` (swap prints). They use the normalized layout of
   archive-layout-v0 § 3. The reference backfill stays a separate archive,
   `reference-backfill`, as the reference-pollers runbook describes.
2. **Partitioned in place.** The partitioners write under
   `<normalized root>/<archive>/`, and the manifests are built there. The
   history API serves the archives together through
   `HISTORY_API_ARCHIVE_NAMES`.
3. **Signed after a lag.** `archive-pipeline.sh records ARCHIVE` signs each
   day after the archive's last manifest once it is `RECORDS_LAG_DAYS`
   (default 14) days old, oldest first, then stamps it with OpenTimestamps,
   publishes it to the public bucket and verifies it over HTTPS. The lag
   must exceed the partitioner's settle days (12 for reference, 2 for swaps).
   A partition that lands in a day already signed is not listed; it is
   logged by `partition-dir` and corrected deliberately (archive-layout § 6).
4. **Notices travel signed.** Before signing, the pipeline copies the
   pollers' `_notices.json` to the archive root, so every manifest carries
   the admissions' notices (ADR 0015).
5. **Signing on the home-archive server** until the node box exists. The
   manifest key lives only there, mode 600, outside the source tree; its
   address is published with the archive.

## Consequences

- Reference and swap history becomes verifiable and mirrorable like the
  Hyperliquid archives: torrent, IPFS and HTTPS from one manifest.
- Days appear in the archive two weeks after their date; the web snapshots
  (ADR 0016) cover the recent days.
- `swaps-normalized` grows by hundreds of megabytes a day, beyond R2's free
  storage within weeks. Publishing it to R2 is a separate decision; it can
  be signed and distributed by torrent and IPFS from the home-archive
  server meanwhile.
- When the node box signs the Hyperliquid archives, the records archives
  can keep their own key or rotate to it (archive.md § 5).
