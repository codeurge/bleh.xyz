//! EIA energy statistics from EIA's own surveys (ADM-0021), read from the
//! bulk download files `PET.zip` and `NG.zip` (the API is for checks only
//! and is not read).
//!
//! Each bulk file holds one JSON object per line: series lines (with
//! `series_id`, `units`, `f`, `copyright`, `source`, `last_updated` and
//! `data`, a list of `[period, value]` pairs) and category lines (with
//! `category_id`), which are skipped. EIA republishes vendor and exchange
//! prices in the same files, and the admission found such series labelled
//! `"copyright":"None"` and `"source":"EIA"`, so the metadata is never
//! trusted to filter:
//!
//! - Only series on [`ALLOWLIST`] (explicit series IDs, each in one
//!   admitted family) are read; every other series is dropped
//!   (`out_of_scope`).
//! - Series matching the excluded families ([`is_excluded_family`]: the
//!   spot and futures tables, wholesale and resale heating oil and
//!   propane, Henry Hub) are dropped as `excluded` before the allowlist is
//!   consulted, and no allowlisted ID may match them (tested).
//! - An allowlisted series whose metadata credits a third party is
//!   dropped as `excluded` and reported (`vendor_credit:<id>`), the
//!   admission's review trigger.
//!
//! | Family | Survey | Records (`eia:ref:<series_id>`) |
//! |---|---|---|
//! | Weekly retail gasoline and on-highway diesel | EIA-878, EIA-888 | `price` in US dollars per gallon, dated the survey Monday |
//! | Weekly residential heating oil and propane | State Heating Oil and Propane Program | `price`, dated the survey Monday |
//! | Monthly refiner sales, crude first purchase, imported crude cost, natural gas city-gate and end-use prices | Monthly price surveys | `price`, dated the first of the month, `exchange_referenced` |
//! | Weekly petroleum stocks (including Cushing) | EIA-800 to EIA-805, EIA-814 | `stock` in `thousand_barrels`, dated the week-ending Friday |
//! | Weekly refinery inputs, production, imports, exports, product supplied | Same | `flow` in `thousand_barrels_per_day` (the week's daily average) |
//! | Weekly operable crude distillation capacity and utilisation | Same | `capacity` in `thousand_barrels_per_day`; `ratio` in `percent` |
//! | Weekly natural gas storage by region | EIA-912 | `stock` in `billion_cubic_feet`, dated the week-ending Friday |
//! | Monthly crude oil production, natural gas production, consumption, storage injections and withdrawals | Monthly surveys | `flow` in `thousand_barrels` or `million_cubic_feet` (the month's total), dated the first of the month |
//!
//! Each allowlisted series names its measure (ADR 0023). The unit comes
//! from the series' `units` text through [`quantity_unit`]; a unit text
//! that is not known, or that does not fit the measure, drops the series
//! and is reported (`units:<id>`), as a price not in dollars is. Values
//! are carried exactly as written in the file (the JSON number's text,
//! never a float, never rescaled). Volumes carry no flag
//! (`exchange_referenced` is for the monthly survey prices only, ADM-0021
//! check 3). Live records take `ts_source_ms` from the series'
//! `last_updated`, EIA's statement of when it published the values.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::sync::OnceLock;
use std::time::Duration;

use mesh_types::{QuantityUnit, ReferenceMeasure};
use serde::Deserialize;
use serde_json::value::RawValue;

use super::{Bases, ParseError, Parsed, Reader, Schedule, Target};
use crate::num;
use crate::obs::{Obs, RefObs};
use crate::source::SourceId;
use crate::time::Date;

/// Default eia.gov base.
pub const BASE: &str = "https://www.eia.gov";

/// A bulk file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dataset {
    /// Petroleum and other liquids (`PET.zip`).
    Pet,
    /// Natural gas (`NG.zip`).
    Ng,
}

impl Dataset {
    /// Both datasets.
    pub const ALL: [Dataset; 2] = [Dataset::Pet, Dataset::Ng];

    /// The dataset code: the file name and the series ID prefix.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Dataset::Pet => "PET",
            Dataset::Ng => "NG",
        }
    }

    fn from_code(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.code() == s)
    }
}

/// An admitted family of ADM-0021.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// Weekly petroleum supply, stocks, refinery inputs and utilisation,
    /// production, imports and exports (EIA-800 to EIA-805, EIA-814).
    WeeklySupply,
    /// Weekly natural gas storage by region (EIA-912).
    WeeklyStorage,
    /// Weekly retail gasoline and on-highway diesel prices (EIA-878,
    /// EIA-888).
    WeeklyRetail,
    /// Weekly residential heating oil and propane prices (State Heating
    /// Oil and Propane Program); residential series only.
    ResidentialHeating,
    /// Monthly petroleum and natural gas supply and disposition.
    MonthlySupply,
    /// Monthly refiner sales prices, crude first-purchase prices, imported
    /// crude costs, natural gas city-gate and end-use prices.
    MonthlyPrice,
}

impl Family {
    /// Price families; the others are volume families, emitted as
    /// quantities (ADR 0023).
    #[must_use]
    pub const fn is_price(self) -> bool {
        matches!(
            self,
            Family::WeeklyRetail | Family::ResidentialHeating | Family::MonthlyPrice
        )
    }

    /// The admission requires `exchange_referenced` (check 3: monthly
    /// survey prices are often priced by formula against an exchange
    /// settlement).
    #[must_use]
    pub const fn exchange_referenced(self) -> bool {
        matches!(self, Family::MonthlyPrice)
    }
}

