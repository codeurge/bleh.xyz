# Backfill runbook

How to backfill history from Hyperliquid's public archive buckets with
`tools/hl-backfill`, under
[ADM-0003](../admissions/ADM-0003-hyperliquid-archive-buckets.md). Bucket
layouts, record formats and the list of unverified facts are in
[hyperliquid-archive-buckets.md](../specs/hyperliquid-archive-buckets.md).

## 1. What runs where

```text
requester-pays S3 buckets ──copy──▶ private raw tier (verbatim, R2 or S3)
                                       │
                                     process ──▶ filtered raw (public, zstd JSON lines)
                                       │      └─▶ Parquet partitions, archive hyperliquid-backfill
                                       │          (records, flag backfill, ADM-0003)
                                       │
                                     compare ◀── live-recorded partitions (overlap days)

replica_cmds ──replay-prototype──▶ book events + trades, checked against market_data L2
```

| Stage | Command | Reads | Writes |
|---|---|---|---|
| Coverage and cost | `hl-backfill estimate` | bucket listings | JSON report |
| Copy | `hl-backfill copy` | admitted prefixes | raw tier, checkpoint file |
| Filter and normalize | `hl-backfill process` | raw tier | filtered raw, Parquet partitions, JSON report |
| Overlap check | `hl-backfill compare` | two record sets | JSON report |
| Order-level replay | `hl-backfill replay-prototype` | simplified `replica_cmds` blocks, L2 snapshot records | records, JSON report |

Reports go to stdout as JSON; logs go to stderr (`RUST_LOG=debug` for
more). Exit status: 0 success, 1 the run finished with failures or
mismatches (see the report), 2 the run could not start.

## 2. What the owner provides

1. **An AWS account that pays for requester-pays access.** A dedicated
   account on AWS's Free plan works: requester-pays requests succeed and S3
   answers `RequestCharged: requester`, with the charges drawn from the
   plan's credits. Give the copier an IAM user or role with only this
   policy, which makes the ADM-0003 denylist hold at the AWS level too (a
   `GetObject` on `misc_events_by_block/` or `asset_ctxs/` is refused with
   `AccessDenied`):

   ```json
   {"Version":"2012-10-17","Statement":[
     {"Sid":"ListAdmittedBuckets","Effect":"Allow","Action":"s3:ListBucket",
      "Resource":["arn:aws:s3:::hl-mainnet-node-data","arn:aws:s3:::hyperliquid-archive"]},
     {"Sid":"GetAdmittedPrefixesADM0003","Effect":"Allow","Action":"s3:GetObject",
      "Resource":["arn:aws:s3:::hl-mainnet-node-data/node_fills_by_block/*",
                  "arn:aws:s3:::hl-mainnet-node-data/node_fills/*",
                  "arn:aws:s3:::hl-mainnet-node-data/node_trades/*",
                  "arn:aws:s3:::hl-mainnet-node-data/replica_cmds/*",
                  "arn:aws:s3:::hl-mainnet-node-data/explorer_blocks/*",
                  "arn:aws:s3:::hyperliquid-archive/market_data/*"]}]}
   ```

   Prefer an instance role on the copy host; otherwise export `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and,
   for temporary credentials, `AWS_SESSION_TOKEN`. For a named profile:
   `eval "$(aws configure export-credentials --profile NAME --format env)"`.
2. **The buckets' regions:** `HL_NODE_DATA_REGION=ap-northeast-1` and
   `HL_ARCHIVE_REGION=us-east-1` (checked 2026-11-14; recheck with
   `curl -sI https://<bucket>.s3.amazonaws.com | grep -i x-amz-bucket-region`).
3. **A private raw-tier bucket** on an S3-compatible store (Cloudflare R2
   suggested: no egress fees) and a second, public bucket or prefix for
   filtered raw. Credentials in `BLEH_RAW_ENDPOINT` (for R2,
   `https://<account-id>.r2.cloudflarestorage.com`),
   `BLEH_RAW_ACCESS_KEY_ID`, `BLEH_RAW_SECRET_ACCESS_KEY`, and optionally
   `BLEH_RAW_REGION` (default `auto`).
