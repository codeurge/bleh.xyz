# Book, bars and analytics

- Crate: [`crates/book`](../../crates/book)
- Owner: book, bars and analytics lane ([ADR 0007](../decisions/0007-code-layout.md))
- Data model: [observation-v0](../specs/observation-v0.md)
- Last updated: 2026-11-12

The `book` crate turns one instrument's trades, book events and book
snapshots into order-level books and derived records: time and activity
bars, footprints, delta and CVD, volume profiles, order activity, queue
positions, sweeps, heatmap tiles and a liquidity grade. The live gateway
and the archive replay run the same code ([ADR 0006](../decisions/0006-phase-1-2-service-architecture.md)).

This document is the method specification for every derived record the
crate emits (inputs, formula, version) and the runbook for operating it.

## 1. Rules that apply to every method

- **No floating point.** Prices and sizes are `mesh-types` fixed point at
  the instrument's scales. Accumulators are `i64` mantissas with checked
  arithmetic (`i128` for products and ratios); an overflow is an error, never
  a wrap. Ratios are integers in basis points (bps) or hundredths of a basis
  point (cbps), rounded toward zero.
- **Determinism.** Outputs depend only on the input records, the block
  times, the configuration and where the run starts and ends. Two runs over
  the same input give byte-identical canonical JSON.
- **Provenance of a derived record.** The provenance of its last input
  record, with the union of every input's flags and no `ts_publish_ns`.
  Records describing the book after a block (checkpoints, queue positions,
  heatmap tiles, liquidity grades) carry `seq = end_of_block(block)`, the
  block time, and the flags of the block's last record without
  `gap_before`.
- **Derived header.** `method` is `name@version`; `inputs_hash` is the
  observation content hash ([observation-v0](../specs/observation-v0.md)
  § 6.2) of the input records; `input_count` is their number.
- **Time intervals** are aligned to the Unix epoch in UTC
  (`start = floor(t / length) * length`), so daily intervals start at UTC
  midnight. An interval without inputs emits nothing.
- **Versioning.** Any change to a method's output is a new version; old
  versions stay documented here.

### 1.1 Input order for hashing

- Inputs of one kind are hashed in `seq` order, which is the archive's
  canonical order for one instrument and kind.
- Inputs of several kinds (trades and book events, plus a snapshot loaded in
  the window) are hashed in **block-major order**: by block number, then
  kind in the order `trade`, `book_event`, `book_snapshot`, then
  `event_index`. This is the archive's canonical order applied block by
  block; it lets the live path hash without buffering a whole window.
- Canonical encodings are written directly by
  `book::inputs::canonical_{trade,event,snapshot}` and are byte-identical
  to `mesh_types::hash::canonical_record` (unit and property tests compare
  them). The processor encodes each input once and feeds every hasher.

## 2. Order book

`book::OrderBook` holds every resting order of one instrument: order ID,
side, price, size, placement time and queue priority.

- Starts empty (a market observed from its first order) or from an L4
  `book_snapshot`. An L2 snapshot cannot build an L4 book.
- Applies `book_event`s in strictly increasing `seq`. `add` takes the next
  queue priority; `modify` keeps the order's priority (the source reports an
  in-place update; see
  [hyperliquid-node-format](../specs/hyperliquid-node-format.md) § 6.1);
  `fill` to zero and `cancel` remove the order.
- Placement time is the snapshot's `ts_order_ms`, or the block time of the
  `add`.
- L2 levels and top of book are derived from the orders, never ingested.
  `TopOfBook::mid` is exact at one more fractional digit than the price
  scale.

### 2.1 Inconsistencies

`OrderBook::apply` is atomic: it either applies the event or returns a typed
`BookError` and leaves the book unchanged.

