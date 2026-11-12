# Incidents

Runbooks for the alerts in `deploy/observability/prometheus/rules/` and for
events no alert can catch (key compromise, takedowns). Two rules apply
throughout:

- **Never fill a gap.** Missing data is published as `gap` records and in
  the coverage map. Backfill from an admitted source, flagged `backfill` or
  `replayed`, is the only way to add history later.
- **Say what happened.** Post on the status page when users are affected,
  and note announced upgrades there before they start.

Command shorthand: `dc` is `docker compose --project-directory
/opt/bleh/src/deploy/compose -f /opt/bleh/src/deploy/compose/compose.yaml
--env-file /opt/bleh/src/deploy/compose/.env --profile <role>`, or simply
`/opt/bleh/src/deploy/up.sh <role> <command>`.

## Both ingest paths down

`BlehAllIngestPathsDown`. In Tier 1 there is one path, so this is any
reader outage.

1. Is the node up? `up.sh node-box logs --tail 100 hl-node`; if not, follow
   [node.md](node.md#node-not-applying-blocks).
2. Is the reader up? `up.sh node-box ps`; `logs hl-node-reader`. A crash
   loop right after an `hl-node` upgrade is usually schema drift
   ([upgrades.md](upgrades.md#schema-drift)).
3. Restart only what failed: `up.sh node-box restart hl-node-reader`.
4. Post on the status page; the gap is recorded automatically.

## One ingest path down

`BlehIngestPathDown`, `BlehPathBehind`, `BlehGatewayUpstreamLost` (Tier 2+).
Users are unaffected while the other path is healthy. Fix within the day as
for "both paths down", one host only. Do not restart the healthy path.

## Node divergence

`BlehIntegrityMismatch`: the two paths disagree on a `(block,
event_index)` (Tier 2+), or `BlehContentHashMismatch`: a replay did not
reproduce a partition's content hash.

1. Do not delete anything. Record the stream, block range and both values
   from the gateway logs (`up.sh node-box logs gateway | grep mismatch`).
2. Check both nodes' versions (`bleh_hl_node_binary_info`): a mismatch
   during an upgrade window usually means one node runs a different
   `hl-node` version with different output. Wait for both to converge and
   check whether it recurs.
3. Compare against a third source: Hyperliquid's public
   `node_fills_by_block` for that hour (requester-pays, ADM-0003), or a
   fresh L4 snapshot for book streams.
4. If one path was wrong: mark the affected blocks with a correction
   (new partition version and manifest entry; old hashes stay listed) and
   find the cause (disk corruption, reader bug, node bug).
5. Content-hash mismatches on replay are method bugs: stop publishing new
   partitions from the changed code and open an issue for the archive lane.

## Gaps and resnapshots

`BlehSeqGaps`, `BlehResnapshotRateHigh`, `BlehBookReconcileMismatch`. Some
gaps are normal (node restarts); a sustained rate is not.

- Gaps on one path only: that node or reader is dropping blocks (disk
  stalls, inotify limits: check `fs.inotify.max_user_watches`, set by
  host-bootstrap).
- Re-snapshots without gaps: the book builder disagrees with the node's L4
  snapshot. Capture one mismatch (instrument, block, both books' top levels)
  for the book lane; affected books are re-synced automatically.

## Gateway down

`BlehGatewayDown`, `BlehAllGatewaysDown`, `BlehGatewayStalled`.

1. `up.sh node-box ps` and `logs gateway`. If the process is up but stalled,
   check its upstream (the reader stream on 7400) and NATS.
2. `up.sh node-box restart gateway`. Relays reconnect by themselves and
   clients resynchronise from snapshots.
3. If relays are down instead: `ssh relay-N` and `deploy/up.sh relay ps`;
   a lost relay can be re-created with Terraform in minutes
   (`terraform apply -replace=...` in its stack) and its IP updated in the
   edge and fallback stacks.

## Public endpoint unavailable

`BlehProbeFailing`, `BlehAvailabilityFastBurn`, `BlehHttpEndpointDown`.
Find the broken layer from the inside out:

1. Origin health over the overlay (`blackbox-origin` job, or `curl
   http://10.77.0.1:8080/health` from the home server).
2. Overlay: `wg show bleh0` on a relay (latest handshake within two
   minutes) and `curl http://10.77.0.1:8080/health` from the relay.
3. Relays: `https://relay-1.bleh.xyz/health` directly (bypasses the CDN).
4. CDN: `https://live.bleh.xyz/health`. If only this fails, check the
   Cloudflare status page and the zone's rules (a rate-limit rule that is
   too strict shows as 429s).
5. If Cloudflare itself is the problem, see "Domain or CDN takedown"
   below: the direct relay names and the fallback domain still work.

## Archive capture stalled

`BlehRawUploadStale`, `BlehArchiveUploadFailures`,
`BlehPrunerBlockedByCapture`. Data is still on the node box; the pruner
keeps uncaptured hours until disk runs low.

1. `up.sh node-box logs archive-capture`: credentials (R2 token expired?),
   network, a malformed file.
2. Check the watermark: `cat /srv/bleh/raw/.captured-through` against the
   current hour.
3. Fix and restart `archive-capture`; it resumes from the watermark.
4. If the disk is about to fill before capture can catch up, copy the
   oldest uncaptured hours elsewhere (`rclone copy` to the private bucket or
   to the home server over the overlay) before the pruner's emergency mode
   deletes them.

## Manifest or anchor failure

`BlehManifestOverdue`, `BlehManifestFailures`, `BlehAnchorOverdue`,
`BlehAnchorFailures`.

- Manifest: `up.sh home-archive logs archive-process`. Signing failures
  point to the manifest key (HSM unplugged, file missing:
  [keys.md](keys.md#signing-failures)). Manifests are hash-chained, so the
  next one must not be built until the missing day is done; the archive
  builds days in order and catches up.
- OpenTimestamps: proofs complete only after Bitcoin confirmation; an
  overdue alert after 36 hours means submission failed (calendar servers
  unreachable). Retry; any calendar works.
- L2 anchor: usually the anchor wallet ran out of gas or the RPC endpoint
  failed. Top up the wallet (small amounts only) or switch
  `ARCHIVE_L2_RPC_URL`.

## Swap pollers stalled

`BlehSwapPollStale`. Part 43 repositories keep only a year online, so hours
of outage are recoverable but weeks are not. Check `up.sh home-archive logs
swap-pollers` for HTTP errors (format change, blocking, rate limits); a
format change needs the swap-pollers lane and possibly the admission record
(ADM-0002, ADM-0004).

## Key compromise

Any suspicion that a signing key, tunnel token, service token or API token
was exposed:

1. Contain: stop the signer or revoke the token at its source (Cloudflare
   dashboard for tunnel and service tokens, R2 for API tokens).
2. For signing keys, publish the revocation with `compromised_from` and a
   replacement key ([keys.md](keys.md#revoke-compromise)). Relays and
   clients drop the old address once their signer list updates.
3. Rotate every other secret on the same host; rebuild the host from
   scratch if the host itself was compromised (deploy.md, "Re-originate").
4. Review what the key signed in the window; issue corrections where
   published data could be affected.
5. Write up what happened and how, and post it.

## Domain or CDN takedown

The data stays reachable because nothing depends on one name:

| What was lost | Still working | Action |
|---|---|---|
| Cloudflare proxy or account | Relays by IP; fallback domain (Route 53) pointing at relays; archive mirrors, torrents, IPFS | Point `bleh.xyz` DNS (if still controlled) at relays unproxied; announce the fallback names |
| `bleh.xyz` (registrar or registry action) | Fallback domain; on-chain registry entries; ENS/IPNS names (Phase 2); torrents and IPFS by hash; mirrors on Hugging Face, Internet Archive, Academic Torrents | Announce on every channel; SDKs rotate to registry-listed gateways |
| R2 bucket | Home-archive mirror (full copy), torrents, IPFS pins, public mirrors | Re-host from the mirror on another S3-compatible store (B2, S3); update `data` DNS |
| A relay provider | Other relays; one Terraform apply on another provider | Re-create relays (`envs/relays-hcloud`), update edge and fallback stacks |

Steps for a domain loss:

1. Confirm the fallback domain resolves and serves: `wss://live.<fallback>/v0/ws`.
2. Publish the switch: update the on-chain registry (gateways and relays
   listed by fallback name and IP), the signed `signers.json` on every
   mirror, the README, and announcements (Nostr, social, the repository).
3. Clients verify block signatures and manifests, so serving from new names
   and untrusted mirrors is safe; nothing needs re-signing.
4. Register a new fallback domain at a third registrar once the first is in
   use.

## Hosting provider termination

A node box provider ending service (for example under an acceptable-use
policy) is a node-box loss: bring up a replacement elsewhere (hosting.md,
provider table; deploy.md, "Re-originate"), give it the overlay address, publish
the new path's signing key. The gap is recorded; fills can be backfilled.

## Home server down

The home-archive server carries processing, seeding, monitoring and
alerting, not the live path. Users are unaffected; the dead man's switch
reports the outage. When it returns, archive processing and the mirror catch
up from R2 and the watermark; manifests for missed days are built in order.
If it will be down for long, run the home-archive profile on any rented VM
with enough disk for scratch (the mirror can wait).
