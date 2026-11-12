# Gateway protocol, v0

- Status: Draft; implemented by `crates/gateway`, reference client in
  `tools/mesh-client`
- Records and batches: [observation-v0](../specs/observation-v0.md)

The gateway streams live records over websocket. A client subscribes by
instrument and kind, receives a book snapshot tagged with its position
followed by contiguous deltas, a heartbeat every block, and (optionally)
signed block commitments it can verify.

## 1. Connection

- Endpoint: `wss://<host>/v0/ws`.
- Encoding is chosen with the websocket subprotocol:
  - `bleh.v0.json`: UTF-8 JSON text frames (default if none is offered).
  - `bleh.v0.cbor`: CBOR (RFC 8949) binary frames carrying the same
    structures as the JSON form; decimals stay strings, `*_ns` stay
    strings, so values are identical in both encodings.

  A DBN-compatible encoding may be added as `bleh.v0.dbn` if ADR 0004 is
  accepted.
- One message per frame. Every message is an object with an `op` (client
  to server) or `type` (server to client) field.
- The server sends a `hello` first:
  `{"type":"hello","protocol":"bleh-gateway-v0","server_time_ns":"...","signers":["0x..."],"limits":{...}}`,
  listing the commitment signer addresses and the limits in § 6.

## 2. Channels

A channel is `(instrument, kind)`:

| `kind` | Delivered as |
|---|---|
| `trade` | `batch` messages (one `BlockBatch` per block with trades) |
| `book_event` | a `snapshot`, then `batch` messages |
| `book_snapshot` | `record` messages (periodic L4 or L2 snapshots) |
| `bar` | `record` messages; `interval` selects the bar length |
| `footprint`, `volume_profile` | `record` messages, when implemented |

`book_event` subscriptions take `depth`: `l4` (order-level book events and
L4 snapshot, default) or `l2` (the same events, with an L2 snapshot for
clients that only keep levels).

## 3. Client messages

| `op` | Fields | Effect |
|---|---|---|
| `subscribe` | `id`, `channels` (array of `{instrument, kind, depth?, interval?}`), `commitments` (bool, default false) | Start channels |
| `unsubscribe` | `id`, `channels` | Stop channels |
| `resnapshot` | `id`, `instrument`, `reason` | Ask for a fresh book snapshot (after the client detects a gap) |
| `proof` | `id`, `stream` (`{venue, kind}`), `block_number`, `instrument`, `event_index` | Ask for a Merkle inclusion proof for one record |
| `ping` | `id` | Liveness check |

`id` is a client-chosen string echoed in the reply.

## 4. Server messages

| `type` | Fields | Meaning |
|---|---|---|
| `hello` | see § 1 | First message |
| `subscribed` / `unsubscribed` | `id`, `channels` | Acknowledgement |
| `snapshot` | `instrument`, `as_of` (`Seq`), `reason`, `record` (a `book_snapshot`) | Book state after block `as_of.block_number` |
| `batch` | `batch` (a `BlockBatch`) | All records of one channel in one block |
| `record` | `record` | A single record (bars, periodic snapshots) |
| `commitment` | `commitment` (a `BlockCommitment`) | Signed Merkle root for one block and stream |
| `block` | `block_number`, `block_time_ms`, `ts_publish_ns`, `last` | Per-block heartbeat (§ 5.3) |
| `proof` | `id`, `commitment`, `proof` (`MerkleProof`), `record` | Inclusion proof |
| `pong` | `id`, `server_time_ns` | Reply to `ping` |
| `error` | `id?`, `code`, `message` | § 7 |

Every record carries `prov.ts_publish_ns`, set when the gateway's upstream
published it, so clients can measure freshness.

## 5. Ordering and book synchronization

### 5.1 Order per block

For each block, the gateway sends, per connection, in this order:

1. `batch` messages for subscribed channels that have records in the
   block;
2. `record` messages produced by the block (closed bars, snapshots);
3. `commitment` messages for the block, if the client asked for them;
4. one `block` heartbeat, which closes the block.

### 5.2 Snapshot and deltas