| Error | Meaning |
|---|---|
| `UnknownOrder` | `modify`, `fill` or `cancel` for an order that is not resting |
| `DuplicateOrder` | `add` for an order ID that is resting |
| `SideMismatch`, `PriceMismatch` | The event's side or price differs from the resting order's |
| `OrigSizeMismatch` | `orig_size` differs from the resting size |
| `NonReducingFill` | A fill whose `new_size` is not below the resting size (zero or negative filled quantity) |
| `InvalidEvent` | The event fails its own validation (zero-size `add`, `modify` to zero) |
| `ScaleMismatch` | Price or size scale differs from the book's |
| `OutOfOrder` | `seq` not after the book's position |
| `InstrumentMismatch` | Another instrument's record |
| `Crossed` | After a block, best bid >= best ask (checked at block ends only: the source does not order events within a block in a way that excludes a transient cross) |
| `Overflow` | A level total does not fit 64 bits |

### 2.2 Reconciliation and resync

`book::reconcile(book, snapshot, options)` returns a `ReconcileReport`:

- `missing`: orders in the snapshot but not in the book;
- `extra`: orders in the book but not in the snapshot;
- `changed`: orders in both with a different side, price, size or (by
  default) placement time;
- `queue`: prices holding the same orders in a different queue order (by
  default);
- `levels`: prices whose aggregated level differs (the only comparison
  against an L2 snapshot).

Lists are in a fixed order: bids before asks, best price first, queue order
within a price. `book::resync` reconciles and then replaces the book with
the snapshot.

### 2.3 Checkpoints (book at time T)

After applying a block that has book events, the processor emits an L4
`book_snapshot` checkpoint when the block's bucket differs from the bucket
of the last checkpoint or loaded snapshot:

| Policy | Bucket |
|---|---|
| `EveryMillis(n)` (default `600_000`, 10 minutes) | `floor(block_time_ms / n)` |
| `EveryBlocks(n)` | `floor(block_number / n)` |
| `Never` | none |

Buckets are absolute, so every replay of the same input emits the same
checkpoints. The checkpoint has `seq = end_of_block(block)` and the block
time. The book at time T is
`book::checkpoint::book_at(nearest_checkpoint(checkpoints, T), events, T)`:
the last checkpoint at or before T plus every book event after it up to T,
applied block by block.

## 3. Bars

### 3.1 Time bars: `bars@1`

- Output: `bar` ([observation-v0](../specs/observation-v0.md) § 2.5).
- Inputs: the interval's trades, in `seq` order.
- Intervals: `1s` and `1m` from trades; `5m`, `15m`, `1h`, `1d` rolled up
  from `1m` (§ 3.2). The daily boundary is UTC midnight.
- A trade at `ts_source_ms = t` belongs to the bar starting at
  `interval.align(t)`.
- `open`/`close` are the first/last trade prices in `seq` order,
  `high`/`low` the extremes, `volume` the sum of sizes, `buy_volume` and
  `sell_volume` the sums by aggressor (trades with an unknown aggressor
  count only in `volume`), `trade_count` the number of trades.
- Emitted when a trade arrives in a later interval, when the clock passes
  the interval end (`advance_to`), or at the end of input.
- Trades must arrive in strictly increasing `seq` and non-decreasing time;
  anything else is an error.

### 3.2 Rollups (same method, `bars@1`)

A rolled-up bar takes open from the first source bar, high and low as the
extremes, close from the last, volumes and trade counts as sums, provenance
from the last source bar, flags as the union, and the derived header of the
interval's trades. It is therefore equal to the bar computed directly from
the trades; property tests check `rollup(1m) == direct` for every target and
for nested rollups (`5m` to `15m`, `1h` to `1d`). The batch form
`bars::rollup(bars, target, trades)` needs the trades only for the header
and rejects bars and trades that disagree.

### 3.3 Tick, volume and range bars: `tick-bars@1`, `volume-bars@1`, `range-bars@1`

- Output: `book::bars::ActivityBar` (not yet an observation kind; § 8).
- Inputs: the bar's trades, in `seq` order.
- **Tick:** a bar closes after every `trades` trades.
- **Volume:** a bar closes after the trade that brings its volume to at
  least `size`. Trades are never split, so a bar may exceed `size`.
- **Range:** a trade that would make `high - low` exceed `width` closes the
  bar without it and opens the next bar with it. Every bar has
  `high - low <= width`; no synthetic prices.
