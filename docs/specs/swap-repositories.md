# Swap repository public dissemination formats

- Status: Draft; implemented by `crates/swap-pollers`
- Research snapshot: 2026-11-12
- Admissions: [ADM-0002](../admissions/ADM-0002-cftc-part43-swaps.md) (CFTC
  Part 43), [ADM-0004](../admissions/ADM-0004-sec-sbsr-public-dissemination.md)
  (SEC Regulation SBSR)
- Output record: `swap_print` ([observation-v0](observation-v0.md) § 2.7)
- Operations: [swap-pollers runbook](../operations/swap-pollers.md)

This document describes the public interfaces the swap pollers read, and
how their records map to `SwapPrint`. Facts are marked **(verified)** when
they were read from a primary source or observed against the live
interface on the snapshot date, and **(unverified)** otherwise. No real
repository data is stored in this repository; tests use synthetic rows
that follow the layout below.

## 1. Repositories in scope

Both admissions are limited to repositories **not operated by an exchange
group**.

| Repository | Regimes | Operator | Status |
|---|---|---|---|
| DTCC Data Repository (U.S.) LLC ("DDR") | CFTC SDR (Part 43) and SEC SBSDR (SBSR), one public site | DTCC (not an exchange group) | **Implemented** (`repository = dtcc-ddr`) |
| ICE Trade Vault, LLC | CFTC SDR and SEC SBSDR | Intercontinental Exchange, Inc., an exchange group | **Excluded** by the admission decisions; not implemented |
| CME Swap Data Repository | CFTC SDR | CME Group, an exchange group | **Excluded**; also reported to have wound down its U.S. SDR **(unverified)** |
| DTCC Canadian buckets (`CA_*`) on the same site | Canadian provincial rules | DTCC | **Not admitted**; the pollers refuse to configure them |

The plan ([phase-1-live-data](../../.agents/plans/phase-1-live-data.md),
task 28) names ICE Trade Vault; the later admission decisions exclude it.
ICE Trade Vault publishes a public real-time ticker for CFTC and SEC
security-based swaps (SEC dissemination live since 2022-02-14, per its
regulatory guide); its formats were not researched further because it may
not be used.

## 2. DTCC public price dissemination (PPD)

One site serves the CFTC, SEC and Canadian jurisdictions. The DTCC *PPD
Dashboard User Guide* v2.8 (May 2025) is the primary description
**(verified)**.

### 2.1 Endpoints

| Purpose | Request | Notes |
|---|---|---|
| Dashboard (human) | `https://pddata.dtcc.com/ppd/cftcdashboard`, `.../secdashboard` | Angular app; tabs for slice reports, cumulative reports and ticker per asset class **(verified)** |
| Data bucket name | `GET https://pddata.dtcc.com/ppd/api/general/bucketname` | `text/plain`; returned `kgc0418-tdw-data-0` on 2026-11-12 **(verified)**. `.../general/regionname` returned `prod` |
| Slice listing | `GET https://<bucket>.s3.amazonaws.com/dashboard/Slice.json` | About 2.9 MB; S3 `ETag` and `Last-Modified` present, so `If-None-Match` and `If-Modified-Since` apply **(verified headers; 304 behaviour is standard S3, unverified against this bucket)** |
| End-of-day listing | `GET https://<bucket>.s3.amazonaws.com/dashboard/Cumulative.json` | About 1.1 MB **(verified)** |
| Ticker | `GET https://<bucket>.s3.amazonaws.com/dashboard/Ticker.json` | Latest 100 messages per bucket, camel-case JSON keys **(verified)**. Not used for capture: it is a rolling window, not a complete record |
| Slice file | `GET https://<bucket>.s3.amazonaws.com/<regime>/slices/<REG>_SLICE_<WORD>_<YYYY>_<MM>_<DD>_<N>.zip` | `<regime>` is `cftc` or `sec`; `<N>` counts from 1 per bucket and UTC day **(verified from listings)** |
| End-of-day file | `GET https://<bucket>.s3.amazonaws.com/<regime>/eod/<REG>_CUMULATIVE_<WORD>_<YYYY>_<MM>_<DD>.zip` | One per bucket and UTC day **(verified from listings)** |