use Family::{
    MonthlyPrice as MP, MonthlySupply as MS, ResidentialHeating as RH, WeeklyRetail as WR,
    WeeklyStorage as WG, WeeklySupply as WS,
};
use ReferenceMeasure::{Capacity as C, Flow as F, Price as P, Ratio as R, Stock as S};

/// Admitted series: explicit bulk series IDs, each in one admitted family
/// and with its measure (`price` for the price families; `stock`, `flow`,
/// `capacity` or `ratio` for the volume families), taken from EIA's data
/// tables (dnav, read 2026-11-14). Any series not listed is dropped.
pub const ALLOWLIST: &[(&str, Family, ReferenceMeasure)] = &[
    // Weekly petroleum stocks (Weekly Petroleum Status Report).
    ("PET.WTTSTUS1.W", WS, S),
    ("PET.WTESTUS1.W", WS, S),
    ("PET.WCRSTUS1.W", WS, S),
    ("PET.WCESTUS1.W", WS, S),
    ("PET.W_EPC0_SAX_NUS_MBBL.W", WS, S),
    ("PET.WCSSTUS1.W", WS, S),
    ("PET.WCESTP11.W", WS, S),
    ("PET.WCESTP21.W", WS, S),
    ("PET.W_EPC0_SAX_YCUOK_MBBL.W", WS, S),
    ("PET.WCESTP31.W", WS, S),
    ("PET.WCESTP41.W", WS, S),
    ("PET.WCESTP51.W", WS, S),
    ("PET.WGTSTUS1.W", WS, S),
    ("PET.WGFSTUS1.W", WS, S),
    ("PET.WBCSTUS1.W", WS, S),
    ("PET.W_EPOOXE_SAE_NUS_MBBL.W", WS, S),
    ("PET.WKJSTUS1.W", WS, S),
    ("PET.WDISTUS1.W", WS, S),
    ("PET.WD0ST_NUS_1.W", WS, S),
    ("PET.WD1ST_NUS_1.W", WS, S),
    ("PET.WDGSTUS1.W", WS, S),
    ("PET.WRESTUS1.W", WS, S),
    ("PET.WPRSTUS1.W", WS, S),
    ("PET.W_EPLLPZ_SAE_NUS_MBBL.W", WS, S),
    ("PET.WUOSTUS1.W", WS, S),
    // Weekly production, refinery inputs, capacity and utilisation.
    ("PET.WCRFPUS2.W", WS, F),
    ("PET.WCRRIUS2.W", WS, F),
    ("PET.WGIRIUS2.W", WS, F),
    ("PET.WOCLEUS2.W", WS, C),
    ("PET.WPULEUS3.W", WS, R),
    ("PET.W_EPOOXE_YOP_NUS_MBBLD.W", WS, F),
    // Weekly refiner and blender net production.
    ("PET.WGFRPUS2.W", WS, F),
    ("PET.WKJRPUS2.W", WS, F),
    ("PET.WDIRPUS2.W", WS, F),
    ("PET.WRERPUS2.W", WS, F),
    ("PET.WPRTP_NUS_2.W", WS, F),
    // Weekly imports, exports and net imports.
    ("PET.WTTIMUS2.W", WS, F),
    ("PET.WCRIMUS2.W", WS, F),
    ("PET.WCEIMUS2.W", WS, F),
    ("PET.WCSIMUS2.W", WS, F),
    ("PET.WRPIMUS2.W", WS, F),
    ("PET.WGTIMUS2.W", WS, F),
    ("PET.WKJIMUS2.W", WS, F),
    ("PET.WDIIMUS2.W", WS, F),
    ("PET.WREIMUS2.W", WS, F),
    ("PET.WPRIM_NUS-Z00_2.W", WS, F),
    ("PET.WTTEXUS2.W", WS, F),
    ("PET.WCREXUS2.W", WS, F),
    ("PET.WRPEXUS2.W", WS, F),
    ("PET.W_EPM0F_EEX_NUS-Z00_MBBLD.W", WS, F),
    ("PET.WKJEXUS2.W", WS, F),
    ("PET.WDIEXUS2.W", WS, F),
    ("PET.WREEXUS2.W", WS, F),
    ("PET.W_EPLLPZ_EEX_NUS-Z00_MBBLD.W", WS, F),
    ("PET.WTTNTUS2.W", WS, F),
    ("PET.WCRNTUS2.W", WS, F),
    ("PET.WRPNTUS2.W", WS, F),
    // Weekly product supplied.
    ("PET.WRPUPUS2.W", WS, F),
    ("PET.WGFUPUS2.W", WS, F),
    ("PET.WKJUPUS2.W", WS, F),
    ("PET.WDIUPUS2.W", WS, F),
    ("PET.WREUPUS2.W", WS, F),
    ("PET.WPRUP_NUS_2.W", WS, F),
    ("PET.WWOUP_NUS_2.W", WS, F),
    // Weekly working gas in underground storage (EIA-912).
    ("NG.NW2_EPG0_SWO_R48_BCF.W", WG, S),
    ("NG.NW2_EPG0_SWO_R31_BCF.W", WG, S),
    ("NG.NW2_EPG0_SWO_R32_BCF.W", WG, S),
    ("NG.NW2_EPG0_SWO_R34_BCF.W", WG, S),
    ("NG.NW2_EPG0_SWO_R35_BCF.W", WG, S),
    ("NG.NW2_EPG0_SWO_R33_BCF.W", WG, S),
    ("NG.NW2_EPG0_SSO_R33_BCF.W", WG, S),
    ("NG.NW2_EPG0_SNO_R33_BCF.W", WG, S),
    // Weekly retail gasoline (EIA-878), United States.
    ("PET.EMM_EPM0_PTE_NUS_DPG.W", WR, P),
    ("PET.EMM_EPM0U_PTE_NUS_DPG.W", WR, P),
    ("PET.EMM_EPM0R_PTE_NUS_DPG.W", WR, P),
    ("PET.EMM_EPMR_PTE_NUS_DPG.W", WR, P),
    ("PET.EMM_EPMRU_PTE_NUS_DPG.W", WR, P),
    ("PET.EMM_EPMRR_PTE_NUS_DPG.W", WR, P),
    ("PET.EMM_EPMM_PTE_NUS_DPG.W", WR, P),
    ("PET.EMM_EPMMU_PTE_NUS_DPG.W", WR, P),
    ("PET.EMM_EPMMR_PTE_NUS_DPG.W", WR, P),
    ("PET.EMM_EPMP_PTE_NUS_DPG.W", WR, P),
    ("PET.EMM_EPMPU_PTE_NUS_DPG.W", WR, P),
    ("PET.EMM_EPMPR_PTE_NUS_DPG.W", WR, P),
    // Weekly retail regular gasoline by PADD region.
    ("PET.EMM_EPMR_PTE_R10_DPG.W", WR, P),
    ("PET.EMM_EPMR_PTE_R1X_DPG.W", WR, P),
    ("PET.EMM_EPMR_PTE_R1Y_DPG.W", WR, P),
    ("PET.EMM_EPMR_PTE_R1Z_DPG.W", WR, P),
    ("PET.EMM_EPMR_PTE_R20_DPG.W", WR, P),
    ("PET.EMM_EPMR_PTE_R30_DPG.W", WR, P),
    ("PET.EMM_EPMR_PTE_R40_DPG.W", WR, P),
    ("PET.EMM_EPMR_PTE_R50_DPG.W", WR, P),
    ("PET.EMM_EPMR_PTE_R5XCA_DPG.W", WR, P),
    ("PET.EMM_EPMR_PTE_SCA_DPG.W", WR, P),
    // Weekly retail on-highway diesel (EIA-888), United States and PADDs.
    ("PET.EMD_EPD2D_PTE_NUS_DPG.W", WR, P),
    ("PET.EMD_EPD2DXL0_PTE_NUS_DPG.W", WR, P),
    ("PET.EMD_EPD2DM10_PTE_NUS_DPG.W", WR, P),
    ("PET.EMD_EPD2D_PTE_R10_DPG.W", WR, P),
    ("PET.EMD_EPD2D_PTE_R1X_DPG.W", WR, P),
    ("PET.EMD_EPD2D_PTE_R1Y_DPG.W", WR, P),
    ("PET.EMD_EPD2D_PTE_R1Z_DPG.W", WR, P),
    ("PET.EMD_EPD2D_PTE_R20_DPG.W", WR, P),
    ("PET.EMD_EPD2D_PTE_R30_DPG.W", WR, P),
    ("PET.EMD_EPD2D_PTE_R40_DPG.W", WR, P),
    ("PET.EMD_EPD2D_PTE_R50_DPG.W", WR, P),
    ("PET.EMD_EPD2D_PTE_R5XCA_DPG.W", WR, P),
    ("PET.EMD_EPD2D_PTE_SCA_DPG.W", WR, P),
    // Weekly residential heating oil and propane (residential only; the
    // wholesale series beside them are OPIS quotes and excluded).
    ("PET.W_EPD2F_PRS_NUS_DPG.W", RH, P),
    ("PET.W_EPLLPA_PRS_NUS_DPG.W", RH, P),
    // Monthly crude oil production.
    ("PET.MCRFPUS1.M", MS, F),
    ("PET.MCRFPP11.M", MS, F),
    ("PET.MCRFPP21.M", MS, F),
    ("PET.MCRFPP31.M", MS, F),
    ("PET.MCRFPP41.M", MS, F),
    ("PET.MCRFPP51.M", MS, F),
    ("PET.MCRFPTX1.M", MS, F),
    ("PET.MCRFPND1.M", MS, F),
    ("PET.MCRFPNM1.M", MS, F),
    ("PET.MCRFP3FM1.M", MS, F),
    ("PET.MCRFPAK1.M", MS, F),
    // Monthly natural gas supply and disposition.
    ("NG.N9070US2.M", MS, F),
    ("NG.N9140US2.M", MS, F),
    ("NG.N5050US2.M", MS, F),
    ("NG.N5060US2.M", MS, F),
    // Monthly refiner motor gasoline prices by grade and sales type.
    ("PET.EMA_EPM0_PTG_NUS_DPG.M", MP, P),
    ("PET.EMA_EPM0_PTR_NUS_DPG.M", MP, P),
    ("PET.EMA_EPM0_POR_NUS_DPG.M", MP, P),
    ("PET.EMA_EPM0_PWG_NUS_DPG.M", MP, P),
    ("PET.EMA_EPM0_PDR_NUS_DPG.M", MP, P),
    ("PET.EMA_EPM0_PRG_NUS_DPG.M", MP, P),
    ("PET.EMA_EPM0_PBR_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMR_PTG_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMR_PTR_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMR_POR_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMR_PWG_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMR_PDR_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMR_PRG_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMR_PBR_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMM_PTG_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMM_PTR_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMM_POR_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMM_PWG_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMM_PDR_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMM_PRG_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMM_PBR_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMP_PTG_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMP_PTR_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMP_POR_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMP_PWG_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMP_PDR_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMP_PRG_NUS_DPG.M", MP, P),
    ("PET.EMA_EPMP_PBR_NUS_DPG.M", MP, P),
    // Monthly refiner No. 2 distillate and No. 2 diesel prices.
    ("PET.EMA_EPD2_PTA_NUS_DPG.M", MP, P),
    ("PET.EMA_EPD2_PRT_NUS_DPG.M", MP, P),
    ("PET.EMA_EPD2_PCS_NUS_DPG.M", MP, P),
    ("PET.EMA_EPD2_PIN_NUS_DPG.M", MP, P),
    ("PET.EMA_EPD2_PTC_NUS_DPG.M", MP, P),
    ("PET.EMA_EPD2_POA_NUS_DPG.M", MP, P),
    ("PET.EMA_EPD2_PWA_NUS_DPG.M", MP, P),
    ("PET.EMA_EPD2D_PTA_NUS_DPG.M", MP, P),
    ("PET.EMA_EPD2D_PCS_NUS_DPG.M", MP, P),
    ("PET.EMA_EPD2D_PIN_NUS_DPG.M", MP, P),
    ("PET.EMA_EPD2D_PTC_NUS_DPG.M", MP, P),
    ("PET.EMA_EPD2D_POA_NUS_DPG.M", MP, P),
    ("PET.EMA_EPD2D_PWA_NUS_DPG.M", MP, P),
    // Monthly refiner propane (consumer grade) prices.
    ("PET.EMA_EPLLPA_PTA_NUS_DPG.M", MP, P),
    ("PET.EMA_EPLLPA_PRT_NUS_DPG.M", MP, P),
    ("PET.EMA_EPLLPA_PCS_NUS_DPG.M", MP, P),
    ("PET.EMA_EPLLPA_PIN_NUS_DPG.M", MP, P),
    ("PET.EMA_EPLLPA_PTC_NUS_DPG.M", MP, P),
    ("PET.EMA_EPLLPA_PPL_NUS_DPG.M", MP, P),
    ("PET.EMA_EPLLPA_POA_NUS_DPG.M", MP, P),
    ("PET.EMA_EPLLPA_PWA_NUS_DPG.M", MP, P),
    // Monthly domestic crude oil first purchase prices.
    ("PET.F000000__3.M", MP, P),
    ("PET.F001000__3.M", MP, P),
    ("PET.F002000__3.M", MP, P),
    ("PET.F003000__3.M", MP, P),
    ("PET.F004000__3.M", MP, P),
    ("PET.F005000__3.M", MP, P),
    ("PET.F002038__3.M", MP, P),
    ("PET.F002040__3.M", MP, P),
    ("PET.F003048__3.M", MP, P),
    ("PET.F005071__3.M", MP, P),
    // Monthly refiner acquisition cost of imported crude oil.
    ("PET.R1300____3.M", MP, P),
    // Monthly natural gas city-gate and end-use prices.
    ("NG.N3050US3.M", MP, P),
    ("NG.N3010US3.M", MP, P),
    ("NG.N3020US3.M", MP, P),
    ("NG.N3035US3.M", MP, P),
    ("NG.N3045US3.M", MP, P),
];

