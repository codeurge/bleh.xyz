# Observation data model, v0

- Status: Draft, implemented by `crates/mesh-types`
- Version tag: `observation-v0`
- JSON Schema: [observation-v0.schema.json](observation-v0.schema.json)

This is the data model every mesh component speaks, as implemented. Where
it refines [architecture.md](../architecture.md) § 1, this document and the
code win; differences are listed at the end.

## 1. Conventions

- **No floating point.** Prices, sizes and notionals are fixed-point: an
  `i64` mantissa and a scale (fractional digits, 0 to 18). The scale is
  fixed per instrument (§ 4). In JSON they are decimal strings with exactly
  `scale` fractional digits: `"100.250"` is mantissa 100250, scale 3.
  Parsers take the scale from the digits, so JSON round-trips exactly. No
  exponent, `+`, leading `.` or trailing `.`.
- **Timestamps** count from the Unix epoch, UTC.
  - `*_ms`: milliseconds, JSON number (venue times; Hyperliquid reports
    milliseconds and the mesh does not invent digits).
  - `*_ns`: nanoseconds, JSON **string** of the integer (values exceed
    2^53, the JavaScript safe-integer limit).
- **IDs and hashes.** SHA-256 digests: 64 lowercase hex digits. Keccak-256
  digests, Ethereum addresses and signatures: `0x` and lowercase hex.
- **Unknown fields are rejected** everywhere. A consumer that sees an
  unknown field is reading a newer version and must not guess.
- **Enumerations** are lowercase snake_case strings.

## 2. Records

Every record is a JSON object with `kind`, a `prov` object and the kind's
fields.

| `kind` | Meaning | Derived |
|---|---|---|
| `trade` | One venue execution | no |
| `book_event` | One order-level book change | no |
| `book_snapshot` | The book after a block (L4 orders or L2 levels) | no |
| `bar` | OHLCV over an interval | yes |
| `footprint` | Per-price volume by aggressor over an interval (stub) | yes |
| `volume_profile` | Volume by price bucket over a range (stub) | yes |
| `swap_print` | A publicly disseminated swap (Part 43 / SBSR) | no |
| `activity_bar` | Tick, volume or range bar (§ 2.10) | yes |
| `delta_bar` | Volume delta and CVD over an interval (§ 2.10) | yes |
| `profile_summary` | Point of control and value area of a volume profile (§ 2.10) | yes |
| `order_activity` | Adds, cancels, fills and order lifetimes over an interval (§ 2.10) | yes |
| `queue_positions` | Top-of-book queues after a block (§ 2.10) | yes |
| `sweep` | An aggressive run across price levels (§ 2.10) | yes |
| `heatmap_tile` | Resting size by time and price (§ 2.10) | yes |
| `liquidity_grade` | A market's liquidity grade (§ 2.10) | yes |
| `coverage` | What the recorder observed of a venue over an interval (§ 2.11) | no |

The order of this table is the canonical kind order (§ 6.2).

### 2.1 Provenance (`prov`)

| Field | Type | Meaning |
|---|---|---|
| `instrument` | InstrumentId | Mesh instrument (§ 4) |
| `venue` | VenueId | Venue or public-data source, e.g. `hyperliquid` |
| `ts_source_ms` | ms | Venue time (Hyperliquid: block or fill time) |
| `ts_ingest_ns` | ns | When the mesh observed the data |
| `ts_publish_ns` | ns, optional | When the mesh published the record. Live only |
| `seq` | Seq | `{block_number, event_index}` (§ 3) |
| `admission` | AdmissionId | Admission covering the record's fields, `ADM-NNNN` |
| `flags` | array of flag names | § 2.9 |

**Determinism.** Archived records must be a pure function of raw files.
`ts_ingest_ns` therefore comes from the raw input where the source records
it: for Hyperliquid, the node's per-block `local_time` (its own receive
time). `ts_publish_ns` is set only on the live path, is never archived, and
is excluded from canonical encodings (§ 6).

### 2.2 `trade`

| Field | Type | Meaning |
|---|---|---|
| `price` | decimal | Execution price |
| `size` | decimal, > 0 | Executed size |
| `aggressor` | `buy`, `sell`, `unknown` | Taker side |
| `trade_id` | integer | Venue trade ID |

