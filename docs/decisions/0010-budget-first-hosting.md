# 0010: Budget-first hosting — one rented node box plus a self-hosted archive server

- Status: Accepted
- Date: 2026-11-12
- Deciders: Repository owner

## Context

bleh.xyz is self-funded open source. The fully built-out plan in ADR 0006
(two origins in Tokyo, active-active) would cost roughly $1,500–3,500 a
month, which is not sustainable before there is usage or sponsorship. The
code is the same at any scale; only where it runs changes.

The latency target is measured from the node's local write to the gateway
send, so the node's location changes lag behind block time (reported, not
gated), not the target itself.

## Decision

Launch on the cheapest setup that keeps every capability:

- **One rented node box** runs the Hyperliquid non-validating node with the
  reader, book builder, signer and gateway: 16+ modern cores, 128 GB RAM
  free for the node, NVMe, from a budget provider whose acceptable-use
  policy allows blockchain nodes. Europe is acceptable; Tokyo comes later.
- **A self-hosted home server** is the archive, seed and processing node:
  full archive mirror on its disk array, always-on BitTorrent seeder and IPFS
  pinning node, backfill and replay/partition/manifest jobs, swap pollers and
  a staging history API. It publishes through a Cloudflare Tunnel with no
  inbound ports.
- **Free or cheap relays** (for example a free-tier ARM VM) serve users; the
  per-block signatures make them untrusted-safe.
- **Cloudflare R2** holds the public archive (zero egress) behind the CDN.

Target: about $150 a month all-in. Active-active (a second origin) and Tokyo
are upgrades taken when usage or sponsorship pays for them.

## Consequences

- No node redundancy at launch: node outages appear as flagged gaps in the
  archive, never filled in. Backfill and the archive are unaffected.
- Lag behind block time is higher outside Tokyo and is published.
- ADR 0006's architecture is unchanged; its two-provider deployment becomes
  a later tier. `docs/operations/hosting.md` and `costs.md` carry the tiers,
  providers and prices.
- Free and sponsored programs (open-data hosting, open-source credits,
  ecosystem grants) are pursued to keep costs near zero.

## Alternatives considered

- Two Tokyo origins from day one: best latency and redundancy, at ten to
  twenty times the cost.
- Running the node on the home server: it lacks free memory for the node
  and its CPUs are too old to be sure of keeping up with the chain.
- No node, public venue API only: free, but trades and top of book only, and
  rate-limited.
