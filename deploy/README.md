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
deploy/up.sh node-box         # on the rented node box (hl-node + origin)
deploy/up.sh home-archive     # on the self-hosted archive server
deploy/up.sh relay            # on each relay
deploy/up.sh origin           # node box and archive on one qualifying machine
```

The `origin` profile name predates the `origin` service; the service runs
in the `node-box` profile (and therefore in the `origin` profile too).

## Layout

| Path | Contents |
|---|---|
| `up.sh` | One command per host role: creates `.env` and secrets placeholders, builds or pulls images, starts the profile |
| `docker/Dockerfile` | Multi-stage build of every service binary; distroless non-root targets (`origin`, `hl-node-reader`, `gateway`, `archive`, `history-api`, `swap-pollers`, `reference-pollers`, `hl-backfill`, `mesh-client`, `tools`) |
| `docker/hl-node/` | Hyperliquid node image (official hl-visor procedure, pinned release key) and its entrypoint |
| `docker/build.sh` | Builds images from `git archive` of a commit, tagged by SHA |
| `compose/compose.yaml` | Profiles `synthetic`, `node-box`, `home-archive`, `relay`, and one-off `backfill` and `archive-jobs` |
| `compose/env.example.conf` | Template for `compose/.env` |
| `compose/caddy/`, `nats/`, `minio/`, `ipfs/`, `vector/`, `prometheus/`, `cloudflared/` | Per-service configuration for compose |
| `observability/` | Prometheus rules and production config, Alertmanager, Grafana dashboards and provisioning, blackbox, Loki, Vector (journal), the availability probe |
| `scripts/` | `host-bootstrap.sh`, `disk-setup.sh`, `wg-mesh.sh`, `hl-prune.sh`, `schema-drift-check.sh`, `archive-pipeline.sh`, `node-install.sh`, `update-host.sh`, `peer-latency.sh`, `host-metrics.sh` |
| `systemd/` | Timers (pruning, drift, probe, host facts, daily archive pipeline and anchor upgrades), `hl-visor.service` and `bleh@.service` as alternatives to compose, env templates |
| `terraform/` | Root stacks `envs/edge`, `envs/relays-oci`, `envs/relays-hcloud`, `envs/tokyo`, `envs/fallback-dns` over modules `host`, `cloudflare-edge`, `cloudflare-tunnel`, `relay-oci`, `relay-hcloud`, `aws-node-box`, `vultr-node-box` |
| [`../.github/workflows/deploy.yml`](../.github/workflows/deploy.yml) | Build, sign and push every image on push to main; manual, environment-gated promote and host update |
| [`../.dockerignore`](../.dockerignore) | Build context filter for `docker build .` (the workflow); `build.sh` uses `git archive` instead |

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
  `GET /health` on a configurable `ip:port` (each merged service has its
  own variable, listed below);
- reads secrets from files named by `*_FILE` variables, never from the
  environment;
- runs as UID 65532 with a read-only root filesystem; writable paths are
  `/tmp` and the `/var/lib/bleh/*` volumes listed below;
- exits non-zero on fatal errors (the orchestrator restarts it) and stops
  cleanly on SIGTERM within 30 s;
- labels nothing with a site: Prometheus adds `site` (and `path` where useful).

Every service is now merged and keeps its own variable names, mapped from
the same `.env` values in compose. Shared conventions: object storage for
the archive tiers is `BLEH_ARCHIVE_{PUBLIC,PRIVATE}_*` (the `x-archive-env`
block in compose), the block-signing key is `BLEH_SIGNING_KEY_FILE`
(32-byte secp256k1 secret as 64 hex characters, optional `0x` and trailing
newline) and the NATS side tap is `BLEH_NATS_URL`.

### origin (integrator, merged)

Implemented ([`crates/origin`](../crates/origin); runbook
[origin.md](../docs/operations/origin.md)). One process on the node box:
the node reader (one or two node directories, active-active merge), the
book builder and derived records, the block signer, the gateway in process
("Library" mode in [gateway.md](../docs/operations/gateway.md)), an
optional JetStream side tap and live capture of the private and filtered
raw tiers. Image `origin`; compose service `origin` (profile `node-box`);
systemd `bleh@origin` ([env/origin.example.conf](systemd/env/origin.example.conf)).
The standalone `hl-node-reader` image remains for the synthetic replay and
debugging.

| | |
|---|---|
| Ports | `BLEH_GATEWAY_LISTEN` (`0.0.0.0:8080`, admitted only on the overlay by the host firewall): `/v0/ws`, `/health` (503 after `BLEH_GATEWAY_MAX_STALENESS_SECS` without a block), `/freshness`, `/proof`, `/signers`, `/metrics` (gateway, bus and signer). `HL_METRICS_ADDR` (9184 on the overlay address): `/metrics` (reader and origin), `/health` |
| Networking | Host network (the node's info server is loopback-only; NATS on `127.0.0.1:4222`). Relays connect to `ws://10.77.0.x:8080/v0/ws` |
| Variables set | Reader (`HL_*`, names of [node-reader.md](../docs/operations/node-reader.md)): `HL_NODE_DIRS` (`/hl/data`; both nodes' directories in Tier 2; capture follows the first), `HL_INFO_URL`, `HL_REGISTRY_OUT` and `HL_CHECKPOINT` (under `/var/lib/bleh/reader`, host `BLEH_READER_DIR`), `HL_SNAPSHOT_INTERVAL_SECS` (600), `HL_SNAPSHOT_PATH` (`/snapshots/l4.json`; the node writes it, the origin reads the same path), `HL_METRICS_ADDR`, `HL_LOG_FORMAT=json`. Origin (`BLEH_*`): `BLEH_SIGNING_KEY_FILE` (`/run/secrets/signing_key`, 64 hex digits), `BLEH_SIGNING_KEY_ID`, `BLEH_GATEWAY_LISTEN`, `BLEH_NATS_URL`, `BLEH_RAW_DIR` (`/var/lib/bleh/raw`, host `BLEH_RAW_DIR` = `/srv/bleh/raw`), `BLEH_CAPTURE_UPLOAD=true`, `BLEH_CAPTURE_METRICS_OUT` (`/textfile/bleh_archive_capture.prom`), `BLEH_BOOK_CHECKPOINT_SECS` (600; the archive replay uses the same `.env` value), and `BLEH_ARCHIVE_{PUBLIC,PRIVATE}_*` for capture uploads. Every flag and default: [`config.rs`](../crates/origin/src/config.rs) |
| Volumes and secrets | Node data (read-only), snapshots, raw staging, reader state, the node-exporter textfile directory (group 65532 writable; `host-bootstrap.sh`); secret `signing_key` (compose bind-mounts it with its host owner, so `up.sh`, run as root, hands the key files to UID 65532, mode 0400) |
| Capture watermark | `<BLEH_RAW_DIR>/.captured-through` (`YYYY-MM-DDTHH`): the last hour sealed in every stream and uploaded. On the host it is `/srv/bleh/raw/.captured-through`, which `hl-prune.sh` reads (its default `BLEH_RAW_DIR`); stream files are deleted only up to it |
| Registry for replay | `registry-sync` (compose, node-box) copies `registry.json` to `s3://<public bucket>/_registry/hyperliquid/registry.json` every 5 minutes; `archive-mirror` brings it to the home server, where `archive-job` reads it as `BLEH_ARCHIVE_REGISTRY` (override: `.env` `ARCHIVE_REGISTRY`) |
| Health | Compose has no healthcheck (distroless image, no shell); `update-host.sh` waits for `:9184/health` and `:8080/health` and rolls back otherwise. Shutdown within 25 s of SIGTERM (checkpoint written, connections closed with 1001, open capture hour kept) |
| Metrics used by rules and dashboards | Reader: `hl_reader_last_block`, `hl_reader_block_lag_seconds`, `hl_reader_emit_lag_seconds`, `hl_reader_parse_errors_total`, `hl_reader_rejected_blocks_total`, `hl_reader_incomplete_blocks_total`, `hl_output_last_block`, `hl_output_block_lag_seconds`, `hl_output_gaps_total`, `hl_merge_mismatches_total`, `hl_merge_one_sided_gap_blocks_total`, `hl_registry_refreshes_total`, `hl_checkpoint_writes_total`. Origin: `bleh_signing_failures_total`, `bleh_publish_errors_total`, `bleh_book_reconcile_mismatch_total`, `bleh_book_process_errors_total`, `bleh_book_desyncs_total`, `bleh_book_snapshots_unusable_total`, `hl_reader_snapshot_failures_total`. Gateway (`:8080`): see below. Capture (textfile): `archive_capture_files_sealed_total`, `archive_capture_upload_errors_total`, `archive_store_put_created_total`, `archive_store_put_conflict_total` and the file's age. Prometheus job `hl-node-reader` (name kept for series continuity) scrapes `:9184` |

Resolved from the earlier requests to the reader, gateway and archive
lanes: network output (the gateway runs in process, so there is no
reader-to-gateway transport and no port 7400); live filtered and
unfiltered raw capture; the `.captured-through` watermark; block signing
with `BLEH_SIGNING_KEY_FILE` and `bleh_signing_failures_total`;
`hl_reader_snapshot_failures_total`; `bleh_book_reconcile_mismatch_total`;
environment variables for every gateway flag (`BLEH_GATEWAY_*`,
`BLEH_GATEWAY_MODE`); the archive `Hooks` (filters and replay parser);
periodic `--metrics-out` while capture runs; `_manifests/latest.json`.

Still open:

- Reader emit lag is a gauge (`hl_reader_emit_lag_seconds`), so its p99 is
  taken over scrape samples. The end-to-end p99 against the 50 ms target
  comes from the `gateway_send_latency_seconds` histogram, which covers
  the reader; a reader histogram would only localise a regression.
- L4 snapshots are not captured into the raw tiers, so book-dependent
  derived records replayed by the archive equal the live ones only from an
  empty book (`BLEH_ARCHIVE_BOOK_START=empty`, a first recorded day)
  ([archive.md](../docs/operations/archive.md), "Known limits").
- The origin does not publish its registry itself; `registry-sync` (deploy
  side) does. With systemd instead of compose the copy is manual.
- The `synthetic` profile still runs the demo gateway and a one-shot
  `hl-node-reader` replay, not the origin; the origin's end-to-end path on
  synthetic data is covered by `crates/origin/tests/e2e.rs` only.
- Elsewhere in this contract: hl-backfill progress metrics while a copy
  runs; swap-pollers ICE Trade Vault sources (not admitted yet).

### gateway (bus, gateway and client lane, merged)

Implemented; reference in [gateway.md](../docs/operations/gateway.md).
Every flag also has a `BLEH_GATEWAY_*` variable and `BLEH_GATEWAY_MODE`
selects `demo` or `relay`, so systemd units need no `BLEH_ARGS`; compose
passes flags.

| | |
|---|---|
| Origin | In process in `origin` (above), not a separate service |
| Relay | Compose: `gateway relay --listen=0.0.0.0:8080 --trust-forwarded-for --upstream=ws://10.77.0.1:8080/v0/ws --signer=0x...` (`RELAY_UPSTREAM_1/2`, `BLEH_SIGNER_1/2`); systemd: `BLEH_GATEWAY_MODE=relay`, `BLEH_GATEWAY_UPSTREAM`, `BLEH_GATEWAY_SIGNER` ([env/relay.example.conf](systemd/env/relay.example.conf)). Caddy in front sets `X-Forwarded-For` and hides `/metrics` |
| Demo | `gateway demo --listen=0.0.0.0:8080 --signing-key=k1=file:/run/secrets/signing_key --nats-url=nats://nats:4222` |
| Metrics used | `gateway_send_latency_seconds` (histogram; the 50 ms target), `gateway_blocks_total`, `gateway_connections`, `gateway_messages_sent_total`, `gateway_bytes_sent_total`, `gateway_forced_snapshots_sent_total`, `gateway_slow_consumer_disconnects_total`, `gateway_rate_limited_total`, `gateway_hub_lagged_total`, `relay_connected`, `relay_streams_rejected_total` |

### archive (archive lane, merged)

Implemented as a batch CLI; reference in
[archive.md](../docs/operations/archive.md). The `archive` binary is built
from `crates/origin` with the reader's line filters and the origin's
replay parser wired in.

| | |
|---|---|
| Node box | Live capture runs inside `origin` (no separate service). Standalone alternative for a host whose origin does not capture: `archive capture run --follow --staging /var/lib/bleh/raw ... --upload --metrics-out /textfile/bleh_archive_capture.prom` ([env/archive-capture.example.conf](systemd/env/archive-capture.example.conf)) |
| Home server | `archive-job` service (profile `archive-jobs`) run by `deploy/scripts/archive-pipeline.sh` from `bleh-archive-daily.timer` (replay, `manifest build --sign`, `anchor ots-stamp`/`evm-submit`, `publish --upload --kubo`, `verify`) and `bleh-archive-anchors.timer` (`ots-upgrade`, `evm-poll`, re-publish); by hand: `deploy/up.sh home-archive archive <command>` |
| Variables set | `BLEH_ARCHIVE_{PUBLIC,PRIVATE}_{URL,ENDPOINT,REGION,ACCESS_KEY_ID,SECRET_ACCESS_KEY,ALLOW_HTTP,PATH_STYLE}` from the `.env` storage settings; `BLEH_ARCHIVE_SIGNING_KEY_FILE` (manifest key), `BLEH_ARCHIVE_KUBO_API`, `BLEH_ARCHIVE_EVM_RPC`, `BLEH_ARCHIVE_EVM_KEY_FILE`; replay: `BLEH_ARCHIVE_REGISTRY` (the origin's registry from the mirror, see origin above), `BLEH_ARCHIVE_BOOK_START` (`.env` `ARCHIVE_BOOK_START`, default `snapshot`), `BLEH_BOOK_CHECKPOINT_SECS` (same `.env` value as the origin) |
| Metrics used | Textfile: capture counters (see origin); `bleh_archive_step_ok{step,archive}`, `bleh_archive_manifest_last_success_timestamp_seconds`, `bleh_archive_anchor_last_success_timestamp_seconds{anchor}`, `bleh_archive_daily_last_run_timestamp_seconds` (written by archive-pipeline.sh). Blackbox probes each archive's `_manifests/latest.json` (written by manifest refreshes, uploaded by `publish`) |

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

### reference-pollers (reference-pollers lane)

Implemented; reference in [reference-pollers.md](../docs/operations/reference-pollers.md).

| | |
|---|---|
| Ports | `REFERENCE_POLLERS_LISTEN` (9188): `/metrics`, `/health` |
| Variables set | `REFERENCE_POLLERS_RAW_DIR`, `REFERENCE_POLLERS_STATE_DIR`, `REFERENCE_POLLERS_OUT_DIR` (under the `reference-data` volume), `REFERENCE_POLLERS_USER_AGENT`, `REFERENCE_POLLERS_SOURCES`, `REFERENCE_POLLERS_ERCOT_PRODUCTS`, `REFERENCE_POLLERS_LOG_JSON` |
| Backfill | `docker compose --profile backfill run --rm reference-backfill` (archive `reference-backfill` under the `reference-data` volume), or `bleh-reference-backfill.service` |
| systemd alternative | `bleh@reference-pollers` with `systemd/env/reference-pollers.example.conf` |
| Metrics used | `ref_last_success_timestamp_seconds{source}`, `ref_unknown_fields_total{source,field}`, `ref_parse_errors_total{source}`, `ref_stale_reports_total{source}` |
| Still requested | Partitioning `records/` into a daily `reference-normalized` archive in `archive-pipeline.sh` (the JSON lines and `_notices.json` are ready for it) |

### hl-backfill (backfill lane, merged)

Implemented; reference in [backfill.md](../docs/operations/backfill.md).

| | |
|---|---|
| Run | `deploy/up.sh home-archive backfill <estimate|copy|process|compare|replay-prototype> [args]` (one-off container on the home server's NVMe scratch) |
| Variables set | Source (requester-pays): `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` from `BACKFILL_SOURCE_*` in `.env`, `HL_NODE_DATA_REGION`, `HL_ARCHIVE_REGION`; destination: `BLEH_RAW_ENDPOINT`, `BLEH_RAW_ACCESS_KEY_ID`, `BLEH_RAW_SECRET_ACCESS_KEY`, `BLEH_RAW_REGION` from the R2 settings |
| Still requested | Progress metrics (`/metrics` while running) for long copies |

### mesh-client (client lane)

Merged ([client.md](../docs/operations/client.md)). `mesh-client probe --url
URL --instrument ID --kind KIND --timeout 5s` prints one JSON line
`{"ok":bool,"first_block_seconds":f,"block_time_ms":n}` (plus `error`) and
does the availability probe (`deploy/observability/prober/ws-probe.sh`,
`PROBE_CLIENT=auto|mesh-client|docker`; websocat is only a fallback).
`mesh-client keys address --key-file F` derives a signer address
([keys.md](../docs/operations/keys.md)).

## Validation status

Checked for the origin integration: `docker compose config` for every
profile (`synthetic`, `node-box`, `home-archive`, `relay`, `archive-jobs`,
`backfill`, `synthetic-relay`) with the template `.env` and with host paths
set; YAML parsing of `compose.yaml`, both Prometheus configs, the rule
files and `.github/workflows/deploy.yml`; JSON validity of the dashboards;
`bash -n` on every script; the prober against a stub `mesh-client`
(success, failure with `error`, `PROBE_SIGNERS`); every metric named in
rules and dashboards checked against the code that emits it. Earlier:
the schema-drift check against the synthetic fixtures and
`scripts/check.sh`.

Not run: image builds (including the new `origin` target) and the stack
itself, so the origin container, `registry-sync` (rclone as UID 65532),
capture uploads to R2 and the watermark on a real host are unproven;
`promtool check rules`, `actionlint`, shellcheck and `terraform validate`
(tools not installed).
