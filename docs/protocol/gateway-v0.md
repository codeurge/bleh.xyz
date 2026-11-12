# Gateway protocol, v0

- Status: Draft; implemented by `crates/gateway`, reference client in
  `tools/mesh-client` (whose `protocol` module holds every message type,
  shared with the gateway)
- Records and batches: [observation-v0](../specs/observation-v0.md)

The gateway streams live records over websocket. A client subscribes by
instrument and kind, receives a book snapshot tagged with its position
followed by contiguous deltas, a heartbeat every block, and (optionally)
signed block commitments it can verify.

## 1. Connection

- Endpoint: `wss://<host>/v0/ws`. A plain `GET` without an upgrade gets
  HTTP 426; more connections from one client IP than
  `limits.connections_per_ip` get HTTP 429 before the upgrade.
- Encoding is chosen with the websocket subprotocol:
  - `bleh.v0.json`: UTF-8 JSON text frames (used if none is offered).
  - `bleh.v0.cbor`: CBOR (RFC 8949) binary frames carrying the same
    structures as the JSON form; decimals stay strings, `*_ns` stay
    strings, so values are identical in both encodings.

  The first supported subprotocol offered is used. If only unsupported
  ones are offered, the server completes the upgrade and closes with 4001.
  A frame of the other type (binary on JSON, text on CBOR) is a
  `bad_request`. A DBN-compatible encoding may be added as `bleh.v0.dbn`
  if ADR 0004 is accepted.
- One message per frame. Every message is an object with an `op` (client
  to server) or `type` (server to client) field. Unknown fields are
  rejected in both directions.
- The server sends a `hello` first:
  `{"type":"hello","protocol":"bleh-gateway-v0","server_time_ns":"...","signers":["0x..."],"limits":{...}}`,
  listing the commitment signer addresses and the limits in § 6.

## 2. Channels

A channel is `(instrument, kind)`, plus `depth` for `book_event` and
`interval` for `bar`. `instrument` is an exact instrument ID, `*` (every
instrument) or a prefix ending in `*` (`hyperliquid:perp:*`). A wildcard
channel covers instruments that appear later too, and counts as **one**
channel towards the limits.

| `kind` | Delivered as |
|---|---|
| `trade` | `batch` messages (one `BlockBatch` per block with trades) |
| `book_event` | a `snapshot`, then `batch` messages (`l4`) or `levels` messages (`l2`) |
| `book_snapshot` | `record` messages (periodic L4 checkpoints) |
| `bar` | `record` messages; `interval` selects one bar length (all if absent) |
| other kinds | `record` messages, when a producer publishes them |

`book_event` subscriptions take `depth`:

- `l4` (default): order-level L4 snapshot, then the book events.
- `l2`: L2 snapshot, then `levels` messages (§ 4.1) with the price levels
  each block changed, instead of the book events. Levels are **derived by
  the gateway** from its L4 book and are **not covered by commitments**;
  clients that verify use `l4`.

`depth` on any other kind, or `interval` on a kind other than `bar`, is
refused with `unsupported_kind`.

## 3. Client messages

| `op` | Fields | Effect |
|---|---|---|
| `subscribe` | `id`, `channels` (array of `{instrument, kind, depth?, interval?}`), `commitments` (bool, default false) | Start channels; answered by `subscribed`, or by one `error` and nothing subscribed |
| `unsubscribe` | `id`, `channels` (exactly as subscribed) | Stop channels; answered by `unsubscribed` |
| `resnapshot` | `id`, `instrument`, `reason` (free text) | Ask for a fresh book snapshot (after the client detects a gap); answered by a `snapshot`, or an `error` |
| `proof` | `id`, `stream` (`{venue, kind}`), `block_number`, `instrument`, `event_index` | Ask for a Merkle inclusion proof for one record (`event_index` 4294967295 for end-of-block records such as checkpoints) |
| `ping` | `id` | Liveness check |

`id` is a client-chosen string echoed in the reply.

## 4. Server messages