4. **A copy host in the buckets' region** (a spot instance is enough):
   transfer from S3 to compute in the same region is free, so only request
   charges apply. Copying from anywhere else pays internet egress
   (USD 0.114/GB from Tokyo, 0.09/GB from us-east-1, at list price).
5. **An instrument registry** (`{"instruments": [...]}`, the
   `mesh_types::Registry` format) covering every coin in the period,
   including delisted ones, built from venue metadata.

Credentials are read from the environment only, are never accepted as
arguments, and are never logged.

### Cost controls

Set these up in the AWS account before the first copy, so the most a month
can cost is known in advance:

- **A budget on gross usage, credits excluded** (for example USD 50 a
  month), with email alerts at 50, 80 and 100 % of actual cost and at 100 %
  of forecast cost. On a Free-plan account this measures how fast the
  credits are being used.
- **An automatic budget action at 100 %** that attaches a deny-all IAM
  policy to the backfill user. AWS Budgets runs it through a role trusted
  by `budgets.amazonaws.com`, scoped with `aws:SourceAccount`, that may
  only attach and detach that one policy on that one user. Detach the
  policy by hand to resume.
- **A budget on net cost, credits included** (for example USD 1), alerting
  on any charge above USD 0.01: the first cent that would reach a card.

Billing data lags by several hours, so the cap stops a runaway copy but is
not exact; size each `copy` run from the estimate below.

## 3. Coverage and cost

```bash
export HL_NODE_DATA_REGION=... HL_ARCHIVE_REGION=...
hl-backfill estimate > estimate.json                      # everything admitted
hl-backfill estimate --datasets market_data --from 2023-09-01 --to 2023-09-30 \
  --transfer-usd-per-gib 0                                 # priced for in-region copy
```

Per dataset the report gives objects, bytes, LIST requests, the first and
last day (or block height) present, and the estimated cost. These first
and last days are the coverage record for the import (ADM-0003: coverage
comes from listings). Record them in the coverage map and in
[hyperliquid-archive-buckets.md](../specs/hyperliquid-archive-buckets.md)
§ 5.

Worked example, from the first listing (2026-11-14, spec § 5): the three
trade-history datasets hold 14,460 objects and about 392 GB of LZ4
(`node_trades` 15.1 GB, `node_fills` 31.8 GB, `node_fills_by_block`
345.0 GB). Copied out of Tokyo at USD 0.114/GB that is about **USD 45 once**,
plus well under a dollar of GET and LIST requests. `market_data`,
`replica_cmds` and `explorer_blocks` were not sized yet.

## 4. Copy

```bash
export BLEH_RAW_ENDPOINT=... BLEH_RAW_ACCESS_KEY_ID=... BLEH_RAW_SECRET_ACCESS_KEY=...
hl-backfill copy --raw s3://<raw-bucket>/hl-archive \
  --checkpoint /var/lib/hl-backfill/copy.jsonl \
  --datasets node_fills_by_block,market_data --from 2025-07-27 --to 2025-08-31 \
  --concurrency 32
```

- `BLEH_RAW_ENDPOINT` may be `https://` (R2 or any public store) or, for
  a private store on the LAN or a Tailscale address, `http://`; plain HTTP
  is turned on only for an `http://` endpoint. Requests are path-style.
- **Write probe.** Before reading anything from the source, a run writes,
  reads back and deletes `<raw prefix>/_bleh-write-probe`. If the
  destination cannot be written the run stops with "destination is not
  writable" and no requester-pays request is made.
- Objects land at `<raw prefix>/<source bucket>/<source key>`, byte for
  byte. The destination size is checked after every write.