Hyperliquid writes one fill per side of a trade. The reader emits one
`trade` per pair; the aggressor is the side of the fill with
`crossed = true`. The globally unique key of a Hyperliquid trade is
`(ts_source_ms, instrument, trade_id)`. A fill pair with no `crossed = true`
side is not an order-book trade and is dropped (see
[hyperliquid-node-format.md](hyperliquid-node-format.md) § 6).

### 2.3 `book_event`

| Field | Type | Meaning |
|---|---|---|
| `order_id` | integer | Venue order ID |
| `side` | `bid`, `ask` | Book side |
| `price` | decimal | Order limit price |
| `action` | object | One of the four below, tagged by `type` |

| `action.type` | Fields | Meaning |
|---|---|---|
| `add` | `size` (> 0) | New resting order |
| `modify` | `orig_size` (optional), `new_size` (> 0) | Size change not caused by a fill |
| `fill` | `orig_size` (optional), `new_size` (>= 0) | Fill against the resting order; `new_size` 0 means removed |
| `cancel` | none | Removed without a fill |

`orig_size` is present when the source reports it (Hyperliquid `update`
diffs). A removal diff carries no prior size; the book holds it.

### 2.4 `book_snapshot`

| Field | Type | Meaning |
|---|---|---|
| `depth.type` | `l4` or `l2` | Granularity |
| `depth.bids`, `depth.asks` | arrays | L4: orders `{order_id, price, size, ts_order_ms}`. L2: levels `{price, size, order_count}` |

- `prov.seq` is the end-of-block marker `{block_number: B, event_index:
  4294967295}`: the state after every event of block `B`. Book events with a
  greater `seq` apply on top.
- Bids are best (highest) first, asks best (lowest) first. In L4, orders at
  one price are in queue (time) order. L2 levels are strictly ordered.
- Sizes are positive, order IDs unique, and the book is never crossed or
  locked.
- Trigger orders are not resting orders and never appear.
- L2 is derived from L4 by summing sizes and counting orders per price.

### 2.5 `bar`

| Field | Type | Meaning |
|---|---|---|
| `derived` | DerivedHeader | § 2.8 (method `bars@1`) |
| `interval` | `1s`, `1m`, `5m`, `15m`, `1h`, `1d` | Length |
| `start_ms` | ms | Inclusive start, a multiple of the interval since the epoch (daily bars start at UTC midnight) |
| `open`, `high`, `low`, `close` | decimal | Prices of the first, highest, lowest and last trades |
| `volume`, `buy_volume`, `sell_volume` | decimal | Total and per-aggressor size; `buy + sell <= volume` (the rest has unknown aggressor) |
| `trade_count` | integer, >= 1 | Number of trades |

No bar is emitted for an interval with no trades. `prov.ts_source_ms` is
the time of the last trade in the bar; `prov.seq` follows the derived-record
rule of § 2.8. `derived.input_count` equals `trade_count`.

### 2.6 `footprint` and `volume_profile` (stubs)

Shapes are fixed; the methods are not yet specified.

- `footprint`: `derived`, `interval`, `start_ms`, `levels[]` of
  `{price, bid_volume, ask_volume, trade_count}` in strictly ascending
  price. `bid_volume` is size traded at the bid (sell aggressor),
  `ask_volume` at the ask (buy aggressor).
- `volume_profile`: `derived`, `start_ms`, `end_ms` (exclusive), `bucket`
  (positive price width), `levels[]` of `{price, volume, buy_volume,
  sell_volume}` in strictly ascending price; empty buckets are omitted.

### 2.7 `swap_print`

For CFTC Part 43 and SEC Regulation SBSR public dissemination (ADM-0002,
ADM-0004, both Admitted), emitted by `crates/swap-pollers`.

