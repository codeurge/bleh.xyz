# Hosting

Where bleh.xyz runs, at each budget, and how to move up a tier without
changing anything but the hosts. The decision for launch is
[ADR 0010](../decisions/0010-budget-first-hosting.md); the architecture is
[ADR 0006](../decisions/0006-phase-1-2-service-architecture.md) at every
tier. Prices are in [costs.md](costs.md); deployment steps in
[deploy.md](deploy.md).

## Recommendation (ADR 0010)

**One rented node box, one self-hosted archive server, free relays, R2.**

```
                 Hyperliquid peers (gossip, TCP 4001-4002)
                              │
   ┌──────────── node box (rented, Europe) ─────────────┐
   │ hl-node → origin (reader, book, signer, gateway)   │── raw capture ──▶ R2 (private + public)
   │            NATS side tap, node exporter, pruner    │                     │
   └──────────────┬─────────────────────────────────────┘                     │ CDN: data.bleh.xyz
                  │ WireGuard overlay (relays verify every signature)        │ torrents, IPFS
                  ▼                                                           │
   relay-1, relay-2 (free ARM VMs): gateway in relay mode, history API ◀─ users: live.bleh.xyz, api.bleh.xyz
                                                                              │
   ┌──────── home-archive (self-hosted home or LAN server) ─────────┐         │
   │ archive processing (replay, partitions, manifests, anchors)    │◀── R2 mirror (zero egress)
   │ full archive mirror on HDD, BitTorrent seeder, IPFS pinning    │── partitions, manifests ──▶ R2
   │ backfill, swap pollers, staging history API, monitoring        │
   └──────── Cloudflare Tunnel for Grafana and staging API ─────────┘
```

| Piece | Runs | Deploy profile |
|---|---|---|
| Node box | Hyperliquid non-validating node and `origin` ([origin.md](origin.md)): reader, book builder, block signer and gateway in one process, live raw capture to R2 with the pruning watermark; NATS side tap | `deploy/up.sh node-box` |
| Home-archive server | Archive processing, mirror, seeder, IPFS, backfill, swap pollers, staging history API, Prometheus, Alertmanager, Grafana, Loki | `deploy/up.sh home-archive` |
| Relays (2) | Gateway in relay mode (verifies signatures), public history API, Caddy | `deploy/up.sh relay` |
| Edge | R2 buckets, `data.bleh.xyz`, DNS, rate limits, the home tunnel | `deploy/terraform/envs/edge` |

Why this shape: the latency target is gateway send minus node local write,
so a European node meets it (lag behind block time is reported, not gated).
Relays carry the user fan-out, so the origin uploads one stream per relay.
Archive work needs disk, not a fast chain-following CPU, which suits a home
server. Node outages show as flagged gaps, never filled.

## Tiers

| Tier | What runs where | Delivers | Monthly (estimate, Nov 2026) |
|---|---|---|---|
| **0: archive-first** | No node. Home server (or occasional spot VM) runs backfill from Hyperliquid's public buckets (ADM-0003) into R2; static archive behind the CDN, torrents, IPFS | Historical trades and book snapshots back to the earliest available date; no live stream | **$5–40** |
| **0.5: self-hosted origin** | A home server that qualifies to run the node runs node-box and home-archive together (`deploy/up.sh origin`); free relays reach it over the overlay, which it dials out to | Everything, with home-grade uptime | **$35–80** including electricity |
| **1: lean live (default)** | Rented node box + home-archive server + free relays + R2 | Everything; one ingest path | **$230–300** including home electricity; $145–195 with the stretch node box below |
| **2: two origins** | Second node box at another provider, both feeding every relay; health-checked load balancing | Active-active ingest, the path-agreement integrity check | **$435–575** |
| **3: Tokyo active-active** | Two node boxes in Tokyo on AWS and Vultr (`deploy/terraform/envs/tokyo`) | ADR 0006 as written: lowest lag behind block time | **$1,700–1,850** |

Each step changes only hosts and DNS: the same images, compose profiles,
systemd units, keys procedure and Terraform modules. Move up when usage or
sponsorship pays for it ([costs.md](costs.md), "Free and sponsored").

## Host requirements

### Node box (and a self-hosted origin)

