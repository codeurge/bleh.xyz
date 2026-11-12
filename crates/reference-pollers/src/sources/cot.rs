//! CFTC Commitments of Traders (ADM-0005) from the cftc.gov static files
//! (channel A), as `positioning` records.
//!
//! Every report family in both forms is read. Columns are mapped by
//! header name, never by position: names are normalized
//! ([`super::norm_header`]) so the legacy spelling (`Noncommercial
//! Positions-Long (All)`) and the underscored spelling
//! (`M_Money_Positions_Long_All`) both match, including the CFTC's
//! occasional doubled underscores. The weekly files have no header and
//! take the column order of the same year's annual file, which live
//! polling therefore reads first; a weekly file whose width differs from
//! that header is rejected.
//!
//! Per row and category: `long`, `short`, `spreading` (where the category
//! reports it) and the row's `open_interest`. Combined
//! (futures-and-options) records carry `exchange_referenced`. The other
//! admitted columns (changes, percentages, trader counts, concentration
//! ratios) are not part of the `positioning` kind and are not emitted.
//! Market and exchange names are source metadata only; the instrument is
//! the contract market code.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use mesh_types::positioning::is_contract_market_code;
use mesh_types::{PositioningCategory as C, PositioningReport as R};

use super::{Bases, ParseError, Parsed, Reader, Schedule, Target, norm_header, unzip};
use crate::num;
use crate::obs::{Obs, PosObs};
use crate::source::SourceId;
use crate::time::Date;

/// Default cftc.gov base.
pub const BASE: &str = "https://www.cftc.gov";

/// One report file family in one form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Form {
    /// Legacy, futures only.
    LegacyFut,
    /// Legacy, combined.
    LegacyCom,
    /// Disaggregated, futures only.
    DisaggFut,
    /// Disaggregated, combined.
    DisaggCom,
    /// TFF, futures only.
    TffFut,
    /// TFF, combined.
    TffCom,
    /// Supplemental (commodity index traders), combined only.
    Cit,
}

impl Form {
    /// Every form.
    pub const ALL: [Form; 7] = [
        Form::LegacyFut,
        Form::LegacyCom,
        Form::DisaggFut,
        Form::DisaggCom,
        Form::TffFut,
        Form::TffCom,
        Form::Cit,
    ];