/// Series ID prefixes of the excluded families: the spot and futures
/// tables (`EER_` series, WTI and Brent spot, NYMEX contracts) and the
/// Henry Hub spot and natural gas futures prices.
pub const EXCLUDED_PREFIXES: &[&str] = &[
    "PET.EER_",
    "PET.RWTC.",
    "PET.RBRTE.",
    "PET.RCLC",
    "NG.RNGWHHD.",
    "NG.RNGC",
];

/// Process codes of the excluded families anywhere in a series ID:
/// wholesale and resale prices (`PWR`, OPIS terminal quotes), spot FOB
/// prices (`PF4`) and futures contracts (`PE1` to `PE4`).
pub const EXCLUDED_PROCESSES: &[&str] = &["_PWR_", "_PF4_", "_PE1_", "_PE2_", "_PE3_", "_PE4_"];

/// Words that mark a third-party credit in a series' `copyright` or
/// `source` (vendors and exchanges the admission excludes).
const VENDOR_WORDS: &[&str] = &[
    "thomson",
    "reuters",
    "refinitiv",
    "lseg",
    "bloomberg",
    "opis",
    "nymex",
    "cme",
    "ice",
    "intercontinental",
    "platts",
    "argus",
];

/// True if `id` belongs to an excluded (republished) family.
#[must_use]
pub fn is_excluded_family(id: &str) -> bool {
    EXCLUDED_PREFIXES.iter().any(|p| id.starts_with(p))
        || EXCLUDED_PROCESSES.iter().any(|p| id.contains(p))
}

