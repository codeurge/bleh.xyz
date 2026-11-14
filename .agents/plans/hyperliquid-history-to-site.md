# Hyperliquid history to bleh.xyz — Plan

Status: Active
Last updated: 2026-11-14

## Goal

Every byte of Hyperliquid history copied into the private raw tier ends up
usable on bleh.xyz: filtered, normalized, signed and published in the open
archive (HTTPS, BitTorrent, IPFS), queryable in the browser on the site,
and verifiable with `archive verify`, within the free or near-free budget
(owner decision, 2026-11-14: AWS credits capped at $50 a month; Cloudflare
usage alert at $5; R2 beyond the free 10 GB acceptable at a few dollars a
month).

This plan covers the path from raw tier to site. It does not repeat the
Phase 2 tasks it depends on; it links them:
[backfill-fills, backfill-book-snapshots, l3-replay, static-catalog,
coverage-map](phase-2-historical-data.md).

## Where things stand (2026-11-14)

- **Copy (running).** `./backfill.sh copy` on the home-archive server copies
  `node_trades` (2025-03-22 to 2025-06-21), `node_fills` (2025-05-25 to
  2025-07-27) and `node_fills_by_block` (2025-07-27 onward), 14,460 objects,
  about 392 GB of LZ4, verbatim into the private raw tier
  (`s3://bleh-raw/hl-archive/<bucket>/<key>`; ADM-0003). Measured cost about
  $45 of AWS credits ([buckets spec § 5](../../docs/specs/hyperliquid-archive-buckets.md)).
- **Out of budget.** `replica_cmds` (74,586 objects, 89.3 TB, about $10,000)
  and `explorer_blocks` (12.2 million objects, 27.7 TB, about $3,160) can
  only be sampled, so order-level replay ([l3-replay](phase-2-historical-data.md))
  stays a prototype on sample days. `market_data` (L2 snapshots) is still
  being sized.
- **Built.** `hl-backfill process` filters and normalizes all three trade
  datasets: by-block fills through the node reader's live code path, bare
  `node_fills` and `node_trades` through the crate's own filter and parser
  (`tools/hl-backfill/src/parse.rs:1-25`, `process.rs:1-30`). Records carry
  `ADM-0003` and `backfill`, and partitions are the archive crate's
  deterministic Parquet under the `hyperliquid-backfill` archive
  (`partition.rs:37`; [archive-layout-v0](../../docs/specs/archive-layout-v0.md) § 1).
  The records pipeline signs and publishes any archive written under
  `LOCAL_NORMALIZED_DIR/<archive>` (`deploy/scripts/archive-pipeline.sh`
  `records` mode), and is scheduled daily on the home-archive server.

## Gaps found (with evidence)

1. **No instrument registry for the backfill.** `process` requires
   `--registry` (`main.rs:112-114`) and skips, and counts, events whose coin
   is not registered (`parse.rs:69`, `parse.rs:262`, `parse.rs:336`). A
   registry entry needs class, venue symbol, `szDecimals` (size scale) and,
   derived from it, the price scale (`6 - szDecimals` for perpetuals;
   `crates/mesh-types/src/registry.rs:104-183`). Live registration reads
   this from the project's own node's info server
   (`hl-node-reader` `markets`; [registry-v0](../../docs/specs/registry-v0.md)
   § 3). ADM-0001 admits market metadata only from that node, and spot
   markets are not registered at all. There is no node, so the backfill
   has no admitted metadata source today. Hyperliquid's public info API is
   **not admitted**.
2. **Block numbers before 2025-07-27.** `node_trades` and bare `node_fills`
   lines carry no block number. They get the last block at or before their
   time from a clock built from batched fill lines or `--clock-lines`, and
   anything outside the clock's span is counted `unresolved` and skipped,
   never guessed (`clock.rs:1-12`; [backfill.md](../../docs/operations/backfill.md)
   § 5, "Blocks for data without them"). Before by-block fills begin
   (2025-07-27) there are no batched lines, and the only exact sources of
   block times, `explorer_blocks` and `replica_cmds`, are out of budget. So
   about 47 GB of raw (2025-03-22 to 2025-07-26) would produce nothing
   as the method stands. To verify on a sample: whether `node_fills` lines
   are batched (buckets spec § 6 item 3).
