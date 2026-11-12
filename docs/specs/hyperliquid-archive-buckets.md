# Hyperliquid archive buckets

- Status: Draft. Facts marked **unverified** come from secondary sources
  or inference and must be checked against a real listing (or a downloaded
  object) before a backfill run relies on them. `tools/hl-backfill` takes
  every unverified layout detail from code that is isolated and tested
  against synthetic objects, so a correction changes one parser.
- Admission: [ADM-0003](../admissions/ADM-0003-hyperliquid-archive-buckets.md)
  (field rules from [ADM-0001](../admissions/ADM-0001-hyperliquid-trades-orders.md)).
- Researched: 2026-11-12.

## 0. Sources

| Ref | Source | Kind |
|---|---|---|
| S1 | Hyperliquid docs, Historical data: <https://hyperliquid.gitbook.io/hyperliquid-docs/historical-data> | Official |
| S2 | Hyperliquid node README: <https://github.com/hyperliquid-dex/node> (Apache-2.0) | Official |
| S3 | Hyperliquid docs, L1 data schemas: <https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/nodes/l1-data-schemas> | Official |
| S4 | Hyperliquid docs, Exchange endpoint: <https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/exchange-endpoint> | Official |
| S5 | Hyperliquid docs, Order types: <https://hyperliquid.gitbook.io/hyperliquid-docs/trading/order-types> | Official |
| S6 | Hyperliquid docs, Self-trade prevention: <https://hyperliquid.gitbook.io/hyperliquid-docs/trading/self-trade-prevention> | Official |
| S7 | Community wiki, Historical data: <https://hyperliquid-co.gitbook.io/wiki/guide/builder-guide/historical-data> | Secondary (author not named) |
| S8 | Third-party data guides (Dwellir data catalog, ironflow.sh fills guide) | Secondary |
| S9 | AWS S3 pricing and Requester Pays documentation: <https://aws.amazon.com/s3/pricing/>, <https://docs.aws.amazon.com/AmazonS3/latest/userguide/RequesterPaysBuckets.html> | Official (AWS) |

Only official sources are taken as verified. Secondary sources are used
for hints and are marked. No data from the buckets is reproduced here or
anywhere in the repository; record shapes below are described in our own
words with placeholder values.

## 1. Buckets and access

| Fact | Status |
|---|---|
| Two buckets: `hyperliquid-archive` (asset data) and `hl-mainnet-node-data` (node data) (S1) | Verified |
| Both are AWS S3 **requester pays**: every request must carry `x-amz-request-payer: requester` (CLI: `--request-payer requester`) and the requester's account pays request and transfer charges (S1, S9) | Verified |
| Uploads to `hyperliquid-archive` happen roughly monthly, with no guarantee of timeliness or completeness (S1) | Verified |
| Objects are compressed with LZ4 and decompressed with `unlz4`/`lz4 -d`, that is, the LZ4 **frame** format (S1) | Verified (format name); frame format inferred from the CLI |
| Bucket region | **Unverified.** Find it without credentials: `curl -sI https://<bucket>.s3.amazonaws.com \| grep -i x-amz-bucket-region`. `hl-backfill` takes the region as configuration because S3 signing needs it |

Requester-pays costs (S9, us-east-1 list prices on 2026-11-12, configurable
in `hl-backfill estimate`): data transfer out to the internet USD 0.09/GB
for the first 10 TB a month; GET USD 0.0004 per 1,000 requests; LIST USD
0.005 per 1,000 requests. Transfer to compute in the bucket's own region is
free (requests are still charged), so running the copier on a spot
instance in that region and writing to the raw tier from there leaves
only the destination's ingress pricing (zero for Cloudflare R2).

## 2. Admitted and excluded prefixes

| Bucket | Prefix | ADM-0003 | `hl-backfill` dataset |
|---|---|---|---|
| `hl-mainnet-node-data` | `node_fills_by_block/` | Admitted | `node_fills_by_block` |
| `hl-mainnet-node-data` | `node_fills/` | Admitted | `node_fills` |
| `hl-mainnet-node-data` | `node_trades/` | Admitted | `node_trades` |
| `hl-mainnet-node-data` | `replica_cmds/` | Admitted (replay only) | `replica_cmds` |
| `hl-mainnet-node-data` | `explorer_blocks/` | Admitted (replay only) | `explorer_blocks` |
| `hyperliquid-archive` | `market_data/` | Admitted | `market_data` |
| `hyperliquid-archive` | `asset_ctxs/` | **Excluded** | none; denylisted |
| `hl-mainnet-node-data` | `misc_events_by_block/` | **Excluded** | none; denylisted |