1. On `subscribe` to `book_event`, the server sends `snapshot` with
   `as_of = end_of_block(B)`: the book after every event of block `B`.
2. It then sends `batch` messages only for blocks greater than `B`, in
   block order, with nothing skipped.
3. The client applies each batch's book events in `event_index` order.
4. **Contiguity.** The client tracks, per channel, the last block it
   applied (`B` after the snapshot). The first batch after the snapshot
   must have `prev_block_number <= B` (or null); every later batch must
   have `prev_block_number` equal to the last applied block. Within a
   batch, `event_index` runs `0..n`. Anything else is a **gap**.
5. **On a gap** the client discards its book for that instrument, sends
   `resnapshot`, and ignores the instrument's batches until the new
   `snapshot` arrives; it then continues from step 2 with the new `B`.
6. **Forced re-snapshot.** The server sends a new `snapshot` without being
   asked when it cannot guarantee contiguity: its upstream reported a gap
   (`reason: "upstream_gap"`), its book reconciliation failed
   (`reason: "reconcile"`), or it restarted from a replay window
   (`reason: "restart"`). The client replaces its book and continues.

Trades use the same contiguity rule without snapshots: on a gap the client
knows trades are missing (and can backfill from the historical API).

### 5.3 Heartbeat

The `block` message is sent once per block per connection, whether or not
any subscribed channel had records. `last` maps each subscribed channel
(`"<instrument>|<kind>"`) to the last block that had a batch for it. A
client that sees `last` move past its own last applied block has missed a
batch, even before the next batch arrives. No `block` message for more than
2 seconds means the stream is stalled.

## 6. Limits

Defaults (the `hello` message carries the values in force):

| Limit | Default |
|---|---|
| Connections per IP | 10 |
| Channels per connection | 200 |
| Client messages | 20 per second, burst 50 |
| `resnapshot` | 1 per instrument per 5 seconds |
| `proof` | 50 per second |
| Send buffer per connection | 8 MiB or 2 seconds behind the newest block, whichever comes first |

Exceeding a rate limit returns `error` with `code: "rate_limited"`;
repeated violations close the connection with code 4029.

**Slow consumers.** When a connection's send buffer exceeds its limit, the
server does not drop messages selectively (that would corrupt books). It
closes the connection with code 4008 (`slow_consumer`). The client
reconnects, resubscribes and resynchronizes from snapshots.

## 7. Errors and close codes

| `error.code` | Meaning |
|---|---|
| `bad_request` | Malformed message or unknown field |
| `unknown_instrument` | Not in the registry |
| `unsupported_kind` | Kind or depth not offered |
| `rate_limited` | § 6 |
| `proof_unavailable` | Block outside the proof retention window |

| Close code | Meaning |
|---|---|
| 1001 | Server going away (restart, deploy) |
| 4000 | Protocol error |
| 4001 | Unsupported protocol version or subprotocol |
| 4008 | Slow consumer |
| 4029 | Rate limited |

## 8. Commitments and verification

With `commitments: true`, the server sends a `commitment` for each block
and each stream (venue and kind) of the client's channels. A commitment
covers **all** records of that stream in the block, across instruments
(observation-v0 § 7), so:

- **Signature:** the client recovers the signer from
  `commitment.signature` and checks it is in `hello.signers` (and in the
  registry). This proves the root came from the mesh.
- **Inclusion:** a client subscribed to some instruments asks for proofs
  (`proof` op) for the records it wants to check and verifies each against
  the root. A client subscribed to every instrument of a stream can instead
  recompute the root from the records it received.
- Verification is optional and off the hot path; the reference client
  verifies signatures always and proofs on a sample.

Proofs are kept for at least 24 hours of blocks; older records are proven
from the archive.

## 9. HTTP endpoints

| Path | Response |
|---|---|
| `GET /health` | `200` with `{"status":"ok"}` when the gateway is connected upstream and within freshness limits, `503` otherwise |
| `GET /freshness` | Per venue and stream: last block, its block time, its publish time, and lag to now |

## 10. Versioning

`v0` is a draft. Breaking changes get a new path (`/v1/ws`) and
subprotocol names; v0 stays available for a deprecation period announced
in `hello`.
