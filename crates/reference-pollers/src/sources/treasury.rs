//! US Treasury (ADM-0006): daily interest rate statistics from
//! home.treasury.gov, and auction results from the Fiscal Data API.
//! treasurydirect.gov is never fetched.
//!
//! ## Rate feeds (OData Atom XML)
//!
//! | Feed (`data=`) | Series (`us-treasury:ref:...`) | Measure |
//! |---|---|---|
//! | `daily_treasury_yield_curve` | `par_yield_curve`, one record per `BC_*` tenor | par yield, semi-annual |
//! | `daily_treasury_real_yield_curve` | `real_par_yield_curve`, per `TC_*` tenor | par yield, semi-annual |
//! | `daily_treasury_bill_rates` | `bill_rates_discount` (`ROUND_B1_CLOSE_{n}WK_2`), `bill_rates_coupon_equivalent` (`ROUND_B1_YIELD_{n}WK_2`), tenor `{n}W`, maturity `MATURITY_DATE_{n}WK` | discount act/360; coupon equivalent simple act/365 (52 weeks: semi-annual) |
//! | `daily_treasury_long_term_rate` | `cmt_20y` (the 20-year CMT row) | par yield, semi-annual, `20Y` |
//!
//! Unknown `BC_*` and `TC_*` elements of the form `BC_<n>MONTH` or
//! `BC_<n>YEAR` are read as new tenors and reported; other unknown
//! elements are reported and not emitted. The 30-year display duplicate,
//! the `CS_*` averages (not in the admission's field list), CUSIPs (no
//! record field), the long-term composite, the extrapolation factor and
//! the real long-term average (no tenor) are not emitted. The curve
//! method changed on 2021-12-06 (see the series catalogue).
//!
//! ## Auctions (Fiscal Data `auctions_query`)
//!
//! Series `auction.<TYPE>.<TERM>.<field>` per security type and term
//! (`TIPS` when `ref_cpi_on_issue_date` is set, `FRN` when
//! `frn_index_determination_rate` is set), dated by `auction_date`, with
//! the security's `maturity_date`: high, low and median yields, discount
//! rates, investment rates and prices, and the coupon (`int_rate`).
//! Amounts, bid-to-cover and bidder classes are admitted but are not
//! rates or prices; they are counted as skipped. Pages are followed until
//! `links.next` is null.

use std::time::Duration;

use mesh_types::{Compounding, DayCount, ReferenceMeasure};
use serde_json::Value;

use super::{Bases, ParseError, Parsed, Reader, Schedule, Target, WEEKDAYS};
use crate::num;
use crate::obs::{Obs, RefObs};
use crate::source::SourceId;
use crate::time::Date;
use crate::xml::{self, Node};

/// Default home.treasury.gov base.
pub const RATES_BASE: &str = "https://home.treasury.gov";
/// Default Fiscal Data API base.
pub const FISCAL_BASE: &str = "https://api.fiscaldata.treasury.gov/services/api/fiscal_service";

/// The five rate feeds and the first year each holds.
pub const FEEDS: [(&str, i32); 5] = [
    ("daily_treasury_yield_curve", 1990),
    ("daily_treasury_real_yield_curve", 2003),
    ("daily_treasury_bill_rates", 2002),
    ("daily_treasury_long_term_rate", 2000),
    ("daily_treasury_real_long_term", 2000),
];

const BILL_WEEKS: [u32; 7] = [4, 6, 8, 13, 17, 26, 52];

/// The rate-feed reader.
#[derive(Debug, Clone)]
pub struct RatesReader {
    base: String,
}

impl RatesReader {
    /// A reader using `bases` (key `treasury`).
    #[must_use]
    pub fn new(bases: &Bases) -> Self {
        Self {
            base: bases.get("treasury", RATES_BASE),
        }
    }

    fn year_targets(&self, year: i32, tag: &str) -> Vec<Target> {
        FEEDS
            .iter()
            .filter(|(_, first)| year >= *first)
            .map(|(feed, _)| {
                Target::poll(
                    format!("treasury:{feed}:{tag}"),
                    format!(
                        "{}/resource-center/data-chart-center/interest-rates/pages/xml?data={feed}&field_tdr_date_value={year}",
                        self.base
                    ),
                    *feed,
                )
            })
            .collect()
    }
}

