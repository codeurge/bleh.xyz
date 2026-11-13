# 0016: Web snapshots for static sites

- Status: Accepted
- Date: 2026-11-12
- Deciders: Repository owner

## Context

The project site is static (`brand/site`, served by a static host). The
self-hosted server runs the swap and reference pollers, which write daily
record files, but nothing published what they collect: the history API
serves trade and book data only, and the signed archive publishes settled
days, not the latest values.

[Hosting](../operations/hosting.md) requires that users never connect to a
home server. A site that called the history API on the home server, even
through a tunnel, would make that server the site's backend.

## Decision

1. **A new published format, `bleh-web-snapshot/v0`**, specified in
   [web-snapshot-v0](../specs/web-snapshot-v0.md): a `latest.json` index and
   one small JSON file per source (reference series, positioning report
   family, swap repository). Files carry admitted fields as the records give
   them, plus each source's admission ID and notice. The only derived
   numbers are swap print counts per day and asset class.
2. **`archive snapshot` builds them**, reading the pollers' daily record
   files incrementally. It writes the files locally and, when
   `BLEH_ARCHIVE_WEB_*` storage is configured, uploads changed files to the
   public bucket under `web/v0/`, `latest.json` last. The home server only
   connects outward.
3. **Snapshots are mutable and short-lived in caches.** The edge caches
   `/web/` for 60 seconds (browsers 30), unlike write-once archive files.
   Snapshots are not part of the signed archive and carry no signature;
   anything needing proof uses the archive's signed days.
4. **Notices travel with the data.** Every source file and the index carry
   the notice the admission requires
   ([ADR 0012](0012-open-terms-route-for-public-bodies-and-market-operators.md)
   decision 5), and a site showing a source's values must show its notice
   with them.
5. **Bounded size.** Daily reference series keep a bounded history (260
   points by default). Intraday series (grid intervals) keep only their
   latest value in v0. Positioning keeps the latest and previous report per
   market. Swap files keep recent prints per asset class and daily counts.

## Consequences

- The site needs no backend and no secrets; any static host or mirror can
  serve it, and anyone can mirror `web/v0/`.
- A new format to keep stable: changes that remove or rename fields need
  `v1` beside `v0`.
- Snapshot freshness depends on the home server; `latest.json` carries
  `generated_ms` and each source's `last_record_ms` so readers can show
  staleness.
- Settled days still reach the public bucket only through the signed
  archive pipeline (`archive manifest` and `archive publish`); this job does
  not upload partitions.
