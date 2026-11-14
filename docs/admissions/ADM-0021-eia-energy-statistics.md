# ADM-0021: EIA — energy statistics from EIA's own surveys (petroleum and natural gas supply, storage, retail and survey prices)

- Status: Requested. Proposed by the research lane; admission is the owner's
  decision.
- Requested: 2026-11-14 by the research lane
  (open sources survey,
  `docs/research/open-sources-survey-2026-11.md`, rank 2)
- Reviewers: Repository owner (engineering review of field lineage)
- Decided:

## Source

The US Energy Information Administration (EIA) is the statistical agency of
the US Department of Energy. It runs mandatory and voluntary surveys of
energy companies and publishes the results. EIA also republishes some
prices that it takes from commercial vendors and from an exchange. This
record requests only series that come from EIA's own surveys, and it lists
the republished families as exclusions.

### Channels

- **Bulk files (primary).** `https://www.eia.gov/opendata/bulk/PET.zip` and
  `NG.zip`. They need no key and are refreshed twice a day.
- **Open Data API v2 (secondary, for checks).** `api.eia.gov/v2/`, with a
  free API key. The key's terms are reviewed under check 2.

**Cadence:**

- weekly petroleum supply (Wednesday) and natural gas storage (Thursday);
- weekly retail fuel prices (Monday);
- monthly series in the monthly publications.

**History** varies by series and goes back decades for the main ones.

## Fields requested

### Admitted families (EIA's own surveys)

| Family | Survey forms | Fields | Admit? |
|---|---|---|---|
| Weekly petroleum supply: crude oil (including the strategic reserve and the Cushing hub), motor gasoline, distillate, jet fuel and propane stocks; refinery inputs and utilisation; production; imports and exports | EIA-800 to EIA-805, EIA-814 (weekly) | Series ID, period, value, units, release time | Requested |
| Weekly natural gas storage by region | EIA-912 | Same | Requested |
| Weekly retail gasoline and on-highway diesel prices | EIA-878, EIA-888 | Same | Requested |
| Residential heating oil and propane prices | State Heating Oil and Propane Program, collected with State Energy Offices | Same | Requested, residential series only |
| Monthly petroleum and natural gas supply and disposition (production, stocks, movements) | Monthly surveys in the Petroleum Supply Monthly and Natural Gas Monthly | Same | Requested |
| Monthly refiner sales prices, crude first-purchase prices, imported crude costs, natural gas city-gate and end-use prices | Monthly price surveys | Same | Requested, flagged `exchange_referenced` (see check 3) |

### Excluded families (republished from a vendor or an exchange)

| Family | Credited source | Admit? |
|---|---|---|
| Petroleum spot prices (the `pet_pri_spt` table: crude, gasoline, diesel, jet fuel, propane) | Refinitiv / LSEG; bulk metadata says `"copyright":"Thomson-Reuters"` for many of them | **Excluded** |
| Henry Hub natural gas spot price | Refinitiv / LSEG ("Spot Price: Refinitiv, an LSEG business") | **Excluded** |
| Petroleum and natural gas futures prices (`pet_pri_fut`, `ng_pri_fut`) | New York Mercantile Exchange | **Excluded.** Exchange prices, including the history EIA still serves |
| Natural gas liquids composite price | Bloomberg | **Excluded** |
| Wholesale and resale heating oil and propane prices | OPIS terminal quotes | **Excluded** |
| Wholesale electricity and natural gas hub prices | "republished, with permission, from data collected by the Intercontinental Exchange (ICE)" | **Excluded** |
| Wholesale Electricity Market Portal | Grid operators' data, already rejected or under review (ADM-0014 to ADM-0019) | **Excluded** |
| Spot and futures price tables in the Weekly Petroleum Status Report | Same vendors and exchange as above | **Excluded** |
| Price narratives in Today in Energy and the Short-Term Energy Outlook | AAA, CME, SNL, Refinitiv credits | **Excluded** |
| Any series, in any table, that credits a third party | Third-party copyright | **Excluded** |

## Policy checks

Under [admission-policy.md](../admission-policy.md):

