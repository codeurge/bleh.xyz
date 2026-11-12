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

Metrics, Loki, the relays' upstream connections to origin gateways
(`ws://10.77.0.x:8080/v0/ws`) and, in Tier 2, the second node's data use
the overlay; nothing else is exposed. Public ports: node box 4001-4002 (gossip); relays 80/443;
home server none (optional 6881 and 4101 forwards for seeding).

## Service contract

What each service lane must implement for these configurations to work.
Every service:

- reads configuration from environment variables (below) and logs JSON
  (`RUST_LOG` sets the level);
- serves Prometheus metrics at `GET /metrics` and liveness at
  `GET /health` on a configurable `ip:port` (`BLEH_METRICS_ADDR` for
  services not yet built; the merged reader and pollers use their own
  variable names, listed below);
- reads secrets from files named by `*_FILE` variables, never from the
  environment;
- runs as UID 65532 with a read-only root filesystem; writable paths are
  `/tmp` and the `/var/lib/bleh/*` volumes listed below;
- exits non-zero on fatal errors (the orchestrator restarts it) and stops
  cleanly on SIGTERM within 30 s;
- labels nothing with a site: Prometheus adds `site` (and `path` where useful).

For services not yet built (archive), object storage settings use the
`object_store` conventions: `AWS_ENDPOINT`, `AWS_REGION` (`auto` for R2),
`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_ALLOW_HTTP` (MinIO only),
plus `BLEH_PUBLIC_BUCKET` and `BLEH_PRIVATE_BUCKET`; also `BLEH_SITE`,
`BLEH_NATS_URL` and `BLEH_SIGNING_KEY_FILE` (32-byte secp256k1 secret as 64
hex characters, optional `0x` and trailing newline). Merged services keep
their own names, mapped from the same `.env` values in compose.

### hl-node-reader (node-reader lane, merged)

Implemented; full reference in
[node-reader.md](../docs/operations/node-reader.md). What the
configurations here use:

| | |
|---|---|
| Ports | `HL_METRICS_ADDR` (9184 on the overlay address): `/metrics`, `/health` |
| Variables set | `HL_NODE_DIRS` (`/hl/data`; both nodes' directories in Tier 2, merged in one reader), `HL_INFO_URL`, `HL_REGISTRY_OUT`, `HL_CHECKPOINT` (under the `reader-state` volume, `/var/lib/bleh/reader`), `HL_SNAPSHOT_INTERVAL_SECS` (600), `HL_SNAPSHOT_PATH` (`/snapshots/l4.json`; the node writes it, the reader reads the same path), `HL_OUTPUT` (`none` until a consumer exists), `HL_LOG_FORMAT=json`; synthetic replay: `HL_REPLAY`, `HL_REGISTRY`, `HL_WRITE_FILTERED` |
| Networking | Host network (the info server is loopback-only) |
| Metrics used by rules and dashboards | `hl_reader_last_block`, `hl_reader_block_lag_seconds`, `hl_reader_emit_lag_seconds`, `hl_reader_parse_errors_total`, `hl_reader_rejected_blocks_total`, `hl_reader_incomplete_blocks_total`, `hl_output_last_block`, `hl_output_block_lag_seconds`, `hl_output_gaps_total`, `hl_merge_mismatches_total`, `hl_merge_one_sided_gap_blocks_total`, `hl_registry_refreshes_total`, `hl_checkpoint_writes_total` |
| Still requested | (1) A network output the gateway can consume (`--output tcp://0.0.0.0:7400` or equivalent; the compose file reserves 7400 on the overlay); stdout JSON lines are not a production transport. (2) Live filtered and unfiltered raw capture (today `--write-filtered` exists only with `--replay`). (3) With archive-capture, the watermark file `<raw dir>/.captured-through` holding the last UTC hour (`YYYY-MM-DDTHH`) whose node files are fully read and whose raw is in object storage; `hl-prune.sh` deletes node output only up to it. (4) Block signing with `BLEH_SIGNING_KEY_FILE` (32-byte secp256k1 secret as 64 hex characters) and `bleh_signing_failures_total`. (5) A histogram for emit lag, so p99 does not depend on scrape sampling. (6) `hl_reader_snapshot_failures_total`, so failed `fileSnapshot` requests alert |

### gateway (bus, gateway and client lane, merged)

Implemented; reference in [gateway.md](../docs/operations/gateway.md). The
binary takes flags, not environment variables; compose passes them.

| | |
|---|---|
| Origin | "Library" mode: the gateway runs inside the reader process. The configurations expect it on `0.0.0.0:8080` of the node box (host network; overlay-only by firewall) serving `/v0/ws`, `/health`, `/freshness`, `/signers`, `/proof` and `/metrics` |
| Relay | `gateway relay --listen=0.0.0.0:8080 --trust-forwarded-for --upstream=ws://10.77.0.1:8080/v0/ws --signer=0x...` (compose: `RELAY_UPSTREAM_1/2`, `BLEH_SIGNER_1/2`); Caddy in front sets `X-Forwarded-For` and hides `/metrics` |
| Demo | `gateway demo --listen=0.0.0.0:8080 --signing-key=k1=file:/run/secrets/signing_key --nats-url=nats://nats:4222` |
| Metrics used | `gateway_send_latency_seconds` (histogram; the 50 ms target), `gateway_blocks_total`, `gateway_connections`, `gateway_messages_sent_total`, `gateway_bytes_sent_total`, `gateway_forced_snapshots_sent_total`, `gateway_slow_consumer_disconnects_total`, `gateway_rate_limited_total`, `gateway_hub_lagged_total`, `relay_connected`, `relay_streams_rejected_total` |
| Still requested | (1) The reader lane wiring the gateway into `hl-node-reader` (listen address flag, signing key from `BLEH_SIGNING_KEY_FILE` or `--signing-key`). (2) Optional: environment variables for the flags, so systemd templates need no `BLEH_ARGS`. (3) Book reconciliation metric `bleh_book_reconcile_mismatch_total` from wherever the book builder runs |

### archive (archive lane)

| | |
|---|---|
| Ports | `BLEH_METRICS_ADDR` (9103) |
| Variables | `ARCHIVE_ROLE` `capture` (node box: upload raw from `ARCHIVE_RAW_DIR`, advance the watermark), `process` (home server: replay, partitions, manifests, anchors, torrents and IPFS from object storage and `ARCHIVE_MIRROR_DIR`) or `all` (synthetic); `ARCHIVE_WORK_DIR`; `ARCHIVE_MIRROR_DIR` (`/srv/archive`, read-only mirror of the public bucket kept by rclone); `BLEH_MANIFEST_KEY_FILE`; `ARCHIVE_OTS_ENABLED`; `ARCHIVE_L2_RPC_URL`, `ARCHIVE_L2_ANCHOR_KEY_FILE`; `ARCHIVE_IPFS_API` (`http://ipfs:5001`; add with `--nocopy` from the mirror path); `ARCHIVE_TORRENT_WATCH_DIR` (torrent files dropped here are seeded by qBittorrent); `ARCHIVE_WEB_SEED_BASE` |
| Requests | Publish `<archive>/_manifests/latest.json` (pointer to the newest manifest; probed by blackbox, cached 60 s at the edge); a KMS or HSM signer behind the manifest-key interface later (keys.md) |
| Metrics | `bleh_archive_raw_upload_last_success_timestamp_seconds{tier,stream}`, `bleh_archive_upload_failures_total{tier}`, `bleh_archive_manifest_last_success_timestamp_seconds{archive}`, `bleh_archive_manifest_failures_total{archive}`, `bleh_archive_anchor_last_success_timestamp_seconds{archive,anchor}`, `bleh_archive_anchor_failures_total{archive,anchor}` (`anchor` is `ots` or `l2`), `bleh_archive_content_hash_mismatch_total`, `bleh_signing_failures_total` |

### history-api (historical API lane, merged)

Implemented; reference in [history-api.md](../docs/operations/history-api.md).

| | |
|---|---|
| Ports | `HISTORY_API_BIND` (8081 here): API, `/health`, `/metrics`, `/mcp` |
| Variables set | `HISTORY_API_ARCHIVE` (`https://data.bleh.xyz` in production, the MinIO bucket locally), `HISTORY_API_TRUST_FORWARDED_FOR`, `HISTORY_API_MCP_ALLOWED_HOSTS`, `HISTORY_API_QUERY_TIMEOUT_MS`, `HISTORY_API_MEMORY_LIMIT_MB`, `HISTORY_API_LOG_FORMAT=json`; a writable `$HOME` (tmpfs) for DuckDB's `httpfs` extension, or `HISTORY_API_EXTENSION_DIR` for offline images |
| Notes | Runs on arm64 relays: the image must build for linux/arm64 (the deploy workflow does) |
| Metrics used | `bleh_history_request_seconds{route,status}`, `bleh_history_query_seconds`, `bleh_history_rate_limited_total` |

### swap-pollers (swap-pollers lane, merged)

Implemented; reference in [swap-pollers.md](../docs/operations/swap-pollers.md).

| | |
|---|---|
| Ports | `SWAP_POLLERS_LISTEN` (9187): `/metrics`, `/health` |
| Variables set | `SWAP_POLLERS_RAW_DIR`, `SWAP_POLLERS_STATE_DIR`, `SWAP_POLLERS_OUT_DIR` (under the `swap-data` volume), `SWAP_POLLERS_USER_AGENT`, `SWAP_POLLERS_S3_BUCKET`/`_PREFIX`/`_ENDPOINT`/`_REGION` with `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` (raw mirrored to the private bucket), `SWAP_POLLERS_LOG_JSON` |
| Metrics used | `swap_last_successful_poll_timestamp_seconds{listing}`, `swap_raw_persist_errors_total{store}`, `swap_unknown_columns_total{bucket,column}`, `swap_lag_seconds{bucket}` |
| Still requested | ICE Trade Vault sources once admitted fields are confirmed (today DTCC only) |

### hl-backfill (backfill lane, merged)

Implemented; reference in [backfill.md](../docs/operations/backfill.md).

| | |
|---|---|
| Run | `deploy/up.sh home-archive backfill <estimate|copy|process|compare|replay-prototype> [args]` (one-off container on the home server's NVMe scratch) |
| Variables set | Source (requester-pays): `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` from `BACKFILL_SOURCE_*` in `.env`, `HL_NODE_DATA_REGION`, `HL_ARCHIVE_REGION`; destination: `BLEH_RAW_ENDPOINT`, `BLEH_RAW_ACCESS_KEY_ID`, `BLEH_RAW_SECRET_ACCESS_KEY`, `BLEH_RAW_REGION` from the R2 settings |
| Still requested | Progress metrics (`/metrics` while running) for long copies |

### mesh-client (client lane)

Merged ([client.md](../docs/operations/client.md)). Requested: a
`mesh-client probe --url URL --instrument ID --kind KIND --timeout 5s`
mode printing one JSON line `{"ok":bool,"first_block_seconds":f,"block_time_ms":n}`,
so the availability probe can run from the `tools` image
(`PROBE_CLIENT=docker`) instead of websocat; and `mesh-client keys address
--key-file F`, so keys.md needs no Foundry.

## Validation status

Checked in this repository: `docker compose config` for every profile,
YAML parsing of every config, JSON validity of dashboards, `bash -n` on
every script, the schema-drift check against the synthetic fixtures
(including an injected unknown field), and `scripts/check.sh`. Not run
here: image builds and the stack itself (no Docker daemon), `terraform
validate` (Terraform not installed), `promtool check rules`, shellcheck.
