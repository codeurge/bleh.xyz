# hl-backfill

Backfill from Hyperliquid's public archive buckets (ADM-0003) into the
private raw tier, filtered raw and Parquet partitions, plus an
order-level replay prototype.

Status: built and tested against synthetic data only; no live AWS call
runs in tests. Owned by the backfill lane; see
`docs/decisions/0007-code-layout.md`.

| Module | Responsibility |
|---|---|
| `buckets` | Buckets, admitted datasets, the hard-coded denylist (`asset_ctxs`, `misc_events_by_block`), key parsing |
| `listing` | Listing admitted prefixes by day, height, coin and shard |
| `estimate` | Coverage and requester-pays cost from listings |
| `copy`, `checkpoint`, `retry` | Verbatim, resumable, concurrent copy with backoff |
| `store` | Object stores from the environment (credentials never logged) |
| `codec` | LZ4 frames in, zstd out |
| `filter`, `parse` | Seams (`RawLineFilter`, `RecordParser`); the defaults run the node reader (`hl-node-reader`) on by-block fills and local code on the other formats |
| `partition` | Seam (`PartitionWriter`); the default is the archive crate's deterministic Parquet writer, under the `hyperliquid-backfill` archive |
| `clock` | Time to block for data without block numbers |
| `process` | Raw tier to filtered raw and partitions (`Pipeline::wired` for the defaults) |
| `compare` | Backfill against live recording per hour (Parquet or JSON lines) |
| `replay` | Emulated matcher and L2 checkpoints |

Which implementation handles what:

| Bucket format | Filter | Parser |
|---|---|---|
| `node_fills_by_block`, batched `node_fills` (node `--batch-by-block`) | `hl_node_reader::filter_line` | `hl_node_reader` parse, `normalize`, `Sequencer`; re-stamped `ADM-0003` + `backfill` |
| bare `node_fills`, `node_trades`, `market_data` L2 | `LocalFilter` | `LocalParser` |

Partitions go to
`<out-dir>/hyperliquid-backfill/kind=.../part-NNNNN.parquet`
(archive-layout-v0); `process --format jsonl` writes canonical JSON lines
instead, for debugging. The history API reads the backfill archive
as written (`tests/history.rs` checks this end to end on synthetic data).

Commands: `estimate`, `copy`, `process`, `compare`, `replay-prototype`.
Runbook: `docs/operations/backfill.md`. Bucket layouts:
`docs/specs/hyperliquid-archive-buckets.md`.