/// The admitted family of `id`, if it is on the allowlist and not in an
/// excluded family.
#[must_use]
pub fn family(id: &str) -> Option<Family> {
    entry(id).map(|(f, _)| f)
}

/// The admitted family and measure of `id`, if it is on the allowlist and
/// not in an excluded family.
#[must_use]
pub fn entry(id: &str) -> Option<(Family, ReferenceMeasure)> {
    if is_excluded_family(id) {
        return None;
    }
    static INDEX: OnceLock<HashMap<&'static str, (Family, ReferenceMeasure)>> = OnceLock::new();
    INDEX
        .get_or_init(|| ALLOWLIST.iter().map(|&(id, f, m)| (id, (f, m))).collect())
        .get(id)
        .copied()
}

/// The unit of a bulk series' `units` text (ADR 0023), for the volume
/// families: thousand barrels, thousand barrels per day (or per calendar
/// day, as capacity is stated), barrels, million and billion cubic feet,
/// and percent. Matching ignores case and surrounding space; anything else
/// is `None`.
#[must_use]
pub fn quantity_unit(units: &str) -> Option<QuantityUnit> {
    let u = units.trim().to_ascii_lowercase();
    Some(match u.as_str() {
        "barrels" => QuantityUnit::Barrels,
        "thousand barrels" => QuantityUnit::ThousandBarrels,
        "thousand barrels per day" | "thousand barrels per calendar day" => {
            QuantityUnit::ThousandBarrelsPerDay
        }
        "million cubic feet" => QuantityUnit::MillionCubicFeet,
        "billion cubic feet" => QuantityUnit::BillionCubicFeet,
        "percent" => QuantityUnit::Percent,
        _ => return None,
    })
}