| Field | Type | Meaning |
|---|---|---|
| `repository` | VenueId | Disseminating swap data repository |
| `dissemination_id` | string | The repository's ID for the print |
| `original_dissemination_id` | string, optional | Required for `cancel`, `correct` and `revive` |
| `action` | `new`, `cancel`, `correct`, `modify`, `terminate`, `revive` | Effect on the public record (`modify` amends terms, `terminate` ends the transaction, `revive` restores a print cancelled in error) |
| `event_type` | enum, optional | Lifecycle event: `trade`, `novation`, `compression`, `early_termination`, `clearing`, `exercise`, `allocation`, `clearing_allocation`, `credit_event`, `porting`, `corporate_event`, `update` |
| `asset_class` | `interest_rate`, `credit`, `equity`, `foreign_exchange`, `commodity` | |
| `upi` | string, optional | Unique Product Identifier |
| `product` | string, optional | Product description or taxonomy |
| `underlier` | string, optional | Rate index, reference entity or asset |
| `tenor` | string, optional | For example `5Y` |
| `expiry` | `YYYY-MM-DD`, optional | Expiration or end date |
| `price` | object, optional | `{type, value}`; `type` is `price`, `spread` (basis points), `rate` (percent) or `spread_amount` (a spread in currency units) |
| `notional` | object, optional | `{amount, currency}` (ISO 4217) |
| `quantity` | object, optional | `{amount, unit}`: a notional quantity with a unit of measure (commodities), 1 to 16 letters or digits |
| `strike` | object, optional | Option strike, shaped like `price` |
| `option_premium` | object, optional | `{amount, currency}`. Not `premium`, which is an excluded venue field name (§ 8) |
| `option_type` | `call`, `put`, optional | |
| `option_style` | `american`, `european`, `bermudan`, `asian`, optional | |
| `first_exercise_date` | `YYYY-MM-DD`, optional | |
| `ts_execution_ms` | ms | Execution time |
| `ts_dissemination_ms` | ms | Dissemination time; equals `prov.ts_source_ms` |

Capped notionals carry flag `size_capped`; delayed block trades carry
`block_trade_delayed`. For these sources `seq.block_number` is the source
batch number (for example, the repository file or poll) and `event_index`
the position within it.

### 2.8 Derived-record header (`derived`)

| Field | Type | Meaning |
|---|---|---|
| `method` | `name@version` | Versioned method, e.g. `bars@1` |
| `inputs_hash` | SHA-256 hex | Content hash (§ 6.2) of the input records in canonical order |
| `input_count` | integer | Number of input records |

Anyone can recompute a derived record from the archive and compare.

**Position of a derived record.** A derived record is published, and
signed (§ 7), in the block whose processing completed it: block `B` when
`B`'s records close it, or when `B`'s block time passes the end of its
interval. Its `prov.seq` is `(B, i)`, where `i` counts that instrument's
records of that kind published in `B` (`0, 1, 2, ...` in emission order),
so derived streams obey § 3 like every other stream.
`prov.ts_source_ms` stays the venue time of its last input. Records that
describe the state at the end of `B` (`queue_positions`, and
`book_snapshot` checkpoints) keep the end-of-block marker
`(B, 4294967295)`. The archive replay drives the same processor block by
block with the same block times, so live and archived derived records are
identical.

### 2.9 Flags

| Bit | Name | Meaning |
|---|---|---|
| 0 | `oracle_anchored_venue` | The venue anchors funding or price bands to an oracle; the record is still a venue trade or order |
| 1 | `backfill` | Imported from a historical archive, not recorded live |
| 2 | `gap_before` | The source skipped data immediately before this record |
| 3 | `low_resolution` | Lower resolution than the live stream (for example, hourly L2 snapshots) |
| 4 | `size_capped` | The source capped the size or notional |
| 5 | `block_trade_delayed` | The source delayed the record as a block trade |
| 6 | `replayed` | Reconstructed by replaying venue actions through an emulated matcher (for example order-level books rebuilt from a public action archive), not observed; always together with `backfill` |

JSON lists names in bit order without duplicates; Arrow stores the `u32`.
Unknown names and bits are rejected; a new flag is a new minor version.

### 2.10 Analytics kinds

Computed by the book builder (`crates/book`) with the versioned methods of
[book.md](../operations/book.md) § 9, live and in archive replay, from the
same code. All carry `derived`. Ratios are integers (basis points or
hundredths) and signed quantities are decimals that may be negative, so no
record has a float.