/// Tenor of a `BC_*` or `TC_*` element name (`BC_10YEAR` is `10Y`,
/// `BC_1_5MONTH` is `6W`).
#[must_use]
pub fn curve_tenor(name: &str) -> Option<String> {
    let rest = name
        .strip_prefix("BC_")
        .or_else(|| name.strip_prefix("TC_"))?;
    if rest == "1_5MONTH" {
        return Some("6W".into());
    }
    let (n, unit) = if let Some(n) = rest.strip_suffix("MONTH") {
        (n, "M")
    } else {
        (rest.strip_suffix("YEAR")?, "Y")
    };
    let n: u32 = n.parse().ok()?;
    (1..=999).contains(&n).then(|| format!("{n}{unit}"))
}

const KNOWN_CURVE: [&str; 20] = [
    "BC_1MONTH",
    "BC_1_5MONTH",
    "BC_2MONTH",
    "BC_3MONTH",
    "BC_4MONTH",
    "BC_6MONTH",
    "BC_1YEAR",
    "BC_2YEAR",
    "BC_3YEAR",
    "BC_5YEAR",
    "BC_7YEAR",
    "BC_10YEAR",
    "BC_20YEAR",
    "BC_30YEAR",
    "TC_5YEAR",
    "TC_7YEAR",
    "TC_10YEAR",
    "TC_20YEAR",
    "TC_30YEAR",
    "BC_30YEARDISPLAY",
];

/// Reads every `m:properties` block of an Atom feed as `(name, text)`
/// pairs, element names with prefixes dropped.
///
/// # Errors
///
/// Fails on malformed XML.
pub fn feed_entries(body: &[u8]) -> Result<Vec<Vec<(String, String)>>, ParseError> {
    let mut entries = Vec::new();
    let mut cur: Option<Vec<(String, String)>> = None;
    let mut field: Option<String> = None;
    xml::walk::<ParseError>(body, |n| {
        match n {
            Node::Start {
                name: "properties", ..
            } => cur = Some(Vec::new()),
            Node::End { name: "properties" } => {
                if let Some(c) = cur.take() {
                    entries.push(c);
                }
            }
            Node::Start { name, attrs } if cur.is_some() => {
                field = (xml::attr(attrs, "null") != Some("true")).then(|| name.to_owned());
            }
            Node::Text(t) => {
                if let (Some(c), Some(f)) = (cur.as_mut(), field.as_ref()) {
                    c.push((f.clone(), t.to_owned()));
                }
            }
            Node::End { .. } => field = None,
            _ => {}
        }
        Ok(())
    })?;
    Ok(entries)
}

