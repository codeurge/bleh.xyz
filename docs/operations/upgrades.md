# Upgrades

Four things change under a running deployment: Hyperliquid's node software
(automatically), our services, the host OS and the base images. Node
operations: [node.md](node.md). Deploy and rollback mechanics:
[deploy.md](deploy.md).

## hl-visor upgrades

`hl-visor` downloads, verifies and runs new `hl-node` versions by itself,
with no notice and no way to pin a version. We cannot schedule these, so we
detect them:

- `deploy/scripts/schema-drift-check.sh` records the sha256 of the running
  `hl-node` every 30 minutes; a change sets
  `bleh_hl_node_binary_changed_timestamp_seconds` and the
  `BlehNodeBinaryChanged` info alert, and appears on the hosts dashboard.
- On a binary change, watch the drift alerts below for an hour and note the
  upgrade on the status page. Hyperliquid-announced upgrades (for example
  network upgrades that stop the chain) are excluded from the availability
  figure when announced in advance (requirements.md).
- With two origins (Tier 2+), upgrades usually reach both within minutes.
  If one node is stuck on an old version or fails to restart, the other
  path carries the stream; see [incident.md](incident.md#one-ingest-path-down).

## Schema drift

A new `hl-node` can add, rename or reshape fields. Two independent checks:

1. **Live, in the reader.** The reader rejects unknown fields or shapes at
   parse time, stops that record kind (never guesses), increments
   `bleh_reader_schema_unknown_total` and sets `bleh_reader_stream_halted`.
   Alert: `BlehSchemaDriftLive` (page).
2. **Scheduled, outside the reader.** `bleh-schema-drift.timer` runs
   `schema-drift-check.sh`, which samples the newest lines of each stream,
   reduces them to key paths and compares them with the key paths of the
   synthetic fixtures (generated from the format spec). Unknown paths or
   missing required paths set `bleh_schema_drift_ok 0`. Alerts:
   `BlehSchemaDriftCheckFailed` (page), `BlehSchemaDriftCheckStale`.

Run it by hand to see the details:

```bash
sudo /opt/bleh/src/deploy/scripts/schema-drift-check.sh
cat /var/lib/node_exporter/textfile/bleh_schema_drift.prom
```

When drift is confirmed:

1. Leave the halted kind halted. The gap is recorded; nothing is invented.
   Other kinds keep flowing.
2. Capture a few affected lines **without** user addresses for the
   integrator (field names and shapes only; never commit node data).
3. If the new field is one ADM-0001 already covers (for example a new
   excluded fee field), the integrator updates
   [hyperliquid-node-format.md](../specs/hyperliquid-node-format.md) and
   the fixtures, the node-reader lane updates the parser, and a release
   resumes the kind. If it is a new kind of data, it needs an admission
   change first ([admission-policy.md](../admission-policy.md)).
4. Fields that the spec already lists but the fixtures lack go in
   `deploy/scripts/schema-known-extra.conf` (with a note), never fields the
   spec does not cover.
5. Backfill the gap from Hyperliquid's public buckets where they have the
   data (fills), flagged `backfill`.

## Our services

Images are built and signed per commit on main; promotion is manual and
per environment (deploy.md). Order for a release that touches several
roles:

1. Relays first (they must accept both the old and new protocol version
   during the change; protocol changes follow
   [gateway-v0.md](../protocol/gateway-v0.md) § 10).
2. The home-archive server (archive processing is replayable).
3. Node boxes last, one origin at a time, waiting for `BlehPathBehind` to
   clear before the next.

`update-host.sh` rolls a host back automatically if it does not become
healthy; a manual rollback is promoting the previous SHA.

Archive and replay changes that alter partition bytes or content hashes
are method changes: they need a new version and a decision record
(AGENTS.md, rule 5), and old partitions stay listed.

## Host OS updates

- Unattended upgrades install security updates daily on every host
  (`/etc/apt/apt.conf.d/52bleh-unattended`).
- Relays reboot automatically at 04:30 UTC when needed (stateless, behind
  the proxy).
- Node boxes and the home-archive server never reboot automatically.
  `bleh_reboot_required` shows pending reboots; `BlehRebootRequired`
  reminds after 7 days. Reboot node boxes one at a time, when the other path
  is healthy (Tier 2+) or in a quiet hour (Tier 1), and announce it if the
  outage counts against availability.
- Docker uses `live-restore`, so a Docker package update does not stop
  containers.
- Kernel and NVMe firmware: follow the provider's notices; schedule like a
  reboot.

## Base images and pinned dependencies

Every image reference is pinned by digest (Dockerfiles and
`deploy/compose/compose.yaml`), and the Rust build uses a dated Debian
snapshot. To refresh:

1. Look up the new tag's digest (`docker buildx imagetools inspect
   <image>:<tag>`), update the reference and the snapshot date.
2. Build, run the synthetic stack (`deploy/up.sh synthetic`) and check the
   dashboards.
3. Release as usual. Reproducibility checks compare content hashes across
   the change, not image bytes.

The Hyperliquid release key fingerprint is pinned in two places
(`deploy/docker/hl-node/Dockerfile`, `deploy/scripts/node-install.sh`). If
Hyperliquid rotates its key, verify the new fingerprint out of band (the
node repository and an announcement channel) before changing both.