| `kind` | Method | Fields |
|---|---|---|
| `activity_bar` | `tick-bars@1`, `volume-bars@1`, `range-bars@1` | `spec` (`{type: tick, trades}`, `{type: volume, size}` or `{type: range, width}`), `session_start_ms`, `first_seq`, `first_ms`, `open`, `high`, `low`, `close`, `volume`, `buy_volume`, `sell_volume`, `trade_count`, `complete` |
| `delta_bar` | `delta@1` | `interval`, `start_ms`, `buy_volume`, `sell_volume`, `volume_delta` (signed: buy minus sell), `anchor_start_ms`, `cvd` (signed: sum of `volume_delta` since the anchor) |
| `profile_summary` | `value-area@1` | `start_ms`, `end_ms`, `bucket`, `value_area_bps`, `poc`, `value_area_low`, `value_area_high`, `value_area_volume`, `total_volume` |
| `order_activity` | `order-activity@1` | `interval`, `start_ms`, `adds`, `modifies`, `cancels`, `partial_fills`, `full_fills`, `added_size`, `cancelled_size`, `filled_size`, `cancel_to_add_bps` (nullable), `cancel_replace_to_fill_bps` (nullable), `lifetimes` (`removed`, `filled`, `cancelled`, `fleeting_cancels`, `min_ms`, `p50_ms`, `p90_ms`, `max_ms`; the times nullable) |
| `queue_positions` | `queue-position@1` | `bid`, `ask` (nullable): `{price, total_size, order_count, orders[{order_id, size, size_ahead, ts_order_ms}], truncated}`; `prov.seq` is an end-of-block marker |
| `sweep` | `sweep@1` | `side` (`buy` or `sell`), `first_seq`, `first_price`, `last_price`, `levels` (at least 2), `size`, `trade_count` |
| `heatmap_tile` | `heatmap@1` | `time_start_ms`, `time_step_ms`, `cols`, `price_start`, `price_step`, `rows`, `cells[{col, row, bid_size, ask_size}]` (by column, then row) |
| `liquidity_grade` | `liquidity-grade@1` | `as_of_ms`, `window_ms`, `samples`, `spread_bps` (decimal, two places, nullable), `depth_bps`, `depth_bid`, `depth_ask` (quote units), `trades`, `trades_per_minute` (two places), `grade` (`A` to `D`), `label` (`benchmark` or `tape`) |

### 2.11 `coverage`

What the recorder observed of a venue over one interval, so a quiet market
(blocks observed, no records) and a recording gap (blocks missing or
flagged) can be told apart, live and in the archive. Not derived: it comes
from the raw block lines.

| Field | Type | Meaning |
|---|---|---|
| `interval` | interval | Length (`1m` by default) |
| `start_ms` | ms | Interval start, aligned, by block time |
| `first_block`, `last_block` | integer | First and last block observed in the interval |
| `blocks` | integer, >= 1 | Blocks observed |
| `missing_blocks` | integer | Block numbers never observed between the previous observed block and `last_block` |
| `gap_blocks` | integer | Observed blocks flagged `gap_before` (data skipped before or inside them) |

- `prov.instrument` is the venue-wide pseudo-instrument
  `<venue>:venue:all` (§ 4); `prov.seq` is `(B, 0)`, where `B` is the
  block in which the interval closed and the record was published (the
  first block after the interval, or `last_block` at the end of input);
  `prov.ts_source_ms` is the last observed block's time and
  `prov.ts_ingest_ns` the same instant in nanoseconds, so the record is a
  pure function of the raw files.
- A registered market with no records in an interval whose coverage has
  `missing_blocks = 0` and `gap_blocks = 0` was quiet, not unrecorded. An
  interval with no coverage record was not recorded.

## 3. Sequence numbers

`seq = {block_number: u64, event_index: u32}`, ordered by block, then index.

- `block_number` is the L1 block height for chain sources and the batch
  number for others.
- `event_index` is the zero-based position within the block **for one
  instrument and one record kind**, in source order. A subscriber to one
  instrument and kind sees `0, 1, 2, ...` in every block.
- `event_index = 4294967295` is reserved for end-of-block markers
  (snapshots).

Hyperliquid writes fills, order statuses and book diffs to separate files,
so the relative order of a trade and a book event within one block is not
observable. The model does not invent one.

## 4. Identifiers

| Type | Grammar | Example |
|---|---|---|
| VenueId | `[a-z0-9][a-z0-9-]{0,31}` | `hyperliquid` |
| InstrumentId | `venue ":" class ":" symbol`, at most 128 bytes | `hyperliquid:perp:xyz:CL` |
| AdmissionId | `ADM-` and 4 to 9 digits | `ADM-0001` |
| MethodId | `[a-z0-9][a-z0-9-]{0,47}@[1-9][0-9]{0,5}` | `bars@1` |

InstrumentId: `class` is `[a-z0-9][a-z0-9-]{0,15}` (`perp`, `spot`,
`outcome`, `swap`); `symbol` is one or more `:`-separated segments of
`[A-Za-z0-9_.@#+-]`. The symbol is the venue's own market name, so any
market in venue metadata is representable without a hard-coded list:

| Market | Venue symbol | InstrumentId |
|---|---|---|
| Hyperliquid core perpetual | `BTC` | `hyperliquid:perp:BTC` |
| Hyperliquid HIP-3 deployer market | `xyz:CL` | `hyperliquid:perp:xyz:CL` |
| Hyperliquid spot | `@107`, `PURR/USDC` | `hyperliquid:spot:@107`, `hyperliquid:spot:PURR-USDC` |
| Hyperliquid HIP-4 outcome market | as in node metadata (format to verify) | `hyperliquid:outcome:<symbol>` |

`/`, `=`, `%`, `~`, whitespace and quotes are excluded so IDs are safe in
partition paths, where `:` is written `~` and `#` is written `%23` (a raw
`#` starts a URL fragment); see
[archive-layout-v0.md](archive-layout-v0.md). `%` never occurs in an ID,
so the encoding is unambiguous.

Records about a whole venue rather than one market (`coverage`) use the
pseudo-instrument `<venue>:venue:all`. No market has class `venue`.

The registry type (`InstrumentSpec`) carries the venue symbol, the HIP-3
namespace, the price and size scales, default flags and the admission. For
Hyperliquid, size scale is `szDecimals` and price scale is
`6 - szDecimals` (perpetuals) or `8 - szDecimals` (spot), from the venue's
documented tick rule; outcome markets take an explicit scale until their
rule is confirmed.

## 5. Block batches (bus unit)

`BlockBatch` carries every record of one instrument and one kind
(`trade` or `book_event`) from one block:

| Field | Meaning |
|---|---|
| `instrument`, `venue`, `kind`, `block_number`, `block_time_ms` | Shared by every record |
| `prev_block_number` | Previous block with a batch for this instrument and kind; `null` only for a reader's first batch |
| `records` | The records, with `event_index` `0..n` and no holes |

Blocks with no records for an instrument publish nothing. A consumer that
last saw block `b` detects a gap when a batch's `prev_block_number` is not
`b`. Snapshots, bars and other derived records travel as single records.

## 6. Canonical encoding and hashes

### 6.1 Canonical encoding of a record

1. Serialize to JSON as specified here.
2. Remove `prov.ts_publish_ns`.
3. Write with object keys sorted by byte order, no whitespace, integers in
   shortest form, strings with minimal escapes (`\"`, `\\`, `\b`, `\f`,
   `\n`, `\r`, `\t`, other control characters as `\u00xx` in lowercase hex,
   everything else raw UTF-8).

With no floats in the model, this coincides with RFC 8785 (JCS).
`mesh_types::canonical` writes trades, book events and book snapshots
directly (no intermediate JSON tree); the reader, signer, book builder and
archive share it, and tests check it byte for byte against the generic
path.

### 6.2 Content hash of a sequence

```text
content_hash = SHA-256( "bleh-observation-v0\n"
                        || for each record: u64_be(len(c)) || c )
```

`c` is the canonical encoding. Order matters; archives use canonical order:
instrument, then kind (in the order of the § 2 table), then `seq`. The hash
never depends on Parquet or other container bytes, so different writers of
the same records agree on it. Manifests list it per partition file.

## 7. Signed scope: block commitments

From Phase 1, the publisher signs every block, per stream.

- **Stream:** a venue and a record kind (`hyperliquid` / `trade`).
- **Leaves:** the canonical encodings of all records of that stream in the
  block, across all instruments, ordered by instrument then `seq`.
- **Tree:** RFC 9162 Merkle tree with Keccak-256:
  `leaf = keccak256(0x00 || c)`, `node = keccak256(0x01 || left || right)`,
  split at the largest power of two below `n`, odd nodes promoted (never
  duplicated), `root([]) = keccak256("")`.
- **Commitment hash:**

  ```text
  h = keccak256( "bleh-block-commitment-v0"
                 || uint8(len(venue)) || venue
                 || uint8(len(kind))  || kind
                 || uint64_be(block_number)
                 || uint32_be(leaf_count)
                 || merkle_root )
  ```

  equal to `keccak256(abi.encodePacked(...))` in Solidity with those types.
- **Signature:** secp256k1 over the EIP-191 personal-message hash
  `keccak256("\x19Ethereum Signed Message:\n32" || h)`, encoded as 65 bytes
  `r || s || v`, low `s`, `v` in `{27, 28}`. Contracts verify with
  `ecrecover`.
