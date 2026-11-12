# Gateway and relay

Runbook for `crates/gateway`. Wire protocol:
[gateway-v0](../protocol/gateway-v0.md). Bus and keys: [bus.md](bus.md).

## Modes

| Mode | Data source | Signs | Use |
|---|---|---|---|
| Library | `bus` ring from the node reader in the same process | yes | Production origin (reader lane wires `BlockPublisher` to `gateway::Gateway`) |
| `gateway demo` | Synthetic `tools/hl-synth` feed on a loop | yes | Local development, demos, load tests |
| `gateway relay` | An upstream gateway, verified | no (re-serves upstream signatures) | Anyone can run one |

```bash
# Local demo with an in-memory key:
cargo run -p gateway -- demo --ephemeral-key
# Demo signed with a key file, exercising upstream gaps every 50 blocks:
cargo run -p gateway -- demo --signing-key k1=file:/run/secrets/block-key --gap-every 50
# Relay:
cargo run -p gateway -- relay --listen 0.0.0.0:8080 \
  --upstream wss://a.example/v0/ws --upstream wss://b.example/v0/ws \
  --signer 0x<published signer address>
```

### Configuration from the environment

Every flag of `gateway demo` and `gateway relay` can also be set with an
environment variable: `BLEH_GATEWAY_` followed by the flag name in upper
case with `-` as `_` (`--send-buffer-ms` is `BLEH_GATEWAY_SEND_BUFFER_MS`).
Repeatable flags take a comma-separated list; switches take `true` or
`false`. A flag on the command line wins over its variable.
`BLEH_GATEWAY_MODE` (`demo` or `relay`) supplies the mode when the binary
gets no argument at all, so a systemd unit needs no `BLEH_ARGS`:

```bash
# /etc/bleh/relay.env
BLEH_GATEWAY_MODE=relay
BLEH_GATEWAY_LISTEN=0.0.0.0:8080
BLEH_GATEWAY_TRUST_FORWARDED_FOR=true
BLEH_GATEWAY_UPSTREAM=wss://a.example/v0/ws,wss://b.example/v0/ws
BLEH_GATEWAY_SIGNER=0x<published signer address>
```

| Variable | Flag | Modes |
|---|---|---|
| `BLEH_GATEWAY_MODE` | (the subcommand) | both |
| `BLEH_GATEWAY_LISTEN` | `--listen` | both |
| `BLEH_GATEWAY_REGISTRY` | `--registry` | both |
| `BLEH_GATEWAY_PROOF_RETENTION_BLOCKS` | `--proof-retention-blocks` | both |
| `BLEH_GATEWAY_CONNECTIONS_PER_IP` | `--connections-per-ip` | both |
| `BLEH_GATEWAY_CHANNELS_PER_CONNECTION` | `--channels-per-connection` | both |
| `BLEH_GATEWAY_CHANNELS_PER_IP` | `--channels-per-ip` | both |
| `BLEH_GATEWAY_MESSAGES_PER_SECOND` | `--messages-per-second` | both |
| `BLEH_GATEWAY_MESSAGE_BURST` | `--message-burst` | both |
| `BLEH_GATEWAY_PROOFS_PER_SECOND` | `--proofs-per-second` | both |
| `BLEH_GATEWAY_SEND_BUFFER_BYTES` | `--send-buffer-bytes` | both |
| `BLEH_GATEWAY_SEND_BUFFER_MS` | `--send-buffer-ms` | both |
| `BLEH_GATEWAY_MAX_MESSAGE_BYTES` | `--max-message-bytes` | both |
| `BLEH_GATEWAY_TRUST_FORWARDED_FOR` | `--trust-forwarded-for` | both |
| `BLEH_GATEWAY_SIGNING_KEY` | `--signing-key` (list) | demo |
| `BLEH_GATEWAY_ACTIVE_KEY` | `--active-key` | demo |
| `BLEH_GATEWAY_EPHEMERAL_KEY` | `--ephemeral-key` | demo |
| `BLEH_GATEWAY_SEED` | `--seed` | demo |
| `BLEH_GATEWAY_BLOCKS` | `--blocks` | demo |
| `BLEH_GATEWAY_INTERVAL_MS` | `--interval-ms` | demo |
| `BLEH_GATEWAY_CHECKPOINT_EVERY` | `--checkpoint-every` | demo |
| `BLEH_GATEWAY_GAP_EVERY` | `--gap-every` | demo |
| `BLEH_GATEWAY_NATS_URL` | `--nats-url` | demo |
| `BLEH_GATEWAY_NATS_STREAM` | `--nats-stream` | demo |
| `BLEH_GATEWAY_REPLAY_WINDOW_SECS` | `--replay-window-secs` | demo |
| `BLEH_GATEWAY_NATS_CREDENTIALS` | `--nats-credentials` | demo |
| `BLEH_GATEWAY_UPSTREAM` | `--upstream` (list) | relay |
| `BLEH_GATEWAY_SIGNER` | `--signer` (list) | relay |
| `BLEH_GATEWAY_TRUST_UPSTREAM_SNAPSHOTS` | `--trust-upstream-snapshots` | relay |
| `BLEH_GATEWAY_UPSTREAM_ENCODING` | `--upstream-encoding` | relay |

