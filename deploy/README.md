# deploy

Containers, compose profiles, host bootstrap, systemd units, observability
and infrastructure as code for running bleh.xyz. Owned by the
infrastructure lane ([ADR 0007](../docs/decisions/0007-code-layout.md)).
The default layout is budget-first
([ADR 0010](../docs/decisions/0010-budget-first-hosting.md)): one rented
node box, one self-hosted archive server, free relays, R2.

Runbooks: [hosting](../docs/operations/hosting.md),
[costs](../docs/operations/costs.md), [deploy](../docs/operations/deploy.md),
[node](../docs/operations/node.md), [upgrades](../docs/operations/upgrades.md),
[keys](../docs/operations/keys.md),
[observability](../docs/operations/observability.md),
[incident](../docs/operations/incident.md).

## Quick start

```bash
deploy/up.sh synthetic        # whole pipeline on a laptop, no node (needs Docker)
deploy/up.sh node-box         # on the rented node box
deploy/up.sh home-archive     # on the self-hosted archive server
deploy/up.sh relay            # on each relay
deploy/up.sh origin           # node box and archive on one qualifying machine
```

## Layout

| Path | Contents |
|---|---|
| `up.sh` | One command per host role: creates `.env` and secrets placeholders, builds or pulls images, starts the profile |
| `docker/Dockerfile` | Multi-stage build of every service binary; distroless non-root targets |
| `docker/hl-node/` | Hyperliquid node image (official hl-visor procedure, pinned release key) and its entrypoint |
| `docker/build.sh` | Builds images from `git archive` of a commit, tagged by SHA |
| `compose/compose.yaml` | Profiles `synthetic`, `node-box`, `home-archive`, `relay`, `backfill` |
| `compose/env.example.conf` | Template for `compose/.env` |
| `compose/caddy/`, `nats/`, `minio/`, `ipfs/`, `vector/`, `prometheus/`, `cloudflared/` | Per-service configuration for compose |
| `observability/` | Prometheus rules and production config, Alertmanager, Grafana dashboards and provisioning, blackbox, Loki, Vector (journal), the availability probe |
| `scripts/` | `host-bootstrap.sh`, `disk-setup.sh`, `wg-mesh.sh`, `hl-prune.sh`, `schema-drift-check.sh`, `node-install.sh`, `update-host.sh`, `peer-latency.sh`, `host-metrics.sh` |
| `systemd/` | Timers (pruning, drift, probe, host facts), `hl-visor.service` and `bleh@.service` as alternatives to compose, env templates |
| `terraform/` | Root stacks `envs/edge`, `envs/relays-oci`, `envs/relays-hcloud`, `envs/tokyo`, `envs/fallback-dns` over modules `host`, `cloudflare-edge`, `cloudflare-tunnel`, `relay-oci`, `relay-hcloud`, `aws-node-box`, `vultr-node-box` |
| `ci/deploy.yml` | Proposed workflow: build, sign, gated promote (the integrator moves it to `.github/workflows/`) |

Example and template files carry `.example.` in their names, because the
repository only admits known extensions (`scripts/check.sh`). Real copies
(`compose/.env`, `compose/secrets/`, `*.tfvars.hcl`, `backend.hcl`) are
git-ignored by `deploy/.gitignore`.

## Overlay

Hosts talk privately over WireGuard (`bleh0`, UDP 51820, `wg-mesh.sh`).
Addresses are fixed by convention:

| Address | Host |
|---|---|
| 10.77.0.1 | node box a |
| 10.77.0.2 | home-archive server (dials out; no endpoint) |
| 10.77.0.3 | node box b (Tier 2+) |
| 10.77.0.11–19 | relays |

Metrics, Loki and the inter-origin reader stream use the overlay; nothing
else is exposed. Public ports: node box 4001-4002 (gossip); relays 80/443;
home server none (optional 6881 and 4101 forwards for seeding).

## Service contract

What each service lane must implement for these configurations to work.
Every service:

- reads configuration from environment variables (below) and logs JSON to
  stdout (`RUST_LOG`);
- serves Prometheus metrics at `GET /metrics` and liveness at
  `GET /health` on `BLEH_METRICS_ADDR` (an `ip:port`);
- reads secrets from files named by `*_FILE` variables, never from the
  environment;
- runs as UID 65532 with a read-only root filesystem; writable paths are
  `/tmp` and the `/var/lib/bleh/*` volumes listed below;
- exits non-zero on fatal errors (the orchestrator restarts it) and stops
  cleanly on SIGTERM within 30 s;
- labels nothing with a site: Prometheus adds `site` and `path`.

