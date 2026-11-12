# Reference client

Runbook for `tools/mesh-client` (library and binary). Wire protocol:
[gateway-v0](../protocol/gateway-v0.md).

## Run

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

## What it guarantees

- **Books.** Per `book_event` instrument it keeps an L4 book from the
  snapshot plus contiguous batches. The first batch after a snapshot must
  have `prev_block_number <= B`; later ones must link to the last applied
  block. A broken link, an invalid batch, an event that does not fit the
  book, or a heartbeat `last` past the last applied block is a **gap**: the
  book is discarded, `resnapshot` is sent (at most once per instrument per
  `hello.limits.resnapshot_interval_ms`), and the instrument is ignored
  until the next snapshot. Forced snapshots from the gateway replace the
  book. With `l2` depth no order-level book can be kept (see the protocol
  proposals); the snapshot is reported only.
- **Trades and other batched kinds:** contiguity is checked and gaps
  reported; data keeps flowing (backfill from the historical API).
- **Signatures:** every commitment's signer must be trusted and its
  signature valid.
- **Roots:** for kinds subscribed with `*` (the whole stream) the client
  recomputes each block's root from the records it received and compares.
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
    // client.book(&instrument), client.stats()
}
```

`mesh_client::protocol` holds every gateway-v0 message type; the gateway
uses the same definitions.

## TLS

`wss://` uses rustls with the operating system's root certificates; no
system OpenSSL.