`<REG>` is `CFTC` or `SEC`; `<WORD>` is `RATES`, `CREDITS`, `EQUITIES`,
`FOREX` or `COMMODITIES`. No authentication, API key or cookie is needed
**(verified)**. The bucket name is resolved at start-up and again after a
`403` or `404`, because it is published through the API rather than fixed.

### 2.2 Listings

Each listing is a JSON object keyed by bucket: `CFTC_IR`, `CFTC_CR`,
`CFTC_EQ`, `CFTC_FX`, `CFTC_CO`, `SEC_IR`, `SEC_CR`, `SEC_EQ` (and
`CA_CR`, `CA_EQ`, `CA_IR`, ignored). SEC and Canada have no FX or
commodity buckets **(verified)**. Each value is an array, newest first, of:

| Key | Meaning |
|---|---|
| `fileName` | File name as above |
| `fullFilePath` | Absolute URL of the file |
| `sliceId` | Integer that increases across all files and buckets; used as the batch number (`seq.block_number`) |
| `rowCount` | Data rows in the file |
| `dissemDTM` | Dissemination time of the file, `YYYY-MM-DDThh:mm:ssZ` |
| `startTs`, `endTs` | Slice window (for end-of-day files, when the file was built) |

The pollers ignore unknown keys and fields, refuse file names outside
`[A-Za-z0-9_.-]`, and only follow `fullFilePath` when it points into the
data base URL (otherwise the URL is built from the layout above).

### 2.3 Cadence and retention

| Item | Value |
|---|---|
| Slice files | Written per bucket when there is data, keyed by dissemination timestamp (UTC). Busy buckets get a new slice every 30–60 seconds: on 2026-11-11/12 `CFTC_IR` had 2,207 slices and `SEC_EQ` 985 in 24 hours **(observed)** |
| Slice listing window | About the last 24 hours **(observed)**. Whether a delisted slice object stays downloadable is **unverified**, so slices must be captured within a day |
| End-of-day files | Published about 00:15 UTC for the previous UTC day **(observed)**; available for 366 days, with data from 2023-12-29 (User Guide) **(verified)** |
| Volume | Very uneven: in the observed 24 hours, `SEC_EQ` slices held about 2.2 million rows and `CFTC_EQ` about 0.96 million, against about 12,500 for `CFTC_IR` and a few dozen for `SEC_IR` **(observed)** |
| Availability | 24×7; DDR maintenance from Saturday 22:00 to Sunday 06:00 ET queues dissemination until it ends (User Guide) **(verified)** |
| Rate limits | None documented, none observed **(unverified)**. The pollers make one request at a time with at least 250 ms between request starts by default, and back off on `429` and `5xx` |
| Regulatory floor | 17 CFR 43.3(c)(1): data stays on the repository's website for at least one year after dissemination |

### 2.4 Terms

No terms-of-use page for the PPD site or bucket was found. A third-party
client project states that DTCC limits the data to internal or personal use
without redistribution; that claim was **not verified** against a DTCC
document. Part 43 requires dissemination free of charge (43.3(c)(2)) and
SBSR on a non-fee, widely accessible basis (Rule 900(cc), (tt)), but
neither speaks to redistribution. This is an ADM-0002 / ADM-0004 review
trigger ("a repository adds conditions to its site or API") and needs an
owner decision before records are published.

## 3. File layout

### 3.1 Container and encoding

Each `.zip` holds one CSV file (multiple entries are parsed in name order;
the entry naming is **unverified**). CSV, comma separated, double-quote
quoting, header row. The PPD User Guide says slice and end-of-day files
share one layout across all jurisdictions, with field applicability varying
by jurisdiction and asset class **(verified)**. Encoding is assumed UTF-8;
invalid bytes are replaced, never fatal.

### 3.2 Columns

The User Guide's Appendix A ("Slice File Fields and Layout", January 2024
Re-Write Phase 2) lists the columns below, in this order **(verified)**.
The names are the CFTC Parts 43/45 Technical Specification data elements.
"Used" marks columns the parser reads.

