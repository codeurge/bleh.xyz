# ADM-0011: USDA AMS dairy — National Dairy Products Sales Report and announced FMMO prices

- Status: Admitted
- Requested: 2026-11-12 by the admissions lane (open reference prices plan, task 1)
- Reviewers: Repository owner (engineering review of field lineage)
- Decided: 2026-11-12 by the repository owner (owner decision, 2026-11-12)

## Source

- **NDPSR.** USDA AMS publishes the weekly National Dairy Products Sales
  Report (NDPSR) under the Dairy Product Mandatory Reporting Program
  (7 U.S.C. 1637–1637b; 7 CFR Part 1170). Manufacturers above the
  thresholds report each week the prices and volumes of their sales of:
  - butter;
  - 40-pound block and 500-pound barrel cheddar;
  - dry whey;
  - nonfat dry milk.
- **Announced FMMO prices.** AMS computes Federal Milk Marketing Order
  prices from NDPSR averages (7 CFR 1000.50) and announces them
  (7 CFR 1000.53). The announcements cover:
  - monthly Class II, III and IV prices;
  - component prices (butterfat, protein, nonfat solids, other solids,
    somatic cell adjustment);
  - advanced Class I pricing factors.
- **How we read it.** We use the LMPR DataMart API, which carries the NDPSR
  and the FMMO statistics. Endpoints are in
  [reference-sources.md](../specs/reference-sources.md#usda-ams-dairy).

### Mechanism: sales that may be priced against an exchange cash market

7 CFR 1170.8 (read 2026-11-12) defines what is reported for each product.

- **What it excludes.** "Intra-company sales, resales of purchased cheese,
  forward pricing sales (sales in which the selling price was set [not
  adjusted] 30 or more days before the transaction was completed)", and
  premium-assisted, organic and similar sales.
- **What it does not exclude.** Sales priced within 30 days by a private
  formula that references an exchange's spot cash market for cheese or
  butter, or an exchange-published price.
- **Consequence.** Reported NDPSR prices may be privately set against an
  exchange price, and the announced FMMO prices inherit that through
  7 CFR 1000.50. USDA does not publish the share of such sales.

Every NDPSR price and every price computed from it therefore carries
`exchange_referenced`
([ADR 0012](../decisions/0012-open-terms-route-for-public-bodies-and-market-operators.md)).
They are still USDA's survey of reported sales, not exchange prices.

## Fields

| Field (DataMart names) | Meaning | Admit? |
|---|---|---|
| NDPSR (`slug_id` 2993): `week_ending_date`, `Butter_Price`, `Butter_Sales`, `cheese_40_Price`, `cheese_40_Sales`, `cheese_500_Price`, `cheese_500_Wtd_Price`, `cheese_500_Sales`, `cheese_500_Wtd_Mst`, `whey_Price`, `whey_Sales`, `nonfat_milk_Price`, `nonfat_milk_Sales`, and the "Final" sections | Weekly weighted-average prices and volumes, with revisions | Prices **admitted with `exchange_referenced`**; sales volumes and moisture **admitted** |
| Class and component prices (`slug_id` 2991, Detail): `class_2_Price`, `class_2_butterfat_Price`, `advanced_skim_milk_class_2_Price`, `class_3_Price`, `class_3_skim_milk_Price`, `class_4_Price`, `class_4_skim_milk_Price`, `butterfat_Price`, `nonfat_solids_Price`, `protein_Price`, `other_solids_Price`, `somatic_cell_adjustment_Rate`, `butter_monthly_avg_Price`, `nfdm_monthly_avg_Price`, `cheese_monthly_avg_Price`, `cheese_monthly_40p_Price`, `cheese_monthly_500p_Price`, `whey_monthly_avg_Price` | Announced monthly prices | **Admitted with `exchange_referenced`** |
| Advanced prices and pricing factors (`slug_id` 2989); Class I prices (3345); advanced and final class prices by order (3354, 3355); uniform milk prices (3351); mailbox milk prices (3357) | Announced FMMO prices | **Admitted with `exchange_referenced`** |
| Utilization, producer receipts and components (3346–3350, 3462, 3361) | Volumes | **Admitted** |
| Dairy Market News commentary and any field quoting an exchange's spot cash or futures prices (for example weekly "CME" cash closes) | Exchange prices | **Excluded** |

## Policy checks

Under [admission-policy.md](../admission-policy.md):

1. **Original price formation.** NDPSR prices are manufacturers' sales,
   reported to USDA under federal law. FMMO prices are USDA computations
   from them. Neither re-publishes an exchange price, but private pricing
   may reference one. That is the mechanism described above, and the flag
   covers it.
2. **No contract on the path.** The data are public-domain government data
   published under statute. The LMPR DataMart (read 2026-11-12) says:
   "There are no restrictions on who may obtain this data or how it may be
   used. However, if the data is published and identified as USDA data, it
   cannot be altered in any way." The DataMart API needs no key.
3. **Venue-matched fields only.** The admitted fields are USDA statistics,
   and the affected prices carry `exchange_referenced`. Fields that quote
   exchange prices are excluded.
4. **Field-level exclusions.** Exchange cash and futures quotations in
   Dairy Market News reports.

## Source basis

Statutory public-domain publication by USDA AMS, read 2026-11-12:

- <https://mpr.datamart.ams.usda.gov/>
- 7 CFR Part 1170 via <https://www.ecfr.gov/current/title-7/subtitle-B/chapter-X/part-1170>
- 7 CFR 1000.50 and 1000.53 via <https://www.ecfr.gov/current/title-7/subtitle-B/chapter-X/part-1000>

## Risks and review triggers

- Changes to 7 CFR 1170.8 exclusions or to the FMMO price formulas. Formula
  changes are versioned with the record.
- USDA publishes data showing how much of the NDPSR volume is
  exchange-referenced.
- The DataMart adds access conditions.

## Decision

Admitted on 2026-11-12 by the repository owner for the fields above. Every
NDPSR price and announced FMMO price carries `exchange_referenced`. Exchange
cash and futures quotations are excluded.
