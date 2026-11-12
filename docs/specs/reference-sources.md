# Reference sources — access specification

Status: Draft v0, checked 2026-11-12. This document is the input for the
reference-pollers task (task 4 of the
[open reference prices plan](../../.agents/plans/open-reference-prices.md)).

It records, for every **Admitted** public settlement-anchor and
reference-data source:

- the documented endpoints;
- formats and field names;
- the publication schedule and the history available;
- rate limits and key requirements.

Pollers are written clean-room from this document and the official pages it
cites. They emit only the fields admitted in each record. The admission
records themselves decide what may be ingested. Where this document and an
admission record differ, the record wins.

Endpoints marked *observed* were seen working on 2026-11-12 but are not
described in official developer documentation. Pollers treat changes to
them as expected and alert on them.

## Common rules

- **Admission ID.** Every record carries its admission ID (`ADM-0005` to
  `ADM-0013`).
- **Required flags:**
  - `exchange_referenced` (ADR 0012, decision 6) goes on:
    - COT futures-and-options-combined records;
    - USDA swine formula prices;
    - every NDPSR and FMMO price.
  - `backfill` goes on history imports.
- **Notices.**
  - Records under [ADM-0007](../admissions/ADM-0007-nyfed-reference-rates.md),
    and EFFR records under
    [ADM-0008](../admissions/ADM-0008-federal-reserve-board-h15.md), carry
    the NY Fed reference-rates notice in the registry entry and the
    manifest.
  - USDA values presented as USDA data are never altered (the DataMart
    terms say so).
  - Source credit is given to the CFTC, the Board and USDA, as their
    policies request.
- **Etiquette.**
  - Poll after the documented release time, not continuously. Use
    conditional requests where the server supports them.
  - Identify the client with a generic user agent that names no person.
  - Back off exponentially on HTTP 429 or 5xx.
- **No samples in the repository.** Tests use synthetic fixtures under
  `fixtures/synthetic/` that mimic these layouts. Real downloads are never
  committed.
- **Revisions.** Several sources revise published values (NY Fed same-day
  revisions, NDPSR four-week revisions, ERCOT price corrections, LMR
  corrections). Pollers store each version with its retrieval time. A
  revision is a new record that references the one it replaces.

## CFTC Commitments of Traders

Admission: [ADM-0005](../admissions/ADM-0005-cftc-commitments-of-traders.md).
Licence basis: CFTC Web Policy, public domain.