Signing keys are still given as `ID=file:PATH` or `ID=env:VAR`; never put
key material itself in these variables.

The demo feed replays hl-synth's ground-truth trades and book events with
current timestamps, inserts a reset block (cancel everything) between
cycles, offsets order IDs per cycle, and publishes L4 `book_snapshot`
checkpoints every `--checkpoint-every` blocks. All values are synthetic.

## Internals that matter operationally

```
bus ring ──▶ hub ──▶ fan-out ring ──▶ one task per connection ──▶ client
```

- The **hub** keeps one L4 book per instrument, seeded only from L4
  `book_snapshot` checkpoints (committed records) or `Resync` events.
  A `book_event` batch whose `prev_block_number` does not link marks the
  book invalid (`gateway_upstream_gaps_total`); so does an event that does
  not fit the book. The next checkpoint reseeds it and every synced
  connection gets a forced snapshot (`upstream_gap` or `reconcile`). A
  valid book that differs from a checkpoint is replaced (`reconcile`,
  `gateway_reconciliations_total`). **The book builder must publish
  checkpoints**; without them a gateway cannot serve snapshots.
- Snapshots are published by the hub onto the fan-out ring, so a snapshot
  at block `B` is followed on every connection by exactly the batches after
  `B`. Batches for an instrument are still forwarded while a connection
  waits for its snapshot (clients ignore them; full-stream verifiers need
  them).
- Messages identical for all connections (batches, records, commitments)
  are encoded once per encoding and shared.

## Limits

Announced in `hello.limits`, set by flags:

| Limit | Flag | Default |
|---|---|---|
| Connections per IP | `--connections-per-ip` | 10 (HTTP 429 beyond) |
| Channels per connection | `--channels-per-connection` | 200 |
| Channels per IP (all connections) | `--channels-per-ip` | 2000 |
| Client messages | `--messages-per-second`, `--message-burst` | 20/s, burst 50 |
| `resnapshot` per instrument | fixed | 1 per 5 s |
| Proof requests | `--proofs-per-second` | 50/s |
| Send buffer | `--send-buffer-bytes`, `--send-buffer-ms` | 8 MiB, 2 s |
| Largest client message / frame | `--max-message-bytes` | 64 KiB (close 1009 beyond) |

A wildcard channel (`*`, `prefix*`) counts as one channel.

- **Rate limits:** a refused message gets `error` `rate_limited`; 10
  violations (rate limits or malformed messages) within 10 s close the
  connection with 4029.
- **Slow consumers:** a connection whose next unread item is older than
  the send-buffer time, whose cursor was overrun by the ring, or whose
  socket write blocks that long, drops everything queued and resyncs every
  book with a `slow_consumer` snapshot (first strike); a second strike
  within 60 s closes it with 4008. The websocket write buffer is capped at
  the send-buffer size. Nothing is ever dropped selectively.
