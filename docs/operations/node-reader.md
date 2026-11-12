# Node reader runbook

How to run `hl-node-reader` against Hyperliquid nodes, what it emits, what
to watch and what to do when something breaks. The reader implements
[hyperliquid-node-format.md](../specs/hyperliquid-node-format.md) and
[observation-v0.md](../specs/observation-v0.md) under
[ADM-0001](../admissions/ADM-0001-hyperliquid-trades-orders.md); the
service layout is [ADR 0006](../decisions/0006-phase-1-2-service-architecture.md).
Code: [`crates/hl-node-reader`](../../crates/hl-node-reader).

## What it does

```text
node A data dir ─▶ tailer A ─┐
                             ├─▶ merge (dedupe, integrity alarms) ─▶ sequencer ─▶ block batches
node B data dir ─▶ tailer B ─┘                                          │
                                                                        └─▶ checkpoint
```

- Tails the three by-block streams (fills, order statuses, raw book diffs)
  of one or two nodes, woken by filesystem notifications (inotify on
  Linux) with a polling fallback, following hourly rotation.
- Joins the streams per block, pairs fills into book-matched trades,
  classifies book diffs (add, modify, fill, cancel), drops every excluded
  field and every non-book fill, and assigns `seq = (block_number,
  event_index)`.
- With two nodes, publishes each block once and raises an integrity alarm
  on any content mismatch or one-sided gap.
- Emits one `BlockBatch` per instrument, kind and block, with
  `prev_block_number` links and `gap_before` flags.
- Registers markets from node metadata; nothing is hard-coded.
- Serves Prometheus metrics.

## Node requirements

Each node runs with exactly

```text
--write-fills --write-order-statuses --write-raw-book-diffs
--batch-by-block --disable-output-file-buffering --serve-info
```

Never enable `--write-misc-events` or `--write-hip3-oracle-updates`
(ADM-0001 excludes them). The reader must be able to read the node's data
directory (`~/hl/data`), and, for L4 snapshots, read the file the node's
info server writes.

## Running

One node:

```bash
hl-node-reader \
  --node-dir /home/hl/hl/data \
  --info-url http://127.0.0.1:3001/info \
  --registry-out /var/lib/hl-node-reader/registry.json \
  --checkpoint /var/lib/hl-node-reader/checkpoint.json \
  --metrics-addr 0.0.0.0:9184 \
  --log-format json
```

Two nodes (active-active): give both data directories, the first is
reader `a`, the second reader `b`. A reader on another host reads a
replicated or network-mounted data directory; notifications may not work
on network filesystems, so polling takes over automatically.

```bash
hl-node-reader --node-dir /mnt/node-a/hl/data,/mnt/node-b/hl/data ...
```