The copier rejects any key with a path segment equal to `asset_ctxs` or
`misc_events_by_block` (or starting with either), in either bucket, before
any request for the object is made, and it never lists outside the
admitted prefixes. Other prefixes in the buckets (for example any future
`misc_events`) are not admitted and are never listed or copied.

## 3. Layouts

`{date}` is `YYYYMMDD`; `{hour}` is the UTC hour **without zero padding**
(`0` to `23`), so listings must be sorted numerically.

| Dataset | Key pattern | Status |
|---|---|---|
| `market_data` | `market_data/{date}/{hour}/{datatype}/{coin}.lz4`; `{datatype}` is `l2Book` | Verified (S1 gives the pattern and an example); unpadded hour verified by S1's example |
| `asset_ctxs` | `asset_ctxs/{date}.csv.lz4` | Verified (S1); excluded |
| `node_fills_by_block` | `node_fills_by_block/hourly/{date}/{hour}.lz4` | **Unverified** (S8). Consistent with the node's own `<stream>/hourly/{date}/{hour}` layout (S2) |
| `node_fills` | `node_fills/hourly/{date}/{hour}.lz4` | **Unverified** (S8; matches the node layout in S2) |
| `node_trades` | `node_trades/hourly/{date}/{hour}.lz4` | **Unverified** (S7 example) |
| `replica_cmds` | `replica_cmds/{start_time}/{date}/{height}.lz4`; `{start_time}` is the RFC 3339 time the writing node started (it contains `:`), `{height}` the first block of the file | Node layout verified (S2: `replica_cmds/{start_time}/{date}/{height}`); bucket layout and `.lz4` suffix **unverified** (S7). One secondary source (S8) describes a different `replica_cmds/{date}/{chunk_start}.lz4` layout, which may be a third-party mirror |
| `explorer_blocks` | `explorer_blocks/{l1}/{l2}/{height}.rmp.lz4`; in S7's example `{l1}` is the height floored to a multiple of 100,000,000 and `{l2}` to a multiple of 100,000 | **Unverified** (S7). The copier does not rely on the multiples: it lists level by level and filters directories by the numeric range they can contain |

The copier's key parser (`tools/hl-backfill/src/buckets.rs`) accepts both
`replica_cmds` layouts and parses `{date}` wherever it appears.

## 4. Record formats

All text formats are JSON lines after LZ4 decompression.

### 4.1 `node_fills_by_block`

Written by a non-validating node with `--write-fills --batch-by-block`
(S1). One line per block (S2):
`{local_time, block_time, block_number, events}`, where each event is a
two-element array `[user, fill]` and `fill` has the API fill fields
(`coin`, `px`, `sz`, `side`, `time`, `startPosition`, `dir`, `closedPnl`,
`hash`, `oid`, `crossed`, `fee`, `tid`, `feeToken`, optional
`liquidation`, `builderFee`, ...) (S3). This is the same format the live
node reader consumes, so the details and the open questions are in
[hyperliquid-node-format.md](hyperliquid-node-format.md) §§ 3-4.

Filtering (ADM-0001, ADM-0003): drop `user` and every excluded fill field;
keep `coin`, `px`, `sz`, `side`, `time`, `crossed`, `tid`, `oid`; keep only
book-matched pairs (one `tid`, two fills, exactly one `crossed: true`);
drop pairs where either fill has a backstop liquidation. A fill from a
market liquidation that matched the book is kept, without its
`liquidation` object.

### 4.2 `node_fills` (older)

"Matches the API format" (S1). **Unverified:** whether lines are batched
by block. `hl-backfill` accepts both a batched line and a bare
`[user, fill]` line. Bare lines carry no block number: the records' block
numbers come from an injected time-to-block clock (built from
`explorer_blocks` or batched data); without one the hour is reported as
unresolved, never guessed.

### 4.3 `node_trades` (older)

"Does not match the API format" (S1). Per S7 (**unverified**): one trade
per line with `coin`, `side` (aggressor, `B`/`A`), `time` (naive UTC
string), `px`, `sz`, `hash`, `trade_dir_override` and `side_info`, a list
of two per-side objects with `user`, `start_pos`, `oid`, `twap_id`,
`cloid`. There is no `tid` and no `crossed`.

