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

## On-chain verification

[`crates/contracts`](../../crates/contracts/README.md) holds a Solidity
verifier for the commitments and proofs this crate produces:
`BlehCommitments` (an internal library) and `BlehBlockVerifier` (a
stateless test contract exposing it), in
[`sol/BlehBlockVerifier.sol`](../../crates/contracts/sol/BlehBlockVerifier.sol).
It accepts exactly what the signer stage signs:

| Function | Checks |
|---|---|
| `verifyCommitment(c)` | `ecrecover` of `c.signature` over the EIP-191 hash of the commitment hash (domain `bleh-block-commitment-v0`, length-prefixed venue and kind, `uint64` block, `uint32` leaf count, root) equals `c.signer`; low `s` and `v` in {27, 28} required, as in Rust |
| `verifyInclusion(leaf, proof, root)` | RFC 9162 § 2.1.3.2 audit path with `keccak256(0x01 ‖ l ‖ r)` nodes |
| `verifyRecord(c, record, proof)` | `proof.leafCount == c.leafCount`, `keccak256(0x00 ‖ record)` is included under `c.merkleRoot`, and `verifyCommitment(c)` |

`c` and `proof` have the fields of the gateway's `commitment` and `proof`
messages (`BlockCommitment`, `MerkleProof`); `record` is the record's
canonical encoding (`mesh_types::hash::canonical_record`). Every function
returns false on invalid input and never reverts (`commitmentHash` alone
reverts on a venue or kind over 255 bytes).

**Trust is the caller's.** A valid commitment proves only that `c.signer`
signed it. The consumer contract must check `c.signer` against the
published, unrevoked signer addresses (the registry), as the reference
client does with its allowlist. `verifyRecord` does not decode the record:
a consumer that reads the record's venue, kind or block must check them
against `c`, as `BlockCommitment::verify_inclusion` does in Rust.

**Tests** (`cargo test -p contracts`) deploy the committed bytecode in an
in-process EVM (`revm`) and drive it with commitments and proofs from
`StreamBlock`, `BlockSigner` and `ProofStore` signed by test keys: every
record of blocks of 1 to 1,024 records verifies, key rotation verifies
under each key, and every tampering the Rust verifier rejects (record
bytes, proof index, count or path, commitment fields, signer, malformed or
high-`s` signatures) the contract rejects too. Gas for `verifyRecord`, as a
whole transaction with a record of about 290 bytes: about 39,000 for a one-record
block and 48,000 for a 1,024-record block (one `ecrecover` plus one hash
per tree level).

**Rebuilding** the bytecode after a source change needs the pinned
compiler (`solc` 0.8.38 from npm, `package-lock.json`):
`cd crates/contracts && npm ci --ignore-scripts && node compile.mjs`. CI
runs `node compile.mjs --check` so the committed bytecode always matches
the source. No deployment happens from this repository.

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