Replay (read once, no tailing; for tests and the archive's replay path):

```bash
hl-node-reader --replay fixtures/synthetic/hl/base \
  --registry fixtures/synthetic/hl/markets.json \
  --write-filtered /tmp/filtered     # optional: also write filtered raw
```

`--replay` reads raw or filtered raw (same layout) and exits non-zero if
any line could not be parsed.

## Configuration

Every flag has an environment variable.

| Flag | Env | Default | Meaning |
|---|---|---|---|
| `--node-dir` | `HL_NODE_DIRS` | | One or two node data directories (comma-separated) |
| `--replay` | `HL_REPLAY` | | Replay a directory once and exit |
| `--write-filtered` | `HL_WRITE_FILTERED` | | With `--replay`: write the filtered-raw tree here |
| `--registry` | `HL_REGISTRY` | | Registry JSON to start from (`mesh_types::Registry`) |
| `--registry-out` | `HL_REGISTRY_OUT` | | Write the registry here whenever markets are added |
| `--info-url` | `HL_INFO_URL` | | Node info server, for metadata and L4 snapshots (`http://` only) |
| `--info-timeout-ms` | `HL_INFO_TIMEOUT_MS` | 10000 | Info request timeout |
| `--registry-refresh-secs` | `HL_REGISTRY_REFRESH_SECS` | 300 | Periodic metadata refresh (0: only on unknown coins) |
| `--checkpoint` | `HL_CHECKPOINT` | | Checkpoint file |
| `--checkpoint-interval-secs` | `HL_CHECKPOINT_INTERVAL_SECS` | 5 | Minimum time between checkpoint writes |
| `--start` | `HL_START` | `latest-file` | Without a checkpoint: `earliest`, `latest-file` or `end` |
| `--fills-dir` | `HL_FILLS_DIR` | `node_fills_by_block` | Stream directory names (to verify, § 11 item 2) |
| `--order-statuses-dir` | `HL_ORDER_STATUSES_DIR` | `node_order_statuses_by_block` | |
| `--raw-book-diffs-dir` | `HL_RAW_BOOK_DIFFS_DIR` | `node_raw_book_diffs_by_block` | |
| `--no-notify` | `HL_NO_NOTIFY` | off | Poll only |
| `--poll-interval-ms` | `HL_POLL_INTERVAL_MS` | 50 | Poll interval without notifications |
| `--max-lag-blocks` | `HL_MAX_LAG_BLOCKS` | 600 | Release blocks without a stream that lags this far |
| `--gap-wait-ms` | `HL_GAP_WAIT_MS` | 1000 | Two nodes: hold blocks after a hole this long |
| `--confirm-window-blocks` | `HL_CONFIRM_WINDOW_BLOCKS` | 100 | Two nodes: lag before missing copies count as one-sided gaps |
| `--stall-after-secs` | `HL_STALL_AFTER_SECS` | 10 | Two nodes: silence before a node is reported stalled |
| `--snapshot-interval-secs` | `HL_SNAPSHOT_INTERVAL_SECS` | 0 | Request an L4 snapshot this often (0: never) |
| `--snapshot-path` | `HL_SNAPSHOT_PATH` | `/tmp/hl-node-reader-l4.json` | File the node writes the snapshot to |
| `--metrics-addr` | `HL_METRICS_ADDR` | `127.0.0.1:9184` | `/metrics` and `/health` listener |
| `--output` | `HL_OUTPUT` | `stdout` | `stdout` (JSON lines) or `none` |
| `--log-format` | `HL_LOG_FORMAT` | `text` | Logs on standard error; `RUST_LOG` sets the level |

Without `--info-url`, `--registry` is required. With it, the registry is
built from metadata at start-up (the file, if given, is the starting
point).

## Output

JSON lines on standard output, one object per line, tagged by `type`:

- `block`: `{block_number, block_time_ms, gap_before, batches: [BlockBatch]}`.
  One per block, in block order, also for blocks without records.
- `snapshot`: an L4 `book_snapshot` record, written after the block it
  reflects.
- `alarm`: an integrity alarm (below).

Library users take the same events from
`hl_node_reader::pipeline::Pipeline::spawn`. In production the reader runs
inside the origin service ([origin.md](origin.md)), which publishes the
blocks to the embedded gateway and captures raw; the standalone binary is
for tools, replays and debugging.

## Filtered raw

`hl_node_reader::filter::filter_line` turns each raw line into its
filtered-raw line, the public reproducibility root:

- same `--batch-by-block` line format; lines are never dropped from the
  three streams, even when no events remain;
- only admitted fields: fills keep `coin`, `crossed`, `oid`, `px`, `side`,
  `sz`, `tid`, `time` and become the bare fill object instead of
  `[user, fill]`; order statuses keep `time`, `status` and the order's
  `coin`, `side`, `limitPx`, `sz`, `origSz`, `oid`, `timestamp`; diffs keep
  `coin`, `oid`, `px`, `side`, `raw_book_diff`;
- no events of markets outside ADM-0001 (spot);
- no fill groups that are not book trades (backstop liquidations, no-taker
  pairs such as halt settlements, unknown liquidation methods);
- malformed fill groups (an unmatched fill, two crossed fills) are kept
  with admitted fields only, so the `gap_before` they cause is
  reproducible.

Lines of any other directory are dropped (`filter_raw_line`). L4
snapshot files go through `filter_l4_snapshot` (no users, no trigger
orders). Tests check over every fixture and generated scenario that
parsing filtered raw yields exactly the records of the original raw, and
that filtering is idempotent.

## Markets

The registry comes from metadata: `meta` (core perpetuals), `perpDexs`
and `meta` with `dex` (every HIP-3 deployer), `outcomeMeta` (HIP-4). It
refreshes at start-up, when a block has an admitted coin that is not
registered, and every `--registry-refresh-secs`. HIP-3 markets get
`oracle_anchored_venue` (`markets::default_flags`). Spot markets are not
registered: they are outside ADM-0001. A refresh never changes a
registered market: a changed `szDecimals` is logged as a conflict and the
old spec kept (a scale change needs an operator decision and a registry
pull request). `--registry-out` keeps the file for those pull requests.

## Checkpoint and restart

The checkpoint holds, per reader, the hourly file and byte offset of the
first line not covered by a published block, and the sequencer state
(each stream's last block and pending gap flags). It is written
atomically after published blocks, at most every
`--checkpoint-interval-secs`, and on shutdown (SIGINT). On restart each
reader resumes at its position and the published streams continue with
unbroken `prev_block_number` links. Delivery is at least once: blocks
re-read after a crash are dropped by block number.

If the checkpoint's hourly file was pruned, the reader continues at the
next newer file and the jump shows up as a gap. Keep node output at least
as long as the longest expected reader outage.

## Metrics

`GET /metrics` (Prometheus text). `GET /health` returns `ok`.

| Metric | Labels | Meaning | Alert when |
|---|---|---|---|
| `hl_output_last_block` | | Last block published | not increasing for 30 s |
| `hl_output_block_lag_seconds` | | Now minus block time of the last published block | above 5 s |
| `hl_output_gaps_total` | | Blocks published with a gap before them | any increase |
| `hl_reader_last_block` | `reader` | Last block per reader | |
| `hl_reader_block_lag_seconds` | `reader` | Now minus block time, per reader | |
| `hl_reader_emit_lag_seconds` | `reader` | Now minus the node's `local_time` when emitted (reader latency) | above 0.05 s at p99 |
| `hl_reader_blocks_total` | `reader` | Blocks emitted | |
| `hl_reader_records_total` | `reader` | Records emitted | |
| `hl_reader_parse_errors_total` | `reader`, `stream` | Lines that failed to parse | any increase |
| `hl_reader_out_of_order_lines_total` | `reader` | Lines whose block repeated or went backwards | any increase |
| `hl_reader_incomplete_blocks_total` | `reader` | Blocks released without every stream | any increase |
| `hl_reader_rejected_blocks_total` | `reader` | Blocks rejected by normalization | any increase |
| `hl_reader_dropped_fill_pairs_total` | `reader`, `reason` | Non-book fill groups dropped | |
| `hl_reader_fill_anomalies_total` | `reader`, `kind` | Malformed fill groups | any increase |
| `hl_reader_unknown_coin_blocks_total` | `reader` | Blocks with unregistered coins | sustained |
| `hl_reader_unadmitted_events_total` | `reader` | Spot events dropped | |
| `hl_reader_truncated_lines_total` | `reader` | Lines cut off by rotation | any increase |
| `hl_reader_rotations_total` | `reader` | Hourly rotations | none for 2 h |
| `hl_registry_instruments` | | Registered instruments | |
| `hl_registry_refreshes_total` | `result` | Metadata refreshes | `error` sustained |
| `hl_merge_confirmed_blocks_total` | | Blocks both nodes agreed on | not increasing with two nodes |
| `hl_merge_mismatches_total` | | Blocks the nodes disagreed on | any increase |
| `hl_merge_one_sided_gap_blocks_total` | `missing` | Blocks one node lacked | any increase |
| `hl_merge_gap_blocks_total` | | Blocks neither node delivered | any increase |
| `hl_merge_late_blocks_total` | | Copies too late to compare | |
| `hl_merge_alarms_total` | `kind` | Integrity alarms | any increase |
| `hl_checkpoint_writes_total` | `result` | Checkpoint writes | `error` any |

## Integrity alarms

| Alarm | Meaning | Action |
|---|---|---|
| `mismatch` | The two nodes' copies of a block differ (ignoring each node's `local_time` and `gap_before`) | Page. Compare the two nodes' raw lines for the block; check node versions; the published copy is the first node's |
| `one_sided_gap` | One node lacks blocks the other had | Check that node's disk, process and gossip; output is complete |
| `merged_gap` | Neither node delivered the blocks; the output skips them with `gap_before` | Page. Books resync from the next snapshot |
| `reader_stalled` / `reader_recovered` | A node delivered nothing for `--stall-after-secs` / delivers again | Check the node; while stalled, one-sided gaps are counted but not alarmed per block |

## Failure modes

| Symptom | Likely cause | What the reader does | Operator action |
|---|---|---|---|
| Output stops, lag grows | Node stopped writing, or a stream stalled | Waits; after `--max-lag-blocks` releases blocks without the stalled stream as incomplete (gap flags) | Check the node (`visor_child_stderr`); restart it |
| `parse_errors` rising | Node upgrade changed a schema (unknown field or shape) | Drops the line; its block is incomplete; never guesses | Stop publishing that node if persistent; update the spec, generator, fixtures and reader together |
| `rejected_blocks` | Fills of a trade disagree, a decimal does not fit the registered scale, an unknown side, inconsistent block times | Publishes no records for the block, flags the gap | Inspect the block's raw lines; a scale error means the registry or the price-scale assumption is wrong |
| `unknown_coin_blocks` | A new market, metadata not yet refreshed or unreachable | Refreshes metadata (at most every 10 s); drops the events meanwhile | Check `--info-url`; if a new market cannot be registered, register it by hand |
| `fill_anomalies` | An unmatched fill or two crossed fills | No trade for the group; next trade of the instrument has `gap_before` | Investigate the node; report if reproducible |
| `truncated_lines` | Node crashed mid-line before rotating | Skips the partial line (block incomplete) | Check the node's crash |
| Checkpoint write errors | Disk full or permissions | Keeps running; restart would re-read more | Fix the disk |
| Notifications fail | Network filesystem, watch limit | Polls every `--poll-interval-ms` | Raise `fs.inotify.max_user_watches` if desired |
| L4 snapshot errors | Info server down, `--snapshot-path` not shared | Logs and retries next interval | Check `--serve-info` and the path |

## Assumptions to verify on a live node

Each assumption the parser makes from
[the spec's § 11](../specs/hyperliquid-node-format.md) is one named
constant or function in `crates/hl-node-reader/src/assumptions.rs` (or the
place named). When a node shows otherwise, change that name, the spec,
`tools/hl-synth` and the fixtures together.

| § 11 | Assumption | Where |
|---|---|---|
| 1 | Paths `<stream>/hourly/{YYYYMMDD}/{unpadded hour}`, UTC | `HOURLY_SUBDIR`, `parse_date_dir`, `parse_hour_file`, `hourly_rel_path` |
| 2 | Stream directories `node_*_by_block` | `DEFAULT_FILLS_DIR`, `DEFAULT_ORDER_STATUSES_DIR`, `DEFAULT_RAW_BOOK_DIFFS_DIR` (overridable by flag) |
| 3 | Naive UTC `block_time` (ms) and `local_time` (ns); a line for every block, also empty ones | `parse_block_time`, `parse_local_time`, `parse_status_time`, `EMPTY_BLOCK_LINES_WRITTEN`, `is_block_gap` |
| 4 | `side` `B` = buy, `A` = sell (fills, orders); `Bid`/`Ask` in diffs | `fill_side_aggressor`, `order_side`, `diff_side` |
| 5 | Order-status `sz` meaning | Not read: book sizes come only from diffs. `FILLED_STATUS` is the only status value used |
| 6 | Modifies are in-place `update` diffs (or `remove` plus `new`) | `normalize::classify_diff` (fed by `reserve_final_fills`) |
| 7 | Halt-settlement fills are pairs with no `crossed = true` fill; liquidation methods `market` and `backstop` | `crossed_count_is_book_trade`, `LIQUIDATION_METHOD_MARKET`, `LIQUIDATION_METHOD_BACKSTOP` |
| 8 | L4 envelope `{"height": H, "snapshot": [...]}` | `L4_HEIGHT_KEY`, `L4_SNAPSHOT_KEY`, `snapshot::parse_l4_envelope` |
| 9 | HIP-3 uses the perp price rule; outcome price scale 4 and default size decimals 0; outcome coins `#{10n}` and `#{10n+1}` (both registered); spot coins `@N` or `A/B` | `HIP3_USES_PERP_PRICE_RULE`, `OUTCOME_PRICE_SCALE`, `OUTCOME_DEFAULT_SZ_DECIMALS`, `outcome_coins`, `classify_coin` |

Further assumptions of this reader, outside § 11:

- The local info server answers `meta`, `perpDexs`, `meta` with `dex` and
  `outcomeMeta` (`markets::fetch_metadata`; a failing `outcomeMeta` is
  tolerated). The `outcomeMeta` response shape is assumed
  (`markets::parse_outcome_meta`).
- `ts_ingest_ns` of a trade is the fills line's `local_time`; of a book
  event, the diffs line's `local_time`.
- Fill or modify classification follows spec § 6.1: a diff is a fill only
  if a maker fill of the same order with the same size change is pending in
  the block; an order that ends `filled` with a `remove` reserves its last
  fill for that `remove`. Equal size changes of a modify and a later
  partial fill of one order in one block remain ambiguous.