- Every series restarts at each reset boundary (`activity_reset`, UTC day
  by default). A bar still open at a reset or at the end of input is emitted
  with `complete = false`.
- Thresholds are per instrument and must be representable at its scale
  (a finer threshold is an error). None are configured by default.

## 4. Order-flow analytics from trades

### 4.1 Footprint: `footprint@1`

- Output: `footprint` (§ 2.6 of the data model), default interval `1m`.
- Inputs: the interval's trades, in `seq` order.
- One level per traded price, ascending. `ask_volume` sums trades with a
  buy aggressor (lifting the ask), `bid_volume` trades with a sell
  aggressor (hitting the bid); `trade_count` counts every trade at the
  price, including unknown-aggressor trades, whose size is in neither
  volume.

### 4.2 Delta and CVD: `delta@1`

- Output: `book::orderflow::DeltaBar`, default interval `1m`, anchor `1d`.
- `delta = buy_volume - sell_volume` per interval (signed fixed point,
  `SignedSize`).
- `cvd` = sum of `delta` from the anchor period start (UTC midnight by
  default) through the interval. It restarts at each anchor boundary.
- Inputs: **every trade from the anchor start through the end of the
  interval**, in `seq` order, because `cvd` depends on all of them.

### 4.3 Volume profile: `volume-profile@1`

- Output: `volume_profile` (§ 2.6 of the data model), default period `1d`.
- Inputs: the period's trades, in `seq` order.
- Each trade's size goes to the bucket `floor(price / bucket) * bucket`
  (mantissas at the price scale; default bucket one tick, mantissa 1), split
  into buy and sell aggressor volume. Empty buckets are omitted. `end_ms` is
  the period end (exclusive).

### 4.4 POC and value area: `value-area@1`

- Output: `book::orderflow::ProfileSummary`, one per profile, with the
  profile's inputs.
- **POC:** the bucket with the most volume; on a tie, the lowest price.
- **Value area** (default 7000 bps = 70%): `target = ceil(total * bps /
  10_000)`. Start with the POC bucket. While the included volume is below
  the target, compare the next traded bucket above the range with the next
  traded bucket below it; add the larger, or both on a tie, or the only one
  left at an edge. Empty buckets between traded ones are skipped, not
  compared.
- `value_area_low` and `value_area_high` are the lower bounds of the lowest
  and highest included buckets.

### 4.5 Sweeps: `sweep@1`

- Output: `book::orderflow::Sweep`.
- Inputs: the run's trades, in `seq` order.
- Within one block, a run is a maximal sequence of consecutive trades with
  the same known aggressor whose prices never move against it
  (non-decreasing for buys, non-increasing for sells). An unknown aggressor
  ends a run; runs never cross blocks. A run with at least `min_levels`
  (default 2) distinct prices is a sweep: side, first and last price,
  distinct levels, total size, trade count, first `seq`.
- Limit: the mesh has no taker order ID (excluded field), so two takers in
  a row on the same side whose prices continue monotonically merge into one
  run.

## 5. Order-flow analytics from the book

These run only while the book is in sync (§ 7.2).

### 5.1 Order activity: `order-activity@1`

- Output: `book::orderflow::OrderActivity`, default interval `1m` of block
  time.
- Inputs: the interval's applied book events, in `seq` order. Sizes and
  lifetimes also depend on the book before the interval, which the archive
  fixes (nearest checkpoint plus events).
- Counts: `adds`, `modifies`, `cancels`, `partial_fills` (size left),
  `full_fills` (size zero); sizes: `added_size`, `cancelled_size` (resting
  size before the cancel), `filled_size` (resting size before the fill
  minus `new_size`).
- `cancel_to_add_bps = floor(cancels * 10_000 / adds)`;
  `cancel_replace_to_fill_bps = floor((cancels + modifies) * 10_000 /
  (partial_fills + full_fills))`; absent when the divisor is zero.
- Lifetimes of orders removed in the interval (full fill or cancel):
  `removal block time - placement time` (floored at 0). `min`, `max`,
  nearest-rank `p50` and `p90` (`sorted[ceil(p * n / 100) - 1]`), and
  `fleeting_cancels`: cancels younger than `fleeting_ms` (default 1000).