    /// Short name used in target kinds.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Form::LegacyFut => "legacy-fut",
            Form::LegacyCom => "legacy-com",
            Form::DisaggFut => "disagg-fut",
            Form::DisaggCom => "disagg-com",
            Form::TffFut => "tff-fut",
            Form::TffCom => "tff-com",
            Form::Cit => "cit",
        }
    }

    fn from_name(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == s)
    }

    /// Report family and whether the figures are combined.
    #[must_use]
    pub const fn family(self) -> (R, bool) {
        match self {
            Form::LegacyFut => (R::Legacy, false),
            Form::LegacyCom => (R::Legacy, true),
            Form::DisaggFut => (R::Disaggregated, false),
            Form::DisaggCom => (R::Disaggregated, true),
            Form::TffFut => (R::Tff, false),
            Form::TffCom => (R::Tff, true),
            Form::Cit => (R::Supplemental, true),
        }
    }

    const fn weekly_file(self) -> &'static str {
        match self {
            Form::LegacyFut => "deafut.txt",
            Form::LegacyCom => "deacom.txt",
            Form::DisaggFut => "f_disagg.txt",
            Form::DisaggCom => "c_disagg.txt",
            Form::TffFut => "FinFutWk.txt",
            Form::TffCom => "FinComWk.txt",
            Form::Cit => "deacit.txt",
        }
    }

    /// First year with data.
    #[must_use]
    pub const fn first_year(self) -> i32 {
        match self {
            Form::LegacyFut => 1986,
            Form::LegacyCom => 1995,
            _ => 2006,
        }
    }

    /// The historical file holding `year`.
    #[must_use]
    pub fn annual_file(self, year: i32) -> Option<String> {
        if year < self.first_year() {
            return None;
        }
        let hist = year <= 2016;
        Some(match self {
            Form::LegacyFut => format!("deacot{year}.zip"),
            Form::LegacyCom if year <= 2003 => format!("deahistfo_{year}.zip"),
            Form::LegacyCom => format!("deahistfo{year}.zip"),
            Form::DisaggFut if hist => "fut_disagg_txt_hist_2006_2016.zip".into(),
            Form::DisaggFut => format!("fut_disagg_txt_{year}.zip"),
            Form::DisaggCom if hist => "com_disagg_txt_hist_2006_2016.zip".into(),
            Form::DisaggCom => format!("com_disagg_txt_{year}.zip"),
            Form::TffFut if hist => "fin_fut_txt_2006_2016.zip".into(),
            Form::TffFut => format!("fut_fin_txt_{year}.zip"),
            Form::TffCom if hist => "fin_com_txt_2006_2016.zip".into(),
            Form::TffCom => format!("com_fin_txt_{year}.zip"),
            Form::Cit if hist => "dea_cit_txt_2006_2016.zip".into(),
            Form::Cit => format!("dea_cit_txt_{year}.zip"),
        })
    }

    /// Categories with their long, short and optional spreading columns
    /// (normalized names, without the trailing `_all`).
    #[must_use]
    pub fn categories(self) -> &'static [(C, &'static str, &'static str, Option<&'static str>)] {
        match self.family().0 {
            R::Legacy => &[
                (
                    C::Noncommercial,
                    "noncommercial_positions_long",
                    "noncommercial_positions_short",
                    Some("noncommercial_positions_spreading"),
                ),
                (
                    C::Commercial,
                    "commercial_positions_long",
                    "commercial_positions_short",
                    None,
                ),
                (
                    C::TotalReportable,
                    "total_reportable_positions_long",
                    "total_reportable_positions_short",
                    None,
                ),
                (
                    C::Nonreportable,
                    "nonreportable_positions_long",
                    "nonreportable_positions_short",
                    None,
                ),
            ],
            R::Disaggregated => &[
                (
                    C::ProducerMerchant,
                    "prod_merc_positions_long",
                    "prod_merc_positions_short",
                    None,
                ),
                (
                    C::SwapDealer,
                    "swap_positions_long",
                    "swap_positions_short",
                    Some("swap_positions_spread"),
                ),
                (
                    C::ManagedMoney,
                    "m_money_positions_long",
                    "m_money_positions_short",
                    Some("m_money_positions_spread"),
                ),
                (
                    C::OtherReportable,
                    "other_rept_positions_long",
                    "other_rept_positions_short",
                    Some("other_rept_positions_spread"),
                ),
                (
                    C::TotalReportable,
                    "tot_rept_positions_long",
                    "tot_rept_positions_short",
                    None,
                ),
                (
                    C::Nonreportable,
                    "nonrept_positions_long",
                    "nonrept_positions_short",
                    None,
                ),
            ],
            R::Tff => &[
                (
                    C::Dealer,
                    "dealer_positions_long",
                    "dealer_positions_short",
                    Some("dealer_positions_spread"),
                ),
                (
                    C::AssetManager,
                    "asset_mgr_positions_long",
                    "asset_mgr_positions_short",
                    Some("asset_mgr_positions_spread"),
                ),
                (
                    C::LeveragedFunds,
                    "lev_money_positions_long",
                    "lev_money_positions_short",
                    Some("lev_money_positions_spread"),
                ),
                (
                    C::OtherReportable,
                    "other_rept_positions_long",
                    "other_rept_positions_short",
                    Some("other_rept_positions_spread"),
                ),
                (
                    C::TotalReportable,
                    "tot_rept_positions_long",
                    "tot_rept_positions_short",
                    None,
                ),
                (
                    C::Nonreportable,
                    "nonrept_positions_long",
                    "nonrept_positions_short",
                    None,
                ),
            ],
            R::Supplemental => &[
                (
                    C::Noncommercial,
                    "ncomm_positions_long_all_nocit",
                    "ncomm_positions_short_all_nocit",
                    Some("ncomm_positions_spread_all_nocit"),
                ),
                (
                    C::Commercial,
                    "comm_positions_long_all_nocit",
                    "comm_positions_short_all_nocit",
                    None,
                ),
                (
                    C::IndexTraders,
                    "cit_positions_long",
                    "cit_positions_short",
                    None,
                ),
                (
                    C::TotalReportable,
                    "tot_rept_positions_long",
                    "tot_rept_positions_short",
                    None,
                ),
                (
                    C::Nonreportable,
                    "nonrept_positions_long",
                    "nonrept_positions_short",
                    None,
                ),
            ],
        }
    }
}

