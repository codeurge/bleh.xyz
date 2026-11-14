# Costs

Monthly cost model for each hosting tier in [hosting.md](hosting.md).
**Every figure is an estimate.** Provider prices were read on 2026-11-12
(sources in hosting.md); data volumes are assumptions until the first node
has run for a week, after which this page is updated with measured values.
Prices exclude VAT. The launch decision is
[ADR 0010](../decisions/0010-budget-first-hosting.md): one rented node box,
a self-hosted archive server, free relays and R2, targeting about $150 a
month.

## Headline

| Tier | Month 1 | Month 12 | Main driver |
|---|---|---|---|
| 0: archive-first | $5–40 | $20–60 | R2 storage of the backfilled archive |
| 0.5: self-hosted origin | $35–80 | $85–130 | Electricity, R2 growth |
| **1: lean live (default)** | **$230–300** | **$280–350** | The node box |
| 1, stretch node box | $145–195 | $195–245 | Unproven 8-core box |
| 2: two origins | $435–575 | $485–625 | Second node box |
| 3: Tokyo active-active | $1,700–1,850 | $1,750–1,900 | AWS instance and egress |

Month 12 adds eleven months of archive growth at the assumed rate. Tier 1
at the ~$150 target needs a node box at about $120 or less, which no
verified offer from a provider that permits nodes reaches yet; the stretch
option (OVHcloud Rise-2 with 128 GB, upgrade price unverified) is the
closest. Sponsorship of storage (below) is what keeps month 12 near
month 1.

## Assumptions

| ID | Assumption | Value | Status |
|---|---|---|---|
| A1 | Public archive growth (filtered raw zstd 19 + normalized partitions + manifests) | 10 GB/day | Estimate; measure from the first week of capture |
| A2 | Unfiltered raw (zstd long range, trained dictionaries) | 20 GB/day; kept on the home array, newest 30 days also in private R2 (Infrequent Access) | Estimate |
| A3 | Node output on the node box before pruning | 100–300 GB/day, pruned 24 h after capture | Official figure is about 100 GB/day at default settings; the mesh's flags add more |
| A4 | Backfill corpus copied from Hyperliquid's requester-pays buckets | 1–3 TB, one-off; trade history measured at about 392 GB | Trade history from the 2026-11-14 listing (hyperliquid-archive-buckets.md § 5); `market_data`, `replica_cmds` and `explorer_blocks` not sized yet |
| A5 | Relay egress to users | Within Oracle's 10 TB/month free allowance per account | Holds until roughly a few hundred continuous full-market subscribers |
| A6 | Home server power draw | 150–350 W (home-archive); 250–450 W with the node | Measure at the wall |
| A7 | Electricity | US $0.1816/kWh (EIA, August 2026); EU about €0.28/kWh (Eurostat via press, H1 2026) | Local tariffs vary widely |
| A8 | Exchange rate for EUR prices | €1 ≈ $1.10 | Approximate |

## Tier 1: lean live (default)

| Item | Choice | Monthly | Notes |
|---|---|---|---|
| Node box | Cherry Servers Ryzen 9 7950X, 16c/32t, 128 GB, 2×1 TB NVMe | $198–233 | Annual vs monthly billing; when out of stock, OVHcloud Advance-4 at about $260 + setup |
| Relays | 2 × Oracle Cloud Always Free Ampere A1 (1 OCPU, 6 GB each) | $0 | Paid fallback: 2 × Hetzner CAX11 ≈ €13 (≈ $14) |
| R2, public archive | A1: 300 GB at the end of month 1 | $4.50, then +$4.50 per month | $0.015/GB-month; egress free |
| R2, private raw buffer | A2: 600 GB Infrequent Access | $6 | $0.01/GB-month; retrieval $0.01/GB |
| R2 operations | Uploads (Class A) and cache misses (Class B) | $0–5 | First 1 M Class A and 10 M Class B free each month |
| Cloudflare | Free plan: DNS, proxy, websockets, one rate-limit rule, Tunnel, Access (up to 50 users) | $0 | Load Balancing ($5+) not needed with one origin |
| Domains | bleh.xyz and a fallback domain on another registrar | ≈ $3 | Annual renewals spread monthly |
| Fallback DNS (Route 53) | One hosted zone | $0.50 | `deploy/terraform/envs/fallback-dns` |
| Alerting dead man's switch | Free tier of a heartbeat service | $0 | Receives the Watchdog alert |
| Home-archive server | Existing hardware | electricity: $20–46 (US) or €30–71 (EU) | A6, A7 |
| **Total** | | **$230–300** (incl. electricity) | $210–255 without electricity |

One-off: backfill transfer from Hyperliquid's buckets. Requester-pays means
we pay AWS internet egress, about $0.09–0.114/GB, so A4 is roughly
**$90–350 once**. Measured so far: the trade history (about 392 GB from
`hl-mainnet-node-data` in Tokyo, $0.114/GB) is about **$45 once**, which fits
inside a new AWS account's Free-plan credits; `market_data` (us-east-1,
$0.09/GB) is pending. Running the copy on a spot instance in the bucket's own
region avoids transfer between AWS and the instance but not the egress to
R2. Under AWS Open Data sponsorship the archive could live in S3 in the same
region and the copy would cost almost nothing.