/// Parses one rate feed.
///
/// # Errors
///
/// Fails on malformed XML or an unknown feed.
pub fn parse_feed(feed: &str, body: &[u8]) -> Result<Parsed, ParseError> {
    let mut out = Parsed::default();
    for entry in feed_entries(body)? {
        let get = |k: &str| entry.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        match feed {
            "daily_treasury_yield_curve" | "daily_treasury_real_yield_curve" => {
                let Some(date) = get("NEW_DATE").and_then(Date::parse_iso) else {
                    out.skip("malformed");
                    continue;
                };
                let (series, prefix) = if feed == "daily_treasury_yield_curve" {
                    ("par_yield_curve", "BC_")
                } else {
                    ("real_par_yield_curve", "TC_")
                };
                for (name, text) in &entry {
                    if !name.starts_with(prefix) {
                        if !matches!(name.as_str(), "NEW_DATE" | "Id") {
                            out.unknown.insert(name.clone());
                        }
                        continue;
                    }
                    if name == "BC_30YEARDISPLAY" {
                        continue;
                    }
                    let Some(tenor) = curve_tenor(name) else {
                        out.unknown.insert(name.clone());
                        continue;
                    };
                    if !KNOWN_CURVE.contains(&name.as_str()) {
                        out.unknown.insert(name.clone());
                    }
                    match num::decimal(text) {
                        Ok(Some(v)) => out.obs.push(Obs::Ref(RefObs::rate(
                            series.into(),
                            ReferenceMeasure::ParYield,
                            v,
                            &tenor,
                            Compounding::SemiAnnual,
                            None,
                            date,
                        ))),
                        Ok(None) => out.skip("missing"),
                        Err(_) => out.skip("malformed"),
                    }
                }
            }
            "daily_treasury_bill_rates" => {
                let Some(date) = get("QUOTE_DATE")
                    .or_else(|| get("INDEX_DATE"))
                    .and_then(Date::parse_iso)
                else {
                    out.skip("malformed");
                    continue;
                };
                for w in BILL_WEEKS {
                    let tenor = format!("{w}W");
                    let maturity = get(&format!("MATURITY_DATE_{w}WK")).and_then(Date::parse_iso);
                    for (field, series) in [
                        (format!("ROUND_B1_CLOSE_{w}WK_2"), "bill_rates_discount"),
                        (
                            format!("ROUND_B1_YIELD_{w}WK_2"),
                            "bill_rates_coupon_equivalent",
                        ),
                    ] {
                        let Some(text) = get(&field) else { continue };
                        let (comp, dc) = match (series, w) {
                            ("bill_rates_discount", _) => {
                                (Compounding::Discount, Some(DayCount::Act360))
                            }
                            (_, 52) => (Compounding::SemiAnnual, None),
                            _ => (Compounding::Simple, Some(DayCount::Act365Fixed)),
                        };
                        match num::decimal(text) {
                            Ok(Some(v)) => {
                                let mut r = RefObs::rate(
                                    series.into(),
                                    ReferenceMeasure::Rate,
                                    v,
                                    &tenor,
                                    comp,
                                    dc,
                                    date,
                                );
                                r.maturity_date = maturity;
                                out.obs.push(Obs::Ref(r));
                            }
                            Ok(None) => out.skip("missing"),
                            Err(_) => out.skip("malformed"),
                        }
                    }
                }
                for (name, _) in &entry {
                    let known = matches!(name.as_str(), "INDEX_DATE" | "QUOTE_DATE" | "Id")
                        || name.starts_with("ROUND_B1_")
                        || name.starts_with("CUSIP_")
                        || name.starts_with("MATURITY_DATE_");
                    if name.starts_with("CS_") {
                        // Not in ADM-0006's field list: never emitted.
                        out.skip("excluded");
                    } else if !known {
                        out.unknown.insert(name.clone());
                    }
                }
            }
            "daily_treasury_long_term_rate" => {
                let Some(date) = get("QUOTE_DATE").and_then(Date::parse_iso) else {
                    out.skip("malformed");
                    continue;
                };
                let kind = get("RATE_TYPE").unwrap_or_default().to_ascii_uppercase();
                if kind.contains("20") && !kind.contains("COMPOSITE") {
                    match get("RATE").map(num::decimal) {
                        Some(Ok(Some(v))) => out.obs.push(Obs::Ref(RefObs::rate(
                            "cmt_20y".into(),
                            ReferenceMeasure::ParYield,
                            v,
                            "20Y",
                            Compounding::SemiAnnual,
                            None,
                            date,
                        ))),
                        Some(Err(_)) => out.skip("malformed"),
                        _ => out.skip("missing"),
                    }
                } else {
                    out.skip("unsupported_no_tenor");
                }
            }
            "daily_treasury_real_long_term" => out.skip("unsupported_no_tenor"),
            other => return Err(ParseError(format!("unknown feed {other}"))),
        }
    }
    Ok(out)
}

impl Reader for RatesReader {
    fn id(&self) -> SourceId {
        SourceId::TreasuryRates
    }

    fn live_targets(&self, today: Date) -> Vec<Target> {
        let mut t = self.year_targets(today.y, "live");
        // Early January: the previous year's last values may still change.
        if today.m == 1 && today.d <= 10 {
            t.extend(self.year_targets(today.y - 1, "live-prev"));
        }
        t
    }