| Requirement | Why | Check |
|---|---|---|
| 16 or more modern cores (Zen 4/5 or recent Xeon), high single-thread speed | The official minimum is 16 vCPUs; block application is largely sequential, so per-core speed decides whether the node keeps up | `BlehNodeBehindChain` stays quiet for a week; `bleh:reader_ingest_lag_seconds:p99_5m` under 25 ms |
| 128 GB RAM **free for the node** | Official minimum; the node alone wants it, so other services must fit beside it | `free -g` with the node running; `BlehMemoryPressure` quiet |
| NVMe (not SATA SSD, never HDD), 1 TB or more | Output written with `--disable-output-file-buffering` at about 100 GB/day at default settings, more with the mesh's flags; 500 GB is the official floor | `deploy/scripts/disk-setup.sh`; disk-growth panel; pruning keeps 24 h after capture |
| Ubuntu 24.04 | The only OS the node supports. On another host OS, use an Ubuntu 24.04 VM with the NVMe passed through | `host-bootstrap.sh` refuses other systems for node roles |
| TCP 4001-4002 reachable from the internet | Gossip; peers deprioritise unreachable nodes | Router port forward at home; provider firewall from the Terraform host module |
| Provider acceptable-use policy allows blockchain nodes | Hosting can be cut off without notice otherwise | See the provider table |
| Stable clock | Latency is measured from node write time | chrony with NTS (host-bootstrap); `BlehClockOffsetHigh` above 5 ms |

An older dual-socket server with about 110 GB of RAM already partly
committed to other virtual machines does **not** qualify: it lacks free
memory for the node and its cores may not keep up with the chain. It is
ideal for the home-archive role.

### Home-archive server

| Requirement | Why |
|---|---|
| Disk array (HDD is fine) sized for the full public archive plus private raw: start at 8–20 TB | Mirror for seeding and pinning; unfiltered raw kept as long as storage allows (ADR 0006) |
| NVMe or SSD scratch, 500 GB+ | Backfill decompression, replay and partition writing |
| 8+ cores, 32+ GB RAM free | Replay, zstd, Parquet writing, DuckDB in the staging API |
| Ubuntu 24.04 VM or host with Docker | Same compose profile as everywhere else |
| Outbound internet only | Cloudflare Tunnel publishes Grafana and the staging API; R2 is reached outbound |

## Self-hosted roles in detail

Two self-hosted roles exist. The **home-archive** role is the default
(ADR 0010). The **self-hosted origin** (Tier 0.5) is for a home server that
meets the node requirements above.

### No inbound ports

Nothing at home accepts inbound connections for the stream or the tools:

- **Live stream:** relays connect to the origin gateway at
  `ws://10.77.0.x:8080/v0/ws` over the WireGuard overlay. A home server has
  no WireGuard endpoint of its own; it dials out to the relays and node box
  and keeps the tunnel open (`PersistentKeepalive`), so no router port is
  needed and the home address appears in no DNS record.
- **Grafana and the staging API:** published through Cloudflare Tunnel
  (`cloudflared` dials out). `grafana.bleh.xyz` and `api-staging.bleh.xyz`
  sit behind Cloudflare Access with operator email login.

The edge stack creates the tunnel, DNS records and Access policies
(`deploy/terraform/modules/cloudflare-tunnel`); a locally managed
alternative is `deploy/compose/cloudflared/config.example.yml`. The module
can also publish a node box's gateway as an Access-gated `origin-<site>`
hostname, but relays do not need it.

Two exceptions, both optional: the BitTorrent seeder (6881) and IPFS (4101)
work without a port forward but reach more peers with one; a self-hosted
origin needs 4001-4002 forwarded for gossip.

### Relays carry the fan-out

Users never connect to a home server. Each relay holds one upstream
websocket to the origin and re-serves the signed stream to its clients;
clients verify signatures, so relays need not be trusted. The origin's
upload is therefore one stream per relay, not one per user.

### Bandwidth (estimates; measure on the first node)

| Flow | Volume | Average rate |
|---|---|---|
| Origin → each relay (all markets, compressed websocket) | 20–80 GB/day | 2–8 Mbit/s, bursts several times higher |
| Origin → R2, filtered raw (zstd 19) | 5–15 GB/day | 0.5–1.5 Mbit/s |
| Origin → R2, unfiltered raw (zstd long range) | 10–30 GB/day | 1–3 Mbit/s |
| Home-archive ← R2 mirror | equal to the public archive's daily growth | about 1 Mbit/s |
| Home-archive → R2, partitions and manifests | 5–15 GB/day | 0.5–1.5 Mbit/s |
| Seeder and IPFS upload | capped in qBittorrent and Kubo settings | as configured |

A self-hosted origin with two relays needs about 10–25 Mbit/s of sustained
upload with headroom for bursts: fibre or a fast cable plan. The
home-archive role alone needs a few Mbit/s.

### Power, connectivity and uptime

- **Electricity:** a server drawing 150–350 W uses 108–252 kWh a month:
  about $20–46 at the US residential average (18.16 ¢/kWh, August 2026) or
  €30–71 at the EU household average (about €0.28/kWh, first half 2026).
- **UPS:** a line-interactive UPS sized for at least 10 minutes rides through
  short cuts and lets the host shut down cleanly; Docker restarts the stack
  on boot.