/// Column index of a normalized name, trying it as given, with `_all`
/// appended and without a trailing `_all`.
fn col(index: &HashMap<String, usize>, name: &str) -> Option<usize> {
    index
        .get(name)
        .or_else(|| index.get(&format!("{name}_all")))
        .or_else(|| name.strip_suffix("_all").and_then(|n| index.get(n)))
        .copied()
}

/// Parses rows of one form given its header.
///
/// # Errors
///
/// Fails if the header lacks the date, code or open-interest column, or
/// a category's long or short column.
pub fn parse_rows(
    form: Form,
    header: &[String],
    rows: &[Vec<String>],
) -> Result<Parsed, ParseError> {
    let index: HashMap<String, usize> = header
        .iter()
        .enumerate()
        .map(|(i, h)| (norm_header(h), i))
        .collect();
    let date_col = [
        "as_of_date_in_form_yyyy_mm_dd",
        "report_date_as_yyyy_mm_dd",
        "as_of_date_in_form_yymmdd",
    ]
    .iter()
    .find_map(|n| col(&index, n))
    .ok_or_else(|| ParseError(format!("{}: no report date column", form.name())))?;
    let code_col = col(&index, "cftc_contract_market_code")
        .ok_or_else(|| ParseError(format!("{}: no contract market code column", form.name())))?;
    let oi_col = col(&index, "open_interest_all")
        .ok_or_else(|| ParseError(format!("{}: no open interest column", form.name())))?;
    let mut cats = Vec::new();
    for (cat, long, short, spread) in form.categories() {
        let (Some(l), Some(s)) = (col(&index, long), col(&index, short)) else {
            return Err(ParseError(format!(
                "{}: header lacks the {cat:?} long or short column",
                form.name()
            )));
        };
        cats.push((*cat, l, s, spread.and_then(|n| col(&index, n))));
    }
    let (report, combined) = form.family();
    let mut out = Parsed::default();
    for row in rows {
        let cell = |i: usize| row.get(i).map_or("", |s| s.trim());
        let (Some(date), code) = (
            Date::parse_any(cell(date_col)),
            cell(code_col).to_ascii_uppercase(),
        ) else {
            out.skip("malformed");
            continue;
        };
        if !is_contract_market_code(&code) {
            out.skip("malformed");
            continue;
        }
        let Ok(Some(oi)) = num::count(cell(oi_col)) else {
            out.skip("malformed");
            continue;
        };
        for (cat, l, s, sp) in &cats {
            match (num::count(cell(*l)), num::count(cell(*s))) {
                (Ok(Some(long)), Ok(Some(short))) => {
                    let spreading = sp.and_then(|i| num::count(cell(i)).ok().flatten());
                    out.obs.push(Obs::Pos(PosObs {
                        code: code.clone(),
                        report,
                        combined,
                        as_of: date,
                        category: *cat,
                        long,
                        short,
                        spreading,
                        open_interest: oi,
                    }));
                }
                (Ok(None), _) | (_, Ok(None)) => out.skip("missing"),
                _ => out.skip("malformed"),
            }
        }
    }
    Ok(out)
}

fn csv_rows(text: &[u8]) -> Result<Vec<Vec<String>>, ParseError> {
    let mut r = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(text);
    let mut out = Vec::new();
    for rec in r.records() {
        let rec = rec.map_err(ParseError::new)?;
        if rec.iter().all(str::is_empty) {
            continue;
        }
        out.push(rec.iter().map(str::to_owned).collect());
    }
    Ok(out)
}

/// The COT reader.
#[derive(Debug)]
pub struct CotReader {
    base: String,
    headers: Mutex<HashMap<Form, Vec<String>>>,
}