    fn backfill_targets(&self, year: i32) -> Vec<Target> {
        self.year_targets(year, &year.to_string())
    }

    fn parse(&self, target: &Target, body: &[u8]) -> Result<Parsed, ParseError> {
        parse_feed(&target.kind, body)
    }

    fn schedule(&self) -> Schedule {
        // Curve rates are usually up by 6:00 p.m. ET (22:00 or 23:00 UTC).
        Schedule {
            days: WEEKDAYS,
            from_min: 21 * 60 + 30,
            to_min: 3 * 60,
            inside: Duration::from_secs(1200),
            outside: Duration::from_secs(4 * 3600),
        }
    }
}

/// The Fiscal Data auctions reader.
#[derive(Debug, Clone)]
pub struct AuctionsReader {
    base: String,
}

const PAGE_SIZE: u32 = 1000;

impl AuctionsReader {
    /// A reader using `bases` (key `fiscaldata`).
    #[must_use]
    pub fn new(bases: &Bases) -> Self {
        Self {
            base: bases.get("fiscaldata", FISCAL_BASE),
        }
    }

    fn page(&self, filter: &str, tag: &str, page: u32) -> Target {
        Target::poll(
            format!("fiscaldata:auctions:{tag}:{page}"),
            format!(
                "{}/v1/accounting/od/auctions_query?filter={filter}&sort=auction_date&page%5Bnumber%5D={page}&page%5Bsize%5D={PAGE_SIZE}",
                self.base
            ),
            format!("auctions:{filter}:{tag}:{page}"),
        )
    }
}

/// Measure, compounding, day count and currency of an auction field.
type Conventions = (
    ReferenceMeasure,
    Option<Compounding>,
    Option<DayCount>,
    Option<String>,
);

fn price_or_rate(field: &str, term_days: i64) -> Option<Conventions> {
    let f = field;
    if f.ends_with("_price") {
        Some((ReferenceMeasure::Price, None, None, Some("USD".into())))
    } else if f.ends_with("discnt_rate") {
        Some((
            ReferenceMeasure::Rate,
            Some(Compounding::Discount),
            Some(DayCount::Act360),
            None,
        ))
    } else if f.ends_with("investment_rate") {
        if term_days <= 183 {
            Some((
                ReferenceMeasure::Rate,
                Some(Compounding::Simple),
                Some(DayCount::Act365Fixed),
                None,
            ))
        } else {
            Some((
                ReferenceMeasure::Rate,
                Some(Compounding::SemiAnnual),
                None,
                None,
            ))
        }
    } else if f.ends_with("_yield") || f == "int_rate" {
        Some((
            ReferenceMeasure::Rate,
            Some(Compounding::SemiAnnual),
            None,
            None,
        ))
    } else {
        None
    }
}

const AUCTION_FIELDS: [&str; 13] = [
    "high_yield",
    "low_yield",
    "avg_med_yield",
    "high_discnt_rate",
    "low_discnt_rate",
    "avg_med_discnt_rate",
    "high_investment_rate",
    "low_investment_rate",
    "avg_med_investment_rate",
    "high_price",
    "low_price",
    "avg_med_price",
    "int_rate",
];