| # | Column | Used |
|---|---|---|
| 1 | Dissemination Identifier | yes |
| 2 | Original Dissemination Identifier | yes |
| 3 | Action type | yes |
| 4 | Event type | yes |
| 5 | Event timestamp | yes (fallback) |
| 6 | Amendment indicator | |
| 7 | Asset Class | yes |
| 8 | Product name | yes |
| 9 | Cleared | |
| 10 | Mandatory clearing indicator | |
| 11 | Execution Timestamp | yes |
| 12 | Effective Date | yes (tenor) |
| 13 | Expiration Date | yes |
| 14 | Maturity date of the underlier | |
| 15 | Non-standardized term indicator | |
| 16 | Platform identifier | |
| 17 | Prime brokerage transaction indicator | |
| 18 | Block trade election indicator | yes |
| 19 | Large notional off-facility swap election indicator | yes |
| 20–21 | Notional amount-Leg 1, -Leg 2 | yes |
| 22–23 | Notional currency-Leg 1, -Leg 2 | yes |
| 24–25 | Notional quantity-Leg 1, -Leg 2 | |
| 26 | Total notional quantity-Leg 1 | yes (quantity, cap marker) |
| 27 | Total notional quantity-Leg 2 | yes (quantity, cap marker) |
| 28–33 | Quantity frequency multiplier, Quantity unit of measure, Quantity frequency (each -Leg 1, -Leg 2) | Quantity unit of measure only |
| 34–39 | Notional amount in effect on associated effective date, Effective date of the notional amount, End date of the notional amount (each -Leg 1, -Leg 2) | |
| 40–43 | Call amount, Call currency, Put amount, Put currency (renamed in 2024; leg 2 removed) | |
| 44 | Exchange rate | yes |
| 45 | Exchange rate basis | |
| 46 | First exercise date | yes (CFTC only) |
| 47–48 | Fixed rate-Leg 1, -Leg 2 | yes |
| 49–50 | Option Premium Amount, Option Premium Currency | yes (CFTC only) |
| 51 | Price | yes |
| 52 | Price unit of measure | |
| 53–54 | Spread-Leg 1, -Leg 2 | yes |
| 55–56 | Spread currency-Leg 1, -Leg 2 | |
| 57 | Strike Price | yes (CFTC only) |
| 58 | Strike price currency/currency pair | |
| 59 | Post-priced swap indicator | |
| 60 | Price currency | |
| 61 | Price notation | yes |
| 62–63 | Spread notation-Leg 1, -Leg 2 | yes |
| 64 | Strike price notation | yes (CFTC only) |
| 65–81 | Day count conventions, floating rate reset and payment frequency periods and multipliers (per leg), Other payment amount | |
| 82–86 | Other payment type, Other payment currency, Settlement currency-Leg 1/-Leg 2, Settlement location | |
| 87–89 | Collateralisation category, Custom basket indicator, Index factor | |
| 90 | Underlier ID-Leg 1 | yes |
| 91–92 | Underlier ID-Leg 2, Underlier ID source-Leg 1 | |
| 93 | Underlying Asset Name | yes |
| 94–95 | Underlying asset subtype or underlying contract subtype-Leg 1, -Leg 2 | |
| 96 | Embedded Option type | |
| 97 | Option Type | yes (CFTC only) |
| 98 | Option Style | yes (CFTC only) |
| 99–105 | Package indicator and package transaction price/spread fields | |
| 106 | Physical delivery location-Leg 1 | |
| 107 | Delivery Type | |
| 108 | Unique Product Identifier | yes |
| 109 | UPI FISN | yes |
| 110 | UPI Underlier Name | yes |
| — | Dissemination Timestamp | yes |

User Guide v2.6 (October 2024) added the **Dissemination Timestamp**; the
ticker carries it (`disseminationTimestamp`) **(verified)**, but its name
and position in the CSV are **unverified**. Whether the 2024 renames
dropped the `-Leg 1` suffix from Call/Put amount and Settlement location is
**unverified** (the ticker keys still carry `Leg1`); both forms are
accepted.

### 3.3 Column matching

