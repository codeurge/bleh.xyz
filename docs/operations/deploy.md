# Deploy

How to bring bleh.xyz up from nothing, anywhere, and how to change and roll
back a running deployment. What runs where and why:
[hosting.md](hosting.md). Files: [deploy/](../../deploy/README.md).

The whole pipeline is re-originated from three things: this repository at a
commit, the secrets in [keys.md](keys.md), and hosts that meet
[hosting.md](hosting.md). No step depends on a particular provider.

## One command per host

Every host runs the same repository and one compose profile:

```bash
sudo git clone https://github.com/codeurge/bleh.xyz /opt/bleh/src
sudo git -C /opt/bleh/src checkout --detach <sha>
sudo /opt/bleh/src/deploy/scripts/host-bootstrap.sh <role> --site <site> --overlay-ip <ip>
# install secrets and .env values (below), then:
sudo /opt/bleh/src/deploy/up.sh <role>
```

| Role | Site | Overlay IP | Secrets needed |
|---|---|---|---|
| `node-box` | `a` (second origin: `b`) | 10.77.0.1 (`b`: 10.77.0.3) | `signing-key.hex`; R2 write token in `.env` (the origin's capture uploads and registry-sync) |
| `home-archive` | `home` | 10.77.0.2 | `manifest-key.hex`, `anchor-key.hex`, `tunnel-token.txt`, `alertmanager/*`; R2 write token, backfill AWS key in `.env` |
| `relay` | `relay-1`, `relay-2`, ... | 10.77.0.11, .12, ... | `origin-ca/`; `BLEH_SIGNER_1` (and `_2`), `RELAY_UPSTREAM_1` (and `_2`), `HISTORY_API_ARCHIVE=https://data.bleh.xyz`, `RELAY_HOST`, `BLEH_DOMAIN`, `CADDY_TLS` (ACME email) in `.env` |
| `origin` | `home` | 10.77.0.2 | node-box and home-archive secrets together |

Without `--no-build`, `up.sh` builds images locally from the checkout; with
`BLEH_REGISTRY=ghcr.io/codeurge/bleh.xyz` and `BLEH_TAG=<sha>` in `.env` it
runs the signed images the deploy workflow published. Local laptop stack:
`deploy/up.sh synthetic` (no secrets needed).

## First deployment (Tier 1, ADR 0010)

1. **State bucket.** Create a private R2 bucket for Terraform state and a
   token for it; copy `deploy/terraform/backend.example.hcl` to
   `backend.hcl`.
2. **Relays.** `cd deploy/terraform/envs/relays-oci` (or `relays-hcloud`),
   copy `production.example.tfvars.hcl` to `production.tfvars.hcl`, set
   `repo_ref` to the commit, then:

   ```bash
   terraform init -backend-config=../../backend.hcl -backend-config="key=bleh/relays-oci.tfstate"
   terraform apply -var-file=production.tfvars.hcl
   terraform output -json relay_ips
   ```

   cloud-init clones the commit and runs `host-bootstrap.sh relay`.
3. **Edge.** In `envs/edge`, set `relays` from the output, `node_boxes =
   ["a"]`, your operator emails; `terraform init ... key=bleh/edge.tfstate`
   and `terraform apply`. This creates the R2 buckets, `data.bleh.xyz`,
   DNS, rate limits, the home tunnel with its Access policies and the
   Origin CA certificate (keep `node_boxes = []`: relays use the overlay).
4. **Node box.** Order the box (hosting.md, provider table), install Ubuntu
   24.04, then run the one-command steps with role `node-box`. Install a
   new block-signing key and publish its address (keys.md).
5. **Home-archive server.** Ubuntu 24.04 VM or host; mount the disk array
   at `/srv/archive` and NVMe scratch at `/srv/scratch`; role
   `home-archive`; install `home_tunnel_token`, the manifest key, anchor
   key and alert webhooks, and set `ARCHIVE_TRUSTED_SIGNER` (and the L2
   chain once chosen) in `/etc/bleh/archive.env` for the daily pipeline
   timers.
6. **Overlay.** Collect `/etc/wireguard/bleh0.pub` from every host into a
   peers file (kept outside the repository; format in
   `deploy/scripts/wg-mesh.sh`) and run `sudo deploy/scripts/wg-mesh.sh
   peers.conf` on each host. The home server has no endpoint and dials out.
   Check with `wg show`, `curl http://10.77.0.1:9184/metrics` (origin) and
   `curl http://10.77.0.1:8080/health` (its gateway) from home.
7. **Relays.** Write the Origin CA files to `secrets/origin-ca/`, and in
   `.env`: `BLEH_SIGNER_1` (the published block-signing address; never take
   it from the upstream), `RELAY_UPSTREAM_1=ws://10.77.0.1:8080/v0/ws`,
   `HISTORY_API_ARCHIVE=https://data.bleh.xyz`,
   `HISTORY_API_MCP_ALLOWED_HOSTS=api.bleh.xyz`, `RELAY_HOST`,
   `BLEH_DOMAIN=bleh.xyz`, `CADDY_TLS=<acme email>`, `CADDY_HTTP_PORT=80`,
   `CADDY_HTTPS_PORT=443`; then `deploy/up.sh relay`.
8. **Fallback DNS.** `envs/fallback-dns` with the same relay map, on a
   domain at a different registrar.
9. **Verify.** Prometheus targets all up; probe green
   (`bleh_probe_success`, from `mesh-client probe`); `wss://live.bleh.xyz/v0/ws`
   sends `hello` and per-block `block` messages; on the node box
   `/srv/bleh/raw/.captured-through` advances hourly and
   `https://data.bleh.xyz/` serves the first hour's files after capture;
   `https://data.bleh.xyz/_registry/hyperliquid/registry.json` exists (the
   archive replay needs it); Grafana via `grafana.bleh.xyz`.

Tier 0 is steps 1, 3 (with `relays = {}` and `node_boxes = []`) and 5.
Tier 0.5 runs role `origin` on the qualifying home server instead of steps
4 and 5.

## Second origin and Tokyo

- **Tier 2:** a second node box (role `node-box`, site `b`, overlay
  10.77.0.3) at a different provider. The origin's reader merges two nodes'
  data directories (`HL_NODE_DIRS`; node-reader.md), so the integrity check
  needs both nodes' output visible to one origin (for example the other
  node box's data directory mounted over the overlay); set
  `RELAY_UPSTREAM_2=ws://10.77.0.3:8080/v0/ws` on relays, uncomment site b in
  `deploy/observability/prometheus/prometheus.yml`, and consider Cloudflare
  Load Balancing.
- **Tier 3:** `deploy/terraform/envs/tokyo` provisions node box `a` on AWS
  (ap-northeast-1, i4i.4xlarge with local NVMe) and `b` on Vultr bare metal
  (nrt). cloud-init bootstraps both; then steps 4, 6 and the Tier 2 wiring.
  Set `r2_location = "apac"` for new buckets if the archive moves with them.

## Releases

1. Merge to main. The deploy workflow
   ([`.github/workflows/deploy.yml`](../../.github/workflows/deploy.yml))
   builds every image (`origin`, `hl-node`, `archive`, `gateway`,
   `history-api`, `swap-pollers`, `hl-backfill`, `tools`, `hl-node-reader`),
   pushes `ghcr.io/codeurge/bleh.xyz/<service>:<sha>` with SBOM and
   provenance, and signs it with cosign. A push deploys nothing. It builds
   only when the push changes an image input (Rust sources, `docs/specs/`,
   `Cargo.*`, the toolchain or `deploy/docker/`), and a newer push cancels
   a build in progress, so promote the latest SHA whose deploy run
   succeeded.
2. **Staging:** Actions → deploy → Run workflow on `main` with the SHA and
   `staging`. Staging can be the synthetic stack on a spare host or a
   second set of hosts; its GitHub environment lists their
   `DEPLOY_HOSTS`.
3. **Production:** the same with `production`. Configure both GitHub
   environments with required reviewers and deployment branches limited to
   `main`; the promote job also refuses to run from another branch and
   refuses a SHA that is not on main. It verifies every image's signature
   before moving any tag, moves the `production` tag and runs
   `update-host.sh` on each listed host in order (relays, then node boxes;
   upgrades.md has the order). Each job's token has only the permissions it
   needs (build: `packages: write`, `id-token: write`; promote:
   `packages: write`).
4. The home-archive server is behind NAT, so update it by hand:
   `sudo /opt/bleh/src/deploy/scripts/update-host.sh home-archive <sha>`.

## Rollback

- **Automatic:** `update-host.sh` restores the previous commit and image
  tag when the role is not healthy within five minutes (node box: the
  origin's `:9184/health` and its gateway's `:8080/health`).
- A promote to a SHA from before the `origin` image existed fails at the
  signature check (every image in the workflow's list must exist at the
  SHA); roll such a host back by hand with `update-host.sh`.
- **Manual:** run the promote workflow with the previous good SHA, or on a
  host `sudo deploy/scripts/update-host.sh <role> <previous-sha>`.
- **Infrastructure:** Terraform changes are reverted by applying the
  previous commit of `deploy/terraform`; relays are disposable
  (`terraform apply -replace=...` re-creates one from cloud-init).
- **Data:** nothing is rolled back in the archive. Bad partitions are
  corrected with new versions and new manifests; old files stay listed.

## Re-originate after losing a provider

Everything above works on any provider with Ubuntu 24.04. After losing a
host:

1. Bring up a replacement with the one-command steps (or the Terraform stack
   for its provider).
2. Restore its `.env` and `secrets/` from the encrypted backup (keys.md),
   or issue new keys and publish them.
3. Give the replacement the old overlay address (node box; `wg-mesh.sh` on
   every host), move the home tunnel token (home server), or update `relays`
   in the edge and fallback stacks (relays). Clients reconnect to the same
   names.
4. The archive is unaffected: it lives in R2, on the home array, in
   torrents, IPFS and mirrors, verifiable against the signed manifests.
   Missing live hours are gaps; backfill fills what Hyperliquid's public
   buckets hold, flagged `backfill`.

## OpenTofu

The stacks use no Terraform-only features; `tofu init` and `tofu apply`
work the same. OpenTofu's state encryption can additionally encrypt state
at rest with a key held outside the bucket.
