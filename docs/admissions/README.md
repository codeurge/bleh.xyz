# Admission register

One record per admission decision, using [TEMPLATE.md](TEMPLATE.md). The
rules are in [admission-policy.md](../admission-policy.md), and the reason
admission is per field is in [ADR 0003](../decisions/0003-field-level-admission.md).

Statuses: `Requested`, `Under review`, `Admitted`, `Rejected`, `Revoked`.

Ingest code may merge only for fields covered by an `Admitted` record.
Every observation carries its admission ID.

| ID | Source | Fields | Status |
|---|---|---|---|
| [ADM-0001](ADM-0001-hyperliquid-trades-orders.md) | Hyperliquid node, every market (core, HIP-3, HIP-4): book-matched fills, order statuses, book diffs, L4 snapshots | Venue-matched fields only; owner addresses, oracle, mark, funding, liquidation and settlement fields excluded | Admitted |
| [ADM-0002](ADM-0002-cftc-part43-swaps.md) | CFTC Part 43 public swap dissemination | Price, size, time, product, option terms, flags | Admitted (Phases 1–2 lane) |
| [ADM-0003](ADM-0003-hyperliquid-archive-buckets.md) | Hyperliquid public archive buckets | Historical fills, L1 data, L2 snapshots (one file per coin per hour); `asset_ctxs` and `misc_events_by_block` excluded | Admitted |
| [ADM-0004](ADM-0004-sec-sbsr-public-dissemination.md) | SEC Regulation SBSR public dissemination | Price, size, time, product, flags | Admitted (Phases 1–2 lane) |
| [ADM-0005](ADM-0005-cftc-commitments-of-traders.md) | CFTC Commitments of Traders: legacy, disaggregated, TFF, supplemental; full history | Every published field; combined reports flagged `exchange_referenced` (exchange-supplied option deltas) | Admitted |
| [ADM-0006](ADM-0006-us-treasury-rates-and-auctions.md) | US Treasury daily par, real, bill and long-term rates; auction results (Fiscal Data) | All series fields; treasurydirect.gov files not used | Admitted |
| [ADM-0007](ADM-0007-nyfed-reference-rates.md) | NY Fed SOFR, BGCR, TGCR, EFFR, OBFR, SOFR averages and index | Rates, percentiles, volumes, revision markers; NY Fed notice carried (ADR 0012 route) | Admitted |
| [ADM-0008](ADM-0008-federal-reserve-board-h15.md) | Federal Reserve Board H.15 Selected Interest Rates | Board, Treasury and FR 2420 series; vendor series (Bloomberg/ICAP, ICE, Moody's, Bond Buyer, Freddie Mac) and undocumented discontinued series excluded | Admitted |
| [ADM-0009](ADM-0009-federal-reserve-board-h10.md) | Federal Reserve Board H.10 Foreign Exchange Rates | Certified noon buying rates; dollar indexes excluded | Admitted |
| [ADM-0010](ADM-0010-usda-livestock-mandatory-reporting.md) | USDA AMS Livestock Mandatory Reporting (DataMart) | Negotiated prices; swine formula prices flagged; volumes; other market formula, formula, forward, packer-owned and mixed prices excluded | Admitted |
| [ADM-0011](ADM-0011-usda-dairy-ndpsr-and-class-prices.md) | USDA AMS NDPSR and announced FMMO class and component prices | Prices flagged `exchange_referenced`; volumes; exchange quotations excluded | Admitted |
| [ADM-0012](ADM-0012-usda-market-news-feeder-cattle-and-grain.md) | USDA AMS Market News feeder cattle auctions, direct and video sales; grain reports | Feeder auction and cash prices; grain bids and basis (futures plus basis) rejected; MyMarketNews API not covered | Admitted (feeder only) |

Admission is a maintainer's engineering decision about field lineage. The
remaining candidates are listed in [docs/sources.md](../sources.md), with
their phases in [docs/roadmap.md](../roadmap.md).