Object storage settings use the `object_store` conventions: `AWS_ENDPOINT`,
`AWS_REGION` (`auto` for R2), `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`,
`AWS_ALLOW_HTTP` (MinIO only), plus `BLEH_PUBLIC_BUCKET` and
`BLEH_PRIVATE_BUCKET`. Common: `BLEH_SITE`, `BLEH_NATS_URL`,
`BLEH_SIGNING_KEY_FILE` (32-byte secp256k1 secret as 64 hex characters,
optional `0x` and trailing newline), `BLEH_SIGNERS` (comma-separated
addresses to accept).

### hl-node-reader (node-reader lane)

| | |
|---|---|
| Ports | `READER_STREAM_LISTEN` (7400): record stream for gateways, local and from the other origin; `BLEH_METRICS_ADDR` (9101) |
| Variables | `READER_MODE` `live` or `replay`; `READER_PATH_ID` (`a`, `b`); `READER_NODE_DATA_DIR` (`/hl/data`, read-only; directory names for the three streams configurable, default trying `node_<s>_by_block` then `node_<s>`); `READER_INFO_URL` (`http://127.0.0.1:3001/info`); `READER_SNAPSHOT_DIR` (`/snapshots`, used as the `fileSnapshot` `outPath` directory; the node writes it, the reader reads it); `READER_RAW_DIR` (`/var/lib/bleh/raw`, writable); `READER_REPLAY_DIR`, `READER_REPLAY_LOOP` (replay mode over `fixtures/synthetic/hl/<case>`) |
| Files | Writes hourly filtered and unfiltered raw under `READER_RAW_DIR`; together with archive-capture maintains `READER_RAW_DIR/.captured-through` (last UTC hour `YYYY-MM-DDTHH` whose node files are fully read and whose raw is in object storage). The pruner deletes node output only up to this watermark |
| Networking | Host network (the info server is loopback-only) |
| Metrics | `bleh_reader_last_block_number`, `bleh_reader_last_block_time_seconds`, `bleh_reader_last_local_time_seconds`, `bleh_reader_ingest_lag_seconds` (histogram, `ts_ingest − ts_node`), `bleh_reader_seq_gaps_total{stream}`, `bleh_reader_parse_errors_total{stream}`, `bleh_reader_schema_unknown_total{stream,field}`, `bleh_reader_stream_halted{stream}`, `bleh_reader_snapshot_failures_total`; book builder in process: `bleh_book_reconcile_mismatch_total`, `bleh_book_resyncs_total`; `bleh_signing_failures_total` |

### gateway (bus, gateway and client lane)

| | |
|---|---|
| Ports | `GATEWAY_LISTEN` (8080): `/v0/ws`, `/health`, `/freshness` (docs/protocol/gateway-v0.md); `BLEH_METRICS_ADDR` (9102) |
| Variables | `GATEWAY_MODE` `live` (upstream readers), `relay` (upstream gateways; verify every commitment against `BLEH_SIGNERS` and re-serve unchanged) or `demo` (synthetic records, signed with `BLEH_SIGNING_KEY_FILE`); `GATEWAY_UPSTREAMS` (comma-separated `tcp://host:7400` readers, deduplicated on `(block, event_index)`, or `ws(s)://.../v0/ws` gateways in preference order); `GATEWAY_UPSTREAM_HEADERS_FILE` (relay: HTTP headers, one `Name: value` per line, sent on the upstream websocket handshake; carries the Cloudflare Access service token); `GATEWAY_TRUSTED_PROXIES` (CIDRs) and `GATEWAY_CLIENT_IP_HEADER` (`X-Real-IP` behind Caddy, `CF-Connecting-IP` behind the tunnel) for per-IP limits; protocol limits overridable as `GATEWAY_MAX_CONNECTIONS_PER_IP` etc. |
| Health | `/health` 200 only when connected upstream and fresh, else 503 (relays and update-host.sh rely on it) |
| Metrics | `bleh_gateway_publish_latency_seconds` (histogram of `ts_publish − ts_node`; buckets must include 0.005, 0.01, 0.025, 0.05, 0.1, 0.25), `bleh_gateway_block_lag_seconds`, `bleh_gateway_last_block_number`, `bleh_gateway_connections`, `bleh_gateway_subscriptions`, `bleh_gateway_bytes_sent_total`, `bleh_gateway_resnapshots_total{reason}`, `bleh_gateway_slow_consumer_closes_total`, `bleh_gateway_rate_limited_total`, `bleh_gateway_upstream_connected{upstream}`; dedupe: `bleh_dedupe_path_last_block{path}`, `bleh_dedupe_mismatch_total{stream}` |