/// Parses one page of `auctions_query`. Returns the observations and
/// whether `links.next` is set.
///
/// # Errors
///
/// Fails if the body is not the documented JSON shape.
pub fn parse_auctions(body: &[u8]) -> Result<(Parsed, bool), ParseError> {
    let v: Value = serde_json::from_slice(body).map_err(ParseError::new)?;
    let rows = v
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| ParseError("no data array".into()))?;
    let more = v
        .get("links")
        .and_then(|l| l.get("next"))
        .is_some_and(|n| !n.is_null());
    let mut out = Parsed::default();
    for row in rows {
        let s = |k: &str| {
            row.get(k)
                .and_then(Value::as_str)
                .filter(|x| !num::is_missing(x))
        };
        let (Some(date), Some(maturity), Some(kind), Some(term)) = (
            s("auction_date").and_then(Date::parse_iso),
            s("maturity_date").and_then(Date::parse_iso),
            s("security_type"),
            s("security_term"),
        ) else {
            out.skip("malformed");
            continue;
        };
        let kind = if s("ref_cpi_on_issue_date").is_some() {
            "TIPS".to_owned()
        } else if s("frn_index_determination_rate").is_some() {
            "FRN".to_owned()
        } else {
            num::slug(kind)
        };
        let term_slug = num::slug(term);
        let issue = s("issue_date").and_then(Date::parse_iso).unwrap_or(date);
        let term_days = maturity.days() - issue.days();
        let slot = u32::try_from((maturity.days() - date.days()).clamp(0, i64::from(u32::MAX)))
            .unwrap_or(0);
        for field in AUCTION_FIELDS {
            let Some(text) = s(field) else { continue };
            let value = match num::decimal(text) {
                Ok(Some(v)) => v,
                Ok(None) => continue,
                Err(_) => {
                    out.skip("malformed");
                    continue;
                }
            };
            let Some((measure, compounding, day_count, currency)) = price_or_rate(field, term_days)
            else {
                continue;
            };
            out.obs.push(Obs::Ref(RefObs {
                series: format!("auction.{kind}.{term_slug}.{field}"),
                measure,
                value,
                tenor: None,
                maturity_date: Some(maturity),
                day_count,
                compounding,
                currency,
                as_of: date,
                period: None,
                exchange_referenced: false,
                slot,
                published_ms: None,
            }));
        }
        for k in [
            "offering_amt",
            "total_tendered",
            "total_accepted",
            "bid_to_cover_ratio",
            "comp_accepted",
            "noncomp_accepted",
            "primary_dealer_accepted",
            "direct_bidder_accepted",
            "indirect_bidder_accepted",
            "soma_accepted",
        ] {
            if s(k).is_some() {
                out.skip("unsupported_amount");
            }
        }
    }
    Ok((out, more))
}

impl Reader for AuctionsReader {
    fn id(&self) -> SourceId {
        SourceId::TreasuryAuctions
    }

    fn live_targets(&self, today: Date) -> Vec<Target> {
        let from = today.add_days(-30);
        vec![self.page(&format!("auction_date:gte:{from}"), "live", 1)]
    }

    fn backfill_targets(&self, year: i32) -> Vec<Target> {
        vec![self.page(
            &format!("auction_date:gte:{year}-01-01,auction_date:lte:{year}-12-31"),
            &year.to_string(),
            1,
        )]
    }

    fn parse(&self, target: &Target, body: &[u8]) -> Result<Parsed, ParseError> {
        let (mut parsed, more) = parse_auctions(body)?;
        if more {
            // kind is auctions:<filter>:<tag>:<page>
            let mut parts = target.kind.rsplitn(3, ':');
            let page: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(1);
            let tag = parts.next().unwrap_or("x").to_owned();
            let filter = parts
                .next()
                .and_then(|f| f.strip_prefix("auctions:"))
                .unwrap_or_default()
                .to_owned();
            parsed.follow.push(self.page(&filter, &tag, page + 1));
        }
        Ok(parsed)
    }

    fn schedule(&self) -> Schedule {
        // Results post shortly after each auction closes (11:30 a.m. or
        // 1:00 p.m. ET).
        Schedule {
            days: WEEKDAYS,
            from_min: 15 * 60 + 30,
            to_min: 20 * 60,
            inside: Duration::from_secs(1800),
            outside: Duration::from_secs(6 * 3600),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obs::tenor_slot;

    #[test]
    fn tenors() {
        assert_eq!(curve_tenor("BC_10YEAR").as_deref(), Some("10Y"));
        assert_eq!(curve_tenor("BC_1_5MONTH").as_deref(), Some("6W"));
        assert_eq!(curve_tenor("TC_5YEAR").as_deref(), Some("5Y"));
        assert_eq!(curve_tenor("BC_9MONTH").as_deref(), Some("9M"));
        assert_eq!(curve_tenor("BC_30YEARDISPLAY"), None);
        assert_eq!(curve_tenor("NEW_DATE"), None);
        assert_eq!(tenor_slot("6W"), 42);
    }
}
