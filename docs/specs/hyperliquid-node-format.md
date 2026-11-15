# Hyperliquid node output format

- Status: Draft. Facts marked **to verify** must be checked against a
  running node before the node reader merges.
- Covers: the files a non-validating Hyperliquid node writes, which of them
  the mesh reads, how the node reader maps them to
  [observation-v0](observation-v0.md) records, and the filtered-raw form
  that is published (§ 12).

## 0. Sources and clean-room note

Written only from Hyperliquid's official documentation and the Apache-2.0
node repository:

- Node README: <https://github.com/hyperliquid-dex/node> (Apache-2.0)
- L1 data schemas:
  <https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/nodes/l1-data-schemas>
- Websocket subscriptions (fill and trade types):
  <https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions>
- Info endpoint (order statuses):
  <https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint>
- Tick and lot size:
  <https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/tick-and-lot-size>
- Asset IDs: <https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/asset-ids>
- HIP-4 outcome markets:
  <https://hyperliquid.gitbook.io/hyperliquid-docs/hyperliquid-improvement-proposals-hips/hip-4-outcome-markets>

No code or data was taken from `hyperliquid-dex/order_book_server` or any
unlicensed repository, and no sample values were copied from the docs.
Field names are facts of the format; descriptions are our own.

## 1. Node basics

- Non-validator minimum: 16 vCPUs, 128 GB RAM, 500 GB SSD, Ubuntu 24.04
  only. Gossip ports 4001 and 4002 open. Tokyo is recommended for latency.
- Output root: `~/hl/data`. With default settings the node writes about
  100 GB of logs per day; the `--write-*` streams add more. Old files must
  be archived or deleted.
- Output files are rotated hourly: `<stream>/hourly/{date}/{hour}`.
  **To verify:** `{date}` is `YYYYMMDD` and `{hour}` is unpadded `0`-`23`
  (the synthetic fixtures assume both), and the hour is UTC.

## 2. Streams

| Directory under `~/hl/data` | Flag | Content | Mesh use |
|---|---|---|---|
| `node_fills/hourly/{date}/{hour}` | `--write-fills` | Fills in the API fills format, one per user | **Read** (trades) |
| `node_order_statuses/hourly/{date}/{hour}` | `--write-order-statuses` | Every L1 order status | **Read** (classification, order times) |
| `node_raw_book_diffs/hourly/{date}/{hour}` | `--write-raw-book-diffs` | Every L1 resting-order change | **Read** (book events) |
| `periodic_abci_states/{date}/{height}.rmp` | always | State snapshot every 10,000 blocks | **Read** via `compute-l4-snapshots` (reconciliation) |
| `node_trades/hourly/{date}/{hour}` | `--write-trades` | Trades with both users (`side_info`) | Not used (`--write-fills` overrides it) |
| `node_twap_statuses/{date}/{hour}` | written with fills | TWAP statuses | Ignored |
| `misc_events/hourly/{date}/{hour}` | `--write-misc-events` | Deposits, delegations, funding, ledger updates (including liquidations) | **Excluded**; do not enable |
| `hip3_oracle_updates/hourly/{date}/{hour}` | `--write-hip3-oracle-updates` | HIP-3 deployer oracle updates | **Excluded**; do not enable |
| `system_and_core_writer_actions/hourly/{date}/{hour}` | `--write-system-and-core-writer-actions` | HyperEVM transfers | Ignored |
| `replica_cmds/{start_time}/{date}/{height}` | always | Blocks as transactions | Research only (Phase 2 replay) |
| `node_logs/...`, `visor_child_stderr/...` | always | Logs | Operations only |

Hyperliquid's public archive also has `node_fills_by_block`.
**To verify:** whether a live node with `--batch-by-block` writes to
`node_fills/` (and so on) or to `*_by_block/` directories. The synthetic
fixtures use `node_fills_by_block/`, `node_order_statuses_by_block/` and
`node_raw_book_diffs_by_block/`; the reader must take directory names from
configuration.

### 2.1 Flags the mesh uses