/// True if `unit` fits `measure` for an EIA volume series: a ratio is in
/// percent and nothing else is; a stock is never per day, and a flow over
/// a week is (EIA states weekly flows as daily averages); a capacity is
/// per day.
#[must_use]
pub fn unit_fits(measure: ReferenceMeasure, unit: QuantityUnit, weekly: bool) -> bool {
    match measure {
        ReferenceMeasure::Ratio => unit == QuantityUnit::Percent,
        _ if unit == QuantityUnit::Percent => false,
        ReferenceMeasure::Stock => !unit.is_per_day(),
        ReferenceMeasure::Flow => !weekly || unit.is_per_day(),
        ReferenceMeasure::Capacity => unit.is_per_day(),
        _ => false,
    }
}

/// True if a `copyright` or `source` text credits a third party: any
/// copyright other than `None`, or a vendor or exchange named in either.
#[must_use]
pub fn credits_third_party(copyright: Option<&str>, source: Option<&str>) -> bool {
    let named = |s: &str| {
        s.split(|c: char| !c.is_ascii_alphanumeric())
            .any(|w| VENDOR_WORDS.contains(&w.to_ascii_lowercase().as_str()))
    };
    let c = copyright.map_or("", str::trim);
    let copyrighted = !(c.is_empty() || c.eq_ignore_ascii_case("none"));
    copyrighted || named(c) || source.is_some_and(named)
}

/// Estimated publication time of a backfilled value of `series` for
/// `date`, Unix milliseconds (UTC, US daylight time not applied):
///
/// | Family | Estimate |
/// |---|---|
/// | Weekly retail prices | the survey Monday, 22:00 (5 p.m. ET) |
/// | Residential heating oil and propane | the Wednesday after the survey Monday, 18:00 |
/// | Weekly petroleum supply | the Wednesday after the week ending Friday, 15:30 |
/// | Weekly natural gas storage | the Thursday after the week ending Friday, 15:30 |
/// | Monthly prices and supply | the first of the third month after the reference month, 15:00 |
#[must_use]
pub fn backfill_published_ms(series: &str, date: Date) -> i64 {
    const H: i64 = 3_600_000;
    let at = |d: Date, h: i64, m: i64| d.start_ms() + h * H + m * 60_000;
    match family(series) {
        Some(Family::WeeklyRetail) => at(date, 22, 0),
        Some(Family::ResidentialHeating) => at(date.add_days(2), 18, 0),
        Some(Family::WeeklySupply) => at(date.add_days(5), 15, 30),
        Some(Family::WeeklyStorage) => at(date.add_days(6), 15, 30),
        Some(Family::MonthlyPrice | Family::MonthlySupply) => {
            let months = date.y * 12 + i32::try_from(date.m).unwrap_or(1) - 1 + 3;
            let d = Date::new(
                months.div_euclid(12),
                u32::try_from(months.rem_euclid(12) + 1).unwrap_or(1),
                1,
            )
            .unwrap_or(date);
            at(d, 15, 0)
        }
        None => at(date, 20, 0),
    }
}

/// One series line of a bulk file (other keys are ignored).
#[derive(Debug, Deserialize)]
struct BulkSeries<'a> {
    series_id: String,
    #[serde(default)]
    units: Option<String>,
    #[serde(default)]
    f: Option<String>,
    #[serde(default)]
    copyright: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    last_updated: Option<String>,
    #[serde(borrow)]
    data: Vec<(&'a RawValue, &'a RawValue)>,
}

