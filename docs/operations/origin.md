# Origin runbook

How to run `origin`, the process that turns Hyperliquid node files into the
public signed live stream and the archive's raw tiers. Design:
[ADR 0006](../decisions/0006-phase-1-2-service-architecture.md); code:
[`crates/origin`](../../crates/origin). The parts have their own runbooks:
[node-reader](node-reader.md), [book](book.md), [bus](bus.md),
[gateway](gateway.md), [archive](archive.md).

## What it does

```text
node dir A ─┐
            ├─▶ reader pipeline ─▶ Deriver ─▶ BlockPublisher + BlockSigner ─▶ ring ─▶ gateway :8080
node dir B ─┘   (tail, merge,      (books,      (one commitment per block   │         /v0/ws /health
                 sequence,          bars,        and stream; proofs)        │         /freshness /metrics
                 checkpoint)        analytics,                              │         /proof /signers
                                    coverage)                               └─▶ JetStream side tap (optional)

node dir A ─▶ live capture ─▶ <BLEH_RAW_DIR>/{hyperliquid-raw-private,hyperliquid-raw}/ready/ ─▶ R2 (optional)
                              <BLEH_RAW_DIR>/.captured-through
```

Per block, in this order: the block's `trade` and `book_event` batches; the
derived records the block completed (`bar`, `footprint`, `delta_bar`,
`volume_profile`, `profile_summary`, `activity_bar`, `order_activity`,
`queue_positions`, `sweep`, `heatmap_tile`, `liquidity_grade`, book
checkpoints as `book_snapshot`, and `coverage` when an interval closed);
book resyncs for books that were just initialised or reset (outside the
signed stream); then `end_block` signs one commitment per stream and the
gateway sends the heartbeat. Derived records carry `seq = (block, i)`
(observation-v0 § 2.8), so they are signed in the block that produced them
and the archive replay reproduces them exactly.

## Running

```bash
origin \
  --node-dir /hl/data \
  --info-url http://127.0.0.1:3001/info \
  --registry-out /var/lib/bleh/reader/registry.json \
  --checkpoint /var/lib/bleh/reader/checkpoint.json \
  --snapshot-interval-secs 600 --snapshot-path /snapshots/l4.json \
  --signing-key-file /run/secrets/signing_key \
  --raw-dir /var/lib/bleh/raw --capture-upload \
  --capture-metrics-out /textfile/bleh_archive_capture.prom \
  --nats-url nats://127.0.0.1:4222 \
  --metrics-addr 10.77.0.1:9184 --log-format json
```

Two nodes: `--node-dir /mnt/a/hl/data,/mnt/b/hl/data` (or `HL_NODE_DIRS`).
Capture follows the first directory.

Synthetic, no node (laptop): write `fixtures/synthetic/hl/base` into a
directory as the node would and run with
`--registry fixtures/synthetic/hl/markets.json --start earliest
--book-start empty --signing-key-file <test key>`; `crates/origin/tests/e2e.rs`
does exactly this.

## Configuration