**Schedule.** Data are as of Tuesday's close. They are released Friday at
about 3:30 p.m. ET, and holiday weeks shift the release
([Release Schedule](https://www.cftc.gov/MarketReports/CommitmentsofTraders/ReleaseSchedule/index.htm)).

### Channel A: cftc.gov static files (primary)

**Current week.** Comma-delimited text with no header row. Column order is
the same as the annual files.

| Report | URL |
|---|---|
| Legacy, futures only | `https://www.cftc.gov/dea/newcot/deafut.txt` |
| Legacy, futures and options combined | `https://www.cftc.gov/dea/newcot/deacom.txt` |
| Disaggregated, futures only | `https://www.cftc.gov/dea/newcot/f_disagg.txt` |
| Disaggregated, combined | `https://www.cftc.gov/dea/newcot/c_disagg.txt` |
| TFF, futures only | `https://www.cftc.gov/dea/newcot/FinFutWk.txt` |
| TFF, combined | `https://www.cftc.gov/dea/newcot/FinComWk.txt` |
| Supplemental (CIT) | `https://www.cftc.gov/dea/newcot/deacit.txt` |

**History.** Annual zip files, each holding one comma-delimited text file
with a header row. The list is on
[Historical Compressed](https://www.cftc.gov/MarketReports/CommitmentsofTraders/HistoricalCompressed/index.htm).
All files are under `https://www.cftc.gov/files/dea/history/`:

| Report | Files | Member file |
|---|---|---|
| Legacy, futures only | `deacot1986.zip` … `deacotYYYY.zip`; also `deacot1986_2016.zip` | `annual.txt` |
| Legacy, combined (from March 1995) | `deahistfo_1995_2016.zip`; yearly `deahistfo_1995.zip` … `deahistfo_2003.zip`, then `deahistfo2004.zip` … `deahistfoYYYY.zip` | — |
| Disaggregated, futures only | `fut_disagg_txt_hist_2006_2016.zip`, then `fut_disagg_txt_YYYY.zip` | `f_year.txt` |
| Disaggregated, combined | `com_disagg_txt_hist_2006_2016.zip`, then `com_disagg_txt_YYYY.zip` (yearly files from 2010) | — |
| TFF, futures only | `fin_fut_txt_2006_2016.zip`, then `fut_fin_txt_YYYY.zip` | `FinFutYY.txt` |
| TFF, combined | `fin_com_txt_2006_2016.zip`, then `com_fin_txt_YYYY.zip` | — |
| Supplemental | `dea_cit_txt_2006_2016.zip`, then `dea_cit_txt_YYYY.zip` | `annualci.txt` |

Header naming differs by family:

- Legacy headers use spaced names, for example `"Market and Exchange
  Names","As of Date in Form YYMMDD","Open Interest (All)"`.
- The other families use underscored names, for example
  `Market_and_Exchange_Names`, `Report_Date_as_YYYY-MM-DD`,
  `Open_Interest_All`, `Prod_Merc_Positions_Long_All`.

Pollers map by header name, never by position. The weekly files (which
have no header) take the column order of the same year's annual file.

Since 1998, grain positions are reported in contracts rather than bushels
(note on the Historical Compressed page).

### Channel B: COT Public Reporting Environment (secondary, for checks)

This is a Socrata (SODA) API at `https://publicreporting.cftc.gov/resource/{id}.json`
(also `.csv`), with SoQL parameters such as `$where`, `$select`, `$order`,
`$limit` and `$offset`.

| Dataset | ID |
|---|---|
| Legacy, futures only | `6dca-aqww` |
| Legacy, combined | `jun7-fc8e` |
| Disaggregated, futures only | `72hh-3qpy` |
| Disaggregated, combined | `kh3c-gbw2` |
| TFF, futures only | `gpe5-46if` |
| TFF, combined | `yw9f-hn96` |
| Supplemental (CIT) | `4zgm-a668` |

- **Column names.** These come from `https://publicreporting.cftc.gov/api/views/{id}.json`,
  in `columns[].fieldName`. Examples: `report_date_as_yyyy_mm_dd`,
  `market_and_exchange_names`, `cftc_contract_market_code`,
  `open_interest_all`, `noncomm_positions_long_all`, `comm_positions_long_all`,
  `m_money_positions_long_all`, `lev_money_positions_long`,
  `cit_positions_long_all`.
- **Coverage.** The legacy futures-only dataset covered 1986-01-15 to
  2026-11-03 on 2026-11-12.
- **No app token.** Tokens need a Socrata profile, whose terms were not
  reviewed. Unauthenticated requests share an IP-based throttling pool and
  receive HTTP 429 when throttled
  ([Socrata app tokens](https://dev.socrata.com/docs/app-tokens.html)).

## US Treasury

Admission: [ADM-0006](../admissions/ADM-0006-us-treasury-rates-and-auctions.md).
Licence basis: US Government works (17 U.S.C. § 105); Fiscal Data open
licence.

### Daily interest rate statistics (home.treasury.gov)

**Source.** The
[Developer Notice](https://home.treasury.gov/developer-notice-xml-changes)
documents the channels.

**Type values:**

- `daily_treasury_yield_curve`
- `daily_treasury_real_yield_curve`
- `daily_treasury_bill_rates`
- `daily_treasury_long_term_rate`
- `daily_treasury_real_long_term`

**XML (OData Atom) feeds:**

- One year: `https://home.treasury.gov/resource-center/data-chart-center/interest-rates/pages/xml?data={type}&field_tdr_date_value={YYYY}`.
- One month (*observed*): `...&field_tdr_date_value_month={YYYYMM}`.
- All history, paged: `...?data={type}&field_tdr_date_value=all&page={n}`.
  Increment `n` until a page holds no entries.
- Elements absent for a date mean no value; empty tags are not used.

**Daily single-file XML:**

- `https://home.treasury.gov/sites/default/files/interest-rates/yield.xml`
- `daily_treas_bill_rates.xml`
- `ltcompositeindex.xml`
- `real_yield.xml`
- `real_ltcompositeindex.xml`

All five are in the same directory.

**CSV for one year** (*observed*, the link behind the site's CSV
download):
`https://home.treasury.gov/resource-center/data-chart-center/interest-rates/daily-treasury-rates.csv/{YYYY}/all?type={type}&field_tdr_date_value={YYYY}&page&_format=csv`.

**Static archive CSVs** cover fixed ranges, for example
`yield-curve-rates-1990-2021.csv` and `bill-rates-2002-2021.csv`. They are
listed on the developer notice.

**Field names:**

| Series | XML elements | CSV headers |
|---|---|---|
| Par yield curve | `NEW_DATE`, `BC_1MONTH`, `BC_1_5MONTH`, `BC_2MONTH`, `BC_3MONTH`, `BC_4MONTH`, `BC_6MONTH`, `BC_1YEAR`, `BC_2YEAR`, `BC_3YEAR`, `BC_5YEAR`, `BC_7YEAR`, `BC_10YEAR`, `BC_20YEAR`, `BC_30YEAR`, `BC_30YEARDISPLAY` | `Date`, `1 Mo`, `1.5 Month`, `2 Mo`, `3 Mo`, `4 Mo`, `6 Mo`, `1 Yr`, `2 Yr`, `3 Yr`, `5 Yr`, `7 Yr`, `10 Yr`, `20 Yr`, `30 Yr` |
| Real par curve | `NEW_DATE`, `TC_5YEAR`, `TC_7YEAR`, `TC_10YEAR`, `TC_20YEAR`, `TC_30YEAR` | `Date`, `5 YR`, `7 YR`, `10 YR`, `20 YR`, `30 YR` |
| Bill rates | `INDEX_DATE`, `QUOTE_DATE`, and per tenor (4, 6, 8, 13, 17, 26, 52 weeks): `ROUND_B1_CLOSE_{n}WK_2` (bank discount), `ROUND_B1_YIELD_{n}WK_2` (coupon equivalent), `CUSIP_{n}WK`, `MATURITY_DATE_{n}WK`, plus `CS_{n}WK_CLOSE_AVG` and `CS_{n}WK_YIELD_AVG` | `Date`, `{n} WEEKS BANK DISCOUNT`, `{n} WEEKS COUPON EQUIVALENT` |
| Long-term rates | `QUOTE_DATE`, `RATE_TYPE`, `RATE`, `EXTRAPOLATION_FACTOR` | `Date`, `LT COMPOSITE (>10 Yrs)`, `TREASURY 20-Yr CMT` |
| Real long-term average | `QUOTE_DATE`, `RATE` | `Date`, `LT Real Average (10> Yrs)` |

New tenors have been added as new elements; the 1.5-month tenor was added
on 2025-02-18 and the 4-month tenor on 2022-10-19. Pollers accept unknown
`BC_*` and `TC_*` elements and alert.

**Schedule.** "Yield curve rates are usually available at Treasury's
interest rate website by 6:00 PM Eastern Time each trading day"
([methodology](https://home.treasury.gov/policy-issues/financing-the-government/interest-rate-statistics/treasury-yield-curve-methodology)).

**History:**

| Series | From |
|---|---|
| Par curve | 1990 |
| Bill rates | 2002 |
| Long-term rates | 2000 |
| Real curve | 2003 |
| Real long-term | 2000 |

**Method versions.** The monotone convex method applies from 2021-12-06.
Before that, Treasury used a quasi-cubic Hermite spline. Each record
carries the method version for its date.

**Limits and keys.** No key. No rate limit is published.

### Auction results (Fiscal Data API)

**Source.** The
[API documentation](https://fiscaldata.treasury.gov/api-documentation/)
describes the API.

**Base URL:** `https://api.fiscaldata.treasury.gov/services/api/fiscal_service`.

**Endpoints:**

- Treasury Securities Auctions Data: `/v1/accounting/od/auctions_query`.
  Coverage runs from 1979-11-15 ("Released As Needed").
- Upcoming auctions: `/v1/accounting/od/upcoming_auctions`.

**Parameters:**

- `fields=` (comma list);
- `filter=` (for example `record_date:gte:2026-01-01`);
- `sort=` (prefix `-` for descending);
- `format=json|csv|xml`;
- `page[number]=`, `page[size]=`.

The default page is 100 rows. Walk `links.next` until it is null, and read
`meta.total-count`. Use `curl -g` or URL-encode the brackets.

**Auction fields** come from `meta.labels`, which lists every field; check
it on each run. Examples:

- identity and dates: `record_date`, `cusip`, `security_type`,
  `security_term`, `auction_date`, `issue_date`, `maturity_date`;
- results: `high_yield`, `high_discnt_rate`, `high_investment_rate`,
  `high_price`, `low_*`, `avg_med_*`, `int_rate`, `bid_to_cover_ratio`;
- amounts: `offering_amt`, `total_tendered`, `total_accepted`,
  `comp_accepted`, `noncomp_accepted`;
- bidder classes: `primary_dealer_accepted`, `direct_bidder_accepted`,
  `indirect_bidder_accepted`;
- SOMA: `soma_accepted`;
- FRN and TIPS terms: `frn_index_determination_rate`,
  `ref_cpi_on_issue_date`, `index_ratio_on_issue_date`;
- file pointers: `pdf_filenm_*`, `xml_filenm_*`. These are names only; the
  files on treasurydirect.gov are not fetched (ADM-0006).

**Upcoming-auction fields:** `record_date`, `security_type`,
`security_term`, `reopening`, `cusip`, `offering_amt`, `announcemt_date`,
`auction_date`, `issue_date`.

**Limits and keys.** The documentation says "Our API is open, meaning that
it does not require a user account or registration for a token." No rate
limit is published.

## Federal Reserve Bank of New York reference rates

Admission: [ADM-0007](../admissions/ADM-0007-nyfed-reference-rates.md).
Licence basis: NY Fed Terms of Use, under ADR 0012. The notice is
mandatory.

**Source.** The
[Markets Data APIs](https://markets.newyorkfed.org/static/docs/markets-api.html)
publish an OpenAPI 3.0 spec at
`https://markets.newyorkfed.org/static/docs/markets-api.yml`.

**Base URL:** `https://markets.newyorkfed.org`.

**Endpoints:**

| Endpoint | Returns |
|---|---|
| `/api/rates/secured/{ratetype}/search.{format}?startDate=YYYY-MM-DD&endDate=YYYY-MM-DD&type=rate\|volume`, `ratetype` ∈ `all, tgcr, bgcr, sofr, sofrai` | Secured rates and SOFR averages and index over a range |
| `/api/rates/unsecured/{ratetype}/search.{format}?startDate=…&endDate=…`, `ratetype` ∈ `effr, obfr` | Unsecured rates over a range |
| `/api/rates/secured/{ratetype}/last/{n}.{format}`, `/api/rates/unsecured/{ratetype}/last/{n}.{format}` | Last *n* values |
| `/api/rates/all/latest.{format}` | Latest value of every rate |

`format` is one of `json`, `xml`, `csv` or `xlsx`.

**JSON layout.** The top-level key is `refRates`, an array.

- SOFR, BGCR, TGCR and OBFR records hold `effectiveDate`, `type`,
  `percentRate`, `percentPercentile1`, `percentPercentile25`,
  `percentPercentile75`, `percentPercentile99`, `volumeInBillions` and
  `revisionIndicator`.
- EFFR records add `targetRateFrom` and `targetRateTo`.
- SOFRAI records hold `effectiveDate`, `type`, `average30day`,
  `average90day`, `average180day`, `index` and `revisionIndicator`.

**Schedule (business days, NY Fed holiday calendar):**

- SOFR, BGCR and TGCR at about 8:00 a.m. ET.
- SOFR Averages and Index shortly after SOFR.
- EFFR and OBFR at about 9:00 a.m. ET.
- Same-day revisions at about 2:30 p.m. ET, only when the change exceeds
  one basis point. Pollers re-read after 2:30 p.m. ET.
- Summary statistics are revised quarterly.

Source: the
[additional information page](https://www.newyorkfed.org/markets/reference-rates/additional-information-about-reference-rates).

**History in the API (checked 2026-11-12):**

| Rate | From |
|---|---|
| SOFR, BGCR, TGCR | 2018-04-02 |
| SOFRAI | 2020-03-02 (SOFR Index base 1.00000000 on 2018-04-02) |
| EFFR | 2000-07-03 |
| OBFR | 2016-03-01 |

EFFR before 2000-07-03 comes from H.15 (ADM-0008). EFFR before 2016-03-01
is a volume-weighted mean of brokered trades, so pollers mark the method
change.

**Limits and keys.** No key. No numeric rate limit is documented. The terms
forbid access that interferes with the website.

**Notice text** (from the Terms of Use; completed per rate): "The [rate
name] is subject to the Terms of Use posted at newyorkfed.org. The New York
Fed is not responsible for publication of the [rate name] by bleh.xyz, does
not sanction or endorse any particular republication, and has no liability
for your use." SOFR and BGCR also carry the DTCC Solutions LLC sentence from
the terms.

## Federal Reserve Board H.15 and H.10

Admissions:
[ADM-0008](../admissions/ADM-0008-federal-reserve-board-h15.md) (H.15) and
[ADM-0009](../admissions/ADM-0009-federal-reserve-board-h10.md) (H.10).
Licence basis: Board website policies, public domain unless indicated.

**Files.** One whole-release SDMX-ML 1.0 zip per release, linked as "XML"
from each release page:

| Release | URL | Contents |
|---|---|---|
| H.15 | `https://www.federalreserve.gov/releases/h15/data/FRB_h15_xml.zip` | `H15_data.xml`, `H15_struct.xml`, schemas |
| H.10 | `https://www.federalreserve.gov/releases/h10/data/FRB_h10_xml.zip` | `H10_data.xml`, `H10_struct.xml`, schemas |

The DDP equivalent is `https://www.federalreserve.gov/datadownload/Output.aspx?rel={H15|H10}&filetype=zip`.
The DDP is being retired
([notice](https://www.federalreserve.gov/data/data-download-fred-information.htm)),
so pollers use the release-page files and treat the DDP URL as a fallback.

**XML layout:**

- Each series is a `kf:Series` element.
- Its attributes include `SERIES_NAME`, `FREQ`, `UNIT`, `UNIT_MULT` and a
  release-specific dimension: `INSTRUMENT` and `MATURITY` for H.15, `FX`
  and `CURRENCY` for H.10.
- `frb:Annotations` carry "Short Description" and "Long Description".
- `frb:Obs` elements carry `TIME_PERIOD`, `OBS_VALUE` and `OBS_STATUS`.
- `OBS_VALUE="ND"` means no data.

**Frequencies.** In `SERIES_NAME`, the suffix `.B` marks daily business-day
data (`FREQ="9"`). Weekly and monthly averages are separate series with
other suffixes. Pollers take the daily series and may recompute averages.

**Series admission** is by `SERIES_NAME` stem; see the tables in ADM-0008
and ADM-0009. Pollers keep an allowlist of admitted stems and drop every
other series. Unknown new series raise an alert and are not emitted until
reviewed.

**Schedule:**

- H.15 is published each business day, and the XML file is regenerated
  daily (Last-Modified on 2026-11-10 when checked).
- H.10 is published weekly on Mondays, covering the previous week (release
  dated 2026-11-09).

**History:**

- **H.15:** EFFR from 1954-07-01; 3-month bill from 1954-01-04; CMTs from
  1962; AA commercial paper from 1997-01-02; discount window primary credit
  from 2003-01-09.
- **H.10:** the oldest bilateral rates start 1971-01-04, and EUR starts
  1999-01-04.

**Limits and keys.** No key. No rate limit is published.

## USDA AMS Livestock Mandatory Reporting

Admission: [ADM-0010](../admissions/ADM-0010-usda-livestock-mandatory-reporting.md).
Licence basis: DataMart statement, "There are no restrictions on who may
obtain this data or how it may be used", with no alteration of data
presented as USDA's.

**Source.** The
[LMPR API User Guide v3.3](https://mpr.datamart.ams.usda.gov/LMPRS-API-User-Guide.pdf)
(November 2024).

**Base URL:** `https://mpr.datamart.ams.usda.gov/services/v1.1/reports`.

**Endpoints:**

| Endpoint | Returns |
|---|---|
| `GET /services/v1.1/reports` | Table of contents: `slug_id`, `slug_name`, `report_title`, `published_date`, `markets`, `market_types`, `offices`, `sectionNames` |
| `GET /services/v1.1/reports/{slug_id}?q={var}={value}&allSections=true` | All sections of one report |
| `GET /services/v1.1/reports/{slug_id}/{Section}?q=…` | One section, for example `/2466/Detail` |

**Query syntax.** `q=report_date=MM/DD/YYYY` selects one date, and a colon
gives a range (`q=report_date=11/02/2026:11/06/2026`). `published_date`,
`report_year` and `report_month` also work. Some summary reports use
`report_end_date` (guide § 1.5). `&sort={var}` orders results.

**Response.** Each section is an object with `reportSection`,
`reportSections`, `stats` and `results` (an array of rows). When nothing
matches, the response is the JSON string `"No Results Found."`.

**Limits:**

- Date ranges are capped at 180 days per request.
- Results are capped at 100,000 records per request.
- No key is needed ("The LMPR API does not require a user key").
- AMS "will ... temporarily block IP addresses found taxing the systems
  with high frequency requests".

**Corrections.** Rows carry `is_correction` (guide § 2.4).

**History.** The DataMart holds mandatory reports released after
2001-04-01 (DataMart home page).

**Schedule.** Reports publish on AMS's daily, weekly and monthly calendar.
Pollers watch each report's `published_date` in the table of contents
rather than hard-coding times.

**Key swine reports:**

- `slug_id` 2511 (LM_HG201, National Daily Direct Prior Day Slaughtered
  Swine). Sections: `Barrows/Gilts`, `Barrows/Gilts Negotiated`,
  `Carcass Measurements`, `14-Day Scheduled Swine`.
  - Row fields: `report_date`, `for_date_begin`, `purchase_type`,
    `head_count`, `base_price`, `avg_net_price`, `lowest_net_price`,
    `highest_net_price`, `avg_live_weight`, `avg_carcass_weight`,
    `avg_sort_loss`, `avg_backfat`, `avg_loin_depth`, `loineye_area`,
    `avg_lean_percent`.
  - Observed `purchase_type` values: `Prod. Sold Negotiated`,
    `Prod. Sold Negotiated Formula`, `Prod. Sold Swine or Pork Market Formula`,
    `Prod. Sold Other Market Formula`, `Prod. Sold Other Purchase Arrangement`,
    `Prod. Sold (All Purchase Types)`, `Pack. Sold (All Purchase Types)`,
    `Packer Owned`, `Prod. Sold Combined`.
  - Pollers filter on these values exactly, per ADM-0010.
- Other swine reports: 2510 (LM_HG200), 2512 and 2513 (LM_HG202, LM_HG203),
  2514–2522 (regional), 2524 (LM_HG214 weekly), 2674 and 2675 (HG216,
  HG217), 3458 (LM_HG218), sow and boar reports 2676–2679 and 3452.

**Key cattle reports:**

- `slug_id` 2466 (LM_CT100, 5 Area Daily Weighted Average Direct Slaughter
  Cattle, Negotiated). Sections: `Summary`, `Detail`, `History`.
  - Detail fields: `class_description`, `selling_basis_description`,
    `grade_description`, `head_count`, `weight_range_low`,
    `weight_range_high`, `weight_range_avg`, `price_range_low`,
    `price_range_high`, `weighted_avg_price`.
- Other negotiated cattle reports: 2660–2672, 2481, 2483–2488, 2477,
  2685–2690, 3928–3930.
- Reports whose sections must be filtered by purchase type: 2469–2471,
  2474–2476, 2478, 2691–2696.
- Formula, formulated base and forward reports are not polled for prices:
  2659, 2479, 2480, 3494–3507.

**Boxed beef and pork:**

- 2453 (LM_XB403, negotiated boxed beef cutout and cuts).
- 2498–2501 (LM_PK602, LM_PK603, LM_PK610, LM_PK611, negotiated pork).
- Formula, forward, export, specialty and comprehensive pork reports
  (2504–2509, 2680, 2681, 2702, 2703) are polled for volumes only.

## USDA AMS dairy

Admission: [ADM-0011](../admissions/ADM-0011-usda-dairy-ndpsr-and-class-prices.md).
Licence basis and API: the same DataMart API as the LMR section above.

| Report | `slug_id` | Query variable | Fields |
|---|---|---|---|
| National Dairy Products Sales Report | 2993 | `week_ending_date` | Sections `Butter Prices and Sales`, `40 Pound Block Cheddar Cheese Prices and Sales`, `500 Pound Barrel Cheddar Cheese Prices, Sales, and Moisture Content`, `Dry Whey Prices and Sales`, `Nonfat Dry Milk Prices and Sales`, and the matching `Final …` sections. Fields: `Butter_Price`, `Butter_Sales`, `cheese_40_Price`, `cheese_40_Sales`, `cheese_500_Price`, `cheese_500_Wtd_Price`, `cheese_500_Sales`, `cheese_500_Wtd_Mst`, `whey_Price`, `whey_Sales`, `nonfat_milk_Price`, `nonfat_milk_Sales` |
| Announcement of Class and Component Prices | 2991 | `report_year`, `report_month` | Section `Detail`: `class_2_Price`, `class_2_butterfat_Price`, `advanced_skim_milk_class_2_Price`, `class_3_Price`, `class_3_skim_milk_Price`, `class_4_Price`, `class_4_skim_milk_Price`, `butterfat_Price`, `nonfat_solids_Price`, `protein_Price`, `other_solids_Price`, `somatic_cell_adjustment_Rate`, `butter_monthly_avg_Price`, `nfdm_monthly_avg_Price`, `cheese_monthly_avg_Price`, `cheese_monthly_40p_Price`, `cheese_monthly_500p_Price`, `whey_monthly_avg_Price` |
| Announcement of Advanced Prices and Pricing Factors | 2989 | `report_year`, `report_month` | Section `Detail` (field list to be read from the response) |
| Class I prices; advanced and final class prices by order; uniform; mailbox | 3345, 3354, 3355, 3351, 3357 | `report_year`, `report_month` | As returned |

DataMart values are strings. The API guide notes that most data need
converting to numeric, so pollers parse to fixed-point decimals.

**Schedule:**

- NDPSR is weekly. Plants report "generally by noon on Tuesday", and the
  report is dated Wednesday.
- "The NDPSR is subject to revisions to the previous four weeks"
  (methodology note in the
  [NDPSR report](https://www.ams.usda.gov/mnreports/dywdairyproductssales.pdf)).
- Class and component prices are announced monthly under 7 CFR 1000.53.

**History.** The DataMart's dairy history starts with the Dairy Product
Mandatory Reporting Program data it holds. Pollers record the first date
returned.

**Flags.** Every price field carries `exchange_referenced`.

## USDA AMS Market News feeder cattle

Admission: [ADM-0012](../admissions/ADM-0012-usda-market-news-feeder-cattle-and-grain.md).
Licence basis: US Government works (17 U.S.C. § 105).

**Channel.** Static report files under `https://www.ams.usda.gov/mnreports/`,
as linked from:

- [feeder cattle summaries](https://www.ams.usda.gov/market-news/feeder-and-replacement-cattle-summary);
- [direct feeder reports](https://www.ams.usda.gov/market-news/direct-feeder-cattle-reports);
- [internet and video auctions](https://www.ams.usda.gov/market-news/feeder-cattle-internet-video-reports);
- [auctions](https://www.ams.usda.gov/market-news/feeder-and-replacement-cattle-auctions).

**Key files:**

| File | Report |
|---|---|
| `sj_ls850.txt` (`SJ_LS850.txt` redirects to it) | National Feeder and Stocker Cattle Summary, weekly, text |
| `LSWNFSS.pdf` | The same summary, PDF |
| `LSDDFSS.pdf`, `LSPDFSS.pdf` | National Daily Feeder and Stocker Cattle Summary, AM and PM |
| `AMS_2906.pdf`, `AMS_3096.pdf`, `AMS_2808.pdf`, `AMS_3097.pdf`, `AMS_2770.pdf`, `AMS_2708.pdf`, `AMS_3059.pdf`, `AMS_3098.pdf`, `AMS_3184.pdf`, `AMS_2709.pdf`, `AMS_2940.pdf`, `AMS_2710.pdf`, `AMS_3237.pdf` | State and regional direct feeder cattle reports, weekly (Friday) |
| `AMS_2713.pdf` and the other `AMS_*.pdf` video auction reports | Internet and video auctions |
| `AMS_1895.pdf` (Kansas), `AMS_1831.pdf` (Oklahoma), `AMS_1860.pdf` (Nebraska), `AMS_1955.pdf` (Texas), `AMS_1821.pdf` (Missouri) and the other state weekly cattle auction summaries | Auction summaries |

**Formats.** Fixed-layout text and PDF. Fields per sale line:

- class (steers, heifers);
- frame and muscle grade (for example "Medium and Large 1", "1-2");
- weight range and average weight;
- price range and weighted average price, in $/cwt;
- head count;
- delivery terms for video and direct lots.

Pollers parse the tables and drop narrative paragraphs, which quote futures
moves. They drop any line the report labels basis, futures or formula
priced.

**Freshness.** On 2026-11-12 `sj_ls850.txt` still carried a report dated
2026-04-06. Pollers must check each file's report date and alert on stale
files, preferring `LSWNFSS.pdf` when the text file lags.

**History.** These files hold only the latest issue. Earlier issues are in
the MyMarketNews report archive, which is not an admitted channel until its
terms are read (ADM-0012). History backfill for feeder cattle therefore
waits on that review.

**Limits and keys.** No key. No rate limit is published.
