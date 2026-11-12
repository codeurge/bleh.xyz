# Bus, block signer and side tap

Runbook for `crates/bus`. Design: [ADR 0006](../decisions/0006-phase-1-2-service-architecture.md);
data model and signatures: [observation-v0](../specs/observation-v0.md) § 5–7.

## What it does

```
reader ──▶ BlockPublisher ──▶ ring ──▶ gateway hub ──▶ connections
              │   (signer stage)
              ├──▶ ProofStore (trees for inclusion proofs)
              └──▶ SideTap (NATS JetStream; durability and replay only)
```

- **Ring** (`bus::ring`): bounded, in-process broadcast. Every receiver
  has its own cursor; producers never wait. A receiver that falls more
  than `capacity` items behind gets `Lagged(n)` and resumes at the oldest
  retained item. Default capacity 65,536 items (`bus::DEFAULT_RING_CAPACITY`).
- **Publisher** (`BlockPublisher`): per block, the reader calls
  `publish_batch` / `publish_record` for each batch and single record, then
  `end_block`. Batches reach the ring immediately; their leaves are hashed
  right after. `end_block` signs one `BlockCommitment` per stream (venue and
  kind) that had records, stores the trees, and publishes the commitments
  and a `BlockEnd`, so the per-block order is batches, records,
  commitments, block end (gateway-v0 § 5.1). `prov.ts_publish_ns` is
  stamped here once if absent. Inputs are validated (`BlockBatch::validate`,
  `Record::validate`) unless `without_validation()` is used; a block that
  is still open rejects items for another block.
- **Signer stage**: leaves are the canonical encodings of all records of a
  stream in the block, across instruments, sorted by instrument then
  `seq` (stable, so equal keys such as bars of different intervals keep
  publication order). RFC 9162 tree with Keccak-256, commitment hash and
  EIP-191 signature exactly as in observation-v0 § 7.
- **Resync** (`BlockPublisher::resync`): authoritative book state outside
  the committed stream (for example after the reader resynchronized from
  the node's `fileSnapshot`). Gateways reseed and send forced snapshots.

## Keys

A key is 64 hex digits (optional `0x`, whitespace ignored) in a file or an
environment variable. Specify keys as `ID=file:PATH` or `ID=env:VAR`
(`bus::KeySpec`). The ID is an operator label, for example `2026-11`.

- Key material never appears in logs, errors or `Debug` output; only the ID
  and Ethereum address do. Buffers that held key text are zeroed.
- A key file readable by group or others logs a warning; use `chmod 600`
  and a secrets mount. Prefer files over environment variables (the
  environment is visible to the process's children and in `/proc`).
- Never commit keys. `*.key` and `*.pem` are ignored by git; test keys in
  tests are well-known constants and must never sign real data.

**Rotation** (`BlockSigner::add_key`, `rotate`, `retire`):

1. Generate the new key in the KMS or on the signing host.
2. Publish its address in the registry, and start the process with both
   keys (`--signing-key old=file:... --signing-key new=file:...
   --active-key old`). `hello.signers` and `/signers` now list both, so
   clients accept either.
3. Rotate: restart with `--active-key new` (or call `rotate("new")` in
   process). New commitments are signed by the new key.
4. After clients have picked up the change, remove the old key from the
   process and revoke it in the registry.

## Proof store

`ProofStore::new(retention_blocks)` keeps the newest `retention_blocks`
block numbers per stream (default 100,000 in the gateway binary, about
2.8 hours at 10 blocks per second). Memory is roughly one leaf hash
(32 bytes) plus a shared reference per record. The protocol asks for at
least 24 hours of proofs (gateway-v0 § 8): size the retention for the
block rate and memory available, and serve older proofs from the archive.

## JetStream side tap

`bus::jetstream::JetStreamTap` (feature `jetstream`, on by default).
Never on the hot path: `publish` enqueues (bounded queue, default 65,536)
and drops with a counter when full; a background task publishes with
pipelined acknowledgements.

| Setting | Default |
|---|---|
| Stream | `BLEH_V0`, subjects `bleh.v0.>` |
| Replay window (`max_age`) | 600 s |
| Size bound (`max_bytes`, discard old) | 4 GiB |
| Deduplication window | min(replay window, 120 s), by `Nats-Msg-Id` |
| Storage | file |

Subjects, one per instrument and kind (`bus::tap::subject`):

| Event | Subject |
|---|---|
| batch, record | `bleh.v0.rec.<kind>.<instrument>` |
| commitment | `bleh.v0.commit.<venue>.<kind>` |
| block end | `bleh.v0.block.<venue>` |
| resync | `bleh.v0.resync.<instrument>` |

`<instrument>` is the instrument ID with `:` written `~` and `.` written
`%2E` (one subject token). Payloads are the JSON form of `bus::BusEvent`.

**Replay on restart.** The gateway binary, given `--nats-url`, replays the
stream with an ordered consumer before going live: books are rebuilt from
the newest checkpoint in the window plus later deltas, freshness is
restored, and proof trees are rebuilt for every block whose commitment
matches. Live events for blocks already replayed are skipped. Make the
window longer than the book checkpoint interval, or books stay invalid
until the next checkpoint.

**Integration tests** need a local server and are ignored by default:

```bash
nats-server -js &                  # or: docker run -p 4222:4222 nats -js
cargo test -p bus -- --ignored jetstream
```

## Metrics

Exposed on the gateway's `/metrics` when the gateway binary owns the bus:

| Metric | Meaning |
|---|---|
| `bus_ring_published_total` | Items published on the bus ring |
| `bus_ring_lag_events_total` | Times a bus receiver lagged (the hub: books invalidated) |
| `bus_records_published_total` | Records published |
| `signer_commitments_total` | Commitments signed |
| `signer_sign_nanoseconds_total` | Time building roots and signing |

`RingSender::stats`, `BlockPublisher::stats` and `SideTap::stats` expose
the same counters to an embedding process.
