//! The series catalogue: the registry entries for reference series.
//!
//! One entry per series family the pollers emit, with the admission, the
//! unit, the flags it always carries, the source notice (ADR 0012,
//! decision 5: notices travel in the registry entry and the manifest) and
//! the source's method periods where the admission asks for them (the
//! Treasury curve method, the EFFR method change). Printed by
//! `reference-pollers catalog` and published beside the archive as the
//! reference registry.

use serde::Serialize;

use crate::source::{SourceId, nyfed_notice};

/// A method in force over a date range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MethodPeriod {
    /// First date, `YYYY-MM-DD`.
    pub from: &'static str,
    /// Last date, or `None` while current.
    pub to: Option<&'static str>,
    /// The method as the source describes it.
    pub method: &'static str,
}

/// One catalogue entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    /// Instrument, or a pattern with `<...>` placeholders.
    pub instrument: String,
    /// Record kind.
    pub kind: &'static str,
    /// Source name.
    pub source: &'static str,
    /// Admission.
    pub admission: &'static str,
    /// What the series is.
    pub description: &'static str,
    /// Unit of `value` (or of the counts).
    pub unit: &'static str,
    /// Flags every record carries.
    pub flags: Vec<&'static str>,
    /// Notice travelling with the series.
    pub notice: String,
    /// Method periods, where the admission asks for them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<MethodPeriod>,
}