- **ISP terms:** many residential plans prohibit servers or cap upload. The
  overlay and the tunnel keep the setup outbound-only, but check the contract for traffic
  caps and acceptable use, and prefer a static or slow-changing address for
  the gossip port forward.
- **Uptime expectation:** a home connection typically gives 99–99.5%, below
  the 99.9% target. That is acceptable for the home-archive role (archive
  work is batch and catches up) and for Tier 0.5 as a starter; the
  availability figure is published as measured.

### Risks

- **Resource contention:** the node alone wants 128 GB of RAM and steady
  NVMe writes. Running it beside other services or VMs on the same host
  starves one or the other. Give the node a dedicated VM with reserved
  memory, or do not host it at home.
- **Outages become gaps:** power or ISP outages stop recording. They appear
  as `gap` records and in the coverage map, never filled in; Hyperliquid's
  public buckets can backfill fills later (flagged `backfill`).
- **Single copy of private raw:** unfiltered raw kept only on the home
  array is one disk failure from loss. Use a redundant array (RAID 6, ZFS
  raidz2) and keep the newest 30 days in the private R2 bucket as well.
- **Home address exposure:** gossip on a self-hosted origin reveals the
  home IP to Hyperliquid peers; the overlay does not hide that. The
  home-archive role exposes nothing.

### Moving to rented hardware

Nothing changes but the host:

1. Order the box (provider table below), run
   `sudo deploy/scripts/host-bootstrap.sh node-box --site a --overlay-ip 10.77.0.1`.
2. Copy `deploy/compose/.env` and `deploy/compose/secrets/` from the old
   host, or issue a new signing key (keys.md: a new ingest path can have its
   own key; publish it before switching).
3. Give the new host the old overlay address (`wg-mesh.sh` with its new
   public key and endpoint in the peers file, applied on every host) and
   stop the node box roles on the old host. Relays reconnect to the same
   `ws://10.77.0.x:8080/v0/ws`.
4. `deploy/up.sh node-box`; the old host keeps its home-archive role.

## Providers

Figures were read on 2026-11-12 from the providers' own pages unless marked
**(secondary)** or **(unverified)**. Prices change; check before ordering.
Prices exclude VAT.

### Node boxes (16 cores or 16+ threads, 128 GB, NVMe)

