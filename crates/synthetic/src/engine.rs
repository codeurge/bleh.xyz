//! The engine: keeps the newest inputs and values every listed month of
//! every exposure at a valuation time (method spec §§ 13–14).
//!
//! Feed it input records in time order with [`Engine::push`], then call
//! [`Engine::value`] at each valuation time. A valuation uses only inputs
//! with `prov.ts_source_ms <= as_of_ms`, so pushing ahead of the
//! valuation time never leaks future data into a value: each spot source
//! keeps its last [`HISTORY`] observations of each kind, so up to that
//! many may be pushed ahead without losing the one a valuation needs.
//! Revisions and cancels take effect when pushed.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use mesh_types::{
    AdmissionId, CarrySource, ContentHasher, DerivedHeader, Flag, Flags, Grade, InstrumentId,
    LiquidityLabel, Price, Provenance, Record, ReferenceMeasure, Seq, Size, SnapshotDepth,
    SwapAction, SwapEventType, SwapPrice, SyntheticFamily, SyntheticMonth, SyntheticQuality,
    UnixMillis, VenueId, synthetic::SYNTHETIC_VENUE,
};

use crate::calendar::{Calendar, CalendarId, cycle_months};
use crate::curve::{
    Curve, Quote, YEAR_DAYS, bootstrap_par, coupons_per_year, quote_for, tenor_days,
    zero_from_quote,
};
use crate::date::{Date, YearMonth, ny_date, ny_to_utc_ms};
use crate::fair::{Node, forward, implied_carry, implied_short_rate, reference_period};
use crate::registry::{CurveSet, Exposure, Registry, SpotTracks};
use crate::spot::{Observation, composite, label_for};

/// Observations kept per spot source and kind (trades, book mids,
/// liquidity grades).
pub const HISTORY: usize = 64;

/// Milliseconds in an act/365F year.
pub const YEAR_MS: i64 = 31_536_000_000;

/// Fractional digits of `financing_rate`, `carry_rate` and
/// `implied_rate`.
pub const RATE_SCALE: u8 = 6;
/// Fractional digits of `year_fraction`.
pub const YEAR_FRACTION_SCALE: u8 = 8;

const POW10: [f64; 19] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18,
];

/// A fixed-point value as `f64` (exact powers of ten from a table).
#[must_use]
pub fn price_f64(p: Price) -> f64 {
    // Mantissas of published prices are far below 2^53.
    #[allow(clippy::cast_precision_loss)]
    let m = p.mantissa() as f64;
    m / POW10[usize::from(p.scale().min(18))]
}

/// Rounds `x` half away from zero to `scale` digits; `None` if not finite
/// or out of range.
#[must_use]
pub fn to_price(x: f64, scale: u8) -> Option<Price> {
    let m = (x * POW10[usize::from(scale.min(18))]).round();
    if !m.is_finite() || m.abs() >= 9.0e18 {
        return None;
    }
    // In range per the check above.
    #[allow(clippy::cast_possible_truncation)]
    let m = m as i64;
    Price::new(m, scale).ok()
}

/// Act/365F years in `ms` milliseconds, exactly rounded half up to eight
/// digits.
#[must_use]
pub fn year_fraction(ms: i64) -> Option<Size> {
    let ms = i128::from(ms.max(0));
    let scaled = (ms * 100_000_000 * 2 + i128::from(YEAR_MS)) / (2 * i128::from(YEAR_MS));
    Size::new(i64::try_from(scaled).ok()?, YEAR_FRACTION_SCALE).ok()
}

#[derive(Debug, Clone)]
struct Latest {
    value: f64,
    ts: i64,
    record: Record,
}

#[derive(Debug, Clone, Default)]
struct SourceState {
    trades: VecDeque<Latest>,
    books: VecDeque<Latest>,
    grades: VecDeque<(Grade, LiquidityLabel, Record)>,
}

/// Inserts `item` in time order, keeping the newest [`HISTORY`] items.
fn remember<T>(q: &mut VecDeque<T>, item: T, ts_of: impl Fn(&T) -> i64) {
    let ts = ts_of(&item);
    let pos = q.iter().rposition(|x| ts_of(x) <= ts).map_or(0, |p| p + 1);
    q.insert(pos, item);
    while q.len() > HISTORY {
        q.pop_front();
    }
}

/// The newest item at or before `as_of_ms`.
fn newest_by<T>(q: &VecDeque<T>, as_of_ms: i64, ts_of: impl Fn(&T) -> i64) -> Option<&T> {
    q.iter().rev().find(|x| ts_of(x) <= as_of_ms)
}

#[derive(Debug, Clone)]
struct SeriesValue {
    revision: u32,
    record: Record,
}