Headers are normalized to lower-case ASCII letters and digits
(`Notional amount-Leg 1` and the ticker key `notionalAmountLeg1` both
become `notionalamountleg1`), with a few aliases (`UPI FISN` and
`uniqueProductIdentifierShortName`; `UPI Underlier Name` and
`uniqueProductIdentifierUnderlierName`). Every Appendix A column is known;
any other column is logged, counted in `swap_unknown_columns_total` and
ignored. A file without `Dissemination Identifier` or `Execution
Timestamp` is unreadable: it is archived, counted and not retried.

### 3.4 Values

| Kind | Format | Source |
|---|---|---|
| Dissemination IDs | Up to 52 alphanumeric characters (observed: 19-digit integers) | Tech Spec D1, D2 **(verified)** |
| Timestamps | `YYYY-MM-DDThh:mm:ssZ`, UTC, to the second | Tech Spec D3; observed **(verified)**. The parser also accepts a space separator, fractions, numeric offsets, a missing zone (read as UTC), `MM/DD/YYYY` dates and epoch seconds or milliseconds |
| Dates | `YYYY-MM-DD`, unadjusted | observed **(verified)** |
| Numbers | Thousands separators in disseminated amounts and prices (`1,250,000`, `99,999.99`) | observed **(verified)** |
| Capped amounts | Trailing `+` on the rounded, capped notional or quantity (`250,000,000+`) | observed **(verified)**; rounding and caps under 17 CFR 43.4. When a notional is capped the repository scales other fields proportionally (Tech Spec § 1) |
| Unknown value | All nines in the field's format (`99999.9999999999999`, `9.9999999999`) | Tech Spec **(verified)**; parsed as absent |
| Booleans | `TRUE` / `FALSE` | observed **(verified)** |
| Price, spread and strike notation | `1` monetary amount; `3` decimal (`0.0257` means 2.57%); `4` basis points (integer) | Tech Spec v3.3 **(verified)**. `2` (decimal, older specification) is treated like `3` |
| Fixed rate | Always decimal (`0.0375` means 3.75%) | Tech Spec; observed `0.04` for USD OIS **(verified)** |
| Action type | `NEWT`, `MODI`, `CORR`, `EROR`, `TERM`, `REVI` (also `PRTO`, `VALU`, `MARU`, Part 45 only) | Tech Spec; all six observed **(verified)** |
| Event type | `TRAD`, `NOVA`, `COMP`, `ETRM`, `CLRG`, `EXER`, `ALOC`, `CLAL`, `CREV`, `PTNG`, `CORP`, `UPDT` | Tech Spec **(verified)** |
| Option type | Tech Spec examples use `PUTO`; one ticker row showed `PUT` | **(partly verified)**; mapping in § 4.2 |
| Option style | `AMER`, `EURO`, `BERM`, `ASIA` assumed from the Tech Spec code style | **(unverified)**; mapping in § 4.2 |
| Quantity unit of measure | Short codes such as `MMBT`, `BBL`, `MWh` | **(unverified)**; kept as disseminated |

### 3.5 What each asset class carries

From the Tech Spec's "Price" definition and observed rows:

| Asset class | Price-forming fields |
|---|---|
| Rates | Fixed rate (leg 1 or 2) and Spread for swaps and FRAs; Strike price and Option premium for swaptions, caps and floors |
| Credit | Spread, Price (index trades quoted in price), Fixed rate (the standard coupon) and upfront payment (Other payment) |
| Equity | Price (initial price of the underlier for swaps), Spread (financing leg) |
| FX | Exchange rate; Strike price and Option premium for options |
| Commodities | Price (the agreed fixed price of the fixed leg); Spread for basis swaps; Strike and premium for options. Notional is mostly a quantity (`Total notional quantity-Leg 1` with a unit of measure) |

Commodity swaps carry `Product name` (DTCC's taxonomy, such as
`Commodity:Energy:NatGas:Swap:Cash`) instead of a UPI, because the CFTC
has not yet designated a commodity UPI (Tech Spec § 1.4.2) **(verified)**.
Other asset classes carry the UPI (ISO 4914, issued by the Derivatives
Service Bureau) with its short name (FISN) and underlier name.

### 3.6 SEC SBSR