| Provider and plan | Monthly / setup | Traffic | Tokyo | Blockchain nodes under AUP | Source |
|---|---|---|---|---|---|
| Cherry Servers, Ryzen 9 7950X (16c/32t, 128 GB, 2×1 TB NVMe) | $232.83 ($197.91 billed annually); setup not listed | 100 TB, 3 Gbit/s | No stock found; EU and SG listed | Markets to Web3 node operators; no clause against nodes in its terms. Out of stock everywhere at check time | [pricing](https://www.cherryservers.com/pricing/dedicated-servers/amd-ryzen-7950x), [terms](https://www.cherryservers.com/legal/terms-of-service), [Solana page](https://www.cherryservers.com/solana-servers) |
| OVHcloud Advance-4 (EPYC 4584PX, 16c, 64–192 GB) | about $260 / about $260 setup; 128 GB price **(unverified)** | 3 Gbit/s unmetered outside APAC | No Japan datacenter found; SG, Sydney, Mumbai | Explicitly supports PoS validators and RPC nodes; no PoW or mining | [Advance](https://www.ovhcloud.com/en/bare-metal/advance/), [blockchain](https://us.ovhcloud.com/solutions/blockchain/) |
| OVHcloud Rise-2 (8c/16t, up to 128 GB) | from $72 / $72 setup; 128 GB upgrade price **(unverified)** | 1–3 Gbit/s | No | As above | [Rise](https://eco.ovhcloud.com/en/rise/) |
| OVHcloud Scale-a1 (EPYC 9124, 16c/32t, 128 GB) | $638 / $638 setup | 5 Gbit/s | No | As above | [Scale](https://www.ovhcloud.com/en/bare-metal/scale/) |
| Hetzner AX102 (Ryzen 9 7950X3D, 16c/32t, 128 GB, 2×1.92 TB NVMe) | €259 / €129 setup (overview page says "from €159": conflicting) | Unlimited at 1 Gbit/s | No (FSN1, HEL1) | Prohibits "operating applications that are used to mine crypto currencies"; press reports Hetzner refusing nodes and blockchain data **(secondary)**. **High risk** | [AX102](https://www.hetzner.com/dedicated-rootserver/ax102/), [policy](https://www.hetzner.com/legal/dedicated-server/), [report](https://www.theblock.co/post/165976) |
| Latitude.sh m4.metal.medium (EPYC 9124, 16c, 128 GB, 2×1.9 TB NVMe) | $456 | 20 TB | Tokyo is a Latitude location; plan stock there **(unverified)** | No restriction found **(unverified)** | [pricing](https://www.latitude.sh/pricing), [locations](https://www.latitude.sh/locations) |
| Vultr vbm-8c-132gb (E-2288G, 8c/16t, 128 GB, 1.92 TB NVMe) | $350 | 10 TB, then $0.01/GB | Yes (nrt) | No restriction found **(unverified)** | [plans API](https://api.vultr.com/v2/plans-metal), [overage](https://docs.vultr.com/support/platform/billing/what-is-the-bandwidth-overage-rate) |
| AWS EC2 i4i.4xlarge, Tokyo (16 vCPU, 128 GiB, 3,750 GB NVMe) | $1.610/h, about $1,175 | $0.114/GB out (first 10 TB) | Yes | Allowed | [AWS price list](https://b0.p.awsstatic.com/pricing/2.0/meteredUnitMaps/ec2/USD/current/ec2-ondemand-without-sec-sel/Asia%20Pacific%20%28Tokyo%29/Linux/index.json) |

**Choice for Tier 1:** a 16-core Zen 4 box from a provider that permits
nodes: Cherry Servers (Ryzen 7950X) when in stock, otherwise OVHcloud
Advance-4. **Stretch option** to reach the ~$150 target: OVHcloud Rise-2
with 128 GB (8 cores / 16 threads meets the documented 16 vCPU minimum but
not ADR 0010's "16+ modern cores"); run it only after a week's trial shows
the node keeping up. Hetzner is cheaper on paper but its policy makes it a
poor home for a node.

### Relays

| Option | Price | Traffic | Notes | Source |
|---|---|---|---|---|
| Oracle Cloud Always Free, Ampere A1 | $0 | 10 TB/month out | 2 OCPU / 12 GB in total since mid-2026 (halved from 4/24); idle instances can be reclaimed | [free tier](https://docs.oracle.com/en-us/iaas/Content/FreeTier/freetier_topic-Always_Free_Resources.htm), [change](https://www.infoq.com/news/2026/07/oracle-cloud-free-tier-limits/) **(secondary)** |
| Hetzner Cloud CAX11 (arm64) / CX23 (x86) | €6.49 / €5.99 | 20 TB in EU; €1/TB beyond | "Currently unavailable" at check time; relays carry no chain data, so the node policy concern does not apply | [cloud](https://www.hetzner.com/cloud/cost-optimized/) |
| Vultr vc2-1c-1gb | $5 | 1 TB; pooled 2 TB free per account; $0.01/GB beyond | 31 locations including Tokyo | [plans API](https://api.vultr.com/v2/plans?type=vc2) |
| Fly.io shared-cpu-1x (512 MB) | $3.69 | $0.02/GB (NA, EU), $0.04 APAC | No free egress; per-GB cost grows with users | [pricing](https://docs.fly.io/about/pricing) |

Default: two Oracle free relays (`deploy/terraform/envs/relays-oci`), with
Hetzner Cloud as the paid fallback (`envs/relays-hcloud`).

### Object storage

| Option | Storage | Egress | Notes | Source |
|---|---|---|---|---|
| Cloudflare R2 | $0.015/GB-month Standard; $0.01 Infrequent Access (+$0.01/GB retrieval) | Free | Class A $4.50/M, Class B $0.36/M; 10 GB free | [pricing](https://developers.cloudflare.com/r2/pricing/) |
| Backblaze B2 | $6.95/TB-month | Free up to 3× stored, free via Cloudflare | Cheaper storage; egress through the CDN partner | [pricing](https://www.backblaze.com/cloud-storage/pricing) |
| AWS S3, Tokyo | $0.025/GB-month | $0.114/GB | Free if accepted into the Open Data Sponsorship Program | [S3 price list](https://b0.p.awsstatic.com/pricing/2.0/meteredUnitMaps/s3/USD/current/s3.json), [program](https://aws.amazon.com/opendata/open-data-sponsorship-program/) |

Default: R2 for the public archive (zero egress behind the CDN). B2 is a
drop-in alternative behind Cloudflare if storage cost dominates; only the
S3 endpoint changes.

### Seeding and pinning

| Option | Price | Notes | Source |
|---|---|---|---|
| Home-archive seeder and Kubo node | electricity only | Default (ADR 0010) | — |
| Whatbox seedbox | $15 (3.9 TB) to $64 (21.7 TB) | About $2.95/TB at the large tier | [plans](https://whatbox.ca/plans) |
| Pinata | Free 1 GB; $20 for 1 TB | Pinning service | [pricing](https://pinata.cloud/pricing) |
| Filebase | Free 5 GB; $7.50 for 500 GB | IPFS pinning over S3 API | [pricing](https://filebase.com/pricing/) |
| Storacha (web3.storage) | — | Pricing page now redirects to Fil One; treat as discontinued | [redirect](https://storacha.network/pricing) **(secondary)** |

Free and sponsored programs are listed in [costs.md](costs.md).