impl CotReader {
    /// A reader using `bases` (key `cftc`).
    #[must_use]
    pub fn new(bases: &Bases) -> Self {
        Self {
            base: bases.get("cftc", BASE),
            headers: Mutex::new(HashMap::new()),
        }
    }

    fn annual(&self, form: Form, year: i32, tag: &str) -> Option<Target> {
        let file = form.annual_file(year)?;
        Some(Target::poll(
            format!("cot:{}:{file}:{tag}", form.name()),
            format!("{}/files/dea/history/{file}", self.base),
            format!("annual:{}", form.name()),
        ))
    }

    /// Parses an annual (zipped, with header) file.
    ///
    /// # Errors
    ///
    /// Fails on a malformed archive or header.
    pub fn parse_annual(&self, form: Form, body: &[u8]) -> Result<Parsed, ParseError> {
        let mut out = Parsed::default();
        let members = if body.starts_with(b"PK\x03\x04") {
            unzip(body)?
        } else {
            vec![("file".to_owned(), body.to_vec())]
        };
        for (_, text) in members {
            let mut rows = csv_rows(&text)?;
            if rows.is_empty() {
                continue;
            }
            let header = rows.remove(0);
            let p = parse_rows(form, &header, &rows)?;
            if let Ok(mut h) = self.headers.lock() {
                h.insert(form, header);
            }
            out.obs.extend(p.obs);
            for (k, v) in p.skipped {
                *out.skipped.entry(k).or_default() += v;
            }
        }
        Ok(out)
    }

    /// Parses a weekly (header-less) file with the header learned from the
    /// annual file.
    ///
    /// # Errors
    ///
    /// Fails if no header is known yet or the width differs.
    pub fn parse_weekly(&self, form: Form, body: &[u8]) -> Result<Parsed, ParseError> {
        let header = self
            .headers
            .lock()
            .ok()
            .and_then(|h| h.get(&form).cloned())
            .ok_or_else(|| {
                ParseError(format!(
                    "{}: no header from the annual file yet",
                    form.name()
                ))
            })?;
        let rows = csv_rows(body)?;
        if let Some(r) = rows.iter().find(|r| r.len() != header.len()) {
            return Err(ParseError(format!(
                "{}: weekly row has {} columns, the annual header {}",
                form.name(),
                r.len(),
                header.len()
            )));
        }
        parse_rows(form, &header, &rows)
    }
}

impl Reader for CotReader {
    fn id(&self) -> SourceId {
        SourceId::Cot
    }

    fn live_targets(&self, today: Date) -> Vec<Target> {
        let mut out = Vec::new();
        for form in Form::ALL {
            if let Some(t) = self.annual(form, today.y, "live") {
                out.push(t);
            }
            out.push(Target::poll(
                format!("cot:{}:weekly", form.name()),
                format!("{}/dea/newcot/{}", self.base, form.weekly_file()),
                format!("weekly:{}", form.name()),
            ));
        }
        out
    }

    fn backfill_targets(&self, year: i32) -> Vec<Target> {
        Form::ALL
            .into_iter()
            .filter_map(|f| self.annual(f, year, "history"))
            .collect()
    }

    fn parse(&self, target: &Target, body: &[u8]) -> Result<Parsed, ParseError> {
        let (kind, form) = target
            .kind
            .split_once(':')
            .ok_or_else(|| ParseError(format!("bad target kind {}", target.kind)))?;
        let form = Form::from_name(form).ok_or_else(|| ParseError(format!("bad form {form}")))?;
        match kind {
            "annual" => self.parse_annual(form, body),
            "weekly" => self.parse_weekly(form, body),
            other => Err(ParseError(format!("bad target kind {other}"))),
        }
    }

    fn schedule(&self) -> Schedule {
        // Released Fridays at about 3:30 p.m. ET; holiday weeks shift.
        Schedule {
            days: 0b0001_0000,
            from_min: 19 * 60 + 30,
            to_min: 23 * 60 + 59,
            inside: Duration::from_secs(900),
            outside: Duration::from_secs(12 * 3600),
        }
    }

    fn revision_days(&self) -> u32 {
        400
    }

    fn live_window_days(&self) -> i64 {
        // The annual file of the current year is read live; its earlier
        // weeks are emitted again only if the CFTC revises them.
        400
    }
}