- **One paid read per object.** Each object is read from the source once:
  into memory up to 64 MiB, above that into a spool file under
  `.copy-spool/` in the working directory (run it on a disk with room for
  `--concurrency` large objects; each spool file is deleted when its object
  is done). Only the upload and its size check are retried, so a
  destination failure never repeats the source download.
- The checkpoint is a JSON-lines log of every copied object (bucket, key,
  size, ETag, destination, SHA-256 of the bytes). Rerunning the same
  command skips objects already copied with the same size and ETag and
  recopies any whose size or ETag changed (reported as
  `recopied_changed`). Keep the checkpoint with the raw tier; it is the
  copy log.
- Source reads and uploads are each retried with exponential backoff
  (`--max-attempts`, default 6). Errors that cannot change on a retry
  (permission denied, bad configuration, 4xx other than 408 and 429) fail
  after one attempt. Objects still failing are listed in the report and
  the exit status is 1; rerun to pick them up.
- `--shard INDEX/COUNT` splits work across hosts by whole days (or
  100,000-block ranges for `explorer_blocks`); give each host its own
  checkpoint file.
- `--dry-run` lists and plans without reading or writing objects.
- **Denylist.** `asset_ctxs` and `misc_events_by_block` are refused at
  three points: listing never builds a prefix under them, every listed key
  is checked, and the copier checks each key again before any request.
  `market_data` types other than `l2Book` are refused the same way.
  Tests assert that no request ever touches an excluded location.

## 5. Process

### 5.0 Registry

`process` needs an instrument registry. Without a node, it is derived from
the admitted fills themselves ([ADR 0019](../decisions/0019-backfill-registry-from-admitted-fills.md),
method `hl-backfill-registry/v3` since [ADR 0022](../decisions/0022-venue-float-noise.md)):

```bash
hl-backfill registry --raw s3://<raw-bucket>/hl-archive \
  --datasets node_fills_by_block --from 2025-07-27 --to 2026-11-14 \
  --out registry.json --concurrency 24 > registry-report.json
```

Objects are fetched, decompressed and parsed in parallel (`--concurrency`,
default 8); per-object observations merge order-independently, so the
registry and report do not depend on the setting.

Objects pass through the same filter as `process`; each coin's class comes
from its symbol, its size scale is the most size decimals seen, and every
price must fit the derived price scale. The report lists every coin (class,
scales, first and last day, trade count) and every coin left out with the
reason; the exit status is 1 if any object failed or any coin was left out.
First run on samples (2026-11-14): 378 perpetuals registered (159 HIP-3);
all 289 outcome coins left out, because their prices have five decimals
and the reader assumed four. ADR 0021 (method v2) gives outcome markets the
spot rule, `8 - szDecimals`, which brings them in.
Full run with v2 (2026-11-15, 11,416 objects, 3.73 billion fill objects,
about 30 minutes at `--concurrency 16` with 16 CPUs): 442 perpetuals and
49,616 outcome coins registered, 0 failed objects; 29 perpetuals left out
because sizes in liquidation fills carried floating-point noise
(`"2946657.1000000001"`). ADR 0022 (v3) rounds such strings to 15
significant digits, here and in the reader.

### 5.1 Run

```bash
hl-backfill process --raw s3://<raw-bucket>/hl-archive \
  --filtered s3://<public-bucket>/hyperliquid-backfill-raw \
  --out-dir ./archives --registry registry.json \
  --datasets node_fills_by_block,market_data --from 2025-07-27 --to 2025-07-27
```

**Splitting a period across runs** (date ranges or `--shard i/n`, whole
days each): each run owns the days of its selected objects and writes
partitions only for those days. Because a day's last records can arrive in
the next day's first hourly file, a run also reads hour 00 of each day
after an owned day that another run owns, and drops records of days it does
not own (`outside_owned_days` in the report). Every day is then written
whole by exactly one run, and the files are byte-identical to a single
serial run. Runs may share one archive root at the same time.

