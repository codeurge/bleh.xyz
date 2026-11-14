# Open sources survey, November 2026

- Date: 2026-11-14. Every terms page cited was read on that date unless
  marked otherwise.
- Author: research lane, for the repository owner.
- Status: research only. Nothing here is admitted. Each source needs its own
  admission record and an owner decision under the
  [admission policy](../admission-policy.md).

This survey looks for legal, open sources that would make bleh.xyz a more
accurate market-data source than the free alternatives. It does not
repeat the candidate list in [sources.md](../sources.md), which it builds on.
It quotes no market data values. Volume and open-interest figures are left
out on purpose. Where a venue's size matters, it is described in words.

The companion
[strategy review](strategy-2026-11-futures-grade.md) maps sources to
exposures and proposes a method v2. The two documents agree on the
Ethereum gold pools and on the EIA exclusions. This survey adds the
non-US sources, the ranked queue and the draft admission records.

Contents:

1. [How extra sources raise accuracy](#how-extra-sources-raise-accuracy)
2. [Ranked candidates](#ranked-candidates)
3. [Recommended next-5 admission queue](#recommended-next-5-admission-queue)
4. [Per-source notes: on-chain venues](#per-source-notes-on-chain-venues)
5. [Per-source notes: US public sources](#per-source-notes-us-public-sources)
6. [Per-source notes: non-US public sources](#per-source-notes-non-us-public-sources)
7. [Rejections](#rejections)
8. [Policy questions for the owner](#policy-questions-for-the-owner)
9. [Anything that needs money](#anything-that-needs-money)
10. [Sources consulted](#sources-consulted)

## How extra sources raise accuracy

An extra source helps only where a method can use it. Four mechanisms
matter, and the ranking scores each candidate against them.

1. **Cross-venue composites for one exposure.** The spot composite in
   [synthetic-months@1](../specs/method-synthetic-months-v1.md) § 8 takes
   the median of fresh sources, and the number of sources changes what it
   can do:
   - **One source:** every value is flagged `spot_single_source` and its
     grade drops one step (§ 10).
   - **Two sources:** the grade becomes the second best of the two. If the
     two disagree, the value can only be flagged, not corrected.
   - **Three or more:** the median can drop an outlier (`spot_dispersed`).
     That is the tolerance to losing any one source that the admission
     policy asks of composites.

   Today every venue exposure has one source, Hyperliquid, and most
   non-crypto exposures have one deployer. The first goal is a second and
   third independent venue for BTC, ETH and gold, the exposures that open
   venues already trade.
2. **Basis against settlement anchors.** A venue perpetual tracks the
   front futures month, or the cash underlying, with a premium that the
   method does not correct (§ 15, "Spot proxies"). Some admitted public
   sources publish the value a futures family settles against: SOFR, EFFR,
   ERCOT hubs, USDA indexes. Against those, the gap between the value the
   venue implies and the published value can be measured daily and
   published as a basis series. Each new anchor (CPI for inflation
   products, a foreign overnight rate) adds a family whose model values can
   be checked against a public number.
3. **Integrity checks.** When two independent sources measure the same
   quantity, a silent error becomes an alarm, whether it is a stuck oracle
   band, a deployer outage, a reader bug or a decimal slip in a government
   file.
   - The project already compares two Hyperliquid nodes (ADM-0001,
     "Risks").
   - The same check across venues needs a second venue.
   - Across public sources it needs two publishers of one quantity. Two
     examples: Treasury par yields against H.15 constant-maturity yields,
     or the ECB's and the Federal Reserve Board's dollar rates for the same
     currency.
4. **Coverage of hours one venue lacks.** On-chain venues trade at weekends
   and on holidays, when futures and most public publications stop.
   Government series also go back before any venue existed: H.15 from 1954,
   the Treasury curve from 1990. A source that trades when Hyperliquid is
   thin, halted or band-capped is worth more than one that repeats
   Hyperliquid's busiest hours. (trade.xyz bounds its mark to the last
   external price while the reference market is closed.)

Three kinds of source add no accuracy, because each is the same price
formed somewhere else:

- an oracle-priced venue, whose fills echo the oracle;
- a token priced at its issuer's net asset value;
- a publication that re-reports an exchange settlement.

All three are rejected on rule 1 or rule 3 before accuracy comes into it.

## Ranked candidates

Column meanings:

- **Cost:** money the project would have to spend. Staff time is not
  counted.
- **Effort:** engineering size of the reader: S is days, M is one to three
  weeks, L is longer.
- **Accuracy:** which of the four mechanisms above the source serves.

| Rank | Candidate | Exposures added | Route (rule 2) | Cost | Expected verdict | Effort | Accuracy value |
|---|---|---|---|---|---|---|---|
| 1 | Ethereum mainnet AMM pools: Uniswap v3 and hookless Uniswap v4 ([ADM-0020](../admissions/ADM-0020-ethereum-uniswap-pools.md)) | ETH, BTC (wrapped), gold (PAXG, XAUT) | Own node | $0 if the home-archive server gets a 2 TB SSD; otherwise one drive, roughly $120–200 one-off (estimate) | Admit with exclusions | M | **High.** Second independent venue for three exposures; weekend hours; cross-venue integrity alarm |
| 2 | EIA energy statistics, EIA's own surveys only ([ADM-0021](../admissions/ADM-0021-eia-energy-statistics.md)) | Crude, products and natural gas stocks and storage; retail fuel prices | Public domain | $0 | Admit with exclusions | M | Medium. Fair-value context for energy months; retail fuel references |
| 3 | BLS consumer price indexes ([ADM-0022](../admissions/ADM-0022-bls-consumer-price-indexes.md)) | US inflation | Public domain | $0 | Admit | S | Medium to high. CPI-U drives TIPS indexation and US inflation products |
| 4 | Foreign overnight rates: ECB €STR, Bank of England SONIA (database copy, next day), Bank of Canada CORRA, RBA cash rate, Riksbank SWESTR | EUR, GBP, CAD, AUD, SEK short rates | Open terms (ADR 0012) | $0 | Admit, once the owner confirms that ADR 0012 covers non-US public bodies | S each | **High for FX and rates.** The foreign leg of FX-forward fair values; anchors for foreign short-rate futures |
| 5 | Eurostat HICP; ONS CPI and RPI | Euro and UK inflation | Open terms (Eurostat reuse notice; OGL v3) | $0 | Admit | S | Anchors for euro and UK inflation products |
| 6 | Fiscal Data "TIPS and CPI Data" (reference CPI, index ratios) | TIPS carry | Public domain; already-admitted channel | $0 | Amend ADM-0006 | S | Real-yield carry for Treasury months (method v2) |
| 7 | AEMO NEM prices; New Zealand Electricity Authority prices | Australian and New Zealand power | Open terms (AEMO permission; CC BY 4.0) | $0 | Admit (AEMO wording to confirm) | M | Settlement anchors for Australian and New Zealand electricity futures |
| 8 | Elexon imbalance prices (BMRS licence) | GB power | Open terms | $0 | Admit with exclusions | M | GB power cross-check |
| 9 | Government auction results: UK DMO gilts, Bank of Canada, Japan MOF | Gilts, Canadian government bonds, JGBs | OGL v3; Bank of Canada terms; Public Data License 1.0 | $0 | Admit | S–M | Fair-value inputs for bond futures families |
| 10 | Federal Reserve Board CP and G.17; BEA PCE; CFTC Weekly Swaps Report | Commercial paper, industrial production, PCE, swap volumes | Public domain | $0 | Admit | S | Low. Cross-checks and context |
| 11 | dYdX v4 | Crypto perpetuals, order-level | Own node | 64 GB RAM, 16 cores: may fit the home-archive server; otherwise about $70–130/month (estimate) | Needs owner decision (licence) | M | Medium. A third crypto venue with an order-level book |
| 12 | FERC Electric Quarterly Reports | US bilateral power trades at hubs | Regulatory filing publication | $0 (about 100 GB download) | Needs owner decision | L | Lagged cross-check for power hubs |
| 13 | Carbon allowance auctions (CARB with Québec, RGGI) | Carbon allowances | None today: needs a written open-licence grant | $0 | Ask the publishers | S | High if granted: original auction prices |
| 14 | Solana tokenized stocks (Raydium, Orca pools) | Single stocks, index ETFs | Own node; free Old Faithful history | Live node about $450/month (anecdotal estimate); history free but very large | Admit with exclusions, **needs money** | L | Medium. Equity exposures beyond HIP-3 |
| 15 | Polymarket | Event odds | Own Polygon node | About $150–250/month (estimate) | Needs owner decision (terms claim on-chain data) | S–M | Low for prices |
| 16 | Injective | Crypto and RWA perpetuals | Own node | 128 GB RAM box, about $120–200/month (estimate) | Needs owner decision, leaning reject (source code private since 2026-09-09) | M–L | Medium if cleared |
| 17 | Norges Bank NOWA, Danmarks Nationalbank DESTR, Banxico TIIE de fondeo, Japan MOF JGB curve, ECB FX reference rates, DMO historical gilt reference prices, Brazil BCB Selic | Various | Open or near-open terms | $0 | Needs owner decision, one by one | S | Cross-checks |

## Recommended next-5 admission queue

1. **ADM-0020, Ethereum mainnet Uniswap pools.** This is the only candidate
   that gives a second independent venue for existing exposures (ETH, BTC,
   gold) at no recurring cost. The node runs on the home-archive server.
2. **ADM-0021, EIA energy statistics.** Already on the roadmap for Phase 3.
   It is public domain, but needs a series denylist, because EIA mixes
   vendor and exchange series into the same tables and files.
3. **ADM-0022, BLS consumer price indexes.** Public domain and small. CPI
   is the anchor for US inflation products. Pair it with an amendment to
   ADM-0006 for Fiscal Data's TIPS index ratios.
4. **Foreign overnight rates (five records: ECB, Bank of England, Bank of
   Canada, RBA, Riksbank).** These give the foreign leg of currency
   fair values and anchors for foreign short-rate futures. The owner first
   needs to confirm that ADR 0012's "public body" includes non-US central
   banks (see [policy questions](#policy-questions-for-the-owner)).
5. **Eurostat HICP and ONS CPI and RPI (two records).** Inflation anchors
   for the euro area and the UK, under attribution terms that the
   project's notice mechanism already handles.

Items 1 to 3 have draft records with status `Requested`. Admission is the
owner's decision.

## Per-source notes: on-chain venues

The test for each venue:

- **Matching:** where it happens, and whether the fills are matched or
  oracle-priced.
- **Access:** whether the data can be read from our own node without a
  contract.
- **Licence:** whether the node software's licence lets us run it.

### Ethereum mainnet AMM pools (rank 1)

- **What:** Uniswap v3 pools, and Uniswap v4 pools without hooks. Each
  swap moves the pool's price along its own liquidity curve. No oracle is
  an input to the contract.
- **Exposures:** ETH against USDC and USDT, wrapped BTC (WBTC, cbBTC) and
  tokenized gold (PAXG, XAUT). The gold pools are thinner than the ETH
  pools. The deepest gold pool is a PAXG-to-XAUT ratio pool, so a dollar
  price comes from the PAXG/USDC and XAUT/USDT pools.
- **Fields:** the `Swap` event (amounts, post-swap price, liquidity, tick;
  v4 adds the fee) and liquidity events for depth. Addresses are excluded.
  Details are in ADM-0020.
- **Access:** an Erigon node (LGPL-3.0) the project runs.
  - **Disk:** Erigon's documentation gives 920 GB required and 2 TB
    recommended for a full node, and 1.77 TB required and 4 TB recommended
    for an archive node.
  - **Receipts:** Reth's `--full` profile keeps receipts only for the last
    10,064 blocks, so it cannot serve historical logs; Reth needs a custom
    prune setting or archive mode. The research found that Erigon keeps
    receipts by default from v3.7, but this is to verify at install.
  - **Pre-Merge history:** Uniswap v3 from 2021 to 2022 comes from era1
    files, which are mirrored by the Ethereum Foundation's devops team and
    by Nimbus and listed with checksums. They are raw chain history, which
    the node checks against the chain.
  - **Free RPC is not used.** PublicNode's terms forbid reselling the data
    "as a standalone product, such as an RPC or data API service, without
    our prior written consent".
- **Caveats:**
  - **Quote currency:** pools quote in stablecoins or WETH, not dollars.
  - **Issuer risk:** wrapped BTC and tokenized gold are claims on an
    issuer that can freeze addresses.
  - **Token premium:** gold tokens trade at a premium or discount to spot
    gold. A composite needs a premium term, which is a method question.
  - **Hooks:** v4 pools with hooks can replace the curve and are excluded.
  - **Finality:** blocks before finality can reorganize.
  - **Grading:** the liquidity grade needs a mapping for AMM pools before
    pools can be graded.
- **Verdict:** admit with exclusions. **Effort:** M.

### Solana tokenized stocks (rank 14)

- **What:** xStocks (Backed) and Ondo Global Markets tokens traded in
  Raydium and Orca pools. Raydium carries most of this volume.
- **Caveats:**
  - xStocks use Token-2022 scaled UI amounts for dividends and splits, so
    prices need adjusting.
  - Issuers hold permanent-delegate powers over the tokens.
  - "Prop AMMs", which update their quotes from off-chain prices, need a
    lineage review before admission.
- **Access:**
  - **Live:** a Solana RPC node needs about 256 GB RAM and 32 cores, which
    the home server cannot provide (about $450/month rented, an anecdotal
    figure).
  - **History:** Old Faithful (files.old-faithful.net, Triton) is free.
    Per-epoch CAR files measured 0.5–0.8 TB each, roughly 100 TB or more a
    year to stream, published 10–20 hours after an epoch ends.
  - **Licences:** Agave is Apache-2.0; yellowstone-faithful is AGPL-3.0.
    Old Faithful's data states no licence. It is raw chain data.
- **Verdict:** admit with exclusions, but **needs money** for live data.
  **Effort:** L.

### dYdX v4 (rank 11)

- **Matching:** each full node keeps the order book in memory, and the
  block proposer matches orders. Fills are final by consensus.
  - Liquidations are matched against the book, with an oracle-derived limit
    but the maker's price. They can be admitted, flagged.
  - Deleveraging, final settlement, oracle, funding and mark fields are
    excluded.
- **Exposures:** crypto only. Its gold-linked market is oracle-anchored.
- **Access:** own full node with full-node streaming, which gives an
  order-level book and fills over gRPC.
  - **Hardware:** 16 cores, 64 GiB RAM, 0.5 TiB SSD.
  - **Snapshots:** Polkachu publishes free pruned snapshots and short-lived
    archive snapshots.
- **Licence, the blocker:** `v4-chain/LICENSE` is AGPL-3.0 with a preamble.
  - The preamble: "Subject to your compliance with applicable law and the
    v4 Terms of Use ... you are granted the right to use the Program".
  - The v4 Terms (updated 2026-03-12) treat residents of the US, Canada and
    the UK as Restricted Persons for the perpetuals software.
  - The proto crates carry `LicenseRef-dYdX-Custom` with the same
    condition, so none of them may be vendored.
- **Verdict:** needs owner decision. **Effort:** M.

### Polymarket (rank 15)

- **Matching:** an off-chain order book run by the operator, with fills
  settled on Polygon (`OrderFilled` events). Exchange contracts changed with
  V2 on 2026-04-28. Polymarket Perps are fully off-chain and are rejected.
- **Access:** a Polygon node (Bor plus Heimdall, multi-terabyte NVMe;
  estimated $150–250/month rented). polygon-rpc.com is now Ankr, and its
  keyless access is deprecated.
- **Terms:** the Terms of Use (effective 2026-08-11) prohibit capital-market
  entities and market-data distributors from accessing "any data ...
  including on-chain, whether in raw, derived, aggregated, or anonymized
  form". Whether that binds someone who reads the chain without using the
  site is a legal question.
- **Verdict:** needs owner decision, leaning reject. **Effort:** S–M.

### Injective (rank 16)

- **Matching:** an on-chain frequent batch auction in the exchange module.
  Liquidations fill in the book and carry an `is_liquidation` flag.
  Settlement-price events, funding, mark and oracle values are excluded.
- **Exposures:** a large set of RWA perpetuals on equities, FX and
  commodities (secondary sources), all oracle-anchored.
- **Access:** 128 GB RAM, 12 fast cores and 2 TB NVMe. Neither current box
  can provide this.
- **Licence:** the last public source was BUSL-1.1, which changed to
  GPL-2.0-or-later on 2026-06-30. From 2026-09-09 the source is private, and
  current binaries ship with no licence stated.
- **Verdict:** needs owner decision, leaning reject. **Effort:** M–L.

### Decibel on Aptos

- **What:** a fully on-chain order book with gold, silver and stock
  perpetuals, marked from an oracle.
- **The blocker:** the aptos-core licence (the "Innovation-Enabling Source
  Code License") allows "non-production and non-commercial use ... solely
  for your internal purposes" until four years after each upload. It also
  excludes products built on other protocols.
- **Hardware:** 128 GB RAM and 3 TB NVMe.
- **Verdict:** reject for now. Revisit when the licence changes.

## Per-source notes: US public sources

### EIA (rank 2)

- **Terms:**
  - [Copyright and reuse](https://www.eia.gov/about/copyrights_reuse.php):
    "U.S. government publications are in the public domain and are not
    subject to copyright protection. You may use and/or distribute any of
    our data, files, databases, reports, graphs, charts, and other
    information products". EIA asks for an acknowledgement that includes
    the publication date.
  - Third-party material "may be protected by U.S. and foreign copyright
    laws" and needs "the written permission of the copyright owners".
  - The [API terms](https://www.eia.gov/opendata/terms-of-service.php)
    ask users to name EIA, not imply endorsement, and not modify content
    "and still claim the source is the EIA". They contain no ban on
    redistribution or commercial use. Bulk files need no key.
- **Exclusions:** EIA republishes vendor and exchange prices beside its own
  surveys:
  - spot petroleum and Henry Hub spot (Refinitiv/LSEG);
  - NYMEX futures;
  - the NGL composite (Bloomberg);
  - wholesale heating oil and propane (OPIS);
  - wholesale electricity and gas hub data (ICE);
  - the wholesale market portal, which republishes grid operators already
    rejected.
- **Metadata is not a safe filter.** The research found exchange and
  vendor series labelled `"copyright":"None"` in the bulk files. The reader
  needs a denylist by table and series family.
- **Verdict:** admit with exclusions. **Effort:** M. Details are in
  ADM-0021.

### BLS (rank 3)

- **Terms:**
  - [Copyright](https://www.bls.gov/opub/copyright-information.htm): BLS
    material "is in the public domain, except for previously copyrighted
    photographs and illustrations", and "You are free to use our public
    domain material without specific permission". BLS asks to be cited.
  - [API terms](https://www.bls.gov/developers/termsOfService.htm): "Data
    accessed through BLS.gov do not, and should not, include controls over
    its end use." Users cite the retrieval date and the disclaimer "BLS.gov
    cannot vouch for the data or analyses derived from these data after the
    data have been retrieved from BLS.gov."
- **Scope:** CPI only, in this record. Some PPI farm and energy indexes
  start from "prices ... quoted on organized commodity exchanges" or
  purchased datasets. Import fuel indexes use spot prices from an unnamed
  source. Both need their own review.
- **Verdict:** admit. **Effort:** S. Details are in ADM-0022.

### BEA, Federal Reserve Board, CFTC (rank 10)

- **BEA:** [FAQ 147](https://www.bea.gov/help/faq/147) says the information
  "is in the public domain and may be used or reproduced without specific
  permission". The API terms require the notice "This product uses the
  Bureau of Economic Analysis (BEA) Data API but is not endorsed or
  certified by BEA."
- **Federal Reserve Board:**
  [disclaimer](https://www.federalreserve.gov/disclaimer.htm): "Unless
  otherwise indicated, information on Board's website is in the public
  domain and may be copied and distributed without permission."
  - The CP rates are the Board's own fitted statistic from DTCC-supplied
    data.
  - G.17 and H.8 are compiled by the Board.
- **CFTC Weekly Swaps Report:** aggregate swap volumes. Its lineage is the
  same as the CFTC sources already admitted.

### FERC Electric Quarterly Reports (rank 12)

- **What:** seller-reported wholesale power transactions under 18 CFR
  35.10b: price, quantity, product, delivery point and standard hub names.
  - Filed about four months after each quarter.
  - Detailed format from 2013Q3; quarterly zips on
    eqrreportviewer.ferc.gov.
- **Terms:** no FERC website notice could be read (ferc.gov returned 403,
  data.ferc.gov 504). The data are private filings, so 17 U.S.C. § 105 does
  not apply directly.
- **Exclusions if admitted:**
  - `type_of_rate` RTO/ISO rows (prices from rejected grid operators);
  - exchange-executed rows (`exchange_brokerage_service` ICE, NODAL or
    NYMEX).
- **Flag if admitted:** index-priced rows as `exchange_referenced`.
- **Verdict:** needs owner decision. **Effort:** L.

### Carbon allowance auctions (rank 13)

- **Why they matter:** auction clearing prices are original price
  formation, and allowance futures trade on exchanges. The publishers'
  terms do not allow redistribution:
- **CARB:** its [conditions of use](https://ww2.arb.ca.gov/privacy-conditions-use)
  say "we ask you not to lift information from this site for placement on
  another website". There is no grant, and Québec co-owns the auctions.
- **RGGI Inc.:** its terms are an as-is disclaimer that says nothing on
  copying. Silence is not permission.
- **Washington Ecology:** "Commercial and political use of any Ecology
  Material are specifically prohibited." Rejected.
- **Action:** ask RGGI Inc. and CARB for an open-licence statement.

### USDA beyond current admissions

- **Cotton spot quotations:** rejected. They are quoted as points off the
  ICE cotton contract ("Settlement prices provided by ICE Futures US").
  - The quality premiums and discounts, and the American Pima quotations,
    are AMS's own and need owner review.
- **Daily National Shell Egg Index:** negotiated spot trades. Admissible,
  but low value with no futures family.
- **Boxed beef cutout:** check whether ADM-0010's field list already covers
  it.

## Per-source notes: non-US public sources

All of these would come in under the open-terms route of
[ADR 0012](../decisions/0012-open-terms-route-for-public-bodies-and-market-operators.md),
with the source's notice carried with each record. Several publishers
compute their own rates but take price inputs from vendors, so the main
work is field-level exclusion, not the licence.

### Overnight rates (rank 4)

- **ECB €STR:**
  - The [€STR page](https://www.ecb.europa.eu/stats/financial_markets_and_interest_rates/euro_short-term_rate/html/index.en.html)
    says "The ECB does not charge for the €STR or license its use."
  - The [ECB copyright notice](https://www.ecb.europa.eu/services/disclaimer/html/index.en.html)
    allows free use if the information "appear[s] accurately and the ECB
    [is] cited as the source". Sellers must tell buyers it is available
    free, and modifications must be stated.
  - The [ESCB reuse policy](https://www.ecb.europa.eu/stats/ecb_statistics/governance_and_quality_framework/html/usage_policy.en.html)
    allows reuse "irrespective of any subsequent commercial or
    non-commercial use".
- **Bank of England SONIA:**
  - The [database](https://www.bankofengland.co.uk/legal) is under OGL
    v3.0, with a required attribution statement for SONIA.
  - Redistributing the same-day print to clients needs a direct licence
    ([SONIA key features](https://www.bankofengland.co.uk/markets/sonia-benchmark/sonia-key-features-and-policies)).
  - The database copy is free from 10:00 on the next working day, with
    history to 1997.
  - **Admit the next-day database copy only.**
  - Exclude the Bank's yield curves ("Sources: Bloomberg Finance L.P.,
    Tradeweb and Bank calculations") and the FX series it holds under
    third-party licence.
- **Bank of Canada CORRA:**
  - The [terms](https://www.bankofcanada.ca/terms/) "permit you to freely
    use, copy, distribute and transmit its website content" with
    attribution and a free-availability notice for paid services.
  - "Use and distribution of CORRA ... do not require a licence agreement."
  - Exclude Bank of Canada FX (sourced from LSEG) and bond yields (CanDeal).
- **RBA cash rate:**
  - The [RBA's material](https://www.rba.gov.au/copyright/) is CC BY 4.0,
    apart from excluded material.
  - The cash rate "may be used, reproduced, published ... for personal or
    commercial use" without implying endorsement.
- **Riksbank SWESTR:** "provided free of charge without any licensing
  expenses being demanded for use or re-publication", with the Riksbank
  named as administrator
  ([conditions](https://www.riksbank.se/en-gb/statistics/swestr/conditions-for-use-and-re-publishing/)).

### Inflation (rank 5)

- **Eurostat:** "Reuse ... for commercial or non-commercial purposes is
  authorised provided the source is acknowledged"
  ([copyright notice](https://ec.europa.eu/eurostat/help/copyright-notice)).
  Data for countries outside the EU, EFTA and candidate countries must be
  removed before commercial reuse.
- **ONS:** OGL v3.0
  ([terms](https://www.ons.gov.uk/help/termsandconditions)).

### Power market operators (ranks 7–8)

- **AEMO:** the [copyright permissions](https://www.aemo.com.au/privacy-and-legal-notices/copyright-permissions)
  give "general permission for anyone to use AEMO Material for any purpose,
  but only with accurate and appropriate attribution".
  - Read from a 2026-10-31 archive snapshot, because the live page refused
    automated fetches. Re-check it in a browser.
  - The grant says "use", not "redistribute". The owner should confirm that
    "any purpose" covers redistribution.
  - Data: 5-minute regional reference prices on NEMWEB, from 1998.
- **New Zealand Electricity Authority:** CC BY 4.0 with a stated
  attribution
  ([legal information](https://www.emi.ea.govt.nz/LegalInformation/)).
- **Elexon:** the [BMRS licence](https://www.elexon.co.uk/bsc/operations-settlement/bsc-central-services/balancing-mechanism-reporting-agent/copyright-licence-bmrs-data/)
  allows anyone to "copy, publish, distribute and transmit ... exploit the
  BMRS Data, including commercially".
  - The attribution must pass into every sub-licence.
  - Exclude the Market Index Price, which comes from power-exchange data,
    and every period where the imbalance price falls back to it.

### Auction results (rank 9)

- **UK DMO:** gilt auction results under OGL
  ([terms](https://dmo.gov.uk/terms-of-use)). The site blocks automated
  clients with a CAPTCHA.
  - Reference prices after 2017 are FTSE-Tradeweb's and are rejected.
- **Bank of Canada:** auction results under the Bank's terms.
- **Japan MOF:** JGB auction results under the Public Data License 1.0
  (commercial use allowed; compatible with CC BY 4.0).
  - The MOF yield curve is computed from JSDA reference prices, and before
    December 1998 from exchange closes. The curve needs an owner decision,
    and the pre-1998 history is excluded.

## Rejections

| Source | Reason (deciding clause, in short) |
|---|---|
| Lighter | No node: blobs carry state diffs only, with no orders or trades; matching by a centralised sequencer |
| Derive | Off-chain matcher; V3 moved off Derive Chain; API terms §10.3 forbid making "Derive Data available to any third party" |
| GMX v2, Gains gTrade, Jupiter Perps, Avantis, Ostium | Oracle-priced fills, or flow routed to outside market makers |
| Drift | Hybrid book plus oracle-pegged AMM backstop; relaunch after the 2026-04-01 exploit unconfirmed |
| Paradex | Trades private (posted encrypted to a privacy council) |
| Aevo, Ondo Perps | Off-chain matching (Ondo Perps in an enclave); nothing to read from a node |
| Vertex | Shut down 2025-08-14 |
| Decibel (Aptos) | Node licence allows only non-commercial internal use |
| MSRB EMMA | "use Content or Services to develop or create a database to be ... made available (either commercially or free of charge)" is prohibited |
| FINRA TRACE | Website content "ONLY for your own non-commercial personal or professional use"; redistribution feed is paid |
| FRED | Commercial use needs the St. Louis Fed's written consent, and many series are owned by third parties; pull from originating agencies instead |
| FedInvest price file (leaning) | TreasuryDirect terms: "non-commercial personal use only"; prices from a dealer survey |
| Washington cap-and-invest auctions | "Commercial and political use of any Ecology Material are specifically prohibited" |
| USDA cotton spot quotations | Points off the ICE contract: futures plus basis |
| FERC Form 552 | Volumes only, no prices |
| Bank of Japan, including TONA | Commercial copying needs advance permission |
| SNB data portal; SARON | SNB data "for non-commercial purposes"; SARON is administered by SIX, an exchange group |
| AESO | "non-commercial, personal or educational purposes" only |
| IESO (leaning) | Licence is limited and non-sublicensable; other uses need written permission |
| ENTSO-E day-ahead prices; JEPX | Power-exchange auction results |
| EU and UK post-trade transparency (APAs, bond tapes) | Delayed data free only for internal use or free website display; other uses need the APA's approval; no free history; operators are vendors (for example the Bloomberg APA Europe policy of 2026-08-23) |
| Bank of England yield curves; Bank of Canada FX and bond yields; FTSE-Tradeweb gilt prices | Vendor inputs (Bloomberg, Tradeweb, LSEG, CanDeal) |
| Brazil CDI | Published by B3, an exchange |
| EIA vendor and exchange series | Field-level exclusions inside ADM-0021 |

## Policy questions for the owner

1. **Non-US public bodies under ADR 0012.** The open-terms route names "a
   public body (a government agency or a Federal Reserve Bank)". The
   research assumes that this covers foreign central banks, statistics
   offices and government agencies. If the owner agrees, a one-line
   amendment ADR makes that explicit before the first non-US record is
   decided (AGENTS.md rule 4).
2. **Attribution licences inside a CC0 dataset.**
   - CC BY 4.0, OGL v3.0, the BMRS licence and PDL 1.0 all fit ADR 0012's
     acceptable conditions, as long as each record carries its notice, the
     licence name and link, and a label when values are modified.
   - The records keep the publisher's licence. They do not become CC0, as
     [terms.md](../terms.md) § 3 already says.
   - Open point: ADR 0012 covers records admitted under the route. It
     should also say that **derived** records (composites, synthetic
     months) carry the notices of their inputs.
3. **Share-alike.** Brazil's open-data portal licenses Selic data under
   ODbL. Share-alike reaches derived databases, so composites built on ODbL
   inputs would inherit it. Keep ODbL inputs out of composites, or reject
   them.
4. **Contact details for bulk downloads.** BLS asks bulk clients to send
   an identifying User-Agent with a contact address. Use a project role
   address, never a personal one.
5. **Terms that claim on-chain data** (Polymarket) and **node licences
   conditioned on venue terms** (dYdX) are legal questions, not lineage
   questions.
6. **Tokenized claims and stablecoin quotes.** AMM prices are in USDC,
   USDT or WETH, for assets that are issuer claims (WBTC, cbBTC, PAXG,
   XAUT).
   - Treating a stablecoin as one dollar is a method assumption. Changing
     it is a method version (AGENTS.md rule 5).
   - A record flag for issuer-claim assets would be a data-model change
     (rule 4).

## Anything that needs money

| Item | Cost | Needed for |
|---|---|---|
| 2 TB SSD or NVMe for the home-archive server | Roughly $120–200 one-off (estimate, not a quote); $0 if a drive is free | ADM-0020 Ethereum node |
| dYdX v4 full node | $0 if the home-archive server has 64 GB RAM and 16 cores free; otherwise about $70–130/month (estimate) | Rank 11 |
| Solana RPC node | About $450/month (anecdotal estimate) | Rank 14, live |
| Old Faithful Solana history | $0, but roughly 100 TB or more a year of transfer | Rank 14, history |
| Polygon node | About $150–250/month (estimate) | Rank 15 |
| Injective node | About $120–200/month (estimate) | Rank 16 |
| Aptos node (Decibel) | 128 GB RAM box | Rejected for now |

No recommended source needs a paid API, a requester-pays bucket or a data
fee.

## Sources consulted

All read 2026-11-14 unless noted.

On-chain venues:

- Erigon hardware requirements: https://erigon-documentation.pages.dev/getting-started/hw-requirements
- Reth pruning: https://reth.rs/run/storage/pruning
- Ethereum history endpoints (era1): https://eth-clients.github.io/history-endpoints/
- Uniswap v3 pool events: https://github.com/Uniswap/v3-core/blob/main/contracts/interfaces/pool/IUniswapV3PoolEvents.sol
- Uniswap v4 pool manager: https://github.com/Uniswap/v4-core/blob/main/src/interfaces/IPoolManager.sol
- PublicNode terms: https://www.publicnode.com/terms
- dYdX order book: https://docs.dydx.xyz/concepts/trading/limit-orderbook
- dYdX full-node streaming: https://docs.dydx.xyz/nodes/full-node-streaming
- dYdX hardware: https://docs.dydx.xyz/nodes/running-node/hardware-requirement
- dYdX v4 terms: https://dydx.exchange/v4-terms
- Polkachu dYdX snapshots: https://polkachu.com/tendermint_snapshots/dydx
- Injective exchange module: https://docs.injective.network/developers-native/injective/exchange/00_derivative_market_concepts.md
- Injective node requirements: https://docs.injective.network/infra/join-a-network
- Injective core releases: https://github.com/InjectiveFoundation/injective-core/releases
- Lighter architecture (secondary): https://ethbeat.org/scaling/projects/lighter
- Lighter terms: https://lighter.xyz/terms
- Derive terms: https://www.derive.xyz/terms-of-use
- Derive V3 (secondary): https://ethbeat.org/scaling/projects/derive
- Polymarket V2 migration: https://docs.polymarket.com/v2-migration.md
- Polymarket terms: https://polymarket.com/tos
- Polygon snapshots: https://docs.polygon.technology/pos/how-to/snapshots.md
- Raydium tokenized-stock share (secondary): https://solanacompass.com/news/raydium-routes-90-of-solanas-tokenized-stock-dex-volume-with-5b-processed-as-september-sets-onchain-ath
- Old Faithful: https://docs.old-faithful.net
- GMX v2 pricing: https://gmxio.substack.com/p/gmx-v2-powered-by-chainlink-data
- Gains gTrade: https://docs.gains.trade/
- Drift recovery plan: https://www.drift.trade/updates/recovery-plan-for-affected-users
- Paradex privacy: https://docs.paradex.trade/trading/privacy
- Aevo (secondary): https://l2beat.com/scaling/projects/aevo
- Vertex closure (secondary): https://www.theblock.co/post/361570/
- Avantis (now Veranta): https://docs.veranta.xyz/
- Decibel on Aptos: https://aptosnetwork.com/currents/the-road-to-mainnet-decibel-and-the-future-of-perpetuals-on-aptos

US public sources:

- EIA copyright and reuse: https://www.eia.gov/about/copyrights_reuse.php
- EIA API terms: https://www.eia.gov/opendata/terms-of-service.php
- EIA wholesale markets (ICE notice): https://www.eia.gov/electricity/wholesale/
- BLS copyright: https://www.bls.gov/opub/copyright-information.htm
- BLS API terms: https://www.bls.gov/developers/termsOfService.htm
- BEA FAQ 147: https://www.bea.gov/help/faq/147
- BEA API terms: https://apps.bea.gov/api/_pdf/bea_api_tos.pdf
- Federal Reserve Board disclaimer: https://www.federalreserve.gov/disclaimer.htm
- Board CP release: https://www.federalreserve.gov/releases/cp/about.htm
- FRED legal: https://fred.stlouisfed.org/legal/
- MSRB EMMA terms: https://emma.msrb.org/tradedata
- FINRA terms: https://www.finra.org/terms-of-use
- FINRA TRACE pricing: https://www.finra.org/filing-reporting/trace/pricing
- TreasuryDirect terms: https://www.treasurydirect.gov/legal-information/terms/
- FERC EQR viewer: https://eqrreportviewer.ferc.gov/
- CARB conditions of use: https://ww2.arb.ca.gov/privacy-conditions-use
- RGGI website terms: https://www.rggi.org/sites/default/files/Uploads/RGGI-Inc-Documents/Website_Terms_Of_Use.pdf
- Washington Ecology copyright: https://ecology.wa.gov/About-us/Accountability-transparency/Website-information/Copyright-information

Non-US public sources:

- ECB €STR: https://www.ecb.europa.eu/stats/financial_markets_and_interest_rates/euro_short-term_rate/html/index.en.html
- ECB copyright: https://www.ecb.europa.eu/services/disclaimer/html/index.en.html
- ESCB reuse policy: https://www.ecb.europa.eu/stats/ecb_statistics/governance_and_quality_framework/html/usage_policy.en.html
- Bank of England legal: https://www.bankofengland.co.uk/legal
- SONIA key features: https://www.bankofengland.co.uk/markets/sonia-benchmark/sonia-key-features-and-policies
- Bank of England yield curves: https://www.bankofengland.co.uk/statistics/yield-curves
- UK DMO terms: https://dmo.gov.uk/terms-of-use
- Bank of Canada terms: https://www.bankofcanada.ca/terms/
- CORRA methodology: https://www.bankofcanada.ca/rates/interest-rates/corra/methodology-calculating-corra/
- RBA copyright: https://www.rba.gov.au/copyright/
- AEMO copyright permissions (archive snapshot of 2026-10-31): https://www.aemo.com.au/privacy-and-legal-notices/copyright-permissions
- Bank of Japan copyright: https://www.boj.or.jp/en/about/copyright.htm
- Japan MOF notice: https://www.mof.go.jp/english/about_mof/notice/index.html
- Public Data License 1.0: https://www.digital.go.jp/resources/open_data/public_data_license_v1.0
- MOF JGB curve Q&A: https://www.mof.go.jp/english/policy/jgbs/reference/interest_rate/qa.htm
- SNB copyright: https://www.snb.ch/en/srv/disclaimer_copyright
- SIX reference rates: https://six-group.com/en/market-data/indices/swiss-reference-rates/reference-rates-other-terms.html
- Riksbank SWESTR conditions: https://www.riksbank.se/en-gb/statistics/swestr/conditions-for-use-and-re-publishing/
- Norges Bank NOWA: https://www.norges-bank.no/en/topics/liquidity-and-markets/nowa/principles-for-calculating-and-publishing-nowa/
- Danmarks Nationalbank copyright: https://www.nationalbanken.dk/en/legal-information/copyright-and-use-of-logo
- Elexon BMRS licence: https://www.elexon.co.uk/bsc/operations-settlement/bsc-central-services/balancing-mechanism-reporting-agent/copyright-licence-bmrs-data/
- IESO terms: https://www.ieso.ca/en/Terms-of-Use
- AESO legal: https://www.aeso.ca/legal/
- UK MiFIR Article 13: https://www.legislation.gov.uk/eur/2014/600/article/13
- ESMA transparency Q&A: https://www.esma.europa.eu/sites/default/files/library/esma70-872942901-35_qas_transparency_issues.pdf
- Bloomberg APA Europe market data policy: https://assets.bbhub.io/professional/sites/27/APAE-market-data-policy_23-August-2026.pdf
- Eurostat copyright: https://ec.europa.eu/eurostat/help/copyright-notice
- ONS terms: https://www.ons.gov.uk/help/termsandconditions
- New Zealand Electricity Authority legal: https://www.emi.ea.govt.nz/LegalInformation/
- Banxico terms: https://www.banxico.org.mx/footer-es/salvedad-politicas-uso-banco-.html
- CC BY 4.0 legal code: https://creativecommons.org/licenses/by/4.0/legalcode.en
- Open Government Licence v3.0: https://www.nationalarchives.gov.uk/doc/open-government-licence/version/3/