The SEC buckets (`SEC_IR`, `SEC_CR`, `SEC_EQ`) on the same site use the
same files and layout **(verified)**; CFTC-only fields (block and large
notional elections) are empty. Observed SEC rows are mostly single-name
CDS and single-stock or basket total return swaps. Under ADM-0004 only
price, spread and notional are admitted, so option terms (strike, premium,
option type and style, first exercise date) are not read for SEC records,
even when the cells are filled.

## 4. Mapping to `SwapPrint`

| `SwapPrint` field | Source |
|---|---|
| `prov.venue` | `cftc-part43` or `sec-sbsr` |
| `prov.admission` | `ADM-0002` or `ADM-0004` |
| `prov.instrument` | `<venue>:swap:<symbol>`; symbol is the UPI, else the sanitized `Product name` and `Underlier ID-Leg 1` (`:`-separated, characters outside `[A-Za-z0-9_.@#+-]` replaced by `-`); over 128 bytes it is cut and a digest suffix `#xxxxxxxx` added |
| `prov.seq` | `block_number` = listing `sliceId` (offline without a sidecar: `YYYYMMDD × 10^6 + slice number`); `event_index` counts per instrument within the file |
| `prov.ts_source_ms`, `ts_dissemination_ms` | Dissemination Timestamp; else the listing's `dissemDTM`; else Event timestamp; else Execution Timestamp (each fallback counted) |
| `prov.ts_ingest_ns` | Fetch time from the raw sidecar; without one, the dissemination time, so replay is deterministic |
| `prov.flags` | `size_capped` if a notional or total quantity ends in `+`; `block_trade_delayed` if Block trade election or Large notional off-facility election is `TRUE`; `backfill` for records read from end-of-day files |
| `repository` | `dtcc-ddr` |
| `dissemination_id` | Dissemination Identifier |
| `original_dissemination_id` | Original Dissemination Identifier (required for cancel, correct and revive; optional for modify and terminate) |
| `action` | `NEWT` → `new`; `MODI` → `modify`; `TERM` → `terminate`; `CORR` → `correct`; `EROR` → `cancel`; `REVI` → `revive`. `PRTO`, `VALU`, `MARU` (Part 45 only) and unknown codes produce no record (§ 4.1) |
| `event_type` | Event type code (§ 4.1), whenever the cell has one |
| `asset_class` | `Asset Class` code (`IR`, `CR`, `EQ`, `FX`, `CO`), else the bucket |
| `upi` | Unique Product Identifier |
| `product` | UPI FISN, else Product name |
| `underlier` | UPI Underlier Name, else Underlier ID-Leg 1, else Underlying Asset Name |
| `tenor` | Only when Effective Date and Expiration Date fall on the same day of the month a whole number of months apart: `5Y`, `18M`. Never rounded or guessed |
| `expiry` | Expiration Date |
| `price` | First available of the candidates below, by asset class |
| `notional` | Notional amount and currency, leg 1 then leg 2; the cap value when capped. Omitted when there is no ISO currency (commodity quantities) |
| `quantity` | Commodities with no `notional` only: Total notional quantity and Quantity unit of measure, leg 1 then leg 2; the cap value when capped. The unit is kept as disseminated and must be 1 to 16 ASCII letters or digits, else the quantity is dropped and counted |
| `strike` | CFTC only: Strike Price with Strike price notation, converted like a price (below) |
| `option_premium` | CFTC only: Option Premium Amount and Option Premium Currency (ISO 4217); dropped and counted if either is malformed |
| `option_type`, `option_style` | CFTC only: Option Type and Option Style codes (§ 4.2) |
| `first_exercise_date` | CFTC only: First exercise date |
| `ts_execution_ms` | Execution Timestamp |

Price candidates (the strike is never one; it has its own field):

| Asset class | Order |
|---|---|
| Rates | Fixed rate leg 1, leg 2, Spread leg 1, leg 2, Price |
| Credit | Spread leg 1, leg 2, Price, Fixed rate leg 1, leg 2 |
| Equity | Price, Spread leg 1, leg 2, Fixed rate leg 1, leg 2 |
| FX | Exchange rate, Price, Spread leg 1, leg 2 |
| Commodities | Price, Spread leg 1, leg 2, Fixed rate leg 1, leg 2 |

