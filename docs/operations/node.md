# Hyperliquid node

Running the non-validating Hyperliquid node that feeds the reader. Format
of its output: [hyperliquid-node-format.md](../specs/hyperliquid-node-format.md).
Admission: [ADM-0001](../admissions/ADM-0001-hyperliquid-trades-orders.md).
Hosting choices: [hosting.md](hosting.md). Upgrades and schema drift:
[upgrades.md](upgrades.md).

## Facts

| Item | Value |
|---|---|
| Minimum (official) | 16 vCPUs, 128 GB RAM, 500 GB SSD; Ubuntu 24.04 only |
| Our floor | 16 modern cores, 128 GB free for the node, 1 TB+ NVMe (hosting.md) |
| Ports | TCP 4001 and 4002 open to the internet (gossip); info server on `localhost:3001` |
| Install | `hl-visor` from `binaries.hyperliquid.xyz`, signature checked against the release key (fingerprint `CF2C2EA3DC3E8F042A55FB6503254A9349F1820B`, pinned in `deploy/docker/hl-node/Dockerfile` and `deploy/scripts/node-install.sh`) |
| Upgrades | `hl-visor` downloads, verifies and runs new `hl-node` versions by itself, without notice |
| Output | About 100 GB/day at default settings; more with the mesh's flags |

### Flags

Exactly these, and nothing else (ADR 0006, ADM-0001):

```
--write-fills --write-order-statuses --write-raw-book-diffs --batch-by-block --disable-output-file-buffering --serve-info
```

The entrypoint (`deploy/docker/hl-node/entrypoint.sh`, also used by the
native unit) refuses to start with any other set, and always refuses
`--write-misc-events`, `--write-hip3-oracle-updates`, `--write-trades` and
`--write-system-and-core-writer-actions`, whose streams the admission
excludes. `HL_ALLOW_FLAG_OVERRIDE=1` exists for local experiments only.

## Provision

1. **Pick the location by measuring.** On a candidate host (or a cheap VM
   in the same datacenter) run `deploy/scripts/peer-latency.sh` and compare
   connect times to the gossip root peers. Lower is better for lag behind
   block time; the 50 ms target is measured from node write time and is not
   affected.
2. **Install Ubuntu 24.04.** On a two-disk box, put the OS on the first
   disk (or a small RAID 1 partition pair) and leave the rest unpartitioned:
   `disk-setup.sh` builds `/srv/hl` from whole unused NVMe disks (RAID 0
   for two or more). If the installer already used both disks, create the
   data device yourself and pass it with `--node-disk /dev/mdX`.
3. **Bootstrap the host** from a checkout at a reviewed commit:

   ```bash
   sudo git clone https://github.com/codeurge/bleh.xyz /opt/bleh/src
   sudo git -C /opt/bleh/src checkout --detach <sha>
   sudo /opt/bleh/src/deploy/scripts/host-bootstrap.sh node-box --site a --overlay-ip 10.77.0.1
   ```

   This installs Docker, chrony, the firewall (22, 4001-4002, WireGuard),
   the pruning and drift timers, and creates the directories below.
4. **Gossip roots.** Mainnet needs at least one root IP. By default the
   entrypoint fetches the current list (`gossipRootIps` on the public info
   endpoint) at every start. To pin peers, set `HL_ROOT_NODE_IPS` (comma
   separated) in `deploy/compose/.env`; the node README lists
   community-run roots.
5. **Secrets and start:** install `secrets/signing-key.hex` and
   `secrets/tunnel-token.txt` ([keys.md](keys.md)), fill R2 settings in
   `.env`, then `deploy/up.sh node-box`.

Catching up takes a while; `docker compose logs -f hl-node` shows
`applied block N` lines once it streams live, and `BlehNodeBehindChain`
clears.

### Container or native