3. **The manifest chain only moves forward.** `next.date > prev.date`
   ([manifest-v0](../../docs/specs/manifest-v0.md) § 4). Once the backfill
   archive signs a day, no earlier day can join that chain, so the start
   date must be settled before the first signature.
4. **No bars for backfilled days.** The book crate builds bars from trades
   (`crates/book/src/bars/mod.rs`), but `process` writes trades only.
5. **IPFS lives in the Docker image.** The `ipfs` service keeps its datastore
   in the named volume `ipfs-data` (`deploy/compose/compose.yaml:384-386`).
   On the home-archive server that volume sits inside an 80 GB `docker.img`
   with about 45 GB free, on the cache NVMe that has bad blocks. Pinning
   tens of GB of backfill would fill it and stop every container.
6. **The site shows one archive.** The "Open archive" section hardcodes
   `reference-normalized` (`brand/site/assets/site.js`, `BASE`), and its
   query view loads every file of a day. A Hyperliquid day has a file per
   instrument (hundreds), so the browser needs an instrument filter.
7. **No public history API.** The history API runs on the home-archive
   server's LAN only. The static archive plus DuckDB in the browser need no
   server.

## Owner decisions (resolved 2026-11-14)

The owner approved all four as recommended.

- **D1. Registry source:** derived from the admitted fills themselves, method
  `hl-backfill-registry/v1` ([ADR 0019](../../docs/decisions/0019-backfill-registry-from-admitted-fills.md)),
  implemented as `hl-backfill registry`. No new source.
- **D2. Early period:** the `hyperliquid-backfill` chain starts at 2025-07-27;
  2025-03-22 to 2025-07-26 stays complete in the private raw tier and is
  published later as `hyperliquid-backfill-early` after its own ADR
  ([ADR 0020](../../docs/decisions/0020-hyperliquid-backfill-archive-start.md)).
- **D3. Filtered raw publication:** measure one processed week; publish
  Parquet and filtered raw on R2 if the projected total stays under about
  $3 a month, otherwise filtered raw by torrent and IPFS only. Sample ratios
  (below) project about $1 a month for both, so publishing both is expected.
- **D4. Public API:** static archive with browser queries now; a read-only
  Cloudflare Tunnel to the history API later if there is demand.

## Sample results (2026-11-14)

Run locally on archived samples with `process --format parquet` and the
derived registry (2 hours of `node_fills_by_block` on 2025-07-27 and on
2026-11-13, 1 hour each of `node_fills` 2025-06-01 and `node_trades`
2025-03-22):

| Sample | Raw (LZ4) | Trades | Parquet | Filtered raw (zstd) | Time | Peak memory |
|---|---|---|---|---|---|---|
| By-block 2025-07-27 10–11h | 38.9 MB | 207,217 | 3.7 MB (0.094×), 173 files | 5.7 MB (0.145×) | 20 s | 0.9 GB |
| By-block 2026-11-13 00–01h | 102.9 MB | 492,247 | 8.7 MB (0.085×), 404 files | 14.4 MB (0.140×) | 49 s | 1.6 GB |
| `node_fills` 2025-06-01 00h | 10.9 MB | 0 (61,239 unresolved: no clock) | — | — | 6 s | 0.5 GB |
| `node_trades` 2025-03-22 10h | 1.1 MB | 0 (9,098 unresolved: no clock) | — | — | 0.2 s | 0.07 GB |

- Projection for the 345 GB by-block period: about 29–32 GB of Parquet and
  48–50 GB of filtered raw, about 80 GB in all: roughly $1 a month on R2
  beyond the free 10 GB.
- Throughput on one Apple core is about 2 MB of raw per second, so the
  by-block period is on the order of two CPU-days; measure on the server
  under its 2-CPU cap before planning runs.
- Real data needed three reader fixes before anything processed:
  `cloid` and `priorityGas` (excluded fields, now named in ADM-0001) were
  rejected as schema drift; `block_time` carries nine fractional digits,
  which the reader rejected as sub-millisecond precision; and filtered bare
  `node_fills` lines were mistaken for block lines.