/// Series values by `as_of_date`, then by tenor or maturity.
type Series = BTreeMap<Date, BTreeMap<String, SeriesValue>>;

/// The rate state at one valuation.
#[derive(Debug, Clone)]
struct RateState {
    curve: Curve,
    records: Vec<Record>,
    rates_ms: i64,
    curve_date: Date,
    fixings: BTreeMap<Date, f64>,
    fixing_records: BTreeMap<Date, Record>,
}

/// One valuation's output.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Valuation {
    /// Records, by exposure (registry order) then month.
    pub records: Vec<SyntheticMonth>,
    /// Problems that prevented a value, for operators.
    pub notices: Vec<String>,
}

/// The synthetic-month engine.
#[derive(Debug, Clone)]
pub struct Engine {
    registry: Registry,
    calendars: BTreeMap<CalendarId, Calendar>,
    govt: Calendar,
    sources: BTreeMap<InstrumentId, SourceState>,
    series: BTreeMap<InstrumentId, Series>,
    wanted_series: BTreeSet<InstrumentId>,
    prints: BTreeMap<(VenueId, String), Record>,
    swap_venues: BTreeSet<VenueId>,
    max_window_ms: i64,
}

fn rec_ts(r: &Record) -> i64 {
    r.prov().ts_source_ms.0
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

fn mid(bid: Option<Price>, ask: Option<Price>) -> Option<f64> {
    Some((price_f64(bid?) + price_f64(ask?)) / 2.0)
}

impl Engine {
    /// An engine for a checked registry.
    #[must_use]
    pub fn new(registry: Registry) -> Self {
        let mut sources = BTreeMap::new();
        let mut wanted_series = BTreeSet::new();
        let mut swap_venues = BTreeSet::new();
        let mut max_window_h = 0u32;
        wanted_series.insert(registry.curve.overnight.clone());
        for set in &registry.curve.sets {
            match set {
                CurveSet::Series { series, .. } => wanted_series.extend(series.iter().cloned()),
                CurveSet::SwapPrints {
                    venue,
                    window_hours,
                    ..
                } => {
                    swap_venues.insert(venue.clone());
                    max_window_h = max_window_h.max(*window_hours);
                }
            }
        }
        for e in &registry.exposures {
            if let Some(s) = &e.spot {
                for src in &s.sources {
                    sources.insert(src.instrument.clone(), SourceState::default());
                }
            }
            if let Some(series) = e.carry.as_ref().and_then(|c| c.series.clone()) {
                wanted_series.insert(series);
            }
            if let Some(a) = &e.anchors {
                swap_venues.insert(a.venue.clone());
                max_window_h = max_window_h.max(a.window_hours);
            }
        }
        Self {
            calendars: [CalendarId::UsTrading, CalendarId::UsGovernmentSecurities]
                .into_iter()
                .map(|id| (id, registry.calendar(id)))
                .collect(),
            govt: registry.calendar(CalendarId::UsGovernmentSecurities),
            registry,
            sources,
            series: BTreeMap::new(),
            wanted_series,
            prints: BTreeMap::new(),
            swap_venues,
            max_window_ms: i64::from(max_window_h) * 3_600_000,
        }
    }

    /// The registry.
    #[must_use]
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// True if the engine reads records of this instrument or venue.
    #[must_use]
    pub fn wants(&self, r: &Record) -> bool {
        let p = r.prov();
        self.sources.contains_key(&p.instrument)
            || self.wanted_series.contains(&p.instrument)
            || (matches!(r, Record::SwapPrint(_)) && self.swap_venues.contains(&p.venue))
    }

    /// Takes one input record. Records the engine does not read are
    /// ignored.
    pub fn push(&mut self, r: &Record) {
        let ts = rec_ts(r);
        let inst = r.prov().instrument.clone();
        match r {
            Record::Trade(t) => {
                if let Some(s) = self.sources.get_mut(&inst) {
                    let item = Latest {
                        value: price_f64(t.price),
                        ts,
                        record: r.clone(),
                    };
                    remember(&mut s.trades, item, |l| l.ts);
                }
            }
            Record::BookSnapshot(b) => {
                let m = match &b.depth {
                    SnapshotDepth::L4 { bids, asks } => {
                        mid(bids.first().map(|o| o.price), asks.first().map(|o| o.price))
                    }
                    SnapshotDepth::L2 { bids, asks } => {
                        mid(bids.first().map(|l| l.price), asks.first().map(|l| l.price))
                    }
                };
                self.push_mid(&inst, m, ts, r);
            }
            Record::QueuePositions(q) => {
                let m = mid(
                    q.bid.as_ref().map(|l| l.price),
                    q.ask.as_ref().map(|l| l.price),
                );
                self.push_mid(&inst, m, ts, r);
            }
            Record::LiquidityGrade(g) => {
                if let Some(s) = self.sources.get_mut(&inst) {
                    remember(&mut s.grades, (g.grade, g.label, r.clone()), |g| {
                        rec_ts(&g.2)
                    });
                }
            }
            Record::ReferenceValue(v) => {
                if !self.wanted_series.contains(&inst) {
                    return;
                }
                let Some(date) = Date::parse(&v.as_of_date) else {
                    return;
                };
                let key = v
                    .maturity_date
                    .clone()
                    .or_else(|| v.tenor.clone())
                    .unwrap_or_default();
                let series = self.series.entry(inst).or_default();
                let slot = series.entry(date).or_default();
                if slot.get(&key).is_none_or(|old| old.revision <= v.revision) {
                    slot.insert(
                        key,
                        SeriesValue {
                            revision: v.revision,
                            record: r.clone(),
                        },
                    );
                }
                // Keep about 400 days of history (fixings of the longest
                // reference period plus slack).
                if let Some(&newest) = series.keys().next_back() {
                    let cutoff = newest.add_days(-400);
                    series.retain(|d, _| *d >= cutoff);
                }
            }
            Record::SwapPrint(p) => {
                if !self.swap_venues.contains(&p.prov.venue) {
                    return;
                }
                let venue = p.prov.venue.clone();
                match p.action {
                    SwapAction::Cancel => {
                        if let Some(orig) = &p.original_dissemination_id {
                            self.prints.remove(&(venue, orig.clone()));
                        }
                    }
                    SwapAction::Correct | SwapAction::New => {
                        if let (SwapAction::Correct, Some(orig)) =
                            (p.action, &p.original_dissemination_id)
                        {
                            self.prints.remove(&(venue.clone(), orig.clone()));
                        }
                        if matches!(p.event_type, None | Some(SwapEventType::Trade)) {
                            self.prints
                                .insert((venue, p.dissemination_id.clone()), r.clone());
                        }
                    }
                    SwapAction::Modify | SwapAction::Terminate | SwapAction::Revive => {}
                }
                let cutoff = ts - self.max_window_ms;
                self.prints.retain(|_, r| rec_ts(r) >= cutoff);
            }
            _ => {}
        }
    }

    fn push_mid(&mut self, inst: &InstrumentId, m: Option<f64>, ts: i64, r: &Record) {
        let (Some(s), Some(m)) = (self.sources.get_mut(inst), m) else {
            return;
        };
        let item = Latest {
            value: m,
            ts,
            record: r.clone(),
        };
        remember(&mut s.books, item, |l| l.ts);
    }

    /// The newest `as_of_date` of a series at or before the valuation date
    /// whose records were all published by `as_of_ms`.
    fn newest<'a>(
        &'a self,
        inst: &InstrumentId,
        as_of_ms: i64,
        today: Date,
    ) -> Option<(Date, Vec<&'a Record>)> {
        let series = self.series.get(inst)?;
        series.iter().rev().find_map(|(d, vals)| {
            let recs: Vec<&Record> = vals
                .values()
                .map(|v| &v.record)
                .filter(|r| rec_ts(r) <= as_of_ms)
                .collect();
            (*d <= today && !recs.is_empty()).then_some((*d, recs))
        })
    }

    fn series_points(&self, recs: &[&Record], date: Date) -> Vec<(f64, f64)> {
        let mut zeros = Vec::new();
        let mut par: BTreeMap<u32, Vec<(f64, f64)>> = BTreeMap::new();
        for r in recs {
            let Record::ReferenceValue(v) = r else {
                continue;
            };
            let days = match (&v.maturity_date, &v.tenor) {
                (Some(m), _) => Date::parse(m).map(|m| date.days_until(m)),
                (None, Some(t)) => tenor_days(t, date, &self.govt),
                (None, None) => None,
            };
            let (Some(days), Some(comp)) = (days, v.compounding) else {
                continue;
            };
            if days <= 0 {
                continue;
            }
            let rate = price_f64(v.value) / 100.0;
            let t = f64::from(days) / YEAR_DAYS;
            match v.measure {
                ReferenceMeasure::Rate | ReferenceMeasure::ZeroYield => {
                    if let Some(z) =
                        quote_for(rate, comp, v.day_count).and_then(|q| zero_from_quote(days, q))
                    {
                        zeros.push((t, z));
                    }
                }
                ReferenceMeasure::ParYield => {
                    if let Some(f) = coupons_per_year(comp) {
                        par.entry(f).or_default().push((t, rate));
                    }
                }
                ReferenceMeasure::Price | ReferenceMeasure::IndexLevel => {}
            }
        }
        for (f, mut pts) in par {
            pts.sort_by(|a, b| a.0.total_cmp(&b.0));
            zeros.extend(bootstrap_par(&pts, f));
        }
        zeros
    }

    fn rate_state(&self, as_of_ms: i64, today: Date) -> Option<RateState> {
        let curve_cfg = &self.registry.curve;
        let mut records: Vec<Vec<Record>> = Vec::new();
        let mut sets: Vec<Vec<(f64, f64)>> = Vec::new();
        let mut dates: Vec<Option<Date>> = Vec::new();
        for set in &curve_cfg.sets {
            let mut pts = Vec::new();
            let mut recs = Vec::new();
            let mut newest: Option<Date> = None;
            match set {
                CurveSet::Series { series, .. } => {
                    for inst in series {
                        if let Some((d, rs)) = self.newest(inst, as_of_ms, today) {
                            pts.extend(self.series_points(&rs, d));
                            recs.extend(rs.into_iter().cloned());
                            newest = newest.max(Some(d));
                        }
                    }
                }
                CurveSet::SwapPrints {
                    venue,
                    underlier,
                    window_hours,
                    ..
                } => {
                    let from = as_of_ms - i64::from(*window_hours) * 3_600_000;
                    let mut by_tenor: BTreeMap<&str, (Vec<f64>, Vec<&Record>)> = BTreeMap::new();
                    for r in self.prints.values() {
                        let Record::SwapPrint(p) = r else { continue };
                        let ts = rec_ts(r);
                        if &p.prov.venue != venue
                            || ts < from
                            || ts > as_of_ms
                            || p.asset_class != mesh_types::AssetClass::InterestRate
                            || p.underlier.as_deref() != Some(underlier.as_str())
                        {
                            continue;
                        }
                        let (Some(SwapPrice::Rate { value }), Some(tenor)) = (p.price, &p.tenor)
                        else {
                            continue;
                        };
                        let e = by_tenor.entry(tenor.as_str()).or_default();
                        e.0.push(price_f64(value) / 100.0);
                        e.1.push(r);
                    }
                    let mut par = Vec::new();
                    for (tenor, (mut rates, rs)) in by_tenor {
                        let Some(days) = tenor_days(tenor, today, &self.govt) else {
                            continue;
                        };
                        if days <= 0 {
                            continue;
                        }
                        let c = median(&mut rates);
                        let t = f64::from(days) / YEAR_DAYS;
                        if days <= 366 {
                            let q = Quote::Simple {
                                rate: c,
                                day_count: mesh_types::DayCount::Act360,
                            };
                            if let Some(z) = zero_from_quote(days, q) {
                                pts.push((t, z));
                            }
                        } else {
                            par.push((t, c * 365.0 / 360.0));
                        }
                        for r in rs {
                            newest = newest.max(Some(ny_date(rec_ts(r))));
                            recs.push(r.clone());
                        }
                    }
                    par.sort_by(|a, b| a.0.total_cmp(&b.0));
                    pts.extend(bootstrap_par(&par, 1));
                }
            }
            pts.sort_by(|a, b| a.0.total_cmp(&b.0));
            sets.push(pts);
            records.push(recs);
            dates.push(newest);
        }
        // The overnight point and the fixings.
        let mut fixings = BTreeMap::new();
        let mut fixing_records = BTreeMap::new();
        let mut front = None;
        let mut front_rec = None;
        if let Some(series) = self.series.get(&curve_cfg.overnight) {
            for (d, vals) in series {
                let Some(v) = vals.get("ON") else { continue };
                let Record::ReferenceValue(rv) = &v.record else {
                    continue;
                };
                if rec_ts(&v.record) > as_of_ms || *d > today {
                    continue;
                }
                fixings.insert(*d, price_f64(rv.value) / 100.0);
                fixing_records.insert(*d, v.record.clone());
                let z = tenor_days("ON", *d, &self.govt).and_then(|days| {
                    let q = quote_for(price_f64(rv.value) / 100.0, rv.compounding?, rv.day_count)?;
                    Some((f64::from(days) / YEAR_DAYS, zero_from_quote(days, q)?))
                });
                if z.is_some() {
                    front = z;
                    front_rec = Some((*d, v.record.clone()));
                }
            }
        }
        let (curve, used) = Curve::build_tagged(front, &sets)?;
        let mut all: Vec<Record> = Vec::new();
        let mut curve_date: Option<Date> = None;
        for (i, u) in used.iter().enumerate() {
            if *u {
                all.extend(records[i].iter().cloned());
                curve_date = curve_date.max(dates[i]);
            }
        }
        if let Some((d, r)) = front_rec {
            all.push(r);
            curve_date = curve_date.max(Some(d));
        }
        let rates_ms = all.iter().map(rec_ts).max()?;
        Some(RateState {
            curve,
            records: all,
            rates_ms,
            curve_date: curve_date?,
            fixings,
            fixing_records,
        })
    }

    /// Values every listed month of every exposure at `as_of_ms`,
    /// positioning records at `block` (method spec § 14).
    #[must_use]
    pub fn value(&self, as_of_ms: i64, block: u64) -> Valuation {
        let mut out = Valuation::default();
        let today = ny_date(as_of_ms);
        let Some(rates) = self.rate_state(as_of_ms, today) else {
            out.notices.push("no rate curve: nothing valued".into());
            return out;
        };
        let stale = self.govt.business_days_between(rates.curve_date, today)
            > i32::try_from(self.registry.curve.max_age_business_days).unwrap_or(i32::MAX);
        for e in &self.registry.exposures {
            if let Err(n) = self.value_exposure(e, &rates, stale, as_of_ms, today, block, &mut out)
            {
                out.notices.push(format!("{}: {n}", e.id));
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn value_exposure(
        &self,
        e: &Exposure,
        rates: &RateState,
        rates_stale: bool,
        as_of_ms: i64,
        today: Date,
        block: u64,
        out: &mut Valuation,
    ) -> Result<(), String> {
        let cal = self
            .calendars
            .get(&e.calendar)
            .cloned()
            .unwrap_or_else(|| self.registry.calendar(e.calendar));
        let (hh, mm) = e.settlement_hm().map_err(|x| x.to_string())?;
        let listed = usize::try_from(e.months_listed).unwrap_or(usize::MAX);
        let mut months: Vec<MonthSpec> = Vec::new();
        let mut active = 0u32;
        for ym in cycle_months(&e.cycle, today.year_month().plus(-3), listed + 24) {
            let ltd = e.expiry.last_trade_date(&cal, ym);
            let expiry_ms = ny_to_utc_ms(ltd, hh, mm);
            if expiry_ms < as_of_ms {
                continue;
            }
            let rolled = today >= e.roll.roll_date(&cal, ym, ltd);
            let rank = if rolled {
                0
            } else {
                active += 1;
                active
            };
            months.push(MonthSpec {
                ym,
                ltd,
                expiry_ms,
                rank,
            });
            if active >= e.months_listed {
                break;
            }
        }
        let years = |ms: i64| -> f64 {
            // Millisecond spans of a few decades are exact in f64.
            #[allow(clippy::cast_precision_loss)]
            let x = (ms - as_of_ms) as f64;
            x / 31_536_000_000.0
        };
        if e.family == SyntheticFamily::ShortRate {
            let kind = e.short_rate.ok_or("short_rate missing")?;
            for m in &months {
                let (start, end) = reference_period(kind, m.ym);
                let sr = implied_short_rate(
                    kind,
                    start,
                    end,
                    today,
                    &rates.fixings,
                    &rates.curve,
                    &self.govt,
                );
                let mut q = BTreeSet::new();
                if sr.fixings_missing {
                    q.insert(SyntheticQuality::FixingsMissing);
                }
                let h_end = f64::from(today.days_until(end)) / YEAR_DAYS;
                if h_end > rates.curve.max_horizon() {
                    q.insert(SyntheticQuality::CurveExtrapolated);
                }
                let mut inputs = rates.records.clone();
                inputs.extend(
                    sr.fixings_used
                        .iter()
                        .filter_map(|d| rates.fixing_records.get(d).cloned()),
                );
                let fair = 100.0 - 100.0 * sr.rate;
                self.emit(
                    e,
                    m,
                    MonthValue {
                        fair,
                        spot: None,
                        implied_rate: Some(100.0 * sr.rate),
                        carry: None,
                        anchors: 0,
                        anchor_ms: None,
                        grade: None,
                        quality: q,
                        inputs,
                        spot_records: Vec::new(),
                    },
                    rates,
                    rates_stale,
                    as_of_ms,
                    block,
                    years(m.expiry_ms),
                    out,
                )?;
            }
            return Ok(());
        }

        let spot_cfg = e.spot.as_ref().ok_or("spot missing")?;
        // Spot composite.
        let mut obs = Vec::new();
        let mut obs_records: Vec<Vec<Record>> = Vec::new();
        for (i, src) in spot_cfg.sources.iter().enumerate() {
            let state = self.sources.get(&src.instrument);
            // The newer of the newest trade and the newest book mid; a mid
            // wins a tie.
            let best = state.and_then(|s| {
                let book = newest_by(&s.books, as_of_ms, |l| l.ts);
                let trade = newest_by(&s.trades, as_of_ms, |l| l.ts);
                match (book, trade) {
                    (Some(b), Some(t)) => Some(if t.ts > b.ts { t } else { b }),
                    (b, t) => b.or(t),
                }
            });
            let mut recs = Vec::new();
            if let Some(l) = best {
                let grade = state.and_then(|s| newest_by(&s.grades, as_of_ms, |g| rec_ts(&g.2)));
                recs.push(l.record.clone());
                if let Some((_, _, r)) = grade {
                    recs.push(r.clone());
                }
                obs.push(Observation {
                    source: i,
                    price: l.value * price_f64(src.multiplier),
                    ts_ms: l.ts,
                    grade: grade.map(|g| g.0),
                    label: grade.map_or(LiquidityLabel::Tape, |g| g.1),
                });
            }
            obs_records.push(recs);
        }
        let spot = composite(&obs, as_of_ms, spot_cfg);
        let front_t = months
            .iter()
            .find(|m| m.rank == 1)
            .map(|m| years(m.expiry_ms));
        let spot_node = match (&spot, spot_cfg.tracks) {
            (Some(c), SpotTracks::Cash) => Some(Node {
                t: 0.0,
                price: c.price,
                anchor: false,
            }),
            (Some(c), SpotTracks::FrontMonth) => front_t.map(|t| Node {
                t,
                price: c.price,
                anchor: false,
            }),
            (None, _) => None,
        };
        let spot_records: Vec<Record> = spot
            .as_ref()
            .map(|c| {
                c.used
                    .iter()
                    .flat_map(|i| obs_records[*i].iter().cloned())
                    .collect()
            })
            .unwrap_or_default();

        // Carry from the registry: observed series, else estimate.
        let mut carry_cfg: Option<(f64, CarrySource, Option<Record>)> = None;
        if let Some(c) = &e.carry {
            if let Some(series) = &c.series
                && let Some((_, recs)) = self.newest(series, as_of_ms, today)
                && let Some(r @ Record::ReferenceValue(v)) = recs.first().copied()
            {
                carry_cfg = Some((
                    price_f64(v.value) / 100.0,
                    CarrySource::Observed,
                    Some(r.clone()),
                ));
            }
            if carry_cfg.is_none()
                && let Some(est) = c.estimate
            {
                carry_cfg = Some((price_f64(est) / 100.0, CarrySource::Estimated, None));
            }
        }
        let c_rate = carry_cfg.as_ref().map_or(0.0, |c| c.0);

        // Swap-print anchors.
        let mut anchor_nodes: Vec<Node> = Vec::new();
        let mut anchor_records: Vec<Record> = Vec::new();
        let mut anchor_ms: Option<i64> = None;
        let mut anchor_stale = false;
        if let Some(a) = &e.anchors {
            let from = as_of_ms - i64::from(a.window_hours) * 3_600_000;
            let mut by_month: BTreeMap<YearMonth, (Vec<f64>, Vec<&Record>)> = BTreeMap::new();
            for r in self.prints.values() {
                let Record::SwapPrint(p) = r else { continue };
                let ts = rec_ts(r);
                if p.prov.venue != a.venue
                    || ts < from
                    || ts > as_of_ms
                    || p.asset_class != mesh_types::AssetClass::Commodity
                    || p.underlier.as_deref() != Some(a.underlier.as_str())
                    || p.tenor.as_deref().is_some_and(|t| t != "1M")
                {
                    continue;
                }
                let (Some(SwapPrice::Price { value }), Some(exp)) =
                    (p.price, p.expiry.as_deref().and_then(Date::parse))
                else {
                    continue;
                };
                let e = by_month.entry(exp.year_month()).or_default();
                e.0.push(price_f64(value) * price_f64(a.multiplier));
                e.1.push(r);
            }
            for (ym, (mut prices, rs)) in by_month {
                let start = ym.first_day().utc_midnight_ms();
                let end = ym.plus(1).first_day().utc_midnight_ms();
                let mid_ms = start + (end - start) / 2;
                if mid_ms <= as_of_ms {
                    continue;
                }
                anchor_nodes.push(Node {
                    t: years(mid_ms),
                    price: median(&mut prices),
                    anchor: true,
                });
                for r in rs {
                    anchor_ms = anchor_ms.max(Some(rec_ts(r)));
                    anchor_records.push(r.clone());
                }
            }
            anchor_stale =
                anchor_ms.is_some_and(|t| as_of_ms - t > i64::from(a.max_age_hours) * 3_600_000);
        }
        let mut nodes: Vec<Node> = anchor_nodes.clone();
        nodes.extend(spot_node);
        if nodes.is_empty() {
            return Err("no spot and no anchors: nothing valued".into());
        }
        // The reference node for implied carry: the spot, else the first
        // anchor.
        let reference = spot_node.or_else(|| {
            anchor_nodes
                .iter()
                .copied()
                .min_by(|a, b| a.t.total_cmp(&b.t))
        });

        for m in &months {
            let t = years(m.expiry_ms);
            let Some(f) = forward(t, &nodes, &rates.curve, c_rate) else {
                out.notices.push(format!("{} {}: no forward", e.id, m.ym));
                continue;
            };
            let mut q: BTreeSet<SyntheticQuality> = BTreeSet::new();
            if let Some(c) = &spot {
                q.extend(c.quality.iter().copied());
            }
            let mut inputs = rates.records.clone();
            inputs.extend(spot_records.iter().cloned());
            let carry = if f.used_anchor {
                if f.interpolated {
                    q.insert(SyntheticQuality::AnchorInterpolated);
                }
                if f.extrapolated {
                    q.insert(SyntheticQuality::AnchorExtrapolated);
                }
                if anchor_stale {
                    q.insert(SyntheticQuality::AnchorStale);
                }
                inputs.extend(anchor_records.iter().cloned());
                // Implied carry from the reference node to the month, or
                // across the nearest segment when they coincide.
                reference
                    .and_then(|r| {
                        implied_carry(&rates.curve, r.t, r.price, t, f.price).or_else(|| {
                            nodes
                                .iter()
                                .filter(|n| (n.t - r.t).abs() >= 1e-9)
                                .min_by(|a, b| (a.t - r.t).abs().total_cmp(&(b.t - r.t).abs()))
                                .and_then(|o| {
                                    implied_carry(&rates.curve, r.t, r.price, o.t, o.price)
                                })
                        })
                    })
                    .map(|c| (c, CarrySource::SwapAnchor))
            } else {
                None
            };
            // The month a front-month tracker tracks is the spot itself:
            // no carry applies.
            let at_spot = !f.used_anchor && spot_node.is_some_and(|n| n.t == t);
            let carry = match carry {
                Some(c) => Some(c),
                None if at_spot => None,
                None => {
                    if let Some((c, src, rec)) = &carry_cfg {
                        if *src == CarrySource::Estimated {
                            q.insert(SyntheticQuality::CarryEstimated);
                        }
                        inputs.extend(rec.iter().cloned());
                        Some((*c, *src))
                    } else {
                        q.insert(SyntheticQuality::CarryUnobserved);
                        None
                    }
                }
            };
            let used_anchors = if f.used_anchor { anchor_nodes.len() } else { 0 };
            self.emit(
                e,
                m,
                MonthValue {
                    fair: f.price,
                    spot: spot.as_ref().map(|c| (c.price, c.ts_ms, c.used.len())),
                    implied_rate: None,
                    carry,
                    anchors: u32::try_from(used_anchors).unwrap_or(u32::MAX),
                    anchor_ms: if f.used_anchor { anchor_ms } else { None },
                    grade: spot.as_ref().map(|c| c.grade),
                    quality: q,
                    inputs,
                    spot_records: spot_records.clone(),
                },
                rates,
                rates_stale,
                as_of_ms,
                block,
                t,
                out,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn emit(
        &self,
        e: &Exposure,
        m: &MonthSpec,
        mut v: MonthValue,
        rates: &RateState,
        rates_stale: bool,
        as_of_ms: i64,
        block: u64,
        t: f64,
        out: &mut Valuation,
    ) -> Result<(), String> {
        let (z, extrapolated) = rates.curve.zero(t);
        if rates_stale {
            v.quality.insert(SyntheticQuality::RatesStale);
        }
        if extrapolated && e.family != SyntheticFamily::ShortRate {
            v.quality.insert(SyntheticQuality::CurveExtrapolated);
        }
        if m.rank == 0 {
            v.quality.insert(SyntheticQuality::Rolling);
        }
        if m.expiry_ms - as_of_ms < 86_400_000 {
            v.quality.insert(SyntheticQuality::Expiring);
        }
        let mut inputs = v.inputs;
        inputs.sort_by(Record::canonical_cmp);
        inputs.dedup();
        let mut hasher = ContentHasher::new();
        for r in &inputs {
            hasher.update(r).map_err(|x| x.to_string())?;
        }
        let admissions: BTreeSet<AdmissionId> =
            inputs.iter().map(|r| r.prov().admission.clone()).collect();
        let admissions: Vec<AdmissionId> = admissions.into_iter().collect();
        let first = admissions.first().cloned().ok_or("no inputs")?;
        let mut flags = Flags::NONE;
        for r in &v.spot_records {
            if r.prov().flags.contains(Flag::OracleAnchoredVenue) {
                flags.insert(Flag::OracleAnchoredVenue);
            }
        }
        for r in &inputs {
            for f in [Flag::Backfill, Flag::Replayed] {
                if r.prov().flags.contains(f) {
                    flags.insert(f);
                }
            }
        }
        let ts_source = inputs.iter().map(rec_ts).max().unwrap_or(as_of_ms);
        let ts_ingest = inputs
            .iter()
            .map(|r| r.prov().ts_ingest_ns)
            .max()
            .ok_or("no inputs")?;
        let id_err = |x: mesh_types::IdError| x.to_string();
        let instrument = InstrumentId::new(format!("{SYNTHETIC_VENUE}:month:{}:{}", e.id, m.ym))
            .map_err(id_err)?;
        let prov = Provenance::new(
            instrument,
            VenueId::new(SYNTHETIC_VENUE).map_err(id_err)?,
            Seq::new(block, 0),
            UnixMillis(ts_source),
            ts_ingest,
            first,
        )
        .with_flags(flags);
        let price = |x: f64, what: &str| {
            to_price(x, e.price_scale)
                .ok_or_else(|| format!("{} {}: {what} out of range", e.id, m.ym))
        };
        let rate = |x: f64, what: &str| {
            to_price(x, RATE_SCALE).ok_or_else(|| format!("{} {}: {what} out of range", e.id, m.ym))
        };
        let grade = v.grade;
        let rec = SyntheticMonth {
            prov,
            derived: DerivedHeader {
                method: self.registry.method.clone(),
                inputs_hash: hasher.finalize(),
                input_count: u64::try_from(inputs.len()).unwrap_or(u64::MAX),
            },
            exposure: e.id.clone(),
            family: e.family,
            month: m.ym.to_string(),
            rank: m.rank,
            as_of_ms: UnixMillis(as_of_ms),
            last_trade_date: m.ltd.to_string(),
            expiry_ms: UnixMillis(m.expiry_ms),
            year_fraction: year_fraction(m.expiry_ms - as_of_ms).ok_or("year fraction")?,
            fair_value: price(v.fair, "fair value")?,
            spot: v.spot.map(|s| price(s.0, "spot")).transpose()?,
            spot_ms: v.spot.map(|s| UnixMillis(s.1)),
            spot_sources: v.spot.map_or(0, |s| u32::try_from(s.2).unwrap_or(u32::MAX)),
            financing_rate: rate(100.0 * z, "financing rate")?,
            rates_ms: UnixMillis(rates.rates_ms),
            carry_rate: v.carry.map(|c| rate(100.0 * c.0, "carry")).transpose()?,
            carry_source: v.carry.map_or(CarrySource::None, |c| c.1),
            anchors: v.anchors,
            anchor_ms: v.anchor_ms.map(UnixMillis),
            implied_rate: v
                .implied_rate
                .map(|r| rate(r, "implied rate"))
                .transpose()?,
            quality: v.quality.into_iter().collect(),
            admissions,
            grade,
            label: label_for(grade),
        };
        rec.validate().map_err(|x| x.to_string())?;
        out.records.push(rec);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct MonthSpec {
    ym: YearMonth,
    ltd: Date,
    expiry_ms: i64,
    rank: u32,
}

#[derive(Debug, Clone)]
struct MonthValue {
    fair: f64,
    /// Price, newest observation time, source count.
    spot: Option<(f64, i64, usize)>,
    implied_rate: Option<f64>,
    carry: Option<(f64, CarrySource)>,
    anchors: u32,
    anchor_ms: Option<i64>,
    grade: Option<Grade>,
    quality: BTreeSet<SyntheticQuality>,
    inputs: Vec<Record>,
    spot_records: Vec<Record>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_point_conversions() {
        assert_eq!(to_price(5032.745, 2).unwrap().to_string(), "5032.75");
        assert_eq!(to_price(-0.125, 2).unwrap().to_string(), "-0.13");
        assert!(to_price(f64::NAN, 2).is_none());
        assert!(to_price(1e30, 2).is_none());
        assert_eq!(price_f64(Price::parse_exact("98.765").unwrap()), 98.765);
        assert_eq!(year_fraction(YEAR_MS).unwrap().to_string(), "1.00000000");
        assert_eq!(year_fraction(86_400_000).unwrap().to_string(), "0.00273973");
        assert_eq!(year_fraction(-5).unwrap().to_string(), "0.00000000");
    }
}