| Flag | Effect |
|---|---|
| `--write-fills` | Fills stream. Overrides `--write-trades` if both are set. Includes `deployerFee` on HIP-3 fills |
| `--write-order-statuses` | Order statuses (large) |
| `--write-raw-book-diffs` | Raw book diffs (large) |
| `--batch-by-block` | One line per block instead of one per event (§ 3) |
| `--disable-output-file-buffering` | Flush every line; lower latency, more disk I/O. Required for the 250 ms publish target |
| `--serve-info` | Local info server at `http://localhost:3001/info` (§ 7.2) |

Related: `--stream-with-block-info` writes events as they are processed
with the batched schema; `--replica-cmds-style` controls `replica_cmds`.
The mesh runs with exactly `--write-fills --write-order-statuses
--write-raw-book-diffs --batch-by-block --disable-output-file-buffering
--serve-info`.

## 3. Line format

With `--batch-by-block`, each line is one JSON object per block:

| Field | Meaning | Mesh use |
|---|---|---|
| `local_time` | When the node processed the block | `ts_ingest_ns` (deterministic: it is in the raw file) |
| `block_time` | Block timestamp | `ts_source_ms` for book events |
| `block_number` | Block height | `seq.block_number` |
| `events` | The block's events for this stream, in processing order | § 4-6 |

`local_time` and `block_time` are naive UTC strings
(`YYYY-MM-DDTHH:MM:SS.fff...`). Archived node output (2025-07-27 and
2026-11-13, sampled 2026-11-14) writes **nine** fractional digits for both;
the reader floors `block_time` to the millisecond for `ts_source_ms`. The
fixtures write `block_time` with 3 and `local_time` with 9 digits.
**To verify:** whether a line is
written for blocks with no events in a stream (the fixtures write every
block, with an empty `events` array). The reader must tolerate both.

Without `--batch-by-block`, each line is one event and block boundaries are
not explicit. The mesh does not support that mode.

## 4. Fills

Each event is a two-element array `[user, fill]`. `user` is an address and
is **excluded**.

| Fill field | Type | Meaning | Mesh |
|---|---|---|---|
| `coin` | string | Market (§ 8) | instrument lookup |
| `px` | string | Price | `trade.price` |
| `sz` | string | Size | `trade.size` |
| `side` | string | `B` (buy) or `A` (sell); **to verify** against a node, the docs show both values without defining them | aggressor (with `crossed`) |
| `time` | number | Milliseconds | `ts_source_ms` |
| `crossed` | boolean | True if the order crossed the spread (taker) | aggressor |
| `tid` | number | Trade ID: a 50-bit hash of the buyer and seller order IDs; unique as `(block_time, coin, tid)` | `trade_id`, pairing |
| `oid` | number | Order ID of this side | classification (§ 6) |
| `startPosition`, `dir`, `closedPnl` | string | Position and PnL | **excluded** |
| `hash` | string | L1 transaction hash | **excluded** |
| `fee`, `feeToken`, `builderFee`, `builder`, `subBuilders`, `deployerFee` | various | Fees and builders | **excluded** |
| `twapId` | number or null | TWAP | **excluded** |
| `cloid` | string, optional | Client order ID, present when the order had one (seen in every archived period sampled, 2025-03 to 2026-11) | **excluded** |
| `priorityGas` | string (decimal), optional | Priority fee paid by the order (seen in archived output from 2026-11; absent on 2025-07-27) | **excluded** |
| `liquidation` | object, optional | `{liquidatedUser?, markPx, method}`, `method` is `market` or `backstop` | **excluded** (`markPx` is a mark price) |

### 4.1 From fills to trades

1. Fills arrive one per side: two fills per trade, sharing `coin` and
   `tid`. Group fills in a block by `(coin, tid)`. The order of the two
   fills in the file is not specified; do not rely on it.
2. A pair with exactly one `crossed = true` fill is an order-book trade.
   The aggressor is that fill's `side` (`B` = buy, `A` = sell).
3. Emit one `trade`: price, size and `trade_id` from the pair (they must
   agree, otherwise reject the block), `ts_source_ms` from `time`.
   `event_index` counts trades of that instrument in the block, in order of
   each pair's first fill.
4. **Liquidations.** A fill with `liquidation.method = market` came from a
   liquidation order that matched the book: keep the trade, drop the
   `liquidation` object. A group with a `method = backstop` fill is a
   transfer to the liquidator at the mark price, not a book trade: drop the
   group. A group with any other `method` is dropped too, never guessed.