Hour by hour (fills before `market_data` within an hour), each object is
decompressed, filtered, written as filtered raw
(`stream=<dataset>/date=<YYYY-MM-DD>/<HH>.jsonl.zst`; `market_data` adds
`/<HH>/<coin>.jsonl.zst`), parsed to records, stamped with `ADM-0003` and
the `backfill` flag (L2 snapshots also `low_resolution`), validated,
checked against the mesh field allowlist, and written to partitions in
canonical order.

- **Which code reads what.** Objects in the node's `--batch-by-block`
  fills format (`node_fills_by_block`, and `node_fills` when its lines are
  block objects) go through the live node reader: its filter
  (`hl_node_reader::filter_line`) writes the filtered raw, and its parser,
  normalizer and sequencer produce the trades, which are then re-stamped
  as backfill. Backfilled trades are therefore produced by exactly the
  live code path, with the same `seq`, trade IDs, instrument flags and
  `gap_before` rules (a block-number jump between lines or between
  consecutive hourly files of a dataset, or a malformed fill group). The
  buckets carry only the fills stream, so the missing order-status and
  book-diff lines do not count as a gap; a block the node reader rejects
  fails its file. A line that repeats an already sequenced block number
  with no fills, as a restarted node can write, is skipped and counted
  (`repeated_empty_blocks` in the report); a repeated block with fills
  fails its file ([ADR 0024](../decisions/0024-repeated-empty-blocks.md)).
  `node_trades`, `market_data` L2 snapshots and bare
  `node_fills` lines, which the node reader cannot read, use this crate's
  own filter and parser.
- **Field rules.** Fills keep `coin`, `px`, `sz`, `side`, `time`,
  `crossed`, `tid`, `oid`. Only book-matched pairs (one `tid`, two fills,
  opposite sides, exactly one `crossed`) become trades; backstop
  liquidations, no-taker pairs and spot markets are dropped. The node
  reader's filter keeps an unpaired fill's admitted fields so its parser
  can flag the gap (`gap_before`); the local filter drops unpaired fills.
  Owner addresses, fees, PnL, hashes, liquidation objects and client
  order IDs never reach the output.