- **Behind a CDN or proxy**, pass `--trust-forwarded-for` so per-IP limits
  apply to the client address from `X-Forwarded-For`; never set it on a
  directly exposed port.
- `--registry markets.json` refuses subscriptions to unknown exact
  instruments (`unknown_instrument`); the demo uses the hl-synth registry.

## HTTP endpoints

| Path | |
|---|---|
| `GET /health` | 200 `{"status":"ok"}` when a block closed within 5 s (and, for a relay, the upstream is connected); 503 otherwise |
| `GET /freshness` | `venues`, `streams` and `instruments`: last block, block time, publish time, `lag_ms` (now − block time) and `age_ms` (now − when this gateway saw it) |
| `GET /metrics` | Prometheus text format |
| `GET /proof?venue=&kind=&block_number=&instrument=&event_index=` | `{commitment, proof, record}`; 404 `proof_unavailable`; 400 on a bad query |
| `GET /signers` | `signers` (active first) and, when signing locally, `keys` with IDs |

## Metrics

| Metric | Meaning |
|---|---|
| `gateway_send_latency_seconds` | Histogram: send time − record `ts_ingest_ns` (node local write time). The Phase 1 target is p99 ≤ 50 ms |
| `gateway_publish_to_send_seconds` | Histogram: send time − `ts_publish_ns` |
| `gateway_connections`, `gateway_connections_total` | Open and accepted connections |
| `gateway_rejected_ip_cap_total`, `gateway_rejected_subprotocol_total` | Refused upgrades, 4001 closes |
| `gateway_messages_sent_total`, `gateway_bytes_sent_total` | Output |
| `gateway_client_messages_total`, `gateway_bad_requests_total`, `gateway_rate_limited_total`, `gateway_rate_limit_disconnects_total` | Input and limits |
| `gateway_slow_consumer_resnapshots_total`, `gateway_slow_consumer_disconnects_total` | Slow-consumer policy |
| `gateway_snapshots_sent_total`, `gateway_forced_snapshots_sent_total` | Snapshots |
| `gateway_upstream_gaps_total`, `gateway_reconciliations_total` | Book integrity |
| `gateway_proofs_served_total`, `gateway_proofs_unavailable_total`, `gateway_proof_store_blocks` | Proofs |
| `gateway_blocks_total`, `gateway_last_block`, `gateway_hub_events_total`, `gateway_hub_lagged_total` | Hub |
| `relay_connected`, `relay_connects_total`, `relay_blocks_verified_total`, `relay_streams_rejected_total` | Relay |

Alert on: `gateway_hub_lagged_total` increasing, `relay_streams_rejected_total`
increasing, p99 of `gateway_send_latency_seconds` above 50 ms, `/health` 503.

## Relay setup

1. Get the signer addresses from the registry (not from the upstream's
   `hello`; a relay must not trust what it verifies).
2. Run `gateway relay` with every address as `--signer` and two or more
   `--upstream` URLs (rotated on failure, with backoff).
3. The relay subscribes to every kind of every instrument with
   commitments, holds each block until its heartbeat, and per stream
   checks signer allowlist, signature, leaf count and Merkle root against
   the records received. Only verified streams are re-served, unchanged
   (records keep the upstream `ts_publish_ns`), and the trees feed its own
   `/proof`. Unverifiable data is dropped and counted
   (`relay_streams_rejected_total`); the partial first block after each
   connect is always dropped.
4. Books are seeded from the committed checkpoints. `--trust-upstream-snapshots`
   also seeds from upstream `snapshot` messages, which are not signed: use
   it only for an upstream you operate.
5. Put it behind TLS (`wss://`) and a CDN; set `--trust-forwarded-for` there.

## Shutdown

`SIGINT` stops accepting, and every connection closes with 1001 so
clients fail over to their next URL.