5. **No taker.** A pair with no `crossed = true` fill (backstop
   liquidations, and settlement fills when a HIP-3 market is halted and
   settled) is not an order-book trade: drop it and count it in metrics.
   **To verify:** how halt-settlement fills are marked on a live node; the
   fixtures model them as a no-taker pair with `dir = "Settlement"`, and the
   rule above does not depend on `dir`.
6. A malformed group is an error: a single unmatched fill, more than two
   fills, two `crossed = true` fills, or two fills on the same side. Emit
   no trade for it, set `gap_before` on the instrument's next trade (in
   this block or a later one) and alert.

Checks apply in this order: liquidation method (step 4), then the number
of fills (one, or more than two, is malformed), then no taker (step 5),
then two takers, then same side. Fills of spot markets are dropped
whatever their group (§ 8).

## 5. Order statuses

Each event: `{time, user, status, order}`. `time` is a naive UTC string;
`user` is **excluded**.

| `order` field | Meaning | Mesh |
|---|---|---|
| `coin` | Market | lookup |
| `side` | `B` or `A` | check |
| `limitPx` | Limit price | check |
| `sz` | The docs' comment says "filled size". **To verify:** on a node, whether this is the remaining or the filled size; the fixtures use the remaining size, as the API's open-orders responses do | not used for book sizes |
| `origSz` | Original size | not used |
| `oid` | Order ID | classification |
| `timestamp` | Placement time, ms | `ts_order_ms` in snapshots |
| `orderType`, `tif`, `reduceOnly`, `isTrigger`, `triggerPx`, `triggerCondition`, `isPositionTpsl`, `children` | Order attributes | not used |
| `cloid` | Client order ID | **excluded** |

Status values documented for the info endpoint include `open`, `filled`,
`canceled`, `triggered`, `rejected`, `marginCanceled`, `selfTradeCanceled`,
`reduceOnlyCanceled`, `siblingFilledCanceled`, `liquidatedCanceled`,
`scheduledCancel`, and a family of `...Rejected` values. Statuses are used
only to classify removals (§ 6). Book sizes always come from the raw book
diffs.

## 6. Raw book diffs

Each event: `{user, oid, coin, side, px, raw_book_diff}`. `user` is
**excluded**. `side` is `Bid` or `Ask` (not `B`/`A`). `px` is a string,
`oid` a number.

| `raw_book_diff` | Meaning |
|---|---|
| `{"new": {"sz": s}}` | New resting order of size `s` |
| `{"update": {"origSz": a, "newSz": b}}` | Resting size changes from `a` to `b` |
| `"remove"` | Resting order removed |

### 6.1 From diffs to book events

One `book_event` per diff, in file order; `event_index` counts book events
of that instrument in the block; `ts_source_ms` is `block_time`.

An order can have several diffs in one block (a partial fill, then a
modify or a cancel), so a diff is classified as a fill only when a fill
accounts for it. Per order (`coin`, `oid`), the block's **pending maker
fills** are the sizes of the trades of § 4.1 in which that order was the
resting side (`crossed = false`), in fill order. A diff that matches a
pending maker fill consumes it.

Before classifying, if the order's last diff in the block is `remove` and
the block has an order status `filled` for it, the last pending maker fill
is reserved for that `remove`: it is taken out of the queue, and the
`remove` still classifies as a fill through the status. A modify earlier
in the block with the same size change is then not mistaken for that
fill.

Then each diff, in file order:

| Diff | Condition | `action` |
|---|---|---|
| `new {sz}` | | `add {size}` |
| `update {a, b}`, `b < a` | the next pending maker fill has size `a - b` (consumed) | `fill {orig_size: a, new_size: b}` |
| `update {a, 0}`, `a > 0` | otherwise | `cancel` |
| `update {a, b}` | otherwise | `modify {orig_size: a, new_size: b}` |
| `remove` | a maker fill is pending (the next one is consumed) | `fill {new_size: 0}` |
| `remove` | status `filled` in the block | `fill {new_size: 0}` |
| `remove` | otherwise | `cancel` |

This replaces the simpler rule "a fill with this `oid` in the block makes
the diff a fill", which misclassifies a modify or cancel next to a partial
fill of the same order. One case stays ambiguous: a modify and a later
partial fill of the same order in the same block with equal size changes.
The earlier diff takes the fill, so the two actions come out swapped;
sizes stay correct.

