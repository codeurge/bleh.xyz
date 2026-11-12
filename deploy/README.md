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

Object storage settings use the `object_store` conventions: `AWS_ENDPOINT`,
`AWS_REGION` (`auto` for R2), `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`,
`AWS_ALLOW_HTTP` (MinIO only), plus `BLEH_PUBLIC_BUCKET` and
`BLEH_PRIVATE_BUCKET`. Common: `BLEH_SITE`, `BLEH_NATS_URL`,
`BLEH_SIGNING_KEY_FILE` (32-byte secp256k1 secret as 64 hex characters,
optional `0x` and trailing newline), `BLEH_SIGNERS` (comma-separated
addresses to accept).

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

### gateway (bus, gateway and client lane)

| | |
|---|---|
| Ports | `GATEWAY_LISTEN` (8080): `/v0/ws`, `/health`, `/freshness` (docs/protocol/gateway-v0.md); `BLEH_METRICS_ADDR` (9102) |
| Variables | `GATEWAY_MODE` `live` (upstream readers), `relay` (upstream gateways; verify every commitment against `BLEH_SIGNERS` and re-serve unchanged) or `demo` (synthetic records, signed with `BLEH_SIGNING_KEY_FILE`); `GATEWAY_UPSTREAMS` (comma-separated reader outputs such as `tcp://host:7400`, or `ws(s)://.../v0/ws` gateways, in preference order with failover; the reader already merges and deduplicates two nodes); `GATEWAY_UPSTREAM_HEADERS_FILE` (relay: HTTP headers, one `Name: value` per line, sent on the upstream websocket handshake; carries the Cloudflare Access service token); `GATEWAY_TRUSTED_PROXIES` (CIDRs) and `GATEWAY_CLIENT_IP_HEADER` (`X-Real-IP` behind Caddy, `CF-Connecting-IP` behind the tunnel) for per-IP limits; protocol limits overridable as `GATEWAY_MAX_CONNECTIONS_PER_IP` etc. |
| Health | `/health` 200 only when connected upstream and fresh, else 503 (relays and update-host.sh rely on it) |
| Metrics | `bleh_gateway_publish_latency_seconds` (histogram of `ts_publish − ts_node`; buckets must include 0.005, 0.01, 0.025, 0.05, 0.1, 0.25), `bleh_gateway_block_lag_seconds`, `bleh_gateway_last_block_number`, `bleh_gateway_connections`, `bleh_gateway_subscriptions`, `bleh_gateway_bytes_sent_total`, `bleh_gateway_resnapshots_total{reason}`, `bleh_gateway_slow_consumer_closes_total`, `bleh_gateway_rate_limited_total`, `bleh_gateway_upstream_connected{upstream}`; the book builder (book lane library, run in the gateway or reader process) adds `bleh_book_reconcile_mismatch_total` |

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