1. **Original price formation.**
   - **Admitted volumes:** these come from mandatory reports by refiners,
     terminals, storage operators and importers.
   - **Admitted prices:** these are EIA's own surveys of retail outlets
     (EIA-878, EIA-888), of residential heating fuel sellers (with State
     Energy Offices), and of refiners, first purchasers and utilities (the
     monthly price surveys).
   - **Excluded families:** each one re-publishes another source's prices,
     so it fails this test.
2. **No contract on the path.** Public-domain government data.
   - EIA's [copyright and reuse page](https://www.eia.gov/about/copyrights_reuse.php)
     says: "U.S. government publications are in the public domain and are
     not subject to copyright protection. You may use and/or distribute any
     of our data, files, databases, reports, graphs, charts, and other
     information products that are on our website or that you receive
     through our email distribution service." EIA asks for an
     acknowledgement that includes the publication date.
   - The same page warns that some material "contributed or licensed by
     private individuals, companies, or organizations ... may be protected
     by U.S. and foreign copyright laws", and that reuse "requires the
     written permission of the copyright owners". That is why the
     third-party families are excluded.
   - The [API terms](https://www.eia.gov/opendata/terms-of-service.php)
     permit using the API "to develop a service to search, display,
     analyze, retrieve, view and otherwise 'get' information from EIA
     data". Their conditions:
     - name EIA as the source;
     - "You may not use the EIA names ... to imply endorsement or approval
       of any product, service";
     - "You may not modify or falsely represent content accessed through
       the API and still claim the source is the EIA";
     - rate limits and termination at EIA's discretion.

     They contain no limit on recipients and no ban on commercial use or
     redistribution, so the key passes the test in rule 2. The bulk files
     need no key.
3. **Venue-matched fields only.** EIA's survey prices are its statistics,
   not exchange prices. But many refiner, first-purchase, import and
   city-gate sales are priced by contract formula against an exchange
   settlement or a vendor index. Those monthly price series therefore carry
   `exchange_referenced`, as
   [ADR 0012](../decisions/0012-open-terms-route-for-public-bodies-and-market-operators.md)
   decision 6 provides. Weekly retail prices and volumes do not.
4. **Field-level exclusions.** As listed in the table above.
   - **Do not trust EIA's metadata to filter.** The research found exchange
     and vendor series in the bulk files labelled `"copyright":"None"` and
     `"source":"EIA"`, including NYMEX futures series, OPIS wholesale series
     and some Refinitiv-sourced diesel spot series.
   - So the poller uses an **allowlist** of admitted table and series
     families. Any series not on the allowlist is dropped.

## Source basis

Read 2026-11-14:

- <https://www.eia.gov/about/copyrights_reuse.php>
- <https://www.eia.gov/opendata/terms-of-service.php>
- <https://www.eia.gov/electricity/wholesale/> (ICE republication notice)
- Natural gas price definitions, which credit Refinitiv, Bloomberg and
  NYMEX: <https://www.eia.gov/dnav/ng/TblDefs/ng_pri_fut_tbldef2.asp>
- Open Data page (API and bulk files): <https://www.eia.gov/opendata/>;
  bulk file checked: <https://www.eia.gov/opendata/bulk/PET.zip>

## Accuracy value

- **Inventories and storage:** stocks at the Cushing hub, product stocks
  and natural gas storage give carry and fair-value context for crude,
  refined-product and natural gas months.
  [synthetic-months@1](../specs/method-synthetic-months-v1.md) does not
  consume them, so using them needs a new method version.
- **Retail fuel prices:** widely used as contractual references.
- **Integrity:** cross-checks against energy swap prints from Part 43
  (ADM-0002).

## Risks and review triggers

- EIA adds a vendor credit to a series family on the allowlist, or moves
  republished prices into an admitted table.
- EIA changes its reuse page or API terms.
- The bulk-file layout changes. The poller rejects unknown shapes.

## Decision

Pending. Requested on 2026-11-14 for the admitted families above, read from
EIA bulk files (API v2 for checks), with every listed republished family
excluded. The repository owner decides.
