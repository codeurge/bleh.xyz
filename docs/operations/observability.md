# Observability

What is measured, where it is collected, what alerts, and how to read it.
Targets come from [requirements.md](../requirements.md): p99 of gateway send
time minus node local write time at most 50 ms, and 99.9% availability over
30 days measured by an external probe. The metric names every service must
expose are listed in [deploy/README.md](../../deploy/README.md) ("Service
contract").

## Where things run

| Component | Host | Files |
|---|---|---|
| Prometheus (90 days), Alertmanager, Grafana, Loki (30 days), blackbox exporter | home-archive server | `deploy/observability/` |
| node_exporter (with the textfile collector) | every host | compose service `node-exporter` |
| Vector (container logs to Loki) | every host | `deploy/compose/vector/vector.yaml` |
| Availability probe | home-archive server and one relay (two vantage points), once a minute | `deploy/observability/prober/ws-probe.sh`, `bleh-probe.timer` |

Prometheus scrapes the other hosts over the WireGuard overlay
(`deploy/observability/prometheus/prometheus.yml`); nothing is scraped over
the public internet. Grafana is reachable at `grafana.bleh.xyz` through the
home tunnel behind Cloudflare Access, or `ssh -L 3000:127.0.0.1:3000` to the
home server.

Locally, `deploy/up.sh synthetic` runs the same Prometheus rules, Grafana
dashboards and Loki at <https://grafana.localhost:8443/>.

## Dashboards

Provisioned from `deploy/observability/grafana/dashboards/` (edit the JSON
in the repository, not in the UI):

| Dashboard | Shows |
|---|---|
| bleh: live pipeline | Availability (30 d, 1 d), p99 and p50 publish latency against 50 ms, share within 50 ms, ingest lag, lag behind block time, path agreement and mismatches, gaps and re-snapshots, schema drift, clients, egress, upstreams, swap pollers |
| bleh: hosts and nodes | Node disk free and growth per day, hours to full, pruner, hl-node binary changes, clock offset and sync, CPU, memory, network, IO, reboots, node warnings from Loki |
| bleh: archive and history | Age of last raw upload, upload failures, last manifest and anchors, content-hash mismatches, signing failures, history API latency and timeouts |

## Latency budget

The measured quantity is `ts_publish − ts_node` per record, exported by the
gateway as the histogram `bleh_gateway_publish_latency_seconds` (buckets
must include 0.05). Its parts:

| Segment | Metric | Budget |
|---|---|---|
| Node write → reader receive (inotify, parse) | `bleh_reader_ingest_lag_seconds` | 25 ms at p99 |
| Reader → gateway send (book, signing, encode) | difference of the two | 25 ms at p99 |

Lag behind `block_time` (`time() - bleh_reader_last_block_time_seconds`) is
reported, not gated: it depends on the node's location (higher in Europe
than in Tokyo; ADR 0010) and is shown on the live dashboard and the status
page.

Alerts: `BlehReaderIngestLagHigh` (p99 > 25 ms for 10 min),
`BlehPublishLatencyP99High` (> 50 ms for 10 min),
`BlehPublishLatencyP99Critical` (> 250 ms for 5 min). When latency rises,
check in order: disk IO on the node volume (output is unbuffered), CPU
saturation from the node, send-buffer backlog from slow consumers in the
gateway (`bleh_gateway_slow_consumer_closes_total`), clock offset.

## Availability

The definition (requirements.md): connect once a minute, subscribe to one
instrument, receive a block heartbeat within 5 seconds.
`ws-probe.sh` does exactly that and writes `bleh_probe_success`,
`bleh_probe_first_block_seconds` and `bleh_probe_block_age_seconds` to the
textfile collector. Run it from at least two networks that are not the
origin's (default: the home server and relay-1; any small VM elsewhere is
better still). Settings in `/etc/bleh/probe.env`.