fn entry(
    source: SourceId,
    symbol: &str,
    kind: &'static str,
    description: &'static str,
    unit: &'static str,
    flags: &[&'static str],
) -> Entry {
    let class = if kind == "positioning" {
        "positioning"
    } else {
        "ref"
    };
    Entry {
        instrument: format!("{}:{class}:{symbol}", source.venue_str()),
        kind,
        source: source.name(),
        admission: source.admission_str(),
        description,
        unit,
        flags: flags.to_vec(),
        notice: source
            .notices()
            .into_iter()
            .next()
            .map(|n| n.notice)
            .unwrap_or_default(),
        methods: Vec::new(),
    }
}

const PCT: &str = "percent per annum";
const XR: &[&str] = &["exchange_referenced"];

/// Every catalogue entry.
#[must_use]
pub fn entries() -> Vec<Entry> {
    use SourceId as S;
    let rv = "reference_value";
    let mut v = vec![
        entry(
            S::Cot,
            "<contract market code>",
            "positioning",
            "Commitments of Traders positions by report family, form and trader category; combined (futures and options) records carry exchange_referenced",
            "contracts (futures equivalents when combined)",
            &[],
        ),
        {
            let mut e = entry(
                S::TreasuryRates,
                "par_yield_curve",
                rv,
                "Daily Treasury par yield curve, one record per tenor",
                PCT,
                &[],
            );
            e.methods = vec![
                MethodPeriod {
                    from: "1990-01-02",
                    to: Some("2021-12-03"),
                    method: "quasi-cubic Hermite spline",
                },
                MethodPeriod {
                    from: "2021-12-06",
                    to: None,
                    method: "monotone convex",
                },
            ];
            e
        },
        entry(
            S::TreasuryRates,
            "real_par_yield_curve",
            rv,
            "Daily Treasury par real yield curve",
            PCT,
            &[],
        ),
        entry(
            S::TreasuryRates,
            "bill_rates_discount",
            rv,
            "Daily Treasury bill rates, bank discount basis, with the input bill's maturity",
            PCT,
            &[],
        ),
        entry(
            S::TreasuryRates,
            "bill_rates_coupon_equivalent",
            rv,
            "Daily Treasury bill rates, coupon equivalent",
            PCT,
            &[],
        ),
        entry(
            S::TreasuryRates,
            "cmt_20y",
            rv,
            "Treasury 20-year constant maturity (long-term rates feed)",
            PCT,
            &[],
        ),
        entry(
            S::TreasuryAuctions,
            "auction.<TYPE>.<TERM>.<field>",
            rv,
            "Treasury auction results: high, low and median yields, discount and investment rates and prices, and the coupon, dated by auction date with the security's maturity",
            "percent per annum; prices per 100 par",
            &[],
        ),
    ];
    for (sym, d) in [
        ("SOFR", "Secured Overnight Financing Rate"),
        ("BGCR", "Broad General Collateral Rate"),
        ("TGCR", "Tri-Party General Collateral Rate"),
        ("EFFR", "Effective Federal Funds Rate"),
        ("OBFR", "Overnight Bank Funding Rate"),
    ] {
        let mut e = entry(S::NyFed, sym, rv, "", PCT, &[]);
        e.description = match sym {
            "SOFR" => "Secured Overnight Financing Rate; percentiles under .P1, .P25, .P75, .P99",
            "BGCR" => "Broad General Collateral Rate; percentiles under .P1, .P25, .P75, .P99",
            "TGCR" => "Tri-Party General Collateral Rate; percentiles under .P1, .P25, .P75, .P99",
            "EFFR" => {
                "Effective Federal Funds Rate; percentiles under .P1 to .P99; FOMC target range under .TARGET_FROM and .TARGET_TO"
            }
            _ => "Overnight Bank Funding Rate; percentiles under .P1, .P25, .P75, .P99",
        };
        e.notice = nyfed_notice(&format!("The {d} ({sym})"));
        if sym == "EFFR" {
            e.methods = vec![
                MethodPeriod {
                    from: "2000-07-03",
                    to: Some("2016-02-29"),
                    method: "volume-weighted mean of brokered trades",
                },
                MethodPeriod {
                    from: "2016-03-01",
                    to: None,
                    method: "volume-weighted median of FR 2420 transactions",
                },
            ];
        }
        v.push(e);
    }
    let mut avg = entry(
        S::NyFed,
        "SOFR.<AVG30|AVG90|AVG180|INDEX>",
        rv,
        "SOFR Averages (compounded, annualized simple act/360) and SOFR Index",
        "percent per annum; index level",
        &[],
    );
    avg.notice = nyfed_notice("The SOFR Averages and SOFR Index");
    v.push(avg);
    v.extend([
        entry(S::FrbH15, "<SERIES_NAME>", rv, "H.15 admitted business-daily series by SERIES_NAME (federal funds, AA commercial paper, discount window, bills, constant maturities); RIFSPFF_N.B is EFFR and carries the New York Fed notice", PCT, &[]),
        entry(S::FrbH10, "<SERIES_NAME, $ written @>", rv, "H.10 certified noon buying rates; RXI@US series in US dollars per unit, RXI series in units per US dollar", "currency units", &[]),
        entry(S::UsdaLmr, "LM_HG201.NEGOTIATED.<field>", rv, "Negotiated barrow and gilt prices (LM_HG201)", "US dollars per hundredweight", &[]),
        entry(S::UsdaLmr, "LM_HG201.<NEGOTIATED_FORMULA|SWINE_PORK_MARKET_FORMULA|COMBINED>.<field>", rv, "Swine formula and combined prices (LM_HG201)", "US dollars per hundredweight", XR),
        entry(S::UsdaLmr, "LM_CT100.<CLASS>.<BASIS>.<GRADE>.<LOW>_<HIGH>.<field>", rv, "Negotiated fed cattle prices (LM_CT100 Detail)", "US dollars per hundredweight", &[]),
        entry(S::UsdaDairy, "NDPSR.<field>", rv, "National Dairy Products Sales Report weighted average prices, revised up to four weeks", "US dollars per pound", XR),
        entry(S::UsdaDairy, "FMMO.<field>", rv, "Announced class and component prices, dated the first of the month", "US dollars per hundredweight (classes) or per pound (components)", XR),
        entry(S::UsdaFeeder, "LS850.<CLASS>.<GRADE>.<LOW>_<HIGH>[.<NOTE>].<field>", rv, "National feeder and stocker cattle summary sale prices", "US dollars per hundredweight", &[]),
        entry(S::Ercot, "<DAM_SPP|DAM_LMP|DAM_MCPC|RT_SPP|RT_LMP|RT_MCPC|RT_MCPC_SCED>.<name>", rv, "ERCOT day-ahead and real-time settlement point prices, LMPs and ancillary service clearing prices, per delivery interval", "US dollars per MWh", &[]),
        entry(S::Eia, "PET.<EMM_*|EMD_*>_PTE_<area>_DPG.W", rv, "EIA weekly retail gasoline (EIA-878) and on-highway diesel (EIA-888) prices by grade and area, dated the survey Monday; bulk series ID", "US dollars per gallon", &[]),
        entry(S::Eia, "PET.W_<EPD2F|EPLLPA>_PRS_NUS_DPG.W", rv, "EIA weekly residential heating oil and propane prices (State Heating Oil and Propane Program, heating season), dated the survey Monday", "US dollars per gallon", &[]),
        entry(S::Eia, "PET.EMA_<product>_<sales type>_NUS_DPG.M", rv, "EIA monthly refiner motor gasoline, No. 2 distillate, diesel and propane prices by sales type, dated the first of the month", "US dollars per gallon", XR),
        entry(S::Eia, "PET.<F<area>__3|R1300____3>.M", rv, "EIA monthly domestic crude oil first purchase prices by area, and the refiner acquisition cost of imported crude oil, dated the first of the month", "US dollars per barrel", XR),
        entry(S::Eia, "NG.<N3050US3|N3010US3|N3020US3|N3035US3|N3045US3>.M", rv, "EIA monthly natural gas city-gate, residential, commercial, industrial and electric power prices, dated the first of the month", "US dollars per thousand cubic feet", XR),
        entry(S::Bls, "CU<S|U><R|S><area><item>", rv, "CPI for All Urban Consumers (CPI-U), seasonally adjusted (S) and not (U), by item and area, monthly, dated the first of the month; CUUR0000SA0 is the all-items U.S. city average index that TIPS accrue against", "index level (base period in the BLS series metadata, mostly 1982-84=100)", &[]),
        entry(S::Bls, "CW<S|U><R|S><area><item>", rv, "CPI for Urban Wage Earners and Clerical Workers (CPI-W), by item and area, monthly", "index level (base period in the BLS series metadata, mostly 1982-84=100)", &[]),
        entry(S::Bls, "SUUR<area><item>", rv, "Chained CPI for All Urban Consumers (C-CPI-U), not seasonally adjusted, monthly; initial and interim values are revised to final, as later revisions", "index level (December 1999=100)", &[]),
        entry(S::Bls, "APU<area><item>", rv, "BLS average retail prices for selected food, household fuel and motor fuel items, monthly", "US dollars per the unit in the BLS item name (per pound, gallon, therm, kWh, dozen)", &[]),
    ]);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_source_is_catalogued_with_a_notice() {
        let e = entries();
        for s in SourceId::ALL {
            assert!(e.iter().any(|x| x.source == s.name()), "{s}");
        }
        assert!(e.iter().all(|x| !x.notice.is_empty()));
        let sofr = e.iter().find(|x| x.instrument == "nyfed:ref:SOFR").unwrap();
        assert!(
            sofr.notice
                .starts_with("The Secured Overnight Financing Rate (SOFR) is subject")
        );
    }
}