Without the order-status line, no status is `filled`; without the fills
line, the block yields no records at all (§ 9).

**To verify:** how user modifies appear (in-place `update`, or `remove`
plus `new` with a new `oid`). The mapping above handles both. The fixtures
model a size-reducing modify as an in-place `update` with status `open`.

## 7. Snapshots

### 7.1 Periodic state

`periodic_abci_states/{date}/{height}.rmp` is written every 10,000 blocks.
`hl-node --chain <chain> translate-abci-state <in> <out>` converts it to
JSON, and `hl-node --chain <chain> compute-l4-snapshots <abci-state> <out>`
computes L4 book snapshots from it.

### 7.2 Info server

With `--serve-info`, a POST to `http://localhost:3001/info` with

```json
{"type": "fileSnapshot",
 "request": {"type": "l4Snapshots", "includeUsers": false, "includeTriggerOrders": false},
 "outPath": "/path/to/out.json",
 "includeHeightInOutput": true}
```

writes the current L4 book to `outPath`. This uses the latest state, not
the last periodic file. Poll `exchangeStatus` and compare its L1 time with
local time to check freshness. The local server does not support `l2Book`
or websockets. Always pass `includeUsers: false`.

### 7.3 L4 snapshot shape

An array of `[coin, [bids, asks]]`. Each order has the order-status
`order` fields (plus `user` if requested). Orders at one price are in time
order; trigger orders come last on each side and have `isTrigger = true`.

**To verify:** the exact top-level shape with `includeHeightInOutput`. The
fixtures assume `{"height": H, "snapshot": [...]}`. The reader must isolate
this parsing.

### 7.4 Mapping

Drop trigger orders (they are not resting), drop `user`, skip spot coins
(§ 8) and unregistered coins, and emit one `book_snapshot` (L4) per
remaining coin with `seq = end_of_block(H)`, `ts_order_ms = timestamp`. The book builder compares its own book after
block `H` with the snapshot and resyncs on mismatch.

## 8. Markets

The reader builds the instrument registry from node metadata at start-up,
whenever an admitted `coin` that is not registered appears, and
periodically. There is no hard-coded market list. The market class is
read from the `coin` syntax alone.

| Market | `coin` in node files | Metadata | Mesh ID |
|---|---|---|---|
| Core perpetual | name, e.g. `BTC` | `meta` | `hyperliquid:perp:BTC` |
| HIP-3 deployer perpetual | `{dex}:{coin}`, e.g. `xyz:CL` | `perpDexs`, `meta` with `dex` | `hyperliquid:perp:xyz:CL` |
| HIP-4 outcome | `#{10 * outcome + side}`, e.g. `#10` | `outcomeMeta` | `hyperliquid:outcome:#10` |
| Spot | `@{index}`, or a pair name containing `/` such as `PURR/USDC` | not read | none: **not ingested** |

**Spot is outside ADM-0001.** The admission covers core perpetuals, HIP-3
deployer perpetuals and HIP-4 outcome markets only. Spot markets are not
registered, and every event with a spot `coin` (fills, order statuses,
raw book diffs, L4 snapshot books) is dropped by the reader (stream events
are counted in metrics, never as unknown coins) and by the filter (§ 12).

- Scales: size scale `szDecimals`; price scale `6 - szDecimals` for
  perpetuals (prices have at most 5 significant figures and at most that
  many decimals; integer prices are always allowed). **To verify:**
  whether HIP-3 markets follow the perp rule, and the outcome-market rule.
  Outcome markets take the spot rule, price scale `8 - szDecimals`
  ([ADR 0021](../decisions/0021-outcome-market-price-scale.md); prices lie
  between 0 and 1 and archived fills show five decimals), with size
  decimals 0 when metadata gives none.
- HIP-4 Yes and No books for one outcome are merged (buying Yes at `p`
  equals selling No at `1 - p`). **To verify:** which coin the node
  reports fills and diffs under for a merged book. Until then both coins
  of an outcome (`#{10n}` and `#{10n+1}`) are registered.
- HIP-3 markets get `oracle_anchored_venue` by default, because the
  deployer's oracle drives the mark price and funding; the registry can
  override flags per market.
- Spot coins are recognized by syntax (`@` prefix or `/`) so they can be
  dropped. **To verify** on a node that no admitted coin uses either form.