Recording rules give `bleh:probe_success:ratio_{5m,30m,1h,6h,1d,30d}`. The
30-day ratio is the published availability. Alerts follow the multi-window
burn-rate pattern for a 0.1% budget: `BlehAvailabilityFastBurn` (14.4× over
1 h and 5 m, page), `BlehAvailabilitySlowBurn` (6× over 6 h and 30 m),
`BlehProbeFailing` (three of five probes failed), `BlehProbeStale`.
Announced Hyperliquid upgrades are excluded from the published figure: note
their windows on the status page and subtract them when publishing.

Blackbox checks add HTTP health for `live`, `api`, each `relay-N`, the
archive's latest manifest, the websocket upgrade through the CDN, and
certificate expiry (`BlehHttpEndpointDown`, `BlehCertificateExpiring`).

Status page: the gateway's `/health` and `/freshness` are public. The
planned status page (Phase 1 observability task, not built yet) is a static
page that reads availability ratios published hourly to the archive bucket,
so it stays up when the live path does not.

## Clients

`bleh_gateway_connections`, `bleh_gateway_subscriptions`,
`bleh_gateway_bytes_sent_total` per site (origin and each relay),
`bleh_gateway_slow_consumer_closes_total` and
`bleh_gateway_rate_limited_total`. Relays carry users, so relay egress is
the number to compare with the Oracle 10 TB/month allowance
([costs.md](costs.md), assumption A5).

## Hosts

node_exporter on every host: CPU, memory, filesystems, disks, network.
Textfile metrics from our timers: pruning (`bleh_hl_prune_*`), schema drift
(`bleh_schema_drift_*`, `bleh_hl_node_binary_changed_timestamp_seconds`),
pending reboots and security updates (`bleh_reboot_required`,
`bleh_security_updates_pending`), and probe results. Alerts:
`BlehNodeDisk*`, `BlehDiskLow`, `BlehMemoryPressure`,
`BlehHostExporterDown`, `BlehRebootRequired`.

## Time sync

Every host runs chrony with NTS-authenticated sources (Cloudflare, Netnod,
PTB) plus the Ubuntu pool; add a provider-local server in
`/etc/chrony/conf.d/91-local.conf` (AWS: `169.254.169.123`). node_exporter's
timex collector exports `node_timex_offset_seconds` and
`node_timex_sync_status`. Alerts: `BlehClockOffsetHigh` (above 5 ms for 5
minutes) and `BlehClockUnsynchronised`. Check with `chronyc tracking` and
`chronyc sources -v`. An offset on the node box skews `ts_node`-based latency
directly.

## Logs

Services log JSON to stdout; Docker keeps 5 × 50 MB per container and
Vector ships everything at info and above to Loki on the home server over
the overlay (disk-buffered for 512 MB if the overlay is down). The node's
own `visor_child_stderr` crash logs are shipped from the native install via
`deploy/observability/vector/vector.yaml` (journal and file sources). Node
output files are data and are never shipped. Query in Grafana Explore, for
example `{unit="hl-node-reader"} |= "error"`.

## Alert routing

Rules: `deploy/observability/prometheus/rules/`. Routing:
`deploy/observability/alertmanager/alertmanager.yml`.

| Severity | Goes to | Examples |
|---|---|---|
| `page` | `page_webhook_url` (phone push or call) | All paths or gateways down, integrity mismatch, schema drift, availability fast burn, raw capture stalled, manifest overdue |
| `ticket` | `ticket_webhook_url` (chat or issue tracker) | One path down, latency over target, disk low, pruner blocked, anchor overdue |
| `info` | dashboard only | hl-node binary changed, slow consumers, reboot pending |
| `Watchdog` | `deadman_webhook_url` every minute | A free heartbeat service alerts if it stops arriving: catches a dead Prometheus, Alertmanager or home server |

Webhook URLs are files in `deploy/compose/secrets/alertmanager/`
(keys.md). Every alert carries a `runbook` annotation pointing at
[incident.md](incident.md), [node.md](node.md), [upgrades.md](upgrades.md)
or this page.