Conversions, all exact decimal shifts:

| Source | `SwapPrice` |
|---|---|
| Fixed rate (decimal) | `rate`, × 100 (percent) |
| Price or strike, notation 1 or none | `price` |
| Price or strike, notation 3 | `rate`, × 100 |
| Spread, notation 3 | `spread`, × 10,000 (basis points) |
| Spread, price or strike, notation 4 | `spread` as given |
| Spread, notation 1 (a monetary spread, such as a commodity basis) | `spread_amount` |
| Exchange rate | `price` |

A price, strike or spread with any other notation is dropped and counted
(`swap_field_errors_total`).

### 4.1 Lifecycle actions and event types

Every action type observation v0 can represent produces a record. `MODI`
amends an existing transaction (often a price change) and `TERM` ends one
(often priced); both are emitted with whatever terms the row repeats, and
`original_dissemination_id` when the row names one. `REVI` restores a print
cancelled in error and, like `CORR` and `EROR`, is rejected as a malformed
row without an original dissemination identifier.

Event type codes map one to one:

| Code | `event_type` |
|---|---|
| `TRAD` | `trade` |
| `NOVA` | `novation` |
| `COMP` | `compression` |
| `ETRM` | `early_termination` |
| `CLRG` | `clearing` |
| `EXER` | `exercise` |
| `ALOC` | `allocation` |
| `CLAL` | `clearing_allocation` |
| `CREV` | `credit_event` |
| `PTNG` | `porting` |
| `CORP` | `corporate_event` |
| `UPDT` | `update` |

**Non-trade `NEWT` messages are emitted.** Earlier versions skipped `NEWT`
with an event type other than `TRAD`, because a cleared or allocated trade
is disseminated again as a new message and would be counted twice. Now that
every record carries `event_type`, the pollers emit them and leave the
choice to consumers: the record is complete and replayable, and a method
that counts volume or prints selects `event_type` `trade` or absent (an
empty cell has always been read as a trade). A `NEWT` whose event type is
not one of the codes above is still skipped
(`swap_records_skipped_total{reason="unknown_event_type"}`), because
without an event type it would look like a trade. For other actions an
unknown event type is dropped from the record and counted as a field error;
the action itself still applies to the earlier print.

Still skipped (`reason="unsupported_action"`): `PRTO`, `VALU` and `MARU`,
which are Part 45 (regulatory reporting) actions not expected in public
dissemination, and any undocumented action code.

### 4.2 Option codes

Read for CFTC records only (ADM-0002). A code not listed is dropped and
counted (`swap_field_errors_total{field="Option Type"}` or
`{field="Option Style"}`).

| Column | Code | Field value | Status |
|---|---|---|---|
| Option Type | `CALL` | `call` | Tech Spec code **(to verify in files)** |
| Option Type | `PUTO` | `put` | Tech Spec examples **(verified)** |
| Option Type | `PUT` | `put` | Seen in one ticker row **(partly verified)** |
| Option Type | `CHOO` (chooser) | none | Tech Spec code **(to verify)**; no `OptionType` value |
| Option Style | `AMER` | `american` | **(to verify)** |
| Option Style | `EURO` | `european` | **(to verify)** |
| Option Style | `BERM` | `bermudan` | **(to verify)** |
| Option Style | `ASIA` | `asian` | **(to verify)** |

The full words (`AMERICAN`, `EUROPEAN`, ...) are accepted as well. For
options, `price` still follows the candidates above (for a swaption, often
the underlying fixed rate); the strike and premium are only in `strike` and
`option_premium`.

## 5. Raw capture

Every fetched file is written verbatim before parsing (see the
[runbook](../operations/swap-pollers.md)), with a JSON sidecar holding the
URL, fetch time, size, SHA-256, response validators and the listing entry.
Records are a pure function of the raw file and its sidecar.

## 6. Gaps and change requests to `mesh-types`

Resolved by the `swap_print` additions in
[observation-v0](observation-v0.md) § 2.7, and now populated by the
pollers (§ 4):