Reader settings keep the `hl-node-reader` names, so one `.env` serves both
(see [node-reader.md](node-reader.md#configuration)): `HL_NODE_DIRS`,
`HL_REGISTRY`, `HL_REGISTRY_OUT`, `HL_INFO_URL`, `HL_INFO_TIMEOUT_MS`,
`HL_REGISTRY_REFRESH_SECS`, `HL_CHECKPOINT`, `HL_CHECKPOINT_INTERVAL_SECS`,
`HL_START`, the stream directory names, `HL_NO_NOTIFY`,
`HL_POLL_INTERVAL_MS`, `HL_MAX_LAG_BLOCKS`, `HL_GAP_WAIT_MS`,
`HL_CONFIRM_WINDOW_BLOCKS`, `HL_STALL_AFTER_SECS`,
`HL_SNAPSHOT_INTERVAL_SECS`, `HL_SNAPSHOT_PATH`, `HL_METRICS_ADDR`,
`HL_LOG_FORMAT`.

| Flag | Env | Default | Meaning |
|---|---|---|---|
| `--signing-key-file` | `BLEH_SIGNING_KEY_FILE` | required | Block signing key: 64 hex digits, optional `0x` |
| `--signing-key-id` | `BLEH_SIGNING_KEY_ID` | `origin` | Its ID in `/signers` |
| `--signing-key` | `BLEH_SIGNING_KEYS` | | Further keys `ID=file:PATH` (rotation, [bus.md](bus.md#keys)) |
| `--active-key` | `BLEH_ACTIVE_KEY` | first key | Key that signs |
| `--gateway-listen` | `BLEH_GATEWAY_LISTEN` | `0.0.0.0:8080` | Gateway (overlay only by firewall) |
| `--trust-forwarded-for` | `BLEH_GATEWAY_TRUST_FORWARDED_FOR` | off | Behind a trusted proxy only |
| `--proof-retention-blocks` | `BLEH_GATEWAY_PROOF_RETENTION_BLOCKS` | 100000 | Proof trees kept per stream |
| `--max-staleness-secs` | `BLEH_GATEWAY_MAX_STALENESS_SECS` | 5 | `/health` fails after this long without a block |
| `--nats-url` | `BLEH_NATS_URL` | | JetStream side tap; its window is replayed into the gateway on start |
| `--nats-stream`, `--nats-replay-window-secs`, `--nats-credentials` | `BLEH_NATS_STREAM`, `BLEH_NATS_REPLAY_WINDOW_SECS`, `BLEH_NATS_CREDENTIALS_FILE` | `BLEH_V0`, 600 | |
| `--raw-dir` | `BLEH_RAW_DIR` | | Enables live capture; staging and watermark directory |
| `--no-capture` | `BLEH_CAPTURE_DISABLED` | off | Keep `BLEH_RAW_DIR` set but do not capture |
| `--capture-no-private` | `BLEH_CAPTURE_NO_PRIVATE` | off | Filtered tier only |
| `--capture-upload` | `BLEH_CAPTURE_UPLOAD` | off | Upload sealed hours write-once (stores from `BLEH_ARCHIVE_{PRIVATE,PUBLIC}_*`, [archive.md](archive.md#4-object-storage)) |
| `--capture-metrics-out` | `BLEH_CAPTURE_METRICS_OUT` | | Archive counters as a node-exporter textfile, rewritten every `BLEH_CAPTURE_METRICS_EVERY_SECS` (60) |
| `--capture-seal-grace-secs` | `BLEH_CAPTURE_SEAL_GRACE_SECS` | 600 | Seal an hour this long after it ended if a stream stopped |
| `--book-start` | `BLEH_BOOK_START` | `snapshot` | `snapshot`: books wait for the first L4 snapshot; `empty`: books start empty (synthetic data) |
| `--book-checkpoint-secs` | `BLEH_BOOK_CHECKPOINT_SECS` | 600 | Book checkpoint interval (block time); the archive replay must use the same value |
| `--coverage-interval` | `BLEH_COVERAGE_INTERVAL` | `1m` | Coverage record interval |
| `--recent-blocks` | `BLEH_RECENT_BLOCKS` | 600 | Blocks of book events kept to roll L4 snapshots forward |

## Books and snapshots

In production books start unknown (`snapshot`): trade analytics run at
once, book analytics, checkpoints and gateway snapshots begin with the first
L4 snapshot (`HL_SNAPSHOT_INTERVAL_SECS`, default 600 in compose). A
snapshot is usually older than the processed blocks by the time it is
read; the origin applies the buffered book events after its height
(`BLEH_RECENT_BLOCKS`) and hands the result to the book builder, which
initialises, reconciles or resets the book. A book that was initialised or
reset is published as a resync, so gateways reseed at once instead of
waiting for the next checkpoint.

## Live capture

Capture follows the node's hourly files directly (not the reader's
records), so the raw tiers hold exactly what the node wrote. An hour is
sealed when every stream has a later file, or `BLEH_CAPTURE_SEAL_GRACE_SECS`
after it ended for a stopped node. `.captured-through` names the last hour
sealed in every stream and (with upload) in object storage; the node
pruner deletes node output only up to it. Details:
[archive.md](archive.md), "Live capture".

## Metrics

`HL_METRICS_ADDR` (`/metrics`, `/health`): every reader metric
([node-reader.md](node-reader.md#metrics)) plus:

| Metric | Meaning | Alert when |
|---|---|---|
| `bleh_book_reconcile_mismatch_total` | Snapshots that differed from the book (book reset) | any increase |
| `bleh_book_reconciliations_total` | Snapshots compared | not increasing with snapshots on |
| `bleh_book_desyncs_total{reason}` | Books that desynchronised (`gap`, `book`) | sustained |
| `bleh_book_snapshots_unusable_total` | Snapshots too old, stale or inconsistent | sustained |
| `bleh_book_process_errors_total` | Book builder input errors | any increase |
| `bleh_signing_failures_total` | Blocks not signed or published | any increase |
| `bleh_publish_errors_total` | Batches or records rejected by the publisher | any increase |
| `bleh_derived_records_total{kind}` | Derived records published | |
| `bleh_origin_blocks_total`, `bleh_origin_last_block` | Blocks published | last block not increasing for 30 s |
| `hl_reader_snapshots_total`, `hl_reader_snapshot_failures_total` | `fileSnapshot` requests | failures sustained |

Gateway, bus and signer metrics are on the gateway's `/metrics` (`:8080`,
[gateway.md](gateway.md#metrics)). Capture counters
(`archive_capture_*`, `archive_store_put_*`) are in the capture textfile.

## Shutdown

SIGTERM or SIGINT: the reader drains and writes its checkpoint, gateway
connections close with 1001, capture keeps the open hour (a restart
continues it). The process exits within 25 seconds. A fatal error in any
part stops the whole process with a non-zero status, for the orchestrator
to restart.

## Failure modes

| Symptom | Likely cause | Action |
|---|---|---|
| No gateway snapshots for a market | No L4 snapshot yet (`snapshot` start) or snapshots unusable | Check `HL_INFO_URL`, `HL_SNAPSHOT_PATH` shared with the node, `bleh_book_snapshots_unusable_total`; raise `BLEH_RECENT_BLOCKS` if snapshots arrive late |
| `bleh_book_reconcile_mismatch_total` rising | Reader misclassification or a node issue | Compare with the reader's alarms; the book was reset to the snapshot |
| `bleh_signing_failures_total` | Key file unreadable after rotation, or blocks out of order | Check the key mount; restart |
| Watermark not advancing | Upload failing (`archive_capture_upload_errors_total`), a write-once conflict, or a stream stopped | See [archive.md](archive.md); conflicts are kept under `conflicts/` |