- Outcome markets (`#N`) are not registered: their prices have five
  decimals while the reader assumes four (`OUTCOME_PRICE_SCALE`). Their
  events are counted `unknown_coin` (about 15,000 per hour on 2026-11-13)
  until the assumption is corrected in the reader and the registry method.

## Acceptance criteria

- [ ] All 14,460 trade-history objects are in the raw tier, each with a
      checkpoint entry whose size, ETag and SHA-256 match.
- [ ] A registry covering every coin that traded in the period is published
      at `data.bleh.xyz/_registry/hyperliquid/registry.json` with its method
      (D1), and no event of a registered class is skipped as unregistered.
- [ ] Every day from 2025-07-27 is processed: partitions written, filtered
      raw written, hour reports with zero `failed` and zero `unresolved`
      (or each nonzero count explained in the coverage record).
- [ ] Overlap checks pass: `node_trades` against `node_fills` over
      2025-05-25 to 2025-06-21 and `node_fills` against by-block fills on
      2025-07-27, within the documented tolerances. A rerun of a processed
      day finds byte-identical partitions.
- [ ] Bars for backfilled days exist, built by the same bar method as live
      data, flagged `backfill`.
- [ ] `hyperliquid-backfill` days are signed, hash-chained, timestamped on
      Bitcoin and published to `data.bleh.xyz/hyperliquid-backfill/`, and
      each day passes `archive verify --url`.
- [ ] The site's "Open archive" section lists both archives, filters by
      instrument and charts bars for a chosen instrument and day.
- [ ] The coverage map shows backfilled, missing and early-period hours per
      market ([coverage-map](phase-2-historical-data.md)).
- [ ] The early period is either published as its own archive or recorded
      as raw-only with the reason (D2).
- [ ] Monthly cost stays within the alerts: AWS credits under $50 a month,
      Cloudflare under $5 a month.

## Ordered tasks

1. **finish-trade-copy**
   - Status: In progress
   - Owner: Repository owner
   - Dependencies: [bucket-copy](phase-2-historical-data.md)
   - PR: Not opened
   - Next action: When `copy-trades.done` appears, check 14,460 checkpoint
     entries and 392 GB. Rerun `backfill-reports/run-copy-trades.sh` after
     the home-archive server's maintenance window (Sunday 06:00–06:45 local)
     if it stopped; it resumes from the checkpoint.

2. **verify-formats-on-samples**
   - Status: Done (2026-11-14; see Sample results)
   - Owner: Unassigned
   - Dependencies: finish-trade-copy (one day of each dataset is enough)
   - PR: Not opened
   - Next action: Done for formats (buckets spec § 6 items 3–4). Still to
     do: recheck the buckets' terms (ADM-0003) at the first real import.

3. **backfill-registry** (D1)
   - Status: In progress (`hl-backfill registry` built; ADR 0019)
   - Owner: Unassigned
   - Dependencies: verify-formats-on-samples
   - PR: Not opened
   - Next action: When the copy is complete, run `./backfill.sh registry`
     over `node_fills_by_block` 2025-07-27 to the latest day, review the
     report (violations, coin count), and publish the document under
     `_registry/`. Correct the outcome price scale (reader assumption and
     method v2) to bring outcome markets in.

4. **process-from-2025-07-27**
   - Status: Ready once the copy completes (registry from task 3)
   - Owner: Unassigned
   - Dependencies: backfill-registry,
     [backfill-fills](phase-2-historical-data.md)
   - PR: Not opened
   - Next action:
     1. Mount `LOCAL_NORMALIZED_DIR` into the backfill container
        (`compose.backfill.yaml`).
     2. Process one week with `--out-dir` on that mount and `--filtered`
        to a separate private bucket or prefix.
     3. Measure time, memory under the 6 GB cap, and output sizes; record
        them here and in `costs.md`.
     4. Process the rest in day order, a month per run.

5. **bars-for-backfill**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: process-from-2025-07-27 (first week)
   - PR: Not opened
   - Next action: Write `bar` partitions from backfilled trades with the
     book crate's bar engine, same method version as live, flag `backfill`.
     Check them against a sample of live-method bars on synthetic data.