### 5.2 Queue positions: `queue-position@1`

- Output: `book::orderflow::QueuePositions`, after any block where the top
  queues changed.
- For the best bid and best ask: the first `max_orders` (default 50) orders
  in queue order, each with size, `size_ahead` (total size of the orders
  ahead at that price) and placement time; the level's total size and order
  count, and whether the list was truncated.
- Positions are **estimates**: they assume strict price-time priority and
  that an in-place `modify` keeps the order's place.
- Inputs: the applied book events (and any snapshot loaded) since the
  previous record, block-major.

### 5.3 L3 heatmap tiles: `heatmap@1`

- Output: `book::orderflow::HeatmapTile`.
- Columns: `col = floor(t / time_step_ms)` (default 1 s). A column holds the
  book at its end: after the last block in the column, or carried forward
  from the previous column when no block falls in it.
- Rows: `row = floor(price / price_step)` on mantissas. A cell sums the
  resting size of the levels in its row over the best `max_levels` (default
  250) levels per side, bids and asks separately.
- Tiles: `tile_cols` by `tile_rows` cells (default 60 by 64), aligned to
  multiples of the tile size; cells use tile-local coordinates, are sparse
  (non-empty only) and ordered by column then row.
- A time tile is emitted when time passes its end (a later block or
  `advance_to`), one record per price tile with a non-empty cell. At the
  end of input the open tile is emitted with columns up to the last
  block's column.
- `price_step` defaults to the power of ten that puts the mid between 1000
  and 9999 steps, chosen at the first sample. Set it per instrument in
  production so tiles align across restarts.
- Inputs: the applied book events (and snapshots) of blocks within the time
  tile, block-major. The first column also depends on the book at the tile
  start.

## 6. Liquidity grade: `liquidity-grade@1`

- Output: `book::liquidity::LiquidityGrade`, every `emit_ms` (default
  5 minutes) of block time.
- **Samples**, every `sample_ms` (default 1 minute), from the book at the
  boundary:
  - spread `= floor((ask - bid) * 2_000_000 / (ask + bid))` cbps; none
    without both sides or with `ask + bid <= 0`;
  - depth per side: `sum(price * size)` over levels within `depth_bps`
    (default 25) of the mid, that is bids with
    `price * 20_000 >= (bid + ask) * (10_000 - depth_bps)` and asks with
    `price * 20_000 <= (bid + ask) * (10_000 + depth_bps)`, in quote units
    at the price scale, rounded down.
- **Window:** the trailing `window_ms` (default 1 hour), shortened to the
  time since the tracker started when that is less. Spread is the lower
  median of the samples (a sample without a spread ranks worst); depth per
  side is the lower median; the graded depth is the thinner side; trades per
  minute is `floor(trades * 100 * 60_000 / window)` hundredths.
- **Grade:** the best grade whose thresholds all three metrics meet, else D:

  | Grade | Spread at most | Depth at least (quote units) | Trades per minute at least |
  |---|---|---|---|
  | A | 5 bps | 500,000 | 20 |
  | B | 20 bps | 50,000 | 2 |
  | C | 100 bps | 5,000 | 0.2 |
  | D | otherwise | | |

- **Label:** grades at or better than `benchmark_min` (default B) are
  `benchmark`; the rest are `tape`, meaning the market is published as a
  record of trades, not as a reference price.
- Inputs: the trades and book events since the previous grade, block-major.
- Thresholds are configuration, not data: a change to the defaults is a new
  method version.

## 7. Running it

### 7.1 Live

One `book::Processor` per instrument:

1. `Processor::new(instrument, config)` (book waits for a snapshot) or
   `Processor::with_empty_book` (a market seen from its first order).
2. Feed bus batches with `push_batch` (trades and book events of a block,
   either first). A block is processed when a later block's batch arrives
   or on `flush`; call `flush` when the reader signals the end of a block
   to publish without waiting.
3. Feed node snapshots with `process_snapshot` after their block.
4. Call `advance_to(block_time)` on every block heartbeat, so quiet markets
   close their bars and tiles.