1. **Lifecycle actions** `MODI`, `TERM`, `REVI`: actions `modify`,
   `terminate`, `revive` (§ 4.1).
2. **Event type**: optional `event_type`; non-trade `NEWT` messages are
   emitted with it (§ 4.1).
3. **Option terms** admitted by ADM-0002: `strike`, `option_premium`,
   `option_type`, `option_style`, `first_exercise_date` (CFTC only). The
   strike is no longer carried as `price`.
4. **Monetary spreads** (notation 1): price type `spread_amount`.
5. **Commodity quantities**: optional `quantity {amount, unit}`.

Still open:

6. **Dissemination provenance**: whether the dissemination time came from
   the row or a fallback is visible only in metrics
   (`swap_dissemination_fallbacks_total`), not in the record.
7. **Quantities outside commodities**: equity and other rows can carry a
   Total notional quantity (for example a share count) with no currency
   notional. They are not mapped to `quantity` yet; whether they are
   admitted as "notional" under ADM-0004 should be settled first.
8. **Price context**: Price currency, Price unit of measure, Spread
   currency and Strike price currency/currency pair are not carried, so a
   `price` or `strike` in currency units has no currency in the record.
9. **Chooser options** (`CHOO`) have no `option_type` value.

The raw files keep everything, so records can be rebuilt when the schema
grows. Raw files captured before this change can be re-parsed with
`--parse` to recover the prints earlier versions skipped.

## 7. Unverified items

- Rate limits and acceptable polling frequency for the bucket and API.
- Whether delisted slice objects remain downloadable after about 24 hours.
- `304` responses from the bucket for conditional requests.
- CSV header name and position of the Dissemination Timestamp; whether
  2024 renames dropped `-Leg 1` suffixes.
- Zip entry naming and whether a zip can hold more than one CSV.
- Option type codes in the files (`PUTO`/`CALL` vs `PUT`/`CALL`) and
  option style codes (`AMER`, `EURO`, `BERM`, `ASIA`) (§ 4.2).
- Quantity unit of measure codes and their case.
- Whether `MODI` and `TERM` rows always carry the Original Dissemination
  Identifier.
- Terms of use for the PPD data (§ 2.4).
- CME SDR status.

## Sources

- DTCC, *Global Trade Repository Public Price Dissemination Dashboard User
  Guide*, v2.8, May 2025:
  <https://kgc0418-tdw-data-0.s3.amazonaws.com/gtr/static/gtr/docs/RT_PPD_quick_ref_guide.pdf>
- DTCC PPD dashboard: <https://pddata.dtcc.com/ppd/cftcdashboard>
- DTCC PPD API (bucket name): <https://pddata.dtcc.com/ppd/api/general/bucketname>
- CFTC, *Parts 43 and 45 Technical Specification*, v3.3, 2023-12-13:
  <https://cftc.gov/media/9921/Part43_45TechnicalSpecification12132023CLEAN/download>
- CFTC, modifications to the technical specification, 2022-09-15:
  <https://www.cftc.gov/PressRoom/PressReleases/8584-22>
- 17 CFR 43.3: <https://www.law.cornell.edu/cfr/text/17/43.3>
- 17 CFR 242.900: <https://www.law.cornell.edu/cfr/text/17/242.900>
- DTCC on the CFTC rewrite (PPD changes live 2022-12-05):
  <https://www.dtcc.com/dtcc-connection/articles/2022/july/22/leading-the-charge-the-cftc-rewrite>
- ICE Trade Vault real-time ticker regulatory guide (for the exclusion
  record only):
  <https://www.ice.com/publicdocs/data/ICE_Trade_Vault_Real-Time_Ticker_Regulatory_Guide.pdf>
- ICE Trade Vault SBSDR approval, 2021-06-30:
  <https://www.businesswire.com/news/home/20210630005562/en/ICE-Announces-that-ICE-Trade-Vault-is-Approved-by-the-SEC-as-a-Security-Based-Swap-Data-Repository>
- Third-party MIT-licensed client documenting the bucket listings (facts
  only, no code used): <https://github.com/pipeworx-io/mcp-dtcc-swaps>