| | Container (default) | Native |
|---|---|---|
| Start | `deploy/up.sh node-box` (service `hl-node`) | `sudo deploy/scripts/node-install.sh && sudo systemctl enable --now hl-visor` |
| User | `hluser` (UID 10000) in the image | `hl` (UID 10000), home `/srv/hl` |
| Data | `/srv/hl/hl` mounted at `/home/hluser/hl` | `/srv/hl/hl` (the user's `~/hl`) |
| hl-node binary | inside the container (re-downloaded on restart) | `/srv/hl/hl-node` |

Use one or the other, never both. The reader, pruner and drift check see
the same paths either way.

## Layout

| Path on the host | Contents | Owner |
|---|---|---|
| `/srv/hl/hl/data/node_fills*/hourly/<YYYYMMDD>/<H>` | Fills, one line per block | node (10000) |
| `/srv/hl/hl/data/node_order_statuses*/hourly/...` | Order statuses | node |
| `/srv/hl/hl/data/node_raw_book_diffs*/hourly/...` | Raw book diffs | node |
| `/srv/hl/hl/data/periodic_abci_states/<date>/<height>.rmp` | State every 10,000 blocks | node |
| `/srv/hl/hl/data/replica_cmds/...` | Blocks as transactions (not used live) | node |
| `/srv/hl/hl/data/node_logs`, `visor_child_stderr` | Logs | node |
| `/srv/hl/snapshots/` | `fileSnapshot` output requested by the reader | node writes, reader reads |
| `/srv/bleh/raw/` | Reader's filtered and unfiltered raw, waiting for capture; `.captured-through` watermark | reader and archive (65532) |

Whether a live node with `--batch-by-block` writes `node_fills/` or
`node_fills_by_block/` is still to verify (format spec, § 2); the reader
takes directory names from configuration and the drift check accepts both.

## Info server

`--serve-info` serves `http://localhost:3001/info`. The reader requests L4
snapshots with `fileSnapshot` (`l4Snapshots`, `includeUsers: false`) and an
`outPath` under `/snapshots/`; the node writes the file there and the reader
reads it from the same path (both containers mount `/srv/hl/snapshots` at
`/snapshots`). The server listens on loopback only, which is why the node
and the reader use host networking.

If `BlehRegistryRefreshFailing` fires, or the reader logs L4 snapshot errors
(`HL_SNAPSHOT_INTERVAL_SECS`, default 600 here, sets how often it asks):

```bash
curl -s -X POST -H 'Content-Type: application/json' \
  --data '{"type":"exchangeStatus"}' http://127.0.0.1:3001/info   # is the server up?
ls -la /srv/hl/snapshots                                          # permissions, free space
docker compose -f /opt/bleh/src/deploy/compose/compose.yaml logs --tail 200 hl-node
```

## Disk and pruning

Node output is data until captured. `deploy/scripts/hl-prune.sh`
(`bleh-prune.timer`, hourly) deletes an hourly stream file only when it is
older than `RETAIN_HOURS` (24) **and** at or before the capture watermark
`/srv/bleh/raw/.captured-through`, which the reader and `archive-capture`
advance once an hour's raw is in R2. Other directories are pruned by age:
`replica_cmds` and TWAP statuses after 24 h, all but the newest three ABCI
states, logs after 7 days, snapshots after 6 h. Settings:
`/etc/bleh/prune.env` (template `deploy/systemd/env/prune.example.conf`).

Until live raw capture and the watermark exist (pending reader and archive
work, deploy/README.md), stream files would be held forever; on such a host
set `CAPTURE_REQUIRED=0` in `/etc/bleh/prune.env` to prune by age alone,
knowing those hours are not archived from this node (backfill covers fills).

Emergency: below 5% free, the oldest stream hours are deleted regardless of
capture, `bleh_hl_prune_emergency_deleted_files` records how many, and those
hours become gaps. A full disk stops the node, which loses more.

Sizing: with pruning, the disk holds roughly one to two days of output
plus headroom. Watch the "Node data disk growth per day" panel during the
first week and record the measured rate in [costs.md](costs.md)
(assumption A3).

```bash
sudo DRY_RUN=1 /opt/bleh/src/deploy/scripts/hl-prune.sh   # what would go
cat /var/lib/node_exporter/textfile/bleh_hl_prune.prom    # last run
```

Alerts: `BlehNodeDiskLow` (< 20% free), `BlehNodeDiskCritical` (< 10%),
`BlehNodeDiskFillsIn24h`, `BlehPrunerFailing`, `BlehPrunerBlockedByCapture`
(files past retention held because capture is behind; see
[incident.md](incident.md#archive-capture-stalled)).

## Node not applying blocks

`BlehNodeBehindChain` (more than 10 s behind) or `BlehNodeStalled` (2 min):

1. `docker compose logs --tail 200 hl-node` (or `journalctl -u hl-visor`):
   look for peer errors, a crash loop in `visor_child_stderr`, or an
   upgrade in progress.
2. Gossip reachability: `ss -ltn '( sport = :4001 or sport = :4002 )'` and
   an outside check of TCP 4001; peers deprioritise unreachable nodes.
3. Peers: if the fetched roots are poor, pin better ones with
   `HL_ROOT_NODE_IPS` and restart the service.
4. Resources: `BlehMemoryPressure`, disk full, IO saturation on the
   hosts dashboard.
5. After an upgrade by `hl-visor`, check [upgrades.md](upgrades.md#schema-drift).

While the node is down the reader emits `gap` records; with a second
origin (Tier 2) the gateway continues from the other path.