## Tier 0: archive-first

| Item | Monthly |
|---|---|
| R2: backfilled public archive (1–2 TB after conversion, A4) | $15–30 |
| Compute: home-archive server (electricity) or an occasional spot VM for backfill runs | $0–20 |
| Domains, DNS | ≈ $3.50 |
| **Total** | **$5–40**, plus the one-off backfill transfer |

## Tier 0.5: self-hosted origin

Tier 1 without the node box; the home server runs both roles.

| Item | Monthly |
|---|---|
| Electricity, 250–450 W | $33–60 (US) |
| R2 (as Tier 1) | $10–15 |
| Relays, Cloudflare | $0 |
| Domains, DNS | ≈ $3.50 |
| **Total** | **$35–80**; month 1 is at the low end before R2 fills |

## Tier 2: two origins

Tier 1 plus a second node box at a different provider and region, so the
two ingest paths fail independently and their agreement is the integrity
check.

| Item | Monthly |
|---|---|
| Tier 1 | $230–300 |
| Second node box (OVHcloud Advance-4, or the same Cherry plan elsewhere) | $200–265 |
| Cloudflare Load Balancing with health checks | $5–10 |
| **Total** | **$435–575** |

## Tier 3: Tokyo active-active (upgrade path)

ADR 0006 as written. Relays, R2 and the home-archive server carry over.

| Item | Monthly |
|---|---|
| AWS i4i.4xlarge, Tokyo, on demand ($1.610/h) | $1,175 |
| AWS Elastic IP ($0.005/h) and 100 GB gp3 root volume | ≈ $14 |
| AWS egress: reader stream to the other origin, raw uploads to R2, relay upstreams (1–2 TB at $0.114/GB) | $115–230 |
| Vultr vbm-8c-132gb, Tokyo (8c/16t, 128 GB, 1.92 TB NVMe) | $350 |
| Relays, R2, Cloudflare Load Balancing, domains, home electricity | $50–80 |
| **Total** | **$1,700–1,850** |

A one-year commitment on the AWS instance lowers the largest line (savings
plan rates unverified). Vultr's 24-core Tokyo plan comes with only a 480 GB
disk, too small for node output.

## Free and sponsored

Eligibility and amounts are **to verify**; apply in this order.

| Program | What it could cover | Fit | Source |
|---|---|---|---|
| AWS Open Data Sponsorship Program | Storage and egress for accepted public datasets on the Registry of Open Data | Strong: CC0 data, Parquet, verifiable. Would remove R2 growth and the backfill transfer | [program](https://aws.amazon.com/opendata/open-data-sponsorship-program/) |
| Cloudflare Project Alexandria | Plan upgrades (Pro or Business) for open-source projects | Good: Apache-2.0 code, public-good mission | [announcement](https://blog.cloudflare.com/expanding-our-support-for-oss-projects-with-project-alexandria/) |
| Hugging Face datasets | Free hosting of public datasets; storage grants case by case | Good mirror for partitions | [limits](https://huggingface.co/docs/hub/storage-limits) |
| Internet Archive | Free long-term storage | Mirror for manifests and daily files (per-item size limits) | [uploading](https://help.archive.org/help/uploading-tips/) |
| Academic Torrents | Free torrent indexing and seeding by the community | Daily torrents | [site](https://academictorrents.com) |
| Filecoin ProPGF and Filecoin Foundation grants | Funding for public-goods data storage | Possible: IPFS and Filecoin distribution | [ProPGF](https://filecoin.io/blog/Announcing-Filecoin-ProPGF-Batch-3-General-Track), [grants](https://fil.org/grants) |
| Hyperliquid ecosystem grants | None open found; the 2026 Hyper Foundation fund was a closed program | Watch for a general program | [report](https://crypto.news/hyper-foundation-allocates-10m-in-grants-to-support-usdh-migration/) **(secondary)** |
| GitHub Sponsors | Recurring donations; no fee from personal sponsors, up to 6% from organisations | Direct | [docs](https://docs.github.com/en/sponsors/getting-started-with-github-sponsors/about-github-sponsors) |
| Open Collective | Transparent budget; free or 5% platform fee, plus a fiscal host's fee (Open Source Collective: 10%) | Direct | [pricing](https://opencollective.com/pricing), [OSC fees](https://docs.oscollective.org/how-it-works/fees) |

### Federation

The cheapest capacity is someone else's. The relay is one compose profile
(`deploy/up.sh relay`) and needs no trust, because clients verify block
signatures; anyone can mirror the archive and verify it against the signed,
anchored manifests. Community relays, mirrors and seeders listed in the
registry move user egress and storage off the project's bill, and Phase 5's
independent operators add ingest paths at no cost to the project.

## Keeping costs down

- Relays take the fan-out; the origin's upload is one stream per relay.
- Unfiltered raw lives on the home array, with only 30 days in R2.
- Cache rules make immutable archive files cache for a year at the edge, so
  R2 Class B reads stay in the free tier.
- Prune node output 24 hours after capture (node.md); disk, not compute,
  sets the node box size.
- Revisit this page with measured A1–A3 after the first week, and when a
  sponsorship lands.