/// The series ID of a bulk line, without parsing the line: the string
/// after the first `"series_id"` key.
fn quick_series_id(line: &str) -> Option<&str> {
    let rest = &line[line.find("\"series_id\"")? + "\"series_id\"".len()..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    Some(&rest[..rest.find('"')?])
}

/// The text of a JSON scalar (quotes removed from strings).
fn scalar_text(v: &RawValue) -> &str {
    let t = v.get().trim();
    t.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(t)
}

/// Parses `last_updated` (`YYYY-MM-DDTHH:MM:SS±HH:MM`) to Unix
/// milliseconds.
#[must_use]
pub fn parse_updated(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = s.split_once('T')?;
    let date = Date::parse_iso(date)?;
    let (clock, offset_min) = if let Some(c) = rest.strip_suffix('Z') {
        (c, 0)
    } else {
        let at = rest.rfind(['+', '-'])?;
        let (c, off) = rest.split_at(at);
        let sign = if off.starts_with('-') { -1 } else { 1 };
        let (oh, om) = off[1..].split_once(':')?;
        (
            c,
            sign * (oh.parse::<i64>().ok()? * 60 + om.parse::<i64>().ok()?),
        )
    };
    let mut parts = clock.split(':');
    let h: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let sec: i64 = parts.next().map_or(Some(0), |x| x.parse().ok())?;
    if !(0..24).contains(&h) || !(0..60).contains(&m) || !(0..61).contains(&sec) {
        return None;
    }
    Some(date.start_ms() + ((h * 60 + m - offset_min) * 60 + sec) * 1000)
}

/// The date a period stands for: `YYYYMMDD` for weekly series (the date
/// the file gives), `YYYYMM` for monthly ones (the first of the month).
fn period_date(period: &str, weekly: bool) -> Option<Date> {
    let p = period.trim();
    if !p.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let num = |r: std::ops::Range<usize>| p.get(r).and_then(|s| s.parse::<u32>().ok());
    let y = i32::try_from(num(0..4)?).ok()?;
    match (weekly, p.len()) {
        (true, 8) => Date::new(y, num(4..6)?, num(6..8)?),
        (false, 6) => Date::new(y, num(4..6)?, 1),
        _ => None,
    }
}

/// What a bulk parse keeps.
#[derive(Debug, Clone, Copy)]
pub struct Scope {
    /// First year kept.
    pub from_year: i32,
    /// Last year kept.
    pub to_year: i32,
    /// Live poll: take `ts_source_ms` from `last_updated`.
    pub live: bool,
}

impl Scope {
    /// Every year, as a live poll would read them.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            from_year: i32::MIN,
            to_year: i32::MAX,
            live: true,
        }
    }
}

fn read_series(dataset: Dataset, line: &str, scope: Scope, out: &mut Parsed) {
    let Some(id) = quick_series_id(line) else {
        if line.contains("\"category_id\"") {
            return;
        }
        out.unknown.insert("bulk_line_shape".into());
        out.skip("malformed");
        return;
    };
    let prefix_ok = id
        .strip_prefix(dataset.code())
        .is_some_and(|r| r.starts_with('.'));
    if !prefix_ok || is_excluded_family(id) {
        out.skip("excluded");
        return;
    }
    let Some((fam, measure)) = entry(id) else {
        out.skip("out_of_scope");
        return;
    };
    let s = match serde_json::from_str::<BulkSeries<'_>>(line) {
        Ok(s) if s.series_id == id => s,
        _ => {
            out.unknown.insert(format!("series_shape:{id}"));
            out.skip("malformed");
            return;
        }
    };
    if credits_third_party(s.copyright.as_deref(), s.source.as_deref()) {
        out.unknown.insert(format!("vendor_credit:{id}"));
        out.skip("excluded");
        return;
    }
    let weekly = id.ends_with(".W");
    let freq = if weekly { "W" } else { "M" };
    if s.f.as_deref().is_some_and(|f| f.trim() != freq) {
        out.unknown.insert(format!("frequency:{id}"));
        out.skip("malformed");
        return;
    }
    // Prices are in dollars; a quantity's unit text must map to the
    // vocabulary and fit the series' measure (ADR 0023).
    let unit = if fam.is_price() {
        let dollars = s
            .units
            .as_deref()
            .is_some_and(|u| u.to_ascii_lowercase().contains("dollar"));
        dollars.then_some(None)
    } else {
        s.units
            .as_deref()
            .and_then(quantity_unit)
            .filter(|&u| unit_fits(measure, u, weekly))
            .map(Some)
    };
    let Some(unit) = unit else {
        out.unknown.insert(format!("units:{id}"));
        out.skip("malformed");
        return;
    };
    let published_ms = if scope.live {
        s.last_updated.as_deref().and_then(parse_updated)
    } else {
        None
    };
    let series = num::symbol_segment(id);
    for (period, value) in s.data {
        let Some(date) = period_date(scalar_text(period), weekly) else {
            out.skip("malformed");
            continue;
        };
        if date.y < scope.from_year || date.y > scope.to_year {
            continue;
        }
        let text = scalar_text(value);
        let parsed = if value.get().trim() == "null" || matches!(text, "W" | "NM" | "(s)") {
            Ok(None)
        } else {
            num::decimal(text)
        };
        match parsed {
            Ok(Some(v)) => {
                let mut r = match unit {
                    Some(u) => RefObs::quantity(series.clone(), measure, v, u, date),
                    None => RefObs::usd_price(series.clone(), v, date),
                };
                r.exchange_referenced = fam.exchange_referenced();
                r.published_ms = published_ms;
                out.obs.push(Obs::Ref(r));
            }
            Ok(None) => out.skip("missing"),
            Err(_) => out.skip("malformed"),
        }
    }
}

