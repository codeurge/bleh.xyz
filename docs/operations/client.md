# Reference client

Runbook for `tools/mesh-client` (library and binary). Wire protocol:
[gateway-v0](../protocol/gateway-v0.md).

## Run

Streaming is the default command (also available as `mesh-client stream`
with the same flags); `probe` and `keys` are subcommands.

```bash
cargo run -p mesh-client -- \
  --url wss://a.example/v0/ws --url wss://b.example/v0/ws \
  --sub '*|book_event' --sub '*|trade' \
  --signer 0x<registry signer address> \
  --proof-every 500 --record stream.jsonl --duration 600
```

| Flag | Default | |
|---|---|---|
| `--url` | required, repeatable | Tried in order; rotated on failure or disconnect, with backoff after a full round (250 ms doubling to 10 s) |
| `--sub` | required, repeatable | `INSTRUMENT\|KIND[\|l4\|l2\|INTERVAL]`; instrument `*` or `prefix*` for wildcards |
| `--encoding` | `json` | `json` (`bleh.v0.json`) or `cbor` (`bleh.v0.cbor`) |
| `--no-commitments` | off | Do not ask for commitments |
| `--signer` | none | Trusted signer addresses. **Without any, the client trusts whatever signers the gateway announces in `hello`** |
| `--proof-every` | 0 | Ask for an inclusion proof every N records |
| `--record` | none | JSON lines of events (`-` for stdout); `--record-events data` keeps only snapshots, batches, records, commitments and blocks |
| `--stall-ms` | 2000 | Reconnect when no block heartbeat arrives for this long |
| `--max-failures` | unlimited | Exit after N consecutive connection failures |
| `--summary-every` | 50 | Log books, gaps, verification counts and publish-to-receive latency every N blocks |

On exit it prints its counters as JSON on stderr.

## Probe

`mesh-client probe` checks that a gateway serves one channel, for
availability monitoring (`deploy/observability/prober/ws-probe.sh` with
`PROBE_CLIENT=mesh-client` or `docker`):

```bash
mesh-client probe --url wss://a.example/v0/ws \
  --instrument hyperliquid:perp:BTC --kind book_event --timeout 5s
{"ok":true,"first_block_seconds":0.061,"block_time_ms":1767229197000,"snapshot_seconds":0.004}
```

It connects, subscribes (without commitments), waits for the first block
heartbeat and, for `book_event`, the book snapshot, then prints exactly one
JSON line on stdout and exits 0 when ok, 1 otherwise. Logs go to stderr
(`warn` by default).

| Field | |
|---|---|
| `ok` | Heartbeat (and snapshot) received within the timeout |
| `first_block_seconds` | From start (before connecting) to the first heartbeat; `null` if none |
| `block_time_ms` | Block time of the last heartbeat received; `null` if none |
| `snapshot_seconds` | `book_event` only: from start to the snapshot |
| `error` | Only when not ok: connect failure, disconnect, server error (with its code, e.g. `unknown_instrument`), missing signer, or `no block heartbeat within 5s` |

| Flag | Default | |
|---|---|---|
| `--url` | required | One gateway URL; no failover |
| `--instrument` | required | Instrument ID (or `*`, `prefix*`) |
| `--kind` | required | Record kind, e.g. `trade`, `book_event` |
| `--depth` | none | `l4` or `l2` (`book_event` only) |
| `--interval` | none | Bar interval (`bar` only) |
| `--signer` | none | Repeatable; the probe fails if `hello.signers` lacks any of them |
| `--timeout` | `5s` | `500ms`, `5s`, `2m`, or plain seconds; covers the connection too |
| `--encoding` | `json` | `json` or `cbor` |

## Signing-key address

```bash
mesh-client keys address --key-file /run/secrets/block-key
0x2c7536e3605d9c16a7a3d7b1898e529396a65c23
```

Reads a secp256k1 secret as 64 hex characters (optional `0x`, trailing
whitespace allowed) and prints its Ethereum address. The key is never
printed, and error messages never quote the file. A file that cannot be
read, has the wrong format, or holds an invalid secret exits 1 with a
message on stderr; a file readable or writable by group or others prints
a warning on stderr (the address is still printed).

## What it guarantees

- **Books.** Per `book_event` instrument it keeps an L4 book from the
  snapshot plus contiguous batches. The first batch after a snapshot must
  have `prev_block_number <= B`; later ones must link to the last applied
  block. A broken link, an invalid batch, an event that does not fit the
  book, or a heartbeat `last` past the last applied block is a **gap**: the
  book is discarded, `resnapshot` is sent (at most once per instrument per
  `hello.limits.resnapshot_interval_ms`), and the instrument is ignored
  until the next snapshot. Forced snapshots from the gateway replace the
  book. With `l2` depth it keeps an L2 book from the L2 snapshot plus the
  gateway's `levels` messages, under the same contiguity, gap and
  resnapshot rules. Levels are derived by the gateway and are not
  covered by commitments; to verify the book, subscribe at `l4`.
- **Trades and other batched kinds:** contiguity is checked and gaps
  reported; data keeps flowing (backfill from the historical API).
- **Signatures:** every commitment's signer must be trusted and its
  signature valid.
- **Roots:** for kinds subscribed with `*` (the whole stream; for
  `book_event` only at `l4` depth) the client recomputes each block's root
  from the records it received and compares.
  Blocks before the first full block of a connection, and the block in
  progress when the gateway drops the client as a slow consumer, are not
  root-checked (they can be partial).
- **Proofs:** with `--proof-every N`, every Nth record is checked with an
  inclusion proof requested after its block closed; the returned record
  must equal the one received (canonically) and verify under the signed
  root.
- **Failover:** on disconnect, stall or close (1001, 4008, 4029), the
  client moves to the next URL, resubscribes and resynchronizes from
  snapshots.

## Library

```rust
let mut client = mesh_client::Client::new(mesh_client::ClientConfig {
    urls: vec!["wss://a.example/v0/ws".into()],
    channels: vec!["*|book_event".parse()?],
    signers: vec!["0x...".parse()?],
    ..Default::default()
});
loop {
    let event = client.next_event().await?;   // ClientEvent
    // client.book(&instrument), client.l2_book(&instrument), client.stats()
}
```

`mesh_client::protocol` holds every gateway-v0 message type; the gateway
uses the same definitions.

## TLS

`wss://` uses rustls with the operating system's root certificates; no
system OpenSSL.