- **`BlockCommitment`** = `{stream: {venue, kind}, block_number,
  leaf_count, merkle_root, signer, signature}`; `signer` is an Ethereum
  address. Signer keys are published in the registry and can be revoked.
- **Inclusion proofs** (`MerkleProof` = `{leaf_index, leaf_count, path}`)
  are RFC 9162 audit paths, verified with the RFC 9162 § 2.1.3.2
  algorithm, and are served on demand.

What is signed is exactly the archived content: `ts_publish_ns` is outside
the signature, everything else in a record is inside it. A client that
received a whole stream for a block recomputes the root; a client that
received part of it verifies proofs.

## 8. Field allowlist

There are no fields for anything ADM-0001 excludes: owner addresses;
oracle, mark, external and funding prices; liquidation details (including
`markPx`); halt-settlement fills; miscellaneous venue events; fees, PnL,
position data, transaction hashes, client order IDs and builder data.

Two mechanisms enforce this:

1. **Type level.** No record type has such a field, and every type rejects
   unknown fields on input.
2. **Runtime.** `mesh_types::allowlist` lists every permitted JSON field
   path per kind, independently of the structs, plus a denylist of venue
   field names (`markPx`, `oraclePx`, `funding`, `liquidation`, `user`,
   `hash`, `closedPnl`, `premium`, `delta`, ...). Mesh fields avoid those
   names even where the meaning is unrelated (`option_premium`,
   `volume_delta`). `check_record` fails on any path not on the
   list or any denylisted name at any depth. Output stages run it in tests
   over all synthetic fixtures; adding a field to a struct without updating
   the allowlist fails the build.

## 9. Arrow schemas

`mesh_types::arrow::schema(kind)` gives one flat schema per kind for the
normalized partitions:

- Leading provenance columns: `instrument`, `venue`, `ts_source_ms`
  (timestamp ms, UTC), `ts_ingest_ns` (timestamp ns, UTC), `block_number`,
  `event_index`, `admission`, `flags` (`u32`). No `ts_publish_ns`.
- Fixed-point values are `Int64` mantissas with `price_scale` and
  `size_scale` (`UInt8`) columns; swap prints carry per-row scales.
- `book_event.size` is the size after the action (null for a cancel), with
  `orig_size`.
- Snapshot sides and analytics levels are lists of structs.
- Derived kinds add `method`, `inputs_hash` (fixed binary 32) and
  `input_count`.
- Analytics kinds and coverage (§§ 2.10–2.11) are flat: one column per
  top-level field. Field metadata `bleh.encoding` says how a column holds
  its value: `price` or `size` (an `Int64` mantissa at the row's
  `price_scale` or `size_scale`; signed values allowed), `decimal` (exact
  decimal text, for method-defined scales such as basis points with two
  places), `json` (the canonical JSON of a nested value: activity-bar
  `spec`, `first_seq`, order lifetimes, queue levels, heatmap cells);
  otherwise a plain scalar. Kinds with no `price` or `size` column
  (`liquidity_grade`, `coverage`) have no scale columns. Parquet does not
  keep field metadata, so readers take the encoding from the schema.
- Swap prints add `quantity`, `quantity_scale`, `quantity_unit`,
  `event_type`, `strike_type`, `strike`, `strike_scale`, `option_premium`,
  `option_premium_scale`, `option_premium_currency`, `option_type`,
  `option_style` and `first_exercise_date` (all nullable).
- Schema metadata: `bleh.schema = observation-v0`, `bleh.kind = <kind>`.

## 10. Versioning

`observation-v0` is a draft and may change until the first public release.
After that, adding a kind, flag or optional field is a minor version;
anything else is a new major version with a decision record. Canonical
encoding changes always need a new major version, because they change
every hash and signature.

## 11. Differences from architecture.md § 1

| architecture.md | This spec | Why |
|---|---|---|
| `ts_source` in nanoseconds | `ts_source_ms` in milliseconds | The venue reports milliseconds |
| `node`, `sig` per record | Per-block, per-stream commitments (§ 7) | One signature per block and stream instead of per record; proofs on demand |
| `seq`: venue sequence where one exists | `(block_number, event_index)` for every record | Total order and per-subscription gap detection |
| `quote`, `rate`, `reference` kinds | Not yet defined | Not needed for Phases 1 and 2 |
| `source_ref` | `seq` (block and index) | Points back to the raw record |
| Kinds `book_add`, `book_modify`, `book_cancel` | `book_event` with `action` | One kind per stream; fills distinguished from cancels |