/// Parses one bulk file (`PET.zip` or `NG.zip`, or its text member),
/// keeping values dated within `scope`.
///
/// # Errors
///
/// Fails on a malformed archive, an unreadable body, or a file with no
/// series lines at all (the layout changed).
pub fn parse_bulk(dataset: Dataset, body: &[u8], scope: Scope) -> Result<Parsed, ParseError> {
    let mut out = Parsed::default();
    let mut lines = 0u64;
    let mut each = |reader: &mut dyn BufRead, out: &mut Parsed| -> Result<(), ParseError> {
        let mut buf = String::new();
        loop {
            buf.clear();
            if reader.read_line(&mut buf).map_err(ParseError::new)? == 0 {
                return Ok(());
            }
            let line = buf.trim();
            if line.is_empty() {
                continue;
            }
            lines += 1;
            if !line.starts_with('{') {
                out.unknown.insert("bulk_line_shape".into());
                out.skip("malformed");
                continue;
            }
            read_series(dataset, line, scope, out);
        }
    };
    if body.starts_with(b"PK\x03\x04") {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(body)).map_err(ParseError::new)?;
        for i in 0..z.len() {
            let f = z.by_index(i).map_err(ParseError::new)?;
            if f.is_dir() {
                continue;
            }
            let mut r = BufReader::with_capacity(1 << 20, f);
            each(&mut r, &mut out)?;
        }
    } else {
        each(&mut BufReader::new(body), &mut out)?;
    }
    let seen = out.obs.len() as u64 + out.skipped.values().sum::<u64>();
    if lines == 0 || seen == 0 {
        return Err(ParseError(format!(
            "{}: no series lines; the bulk file layout changed",
            dataset.code()
        )));
    }
    Ok(out)
}

/// The EIA reader.
#[derive(Debug)]
pub struct EiaReader {
    base: String,
}

impl EiaReader {
    /// A reader using `bases` (key `eia`).
    #[must_use]
    pub fn new(bases: &Bases) -> Self {
        Self {
            base: bases.get("eia", BASE),
        }
    }

    fn target(&self, dataset: Dataset, tag: &str, from: i32, to: i32) -> Target {
        let code = dataset.code();
        Target::poll(
            format!("eia:{code}:{tag}"),
            format!("{}/opendata/bulk/{code}.zip", self.base),
            format!("bulk:{code}:{tag}:{from}:{to}"),
        )
    }
}

impl Reader for EiaReader {
    fn id(&self) -> SourceId {
        SourceId::Eia
    }

    fn live_targets(&self, today: Date) -> Vec<Target> {
        // The live window is 400 days; years before it are the backfill's.
        Dataset::ALL
            .into_iter()
            .map(|d| self.target(d, "live", today.y - 2, 9999))
            .collect()
    }

    fn backfill_targets(&self, year: i32) -> Vec<Target> {
        // One file per dataset holds the whole history; the backfill
        // fetches it once and keeps the year's values.
        Dataset::ALL
            .into_iter()
            .map(|d| self.target(d, "history", year, year))
            .collect()
    }

    fn parse(&self, target: &Target, body: &[u8]) -> Result<Parsed, ParseError> {
        let bad = || ParseError(format!("bad target kind {}", target.kind));
        let mut parts = target.kind.split(':');
        if parts.next() != Some("bulk") {
            return Err(bad());
        }
        let dataset = parts.next().and_then(Dataset::from_code).ok_or_else(bad)?;
        let live = parts.next() == Some("live");
        let from_year = parts.next().and_then(|s| s.parse().ok()).ok_or_else(bad)?;
        let to_year = parts.next().and_then(|s| s.parse().ok()).ok_or_else(bad)?;
        parse_bulk(
            dataset,
            body,
            Scope {
                from_year,
                to_year,
                live,
            },
        )
    }

    fn schedule(&self) -> Schedule {
        // The bulk files are rebuilt at 5 a.m. and 3 p.m. ET (ready 30 to
        // 45 minutes later). The Weekly Petroleum Status Report (Wednesday
        // 10:30 a.m. ET) and natural gas storage (Thursday 10:30 a.m. ET)
        // reach the 3 p.m. build; retail prices (Monday) and the monthly
        // publications are caught by the daily poll. Each changed file is
        // about 60 MB, so polls outside the window are daily.
        Schedule {
            days: 0b0000_1100,
            from_min: 19 * 60 + 30,
            to_min: 23 * 60 + 30,
            inside: Duration::from_secs(1800),
            outside: Duration::from_secs(24 * 3600),
        }
    }

    fn revision_days(&self) -> u32 {
        400
    }