| `type` | Fields | Meaning |
|---|---|---|
| `hello` | see § 1 | First message |
| `subscribed` / `unsubscribed` | `id`, `channels` | Acknowledgement |
| `snapshot` | `instrument`, `as_of` (`Seq`), `reason`, `record` (a `book_snapshot`, L4 or L2 by the channel's depth) | Book state after block `as_of.block_number` |
| `batch` | `batch` (a `BlockBatch`) | All records of one channel in one block |
| `levels` | `instrument`, `venue`, `block_number`, `block_time_ms`, `prev_block_number`, `bids`, `asks`, `ts_publish_ns?` | Price levels one block changed, for `l2` subscribers (§ 4.1) |
| `record` | `record` | A single record (checkpoints, bars) |
| `commitment` | `commitment` (a `BlockCommitment`) | Signed Merkle root for one block and stream |
| `block` | `venue`, `block_number`, `block_time_ms`, `ts_publish_ns`, `last` | Per-block heartbeat of one venue (§ 5.3) |
| `proof` | `id`, `commitment`, `proof` (`MerkleProof`), `record` | Inclusion proof |
| `pong` | `id`, `server_time_ns` | Reply to `ping` |
| `error` | `id?`, `code`, `message` | § 7 |

Snapshot `reason` values:

| `reason` | Sent when |
|---|---|
| `subscribe` | First snapshot after `subscribe` (also for an instrument first seen under a wildcard channel, or after the channels changed its depth) |
| `resnapshot` | Answer to the client's `resnapshot` |
| `upstream_gap` | The gateway's upstream skipped a block, or the gateway fell behind its own input (forced) |
| `reconcile` | The gateway's book did not match a checkpoint, or an event did not fit it (forced) |
| `restart` | The gateway's book was rebuilt after a restart (forced) |
| `slow_consumer` | The connection fell behind and its queued data was dropped (§ 6) |

Every record carries `prov.ts_publish_ns`, set when the gateway's upstream
published it, so clients can measure freshness; `levels` carries the same
value as `ts_publish_ns`.

### 4.1 Levels

```json
{"type":"levels","instrument":"hyperliquid:perp:BTC","venue":"hyperliquid",
 "block_number":1000012,"block_time_ms":1767229197000,"prev_block_number":1000009,
 "bids":[{"price":"99.5","size":"3.0","order_count":2}],
 "asks":[{"price":"100.0","size":"0","order_count":0}]}
```

After applying a block's book events for an instrument, the gateway sends
each price level an event touched whose aggregate size or order count
changed, with its new `size` and `order_count`; `size` `"0"` with
`order_count` 0 means the level is gone. Bids are best (highest) first,
asks best (lowest) first; either may be empty. One `levels` message is
sent for every `book_event` batch the gateway applies, even when no level
changed, and it carries that batch's `block_number` and
`prev_block_number`, so contiguity (§ 5.2) works exactly as for batches.

## 5. Ordering and book synchronization

### 5.1 Order per block

For each block, the gateway sends, per connection, in this order:

1. `batch` (or `levels`) messages for subscribed channels that have
   records in the block;
2. `record` messages produced by the block (checkpoints, closed bars);
3. `commitment` messages for the block, if the client asked for them;
4. one `block` heartbeat, which closes the block for its venue.

A `snapshot` can arrive at any point; it reflects every message before it
on the connection.

### 5.2 Snapshot and deltas

1. On `subscribe` to `book_event`, the server sends `snapshot` with
   `as_of = end_of_block(B)`: the book after every event of block `B`.
   For a wildcard channel it does so for each instrument with a book now,
   and for others when their first batch arrives. A gateway serves
   snapshots only once its book has been seeded from a committed
   checkpoint, so the first snapshot can take until the next checkpoint.
2. It then sends `batch` (or `levels`) messages only for blocks greater
   than `B`, in block order, with nothing skipped. Messages for the
   instrument may also arrive before the snapshot (they are for full-stream
   verifiers); clients ignore them.
3. The client applies each batch's book events in `event_index` order (or
   each `levels` message's levels).
4. **Contiguity.** The client tracks, per channel, the last block it
   applied (`B` after the snapshot). The first batch after the snapshot
   must have `prev_block_number <= B` (or null); every later batch must
   have `prev_block_number` equal to the last applied block. Within a
   batch, `event_index` runs `0..n`. Anything else, an event that does not
   fit the book, or a heartbeat `last` past the last applied block (§ 5.3),
   is a **gap**.
5. **On a gap** the client discards its book for that instrument, sends
   `resnapshot`, and ignores the instrument's batches until the new
   `snapshot` arrives; it then continues from step 2 with the new `B`.
6. **Forced re-snapshot.** The server sends a new `snapshot` without being
   asked when it cannot guarantee contiguity (`upstream_gap`, `reconcile`,
   `restart`, `slow_consumer`; § 4). Until it can, it sends no `levels`
   for the instrument, and its heartbeats reveal the missing blocks. The
   client replaces its book and continues.

Trades use the same contiguity rule without snapshots: on a gap the client
knows trades are missing (and can backfill from the historical API).

### 5.3 Heartbeat

```json
{"type":"block","venue":"hyperliquid","block_number":1000012,"block_time_ms":1767229197000,
 "ts_publish_ns":"1767229197010000000","last":{"hyperliquid:perp:BTC|book_event":1000012}}
```

Streams are per venue, so heartbeats are too: one `block` message is sent
for each block of each venue, per connection, whether or not any subscribed
channel had records. `last` maps each channel of that venue with data on
this connection (`"<instrument>|<kind>"`, per instrument also under
wildcard channels) to the last block that had a batch for it (for
`book_event`, whether it was sent as `batch` or `levels`). A client that
sees `last` move past its own last applied block has missed a batch, even
before the next batch arrives. No `block` message for more than 2 seconds
means the stream is stalled.

## 6. Limits

The `hello` message carries the values in force (`hello.limits`):

| Field | Default | Meaning |
|---|---|---|
| `connections_per_ip` | 10 | Open connections per client IP (HTTP 429 beyond) |
| `channels_per_connection` | 200 | Channels per connection (`limit_exceeded` beyond) |
| `messages_per_second` | 20 | Sustained client messages per second |
| `message_burst` | 50 | Client message burst |
| `resnapshot_interval_ms` | 5000 | Minimum interval between `resnapshot`s for one instrument |
| `proofs_per_second` | 50 | `proof` requests per second (burst equal to the rate) |
| `send_buffer_bytes` | 8388608 | Server-side websocket write buffer (at least 256 KiB) |
| `send_buffer_ms` | 2000 | How far behind the newest data a connection may fall |
| `max_message_bytes` | 65536 | Largest client message or frame (close 1009 beyond) |

The gateway also limits channels per client IP over all its connections
(default 2000, `limit_exceeded` beyond); that value is not announced.

Exceeding a rate limit returns `error` with `code: "rate_limited"`. Rate
limit errors and `bad_request` errors count as violations; 10 violations
within 10 seconds close the connection with code 4029.

**Slow consumers.** A connection is behind when its next unread message is
older than `send_buffer_ms`, or when the server's fan-out buffer overran
it. The server never drops messages selectively (that would corrupt
books):

1. **First strike:** it drops everything queued for the connection,
   continues from the newest data, and resynchronizes every book of the
   connection with a forced `slow_consumer` snapshot. Batches of other
   kinds dropped meanwhile show up as gaps in heartbeats.
2. **Second strike** within 60 seconds of the first: it closes the
   connection with code 4008 (`slow_consumer`).

A connection whose socket write does not complete within `send_buffer_ms`
(the client stopped reading and the write buffer is full) is dropped
without a close frame. In every case the client reconnects, resubscribes
and resynchronizes from snapshots.

## 7. Errors and close codes

| `error.code` | Meaning |
|---|---|
| `bad_request` | Malformed message, unknown field, wrong frame type, or a request that does not apply (`resnapshot` for an instrument without a `book_event` channel) |
| `limit_exceeded` | A channel cap (per connection or per client IP) would be exceeded; nothing was subscribed |
| `unknown_instrument` | An exact instrument not in the gateway's registry (wildcards are not checked) |
| `unsupported_kind` | `depth` or `interval` on a kind that does not take it |
| `rate_limited` | Message rate, `resnapshot` interval or `proof` rate exceeded (§ 6) |
| `proof_unavailable` | No proof for that record (unknown, or outside the proof retention window) |

| Close code | Meaning |
|---|---|
| 1001 | Server going away (shutdown, restart, deploy) |
| 1009 | Client message or frame larger than `max_message_bytes` |
| 4000 | Protocol error (the server could not encode a message) |
| 4001 | Unsupported subprotocol |
| 4008 | Slow consumer (second strike, § 6) |
| 4029 | Too many violations (§ 6) |

## 8. Commitments and verification

With `commitments: true`, the server sends a `commitment` for each block
and each stream (venue and kind) covered by the client's channels. A
commitment covers **all** records of that stream in the block, across
instruments (observation-v0 § 7), so:

- **Signature:** the client recovers the signer from
  `commitment.signature` and checks it is in `hello.signers` (and in the
  registry). This proves the root came from the mesh.
- **Inclusion:** a client subscribed to some instruments asks for proofs
  (`proof` op or `GET /proof`) for the records it wants to check and
  verifies each against the root. A client subscribed to every instrument
  of a stream (`*`, and `l4` for `book_event`) can instead recompute the
  root from the records it received.
- `snapshot` and `levels` messages are derived by the gateway and are not
  covered by commitments; checkpoints (`book_snapshot` records) are.
- Verification is optional and off the hot path; the reference client
  verifies signatures always and proofs on a sample.

Each gateway keeps proofs for the newest blocks of each stream (default
100 000 blocks per stream, an operator setting); older records are proven
from the archive.

## 9. HTTP endpoints

| Path | Response |
|---|---|
| `GET /health` | `200` `{"status":"ok"}` when a block closed within the last 5 seconds (and, for a relay, its upstream is connected); `503` `{"status":"unavailable","upstream_connected":bool or null}` otherwise |
| `GET /freshness` | `{"now_ns", "venues", "streams", "instruments"}`; each row has `venue` and/or `instrument` and `kind`, `last_block`, `block_time_ms`, `ts_publish_ns` (when known), `lag_ms` (now − block time) and `age_ms` (now − when this gateway saw it) |
| `GET /metrics` | Prometheus text format |
| `GET /proof?venue=&kind=&block_number=&instrument=&event_index=` | `200` `{"commitment", "proof", "record"}`, as the `proof` message; `404` `{"code":"proof_unavailable","message"}`; `400` `{"code":"bad_request","message"}` |
| `GET /signers` | `{"signers": [...], "keys": [{"id", "address", "active"}]}`; `keys` is empty unless the gateway signs locally |

## 10. Versioning

`v0` is a draft. Breaking changes get a new path (`/v1/ws`) and
subprotocol names; v0 stays available for a deprecation period announced
in `hello`.