### archive (archive lane)

| | |
|---|---|
| Ports | `BLEH_METRICS_ADDR` (9103) |
| Variables | `ARCHIVE_ROLE` `capture` (node box: upload raw from `ARCHIVE_RAW_DIR`, advance the watermark), `process` (home server: replay, partitions, manifests, anchors, torrents and IPFS from object storage and `ARCHIVE_MIRROR_DIR`) or `all` (synthetic); `ARCHIVE_WORK_DIR`; `ARCHIVE_MIRROR_DIR` (`/srv/archive`, read-only mirror of the public bucket kept by rclone); `BLEH_MANIFEST_KEY_FILE`; `ARCHIVE_OTS_ENABLED`; `ARCHIVE_L2_RPC_URL`, `ARCHIVE_L2_ANCHOR_KEY_FILE`; `ARCHIVE_IPFS_API` (`http://ipfs:5001`; add with `--nocopy` from the mirror path); `ARCHIVE_TORRENT_WATCH_DIR` (torrent files dropped here are seeded by qBittorrent); `ARCHIVE_WEB_SEED_BASE` |
| Requests | Publish `<archive>/_manifests/latest.json` (pointer to the newest manifest; probed by blackbox, cached 60 s at the edge); a KMS or HSM signer behind the manifest-key interface later (keys.md) |
| Metrics | `bleh_archive_raw_upload_last_success_timestamp_seconds{tier,stream}`, `bleh_archive_upload_failures_total{tier}`, `bleh_archive_manifest_last_success_timestamp_seconds{archive}`, `bleh_archive_manifest_failures_total{archive}`, `bleh_archive_anchor_last_success_timestamp_seconds{archive,anchor}`, `bleh_archive_anchor_failures_total{archive,anchor}` (`anchor` is `ots` or `l2`), `bleh_archive_content_hash_mismatch_total`, `bleh_signing_failures_total` |

### history-api (historical API lane)

| | |
|---|---|
| Ports | `HISTORY_LISTEN` (8081) with `/health`; `BLEH_METRICS_ADDR` (9104) |
| Variables | `HISTORY_DATA_URL` (`s3://bucket` or `https://data.bleh.xyz`), `HISTORY_QUERY_TIMEOUT`, `HISTORY_MEMORY_LIMIT`, `HISTORY_TRUSTED_PROXIES`, `HISTORY_CLIENT_IP_HEADER` |
| Notes | Runs on arm64 relays: the image must build for linux/arm64 |
| Metrics | `bleh_history_http_request_duration_seconds{route,status}` (histogram), `bleh_history_query_timeouts_total` |

### swap-pollers (swap-pollers lane)

| | |
|---|---|
| Ports | `BLEH_METRICS_ADDR` (9105) |
| Variables | `SWAP_SOURCES` (`cftc-dtcc,cftc-ice,sbsr-dtcc,sbsr-ice`), `SWAP_RAW_DIR`, `SWAP_USER_AGENT`, `BLEH_NATS_URL`, object-storage variables for raw capture |
| Metrics | `bleh_swap_poll_last_success_timestamp_seconds{source}`, `bleh_swap_poll_failures_total{source}` |

### hl-backfill (backfill lane)

| | |
|---|---|
| Ports | `BLEH_METRICS_ADDR` (9106) while running |
| Variables | Command-line range (`--from`, `--to`, paths); `BACKFILL_SOURCE_AWS_ACCESS_KEY_ID`, `BACKFILL_SOURCE_AWS_SECRET_ACCESS_KEY`, `BACKFILL_SOURCE_REGION` (requester-pays source, separate from the R2 destination credentials); `BACKFILL_WORK_DIR` |
| Run | `deploy/up.sh home-archive backfill --from ... --to ...` (one-off container, exits when done) |

### mesh-client (client lane)

`mesh-client probe --url URL --instrument ID --kind KIND --timeout 5s`
printing one JSON line `{"ok":bool,"first_block_seconds":f,"block_time_ms":n}`
lets the probe run from the `tools` image (`PROBE_CLIENT=docker`) instead of
websocat. `mesh-client keys address --key-file F` printing the address of a
signing key would remove the need for Foundry in keys.md.

## Validation status

Checked in this repository: `docker compose config` for every profile,
YAML parsing of every config, JSON validity of dashboards, `bash -n` on
every script, the schema-drift check against the synthetic fixtures
(including an injected unknown field), and `scripts/check.sh`. Not run
here: image builds and the stack itself (no Docker daemon), `terraform
validate` (Terraform not installed), `promtool check rules`, shellcheck.