- **Partitions.** The archive crate's deterministic Parquet writer
  (`docs/specs/archive-layout-v0.md` § 3) writes daily partitions of the
  backfill archive, its own archive per § 1:
  `<out-dir>/hyperliquid-backfill/kind=<kind>/venue=<venue>/instrument=<id>/date=<YYYY-MM-DD>/part-<NNNNN>.parquet`
  (`--archive` changes the name). A day's partitions are written once the
  hour after the day has been processed (records stamped just before
  midnight can arrive in the next day's first file), or at the end of the
  run. Files are write-once: rerunning a day on the same input finds
  byte-identical files (`already_present` in the report); different
  content for an existing file stops the run. Process whole days, and all
  datasets of a day in one run. The report's `partitions` list gives each
  file's path, record count, content hash (`bleh.content_hash`) and
  SHA-256. `--format jsonl` writes canonical JSON lines
  (`records.jsonl` per partition) instead, for debugging.
- **Reading the result.** The history API reads the backfill archive
  directly: point it at `<out-dir>` with `hyperliquid-backfill` among its
  archives (`--archive <out-dir> --archive-name hyperliquid-normalized,hyperliquid-backfill`;
  it unions archives, and reports hours whose records all carry
  `backfill` as `backfilled` in `/v0/coverage`).
- **Blocks for data without them.** L2 snapshots and unbatched fills or
  trades get the last block at or before their time from a clock built
  from batched fill lines (the last two hours are kept), or from
  `--clock-lines`. Times outside the known blocks are counted as
  `unresolved` and skipped, never guessed. Several snapshots that resolve
  to one block keep the last (`superseded`), also across the hourly
  files of a day.
- **Snapshot frequency.** Each hour's report gives
  `snapshot_interval_median_ms`, the measured interval between
  consecutive snapshots in a file, which settles the frequency ADM-0003
  asks for.
- **Failures.** A file that fails any step contributes nothing; the hour
  is listed with `failed` entries and the exit status is 1.
- **Seams.** The filter, parser and partition writer are traits
  (`RawLineFilter`, `RecordParser`, `PartitionWriter`), now wired: the
  binary and `Pipeline::wired` use `DefaultFilter` and `DefaultParser`
  (node reader for by-block fills, local implementations otherwise) and
  `ParquetWriter` (the archive crate's writer). The local implementations
  (`LocalFilter`, `LocalParser`, `JsonlWriter`, `MemoryWriter`) stay
  available for the other formats, debugging and tests.
- **Tests.** `tools/hl-backfill/tests/history.rs` runs the whole path in
  CI with no network: hl-synth's synthetic node output re-framed as LZ4
  bucket objects, `hl-backfill process`, Parquet partitions, and the
  history API, checking trades against hl-synth's ground truth, the
  `ADM-0003` admission and `backfill` flag on every record, coverage, and
  identical partition hashes across two runs.

## 6. Compare against live recording

```bash
hl-backfill compare --backfill ./archives/hyperliquid-backfill \
  --live ./archives/hyperliquid-normalized
```

Both sides may be Parquet partitions (read with the archive crate's
reader, which checks each file's content hash) or JSON-lines files.

For each UTC hour, kind and instrument the tool hashes a comparable
projection of the records (admission set to a placeholder, `backfill` and
`low_resolution` cleared, `ts_ingest_ns` zeroed, because those differ by
construction) and reports `equal`, `different` (with counts and the first
differing position) or one-sided groups. Only hours present on both sides
count towards `matches`. Run it on every overlap day before publishing a
backfilled range; a difference is a review trigger under ADM-0003.

## 7. Order-level replay

### 7.1 Design

Raw book diffs exist only from the project's own nodes. Earlier
order-level history is reconstructed from the L1 transaction log:

1. **Decode** each `replica_cmds` block into book actions: `order`,
   `cancel`, `cancelByCloid`, `modify`, `batchModify`, `scheduleCancel`.
   Recover each signer from the action signature (EIP-712) in memory, and
   map agent wallets and vault actions to their accounts; the account is
   needed for self-trade prevention and client-order-ID cancels and is
   never written out. Map asset indices to instruments with the metadata
   valid at that block (indices change as markets are listed).
2. **Order IDs.** With the `actions-and-responses` style the responses
   carry each order's ID (and fills); otherwise the matcher assigns IDs,
   which then differ from the venue's. Responses are strongly preferred.
3. **Match** in block order through the emulated matcher, emitting book
   events (add, fill, cancel) and trades.
4. **Checkpoint** at every archived L2 snapshot: resolve the snapshot to
   its block (as in § 5) and compare the emulated book's levels with the
   archived ones (price, size and order count per level, to the archive's
   depth).
5. **Publish** a day's replay output only if every checkpoint that day
   matched; otherwise publish nothing for that day and record the first
   mismatch per instrument in the replay report and the coverage map.
6. **Start state.** An L2 snapshot cannot seed an L3 book (it has no
   order IDs or queue order). Replay starts from the earliest available
   block with empty books; resting orders placed before that are unknown,
   so early checkpoints fail until those orders are gone. Days before the
   first matching checkpoint are not published.

### 7.2 Prototype

`hl-backfill replay-prototype` runs steps 3 and 4 on a simplified
representation: JSON lines of `{block_number, block_time_ms, actions}`,
each action `{account, action}` where `action` uses the documented
exchange-endpoint shapes (so a real action object parses directly) and
`account` is an opaque tag. Optional `oids`/`new_oid` carry order IDs
from responses.

```bash
hl-backfill replay-prototype --blocks blocks.jsonl --assets assets.json \
  --checkpoints ./archives/hyperliquid-backfill --out replayed.jsonl
```

`assets.json` maps asset indices to instruments and scales
(`[{"asset": 0, "instrument": "hyperliquid:perp:BTC", "price_scale": 1, "size_scale": 5}]`);
`--checkpoints` takes the L2 `book_snapshot` records `process` wrote
(Parquet or JSON lines).
Output records carry `ADM-0003` and `backfill`; the report lists what was
not emulated and each checkpoint result.

The matcher implements price-time priority per asset; `Gtc` (match then
rest), `Ioc` and front-end market orders (match, drop the remainder),
`Alo` (rejected if it would cross); self-trade prevention by canceling
the resting order of the same account (Hyperliquid's documented
behaviour); cancels by order ID and client order ID, restricted to the
owner; modifies as cancel plus a post-only replacement with a new order
ID at the back of the queue, rejected (old order kept) if the
replacement would cross. Trades execute at the resting price. Tests run
synthetic streams with known outcomes for each rule, plus a generated
stream checking that the book never crosses and that traded size equals
filled size.

### 7.3 Fidelity limits

| Limit | Effect | Mitigation |
|---|---|---|
| Trigger orders (stop, take-profit, TP/SL groupings) fire on the **mark price**, an oracle-derived input ADM-0001 excludes | Triggered orders that then trade are missing; the book diverges | Infer triggers from fills in `node_fills_by_block` where those exist; otherwise the day fails its checkpoints |
| **Liquidations** are protocol-generated orders driven by mark price and margin | Liquidation orders and their book impact are missing | Same as triggers; backstop transfers are not book events anyway |
| **ADL** (auto-deleveraging) | Position transfers with no order-book action; no book effect, but fills in other data may disagree | None needed for the book; excluded from published trades unless book-matched |
| **Margin checks**: orders rejected for insufficient margin, reduce-only rejections, `marginCanceled` / `reduceOnlyCanceled` when an account's margin or position changes | The emulator accepts orders the venue rejected and keeps orders the venue canceled | Needs account state (balances, positions, funding, oracle prices), which the project does not replay; responses (`actions-and-responses`) reveal rejections |
| **Oracle price bands**: orders too far from the reference price are rejected | As above | Responses |
| **TWAP** suborders are generated by the protocol every 30 seconds | Missing orders | Partly visible as fills; not emulated |
| **`scheduleCancel`** (dead man's switch) fires later | Orders stay that the venue canceled | Emulate once the trigger time semantics are confirmed |
| **HIP-3 `haltTrading`** cancels every order and settles at mark | Orders stay | Treat a halt action as cancel-all when its shape is confirmed |
| **Modify semantics** (same or new order ID, queue priority, `always_place`) are not documented | Queue order and IDs may differ | Verify against raw book diffs on an overlap day |
| **Ordering inside a block** (whether the venue processes some action types, such as cancels or post-only orders, before others) | Different matches within one block | Verify on overlap days; reorder before matching if needed |
| **Order IDs and trade IDs** without responses are emulator-assigned; the venue's `tid` hash is undocumented | IDs differ from the venue's | Use responses; trade IDs in replay output are emulator IDs |
| **Agent wallets, vaults and sub-accounts** | Wrong account grouping breaks self-trade prevention and client-order-ID cancels | Replay `approveAgent` and vault actions to map signers to accounts |
| **Signer recovery** | Not implemented in the prototype | k256 recovery over the EIP-712 action hash |
| **Start state** | Orders resting before the first replayed block are unknown | § 7.1 step 6 |

Replay records carry both `backfill` and `replayed` (observation-v0
§ 2.9, bit 6): they are reconstructed by the emulated matcher, not
observed.

## 8. Integration notes

- `process` writes partitions but no manifests: the backfill archive's
  daily manifests, signatures and Iceberg metadata come from the archive
  crate's commands run over the archive root `process` wrote
  (`--archive hyperliquid-backfill`), as for any archive.
- Filtered raw for the backfill is its own archive
  (`archive-layout-v0.md` § 1) under its own prefix, with its own manifest
  chain.
