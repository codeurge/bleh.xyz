# ADM-0010: USDA AMS Livestock Mandatory Reporting — swine, cattle, lamb, boxed beef and pork

- Status: Admitted
- Requested: 2026-11-12 by the admissions lane (open reference prices plan, task 1)
- Reviewers: Repository owner (engineering review of field lineage)
- Decided: 2026-11-12 by the repository owner (owner decision, 2026-11-12)

## Source

USDA's Agricultural Marketing Service (AMS) publishes Livestock Mandatory
Reporting (LMR) reports. The legal basis is the Livestock Mandatory
Reporting Act of 1999 (7 U.S.C. 1635–1636h) and 7 CFR Part 59.

- **Who reports.** Packers above the regulation's size thresholds report
  their purchases of slaughter swine, cattle and lambs, and their wholesale
  boxed beef, pork and lamb sales.
- **How AMS publishes.** Reports are released several times a day, broken
  down by purchase type.
- **How we read it.** We use the LMPR DataMart API
  (mpr.datamart.ams.usda.gov). It needs no key. History is available from
  2001-04-01. Endpoints are in
  [reference-sources.md](../specs/reference-sources.md#usda-ams-livestock-mandatory-reporting).

### Purchase types and their lineage

The definitions below are from 7 CFR 59.30, 59.100, 59.200 and 59.300 on
eCFR, read 2026-11-12. Admission follows them.

| Purchase type | Definition (summary, with the decisive words quoted) | Admit prices? |
|---|---|---|
| Negotiated purchase or sale | "a cash or spot market purchase ... under which the base price ... is determined by seller-buyer interaction and agreement on a delivery day" | **Yes** |
| Negotiated grid purchase (cattle) | Base price negotiated as above; grid premiums and discounts applied | **Yes** (base price) |
| Negotiated formula purchase (swine) | A swine or pork market formula purchase whose formula is negotiated lot by lot | **Yes, with `exchange_referenced`** |
| Swine or pork market formula purchase | "a formula price based on a market for swine, pork, or a pork product, other than a future or option for swine, pork, or a pork product" | **Yes, with `exchange_referenced`** |
| Other market formula purchase (swine) | Formula based on any other market; "includes a formula purchase in a case which the price formula is based on 1 or more futures or options contracts" | **No** |
| Other purchase arrangement (swine) | Anything not covered by the other types, such as long-term contracts | **No.** Pricing mechanism not disclosed |
| Formula marketing arrangement (cattle, lamb; boxed beef and lamb) | "the price is determined at a future date" by a formula | **No.** The formula basis is not disclosed and may reference futures |
| Forward contract (cattle) | "base price is established by reference to prices quoted on the Chicago Mercantile Exchange, or other comparable publicly available prices" | **No** |
| Forward contract (lamb; boxed beef and lamb) | Base price set by reference to publicly available prices | **No** |
| Formula and forward sales of wholesale pork | Price "established in reference to publicly-available quoted prices" | **No** |
| Packer-owned livestock | Internal transfer, no market price | **No** |
| "All purchase types" aggregates | Mix the excluded types in | **No** |

Why the two swine formula types carry the flag:

- The rule bars futures and options from swine or pork market formulas.
- It does not bar a formula that references an exchange-published cash
  index for hogs or pork.
- Those prices may therefore be privately set against an exchange-published
  number. They carry `exchange_referenced` under
  [ADR 0012](../decisions/0012-open-terms-route-for-public-bodies-and-market-operators.md).

## Fields

| Field | Meaning | Admit? |
|---|---|---|
| Report metadata: `report_date`, `for_date_begin`, `published_date`, `slug_id`, `slug_name`, `report_title`, `office_*`, `market_location_*`, `market_type`, `is_correction`, `narrative` | Identity, timing, corrections | **Admitted** |
| Swine, by `purchase_type` (`Prod. Sold Negotiated`): `base_price`, `avg_net_price`, `lowest_net_price`, `highest_net_price` and weights | Negotiated barrow and gilt prices (LM_HG200–LM_HG212, LM_HG201, LM_HG216–LM_HG218, sow and boar reports) | **Admitted** |
| Swine price fields for `Prod. Sold Negotiated Formula`, `Prod. Sold Swine or Pork Market Formula` and `Prod. Sold Combined` (negotiated plus negotiated formula) | Formula prices | **Admitted with `exchange_referenced`** |
| Swine price fields for `Prod. Sold Other Market Formula`, `Prod. Sold Other Purchase Arrangement`, `Packer Owned`, `Pack. Sold`, `(All Purchase Types)`; report-wide price averages that mix types (for example `wtd_avg_base` and `wtd_avg_net_price` in Carcass Measurements, LM_HG213 slaughter cost, LM_HG215 net price distribution) | Futures-referenced, undisclosed or mixed | **Excluded** |
| Swine volumes and carcass measurements for every purchase type: `head_count`, weights, `avg_backfat`, `avg_loin_depth`, `loineye_area`, `avg_lean_percent`, 14-day scheduled swine | Not prices | **Admitted** |
| Cattle negotiated purchases (LM_CT100, LM_CT103, LM_CT113–LM_CT124, LM_CT133–LM_CT137, LM_CT149, LM_CT150, LM_CT154, LM_CT156–LM_CT158, LM_CT166–LM_CT168, LM_CT180–LM_CT185, LM_CT201, LM_CT202): `price_range_low`, `price_range_high`, `weighted_avg_price`, `head_count`, weights, `class_description`, `selling_basis_description`, `grade_description` | Negotiated fed-cattle and cow prices | **Admitted** |
| Cattle negotiated grid base prices (negotiated-grid rows of LM_CT145, LM_CT151 and the state formula, grid and contract reports) | Negotiated base prices | **Admitted** |
| Plant-delivered bids for cows and bulls (LM_CT138); premiums and discounts schedules (LM_CT155, LM_CT169) | Packers' cash bids and grid adjustments | **Admitted** |
| Cattle formula, formulated base and forward-contract prices (LM_CT109, the formulated base and forward contract base reports, formula and forward rows of LM_CT145, LM_CT151, LM_CT152, LM_CT153, including its "Basis Distribution"); mixed-type prices in LSWFEDCC | Formula and futures-referenced | **Excluded** |
| Cattle volumes (committed and delivered, packer-owned, head counts of every type) | Not prices | **Admitted** |
| Boxed beef negotiated sales (LM_XB403 cutout and cuts, and the negotiated rows of other boxed beef reports) | Negotiated wholesale | **Admitted** |
| Boxed beef formula and forward sales (for example LM_XB450 formulated sales) | Formula | **Excluded** |
| Pork negotiated sales (LM_PK602, LM_PK603, LM_PK610, LM_PK611): cutout and primal values, cut prices, volumes | Negotiated wholesale | **Admitted** |
| Pork formula (LM_PK620, LM_PK621), forward (LM_PK630, LM_PK631), export, specialty and comprehensive (LSDLM_PK680, LSDLM_PK681) price fields | Formula, forward or mixed | **Excluded** (volumes admitted) |
| Lamb negotiated purchases and negotiated boxed lamb sales | Negotiated | **Admitted** |
| Lamb formula and forward prices | Formula | **Excluded** |

Rule for reports not named above: admit price fields only in rows or
sections the report labels negotiated (or swine or pork market formula,
with the flag). Exclude price fields in every other row.

## Policy checks

Under [admission-policy.md](../admission-policy.md):

1. **Original price formation.** Admitted prices are cash transactions
   between producers and packers, or between packers and wholesale buyers,
   reported to USDA under federal law. Prices set from futures, by
   undisclosed formulas, or as internal transfers are excluded by purchase
   type.
2. **No contract on the path.** The data are public-domain government data
   published under statute. The LMPR DataMart home page
   (<https://mpr.datamart.ams.usda.gov/>, read 2026-11-12) says: "There are
   no restrictions on who may obtain this data or how it may be used.
   However, if the data is published and identified as USDA data, it cannot
   be altered in any way."
   - The [LMPR API User Guide](https://mpr.datamart.ams.usda.gov/LMPRS-API-User-Guide.pdf)
     (v3.3, November 2024) says: "The LMPR API does not require a user key
     like the MyMarketNews API requires."
   - Values presented as USDA data are carried unaltered. Derived values
     are labelled as the project's.
3. **Venue-matched fields only.** The admitted price fields are USDA
   statistics of negotiated cash trades. The formula types that may
   reference an exchange-published cash index carry `exchange_referenced`.
   Nothing quoting a futures price is admitted.
4. **Field-level exclusions.** As listed above.

## Source basis

Statutory public-domain publication by USDA AMS, read 2026-11-12:

- <https://mpr.datamart.ams.usda.gov/>
- <https://mpr.datamart.ams.usda.gov/services/v1.1/reports>
- 7 CFR 59.30, 59.100, 59.200, 59.300 via <https://www.ecfr.gov/current/title-7/subtitle-B/chapter-I/subchapter-C/part-59>

## Risks and review triggers

- Changes to the 7 CFR Part 59 purchase-type definitions, or to the
  categories AMS reports.
- Evidence that negotiated purchases are commonly priced from futures.
- The DataMart adds access conditions.
- LMR lapses in Congress. Publication has paused during past lapses in
  authorization.

## Decision

Admitted on 2026-11-12 by the repository owner for the fields above:

- negotiated prices;
- swine formula prices, with `exchange_referenced`;
- volumes and carcass data of every type.

Other market formula, other purchase arrangement, formula, forward contract
and packer-owned prices are excluded.