5. Publish every returned `Output`. `Output::record()` gives an observation
   `Record` for bars, footprints, volume profiles and checkpoints; the other
   outputs serialize with serde (canonical JSON via
   `mesh_types::canonical_json`).

Trade analytics run whatever the book state. Builder errors (out-of-order
trades, mixed scales, overflow) are returned as `ProcessError` and mean the
input is wrong.

### 7.2 Book state and alerts

| Output | Meaning | Action |
|---|---|---|
| `Desync { reason: book }` | An event or block end contradicted the book | Alert. Book analytics pause, events are skipped until the next snapshot resyncs the book |
| `Desync { reason: gap }` | The book-event stream skipped blocks (`prev_block_number` link broken, or an event flagged `gap_before`) | Same; also check the reader's gap metrics |
| `Reconciliation` with differences | The snapshot differed; the book was replaced by it | Alert with the report (missing, extra, changed, queue, levels) |
| `Reconciliation`, clean | The book matched the snapshot | Record for the reconciliation metric |
| `StaleSnapshot` | A snapshot older than the processed blocks arrived | Request a fresh snapshot |

On a desync the open heatmap tile and order-activity interval are dropped
(their book state cannot be trusted) and checkpoints stop until the resync.

### 7.3 Archive replay

`book::batch::run(instrument, config, input)` replays a range through the
same processor and returns outputs sorted by `Output::sort_key`. Pass the
block times from the raw files (`block_times`), the blocks that follow a
missing block (`event_gaps`), how the book starts (from the first snapshot,
or empty), and the range end (`end`, for example the next UTC midnight).

Live and batch outputs are identical (after sorting) when both see the same
records, snapshots, block times, start and end; `tests/parity.rs` checks
this over 6,000 simulated blocks across a UTC midnight with batches in both
kind orders, clock heartbeats and periodic snapshots. The trade-only slice
functions (`bars::time_bars`, `bars::rollup`, `bars::activity_bars`,
`orderflow::footprints`, `orderflow::deltas`, `orderflow::volume_profiles`,
`orderflow::sweeps`) give the same records as the processor.

### 7.4 Performance

`cargo bench -p book` on a 100,000-order synthetic book (arm64 developer
machine shared with other builds, release profile; indicative only, rerun
on the target host before relying on them):

| Benchmark | Time |
|---|---|
| Apply book events | ~4.4 million events/s |
| Top of book | ~70 ns |
| L2, best 100 levels | ~1.6 µs |
| L4 snapshot out / in | ~3 ms / ~11 ms |
| Reconcile against a clean snapshot | ~55 ms |
| Full processor replay, every analytic on | ~260,000 records/s |

## 8. Data-model gaps

These outputs have no observation kind yet, so they travel as serde JSON
with a derived header rather than as `Record`s: `ActivityBar`, `DeltaBar`,
`ProfileSummary` (POC and value area, which `volume_profile` has no fields
for), `OrderActivity`, `QueuePositions`, `Sweep`, `HeatmapTile`,
`LiquidityGrade`. Adding them is an integrator change to `mesh-types` and
the observation spec; until then the shapes above are the contract.

## 9. Method versions

| Method | Output | Since |
|---|---|---|
| `bars@1` | `bar` | 2026-11-12 |
| `tick-bars@1`, `volume-bars@1`, `range-bars@1` | `ActivityBar` | 2026-11-12 |
| `footprint@1` | `footprint` | 2026-11-12 |
| `delta@1` | `DeltaBar` | 2026-11-12 |
| `volume-profile@1` | `volume_profile` | 2026-11-12 |
| `value-area@1` | `ProfileSummary` | 2026-11-12 |
| `order-activity@1` | `OrderActivity` | 2026-11-12 |
| `queue-position@1` | `QueuePositions` | 2026-11-12 |
| `sweep@1` | `Sweep` | 2026-11-12 |
| `heatmap@1` | `HeatmapTile` | 2026-11-12 |
| `liquidity-grade@1` | `LiquidityGrade` | 2026-11-12 |