    fn live_window_days(&self) -> i64 {
        // Monthly survey prices are revised for several months.
        400
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_never_meets_the_excluded_families() {
        let mut seen = std::collections::BTreeSet::new();
        for (id, fam, measure) in ALLOWLIST {
            assert!(!is_excluded_family(id), "{id}");
            assert!(seen.insert(*id), "duplicate {id}");
            // Price families hold prices, volume families quantities.
            assert_eq!(fam.is_price(), *measure == ReferenceMeasure::Price, "{id}");
            assert_eq!(!fam.is_price(), measure.is_quantity(), "{id}");
            if *fam == Family::WeeklyStorage {
                assert_eq!(*measure, ReferenceMeasure::Stock, "{id}");
            }
            if *fam == Family::MonthlySupply {
                assert_eq!(*measure, ReferenceMeasure::Flow, "{id}");
            }
            assert_eq!(entry(id), Some((*fam, *measure)));
            assert!(id.starts_with("PET.") || id.starts_with("NG."), "{id}");
            let weekly = matches!(
                fam,
                Family::WeeklySupply
                    | Family::WeeklyStorage
                    | Family::WeeklyRetail
                    | Family::ResidentialHeating
            );
            assert_eq!(id.ends_with(".W"), weekly, "{id}");
            assert_eq!(id.ends_with(".M"), !weekly, "{id}");
            assert_eq!(family(id), Some(*fam));
        }
        for id in [
            "PET.RWTC.D",
            "PET.RBRTE.W",
            "PET.EER_EPMRU_PF4_Y35NY_DPG.D",
            "PET.EER_EPD2F_PE1_Y35NY_DPG.D",
            "PET.RCLC1.D",
            "PET.W_EPD2F_PWR_NUS_DPG.W",
            "PET.W_EPLLPA_PWR_NUS_DPG.W",
            "NG.RNGWHHD.D",
            "NG.RNGC1.D",
        ] {
            assert!(is_excluded_family(id), "{id}");
            assert_eq!(family(id), None, "{id}");
        }
    }

    #[test]
    fn units_map_to_the_vocabulary_and_fit_the_measure() {
        use QuantityUnit as U;
        assert_eq!(quantity_unit("Thousand Barrels"), Some(U::ThousandBarrels));
        assert_eq!(
            quantity_unit(" thousand barrels per day "),
            Some(U::ThousandBarrelsPerDay)
        );
        assert_eq!(
            quantity_unit("Thousand Barrels per Calendar Day"),
            Some(U::ThousandBarrelsPerDay)
        );
        assert_eq!(
            quantity_unit("Billion Cubic Feet"),
            Some(U::BillionCubicFeet)
        );
        assert_eq!(
            quantity_unit("Million Cubic Feet"),
            Some(U::MillionCubicFeet)
        );
        assert_eq!(quantity_unit("Percent"), Some(U::Percent));
        for t in [
            "",
            "Thousand Barrels per Stream Day",
            "Dollars per Gallon",
            "Mbbl",
            "Thousand Cubic Feet",
        ] {
            assert_eq!(quantity_unit(t), None, "{t:?}");
        }
        use ReferenceMeasure as M;
        assert!(unit_fits(M::Stock, U::ThousandBarrels, true));
        assert!(!unit_fits(M::Stock, U::ThousandBarrelsPerDay, true));
        assert!(unit_fits(M::Flow, U::ThousandBarrelsPerDay, true));
        assert!(
            !unit_fits(M::Flow, U::ThousandBarrels, true),
            "weekly flows are per day"
        );
        assert!(
            unit_fits(M::Flow, U::ThousandBarrels, false),
            "monthly totals"
        );
        assert!(unit_fits(M::Flow, U::MillionCubicFeet, false));
        assert!(unit_fits(M::Capacity, U::ThousandBarrelsPerDay, true));
        assert!(!unit_fits(M::Capacity, U::Percent, true));
        assert!(unit_fits(M::Ratio, U::Percent, true));
        assert!(!unit_fits(M::Ratio, U::ThousandBarrels, true));
        assert!(!unit_fits(M::Stock, U::Percent, true));
        assert!(!unit_fits(M::Price, U::Usd, true));
    }

    #[test]
    fn third_party_credits_are_recognised() {
        assert!(!credits_third_party(
            Some("None"),
            Some("EIA, U.S. Energy Information Administration")
        ));
        assert!(!credits_third_party(None, None));
        assert!(credits_third_party(Some("Thomson-Reuters"), Some("EIA")));
        assert!(credits_third_party(
            Some("None"),
            Some("Refinitiv, an LSEG business")
        ));
        assert!(credits_third_party(
            None,
            Some("Intercontinental Exchange (ICE)")
        ));
        // "price" and "service" contain "ice" but are not the word.
        assert!(!credits_third_party(
            Some("none"),
            Some("EIA price service")
        ));
    }

    #[test]
    fn periods_times_and_estimates() {
        assert_eq!(period_date("20261109", true), Date::new(2026, 11, 9));
        assert_eq!(period_date("202610", false), Date::new(2026, 10, 1));
        assert_eq!(period_date("202610", true), None);
        assert_eq!(period_date("2026", false), None);
        assert_eq!(period_date("2026Q3", false), None);
        let d = Date::new(2026, 11, 12).unwrap();
        assert_eq!(
            parse_updated("2026-11-12T10:35:44-05:00"),
            Some(d.start_ms() + (15 * 3600 + 35 * 60 + 44) * 1000)
        );
        assert_eq!(
            parse_updated("2026-11-12T15:35:44Z"),
            parse_updated("2026-11-12T10:35:44-05:00")
        );
        assert_eq!(parse_updated("yesterday"), None);
        let oct = Date::new(2026, 10, 1).unwrap();
        assert_eq!(
            backfill_published_ms("NG.N3050US3.M", oct),
            Date::new(2027, 1, 1).unwrap().start_ms() + 15 * 3_600_000
        );
        let fri = Date::new(2026, 11, 6).unwrap();
        assert_eq!(
            backfill_published_ms("PET.WCESTUS1.W", fri),
            Date::new(2026, 11, 11).unwrap().start_ms() + 15 * 3_600_000 + 30 * 60_000
        );
        assert_eq!(
            quick_series_id(r#"{"series_id": "PET.X.W","name":"y"}"#),
            Some("PET.X.W")
        );
        assert_eq!(quick_series_id(r#"{"category_id":"1"}"#), None);
    }
}