## 9. Gaps and ordering

- Within a stream, `block_number` must increase by exactly one from line
  to line (if empty blocks are written) or never decrease (if not). A
  jump in the three streams together is a missing block: set `gap_before`
  on the next record of each affected instrument, alert, and resync books
  from the next snapshot.
- The three streams are separate files written by one process. The reader
  joins them per block: it waits until all three have the block (or a
  later one) before emitting that block's records, so fill-based
  classification (§ 6.1) is complete. A block released without one of its
  lines is incomplete: the first record of every known stream from that
  block on carries `gap_before`. Without the fills line the block yields
  no records, without the diffs line no book events.
- Never infer order between fills and diffs within a block (see
  observation-v0 § 3).

## 10. Synthetic fixtures

`tools/hl-synth` writes files in this format under
`fixtures/synthetic/hl/`, with an `expected.json` of the trades, book
events and books a correct reader produces. The fixtures encode every
assumption marked **to verify** above, so when a live node disagrees,
update this spec, the generator and the reader together.

## 11. To verify (summary)

1. `{date}` and `{hour}` path formats, and UTC hours.
2. Directory names under `--batch-by-block` (`node_fills` or
   `node_fills_by_block`).
3. Lines for empty blocks (the `local_time` and `block_time` formats were verified on archived output, 2026-11-14).
4. `side` values `B`/`A` meaning buy/sell in fills and order statuses.
5. Order status `sz`: remaining or filled size.
6. How modifies appear in raw book diffs.
7. How halt-settlement fills are marked.
8. L4 snapshot top-level shape with `includeHeightInOutput`.
9. Price-scale rule for HIP-3 and outcome markets; coin used for merged
   HIP-4 books; spot coin syntax (`@index`, `A/B`).

## 12. Filtered raw

Filtered raw is node output with everything ADM-0001 excludes removed: the
public reproducibility root from which normalized records are rebuilt
([archive-layout-v0](archive-layout-v0.md) § 2). It is defined line by
line, so it can be produced while tailing.

**Directories.** Lines of the three read streams (§ 2) are filtered.
Lines of every other directory are dropped, as are blank lines. A filtered
file keeps its node path (`<stream>/hourly/{date}/{hour}`) unless the
archive layout renames it.

**Lines.** One filtered line per raw line, in the same `--batch-by-block`
format (§ 3) with `block_number`, `block_time` and `local_time` unchanged
and the remaining events in their original order. A line of the three
streams is never dropped, even when no events remain, so block continuity
and gap detection (§ 9) survive filtering.

**Events and fields.** Events of spot markets (§ 8) are removed from every
stream. Otherwise:

| Stream | Events kept | Fields kept |
|---|---|---|
| Fills | Fills of book trades and of malformed groups (§ 4.1). Dropped groups (backstop and unknown liquidation methods, no-taker pairs such as halt settlements) are removed whole | The bare fill object `{coin, crossed, oid, px, side, sz, tid, time}` instead of `[user, fill]`; `liquidation` is removed from kept `market` liquidation fills |
| Order statuses | All | `{order, status, time}`, with `order` reduced to `{coin, limitPx, oid, origSz, side, sz, timestamp}` (`origSz` only when the raw order has it) |
| Raw book diffs | All | `{coin, oid, px, raw_book_diff, side}` |

Malformed fill groups keep their admitted fields so that the `gap_before`
they cause is reproducible from filtered raw.

**Encoding.** Compact JSON with no whitespace. Keys are written in sorted
order, except inside an `update` diff, where `origSz` precedes `newSz`.
Strings, including decimal prices and sizes and time texts, are copied
unchanged.

**Strictness.** A line that does not parse, or that has a field the
format does not document, fails filtering exactly as it fails parsing; it
is never passed through.

**L4 snapshots.** A snapshot file (§ 7.3) keeps its envelope
`{height, snapshot}`. Spot coins are removed, trigger orders are removed,
and each order is reduced to `{coin, limitPx, oid, origSz, side, sz,
timestamp}` (no `user`, no order attributes).

**Properties.** The parser accepts both fill forms, so filtered raw parses
like raw: parsing filtered raw yields exactly the records, including
`gap_before` flags, that parsing the original raw yields, and filtering
filtered raw changes nothing. The reader's tests check both over every
fixture and generated scenario.