Filtering: keep `coin`, `side`, `time`, `px`, `sz`, and the two
`side_info[].oid` values as `oids: [oid, oid]` (`side_info` itself is a
denylisted name because it holds owner addresses); drop everything else. Lines whose `trade_dir_override` is anything other
than `Na` are dropped, because its other values are undocumented and may
mark liquidation or settlement trades. **Unverified:** whether this drops
any book-matched trades. A trade ID for these records is derived
deterministically from the two `oid`s (documented in
`tools/hl-backfill/src/parse.rs`); it is not the venue's `tid`.

### 4.4 `market_data` L2 snapshots

One file per coin per hour (S1). Per S7 (**unverified**): one snapshot per
line, `{time, ver_num, raw}` with `time` a naive UTC string with
nanoseconds, `ver_num` 1 and `raw` the websocket `l2Book` message
`{channel: "l2Book", data: {coin, time, levels: [bids, asks]}}`; each level
is `{px, sz, n}`. Bids first, asks second follows the websocket `l2Book`
documentation; up to 20 levels per side as served by the API
(**unverified** for the archive).

**Snapshot frequency inside each file: unverified.** S7 says "twice per
second"; ADM-0003 asks for it to be measured. `hl-backfill process`
reports the measured median interval and snapshot count per file in its
summary, so the first processed day settles it.

L2 snapshots carry no block number. Records take `seq =
end_of_block(B)` where `B` is the last block at or before the snapshot's
`data.time`, from the injected time-to-block clock; unresolved snapshots
are counted and skipped. They carry flags `backfill` and `low_resolution`.

### 4.5 `replica_cmds`

"Historical L1 transactions" (S1). The node writes "blocks parsed as
transactions" there, with `--replica-cmds-style` choosing `actions`
(default), `actions-and-responses` or `recent-actions` (S2). Which style
the archive used is **unverified**. Per S7 (**unverified**), a signed
action looks like `{signature: {r, s, v}, vaultAddress?, action: {type,
...}, nonce}`; the exact per-block line wrapper is not documented
publicly and must be read from a downloaded object.

Action shapes are documented in S4: `order` (`orders[]` of `{a, b, p, s,
r, t, c?}` with `t` either `{limit: {tif: Alo|Ioc|Gtc}}` or `{trigger:
{isMarket, triggerPx, tpsl}}`, plus `grouping`), `cancel` (`cancels[]` of
`{a, o}`), `cancelByCloid` (`cancels[]` of `{asset, cloid}`), `modify`
(`{oid, order}`), `batchModify` (`modifies[]`), `scheduleCancel`
(`{time?}`). Signer addresses are not in the action; they are recovered
from the signature. Neither signers nor `vaultAddress` are ever
published; the replay uses them only in memory.

### 4.6 `explorer_blocks`

Historical explorer blocks (S1). Per S7 (**unverified**): despite `.rmp`
in the name the content is described as JSON; each file covers a short
span of blocks and holds `{timestamp, data}` entries with the same action
shapes as § 4.5. Whether the content is MessagePack or JSON must be checked
on a downloaded object. Used for replay and for the time-to-block clock.

## 5. Coverage

Coverage start and end dates are taken from bucket listings, not from
secondary sources (ADM-0003). `hl-backfill estimate` lists each dataset
and reports, per dataset, the object count, total bytes, and the first and
last date (or height for `explorer_blocks`) present, which is the coverage
record for that import.

Secondary hints, for planning only (**unverified**):

| Dataset | Hint |
|---|---|
| `market_data` | From 2023 (S7) |
| `node_trades` | About 2025-03-22 to 2025-05-25 (S7, S8) |
| `node_fills` | About 2025-05-25 to 2025-07-27 (S8) |
| `node_fills_by_block` | About 2025-07-27 onward (S8) |
| `replica_cmds`, `explorer_blocks` | From an early block onward (S7); one source says 2025-01 for `replica_cmds` (S8) |

## 6. To verify (summary)

1. Bucket regions.
2. Key patterns for every `hl-mainnet-node-data` prefix (§ 3).
3. `node_fills` line format (batched or not).
4. `node_trades` fields and the meaning of `trade_dir_override`.
5. `market_data` line wrapper, level order, levels per side and snapshot
   frequency.
6. `replica_cmds` style (actions only, or with responses) and line wrapper.
7. `explorer_blocks` encoding (JSON or MessagePack) and file span.
8. Coverage dates per prefix (from `hl-backfill estimate`).