6. **overlap-verification**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: process-from-2025-07-27; early-period-method for the
     `node_trades` overlap
   - PR: Not opened
   - Next action: Extend `hl-backfill compare` to compare two backfill
     sources. Run `node_fills` against by-block fills on 2025-07-27; rerun
     one day and confirm `already_present` for every partition.

7. **ipfs-datastore-off-docker-img**
   - Status: Ready
   - Owner: Home-archive server operator
   - Dependencies: None
   - PR: Not opened
   - Next action: Move the IPFS datastore from the `ipfs-data` named volume
     to a bind mount on an array or mediacache share, with a size check,
     before any backfill day is pinned. Until then, publish with IPFS
     disabled for this archive.

8. **sign-and-publish-backfill**
   - Status: Blocked (process-from-2025-07-27, ipfs-datastore-off-docker-img)
   - Owner: Unassigned
   - Dependencies: 4, 6, 7; [static-catalog](phase-2-historical-data.md)
   - PR: Not opened
   - Next action: Add `hyperliquid-backfill` to the archive loop in the
     server's `archive-records.sh`. Sign from 2025-07-27, oldest first, and
     publish to `data.bleh.xyz/hyperliquid-backfill/`. Verify each day with
     `archive verify --url`, and record coverage in the buckets spec § 5.

9. **site-multi-archive**
   - Status: Ready
   - Owner: Unassigned
   - Dependencies: sign-and-publish-backfill (first days)
   - PR: Not opened
   - Next action: Generalize the "Open archive" section: an archive picker,
     an instrument filter (the manifest's paths give instruments), and a
     per-instrument query and bar chart over that day's file only.

10. **coverage-page**
    - Status: Ready
    - Owner: Unassigned
    - Dependencies: sign-and-publish-backfill;
      [coverage-map](phase-2-historical-data.md)
    - PR: Not opened
    - Next action: Publish backfilled, missing and early-period hours per
      market as a static file next to the manifests, and show it on the
      site.

11. **early-period-method** (D2)
    - Status: Ready (ADR 0020 fixes the split; the sequencing ADR is next)
    - Owner: Unassigned
    - Dependencies: verify-formats-on-samples
    - PR: Not opened
    - Next action: Draft the ADR for sequencing block-less backfill trades
      as a new method version, then process and publish
      `hyperliquid-backfill-early` as its own chain.

12. **book-snapshots-next-month**
    - Status: Blocked (market_data size, credit budget)
    - Owner: Unassigned
    - Dependencies: [backfill-book-snapshots](phase-2-historical-data.md)
    - PR: Not opened
    - Next action: When the `market_data` listing finishes, cost it per
      month and per coin. Copy within a later month's $50 of credits, major
      markets first.

## Timeline (rough, from 2026-11-14)

| When | Milestone |
|---|---|
| +1 day | Trade-history copy complete and verified |
| +2–3 days | Formats verified, registry decided and published, one week processed and measured |
| +4–7 days | 2025-07-27 onward processed; bars; overlap checks |
| +1–2 weeks | First Hyperliquid days signed and on the site; coverage page |
| Later | Early period (after its ADR); book snapshots (credit-limited) |

Processing speed is unmeasured. The whole span depends on CPU time for
decompressing and parsing 345 GB under a 2-CPU, 6 GB cap; the first
processed week sets the estimate.

## Costs (estimates until measured)

| Item | Estimate | Basis |
|---|---|---|
| AWS: trade-history copy | about $45 once, from credits | Measured listing at $0.114/GB from Tokyo |
| AWS: `market_data` | to be measured | Listing pending; a later month's credits |
| R2: normalized Parquet | about 30–60 GB, $0.30–0.75 a month above the free 10 GB | Rough: about 3–4 million trades a day at 15–25 bytes each in compressed Parquet; measure on one week |
| R2: filtered raw (if published, D3) | about 100–150 GB, $1.50–2.25 a month | Rough: excluded fields removed, zstd instead of LZ4; measure on one week |
| R2: uploads | Within the free 1 million Class A operations | About 100–200 thousand partition files once |
| R2: downloads | $0 | R2 has no egress charge |
| Home-archive server | $0 (electricity already spent) | Raw tier on the array; processing under the 6 GB cap |
