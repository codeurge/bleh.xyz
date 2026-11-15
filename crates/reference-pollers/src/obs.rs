//! Parsed observations and how they become mesh records.
//!
//! Source parsers produce [`Obs`]: one published value (a reference value)
//! or one category's positions, with the source's own series name, the
//! date it is for and a `slot` that orders it within its series on that
//! date. [`Builder`] turns observations into records:
//!
//! - **Revisions.** Each observation has a revision key (instrument,
//!   tenor, maturity, period and date; or instrument, report family, form,
//!   category and date). The [`RevisionStore`] remembers the value last
//!   emitted under each key: an unchanged value is a duplicate and is not
//!   emitted, a changed value is emitted with the next `revision`. This is
//!   how New York Fed same-day revisions, H.15 revisions, NDPSR four-week
//!   revisions, LMR corrections and ERCOT price corrections are handled,
//!   uniformly, live and in backfill.
//! - **Sequence.** `seq.block_number` is `T * 100 + revision`, where `T` is
//!   the period start (seconds) for intra-day values and the date's
//!   midnight (seconds) otherwise; `seq.event_index` is the slot. Both
//!   depend only on the data, so live capture, backfill and replay agree,
//!   and a revision never collides with what it revises.
//! - **Time.** Live records take `ts_source_ms` from the source's
//!   publication time where it states one, else the response's
//!   `Last-Modified`, else the fetch time; `ts_ingest_ns` is the fetch
//!   time. Backfill records take a documented publication estimate per
//!   source and `ts_ingest_ns` equal to it, so a backfill is a pure
//!   function of the files it reads.
//! - **Flags.** `backfill` on history imports; `exchange_referenced` where
//!   the admission requires it (the parser marks the observation).

use mesh_types::positioning::POSITIONING_CLASS;
use mesh_types::reference::REFERENCE_CLASS;
use mesh_types::{
    Compounding, DayCount, Flag, Flags, Positioning, PositioningCategory, PositioningReport, Price,
    Provenance, QuantityUnit, Record, ReferenceMeasure, ReferenceValue, Seq, UnixMillis, UnixNanos,
};

use crate::num;
use crate::revision::{Outcome, RevisionStore};
use crate::source::SourceId;
use crate::time::Date;

/// One published reference value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefObs {
    /// Series name: the instrument symbol after `<venue>:ref:`.
    pub series: String,
    /// What the value measures.
    pub measure: ReferenceMeasure,
    /// The value as published.
    pub value: Price,
    /// Tenor, for rates and yields.
    pub tenor: Option<String>,
    /// Maturity of the referenced instrument.
    pub maturity_date: Option<Date>,
    /// Day count (simple and discount rates).
    pub day_count: Option<DayCount>,
    /// Compounding (rates and yields).
    pub compounding: Option<Compounding>,
    /// Currency (prices).
    pub currency: Option<String>,
    /// The date the value is for.
    pub as_of: Date,
    /// Intra-day interval, Unix milliseconds, start inclusive, end
    /// exclusive.
    pub period: Option<(i64, i64)>,
    /// The admission requires the `exchange_referenced` flag.
    pub exchange_referenced: bool,
    /// Order within the series on this date (or period); unique per
    /// series, date and period.
    pub slot: u32,
    /// Publication time the source states, Unix milliseconds.
    pub published_ms: Option<i64>,
    /// Unit of a quantity (ADR 0023); `None` for rates, prices and index
    /// levels.
    pub unit: Option<QuantityUnit>,
}

impl RefObs {
    /// A quantity (`stock`, `flow`, `capacity` or `ratio`) in `unit` for
    /// `as_of`, carried as published (never rescaled).
    #[must_use]
    pub fn quantity(
        series: String,
        measure: ReferenceMeasure,
        value: Price,
        unit: QuantityUnit,
        as_of: Date,
    ) -> Self {
        Self {
            series,
            measure,
            value,
            tenor: None,
            maturity_date: None,
            day_count: None,
            compounding: None,
            currency: None,
            as_of,
            period: None,
            exchange_referenced: false,
            slot: 0,
            published_ms: None,
            unit: Some(unit),
        }
    }

    /// A price in US dollars for `as_of`.
    #[must_use]
    pub fn usd_price(series: String, value: Price, as_of: Date) -> Self {
        Self {
            series,
            measure: ReferenceMeasure::Price,
            value,
            tenor: None,
            maturity_date: None,
            day_count: None,
            compounding: None,
            currency: Some("USD".into()),
            as_of,
            period: None,
            exchange_referenced: false,
            slot: 0,
            published_ms: None,
            unit: None,
        }
    }

    /// A rate or yield for `as_of`.
    #[must_use]
    pub fn rate(
        series: String,
        measure: ReferenceMeasure,
        value: Price,
        tenor: &str,
        compounding: Compounding,
        day_count: Option<DayCount>,
        as_of: Date,
    ) -> Self {
        Self {
            series,
            measure,
            value,
            tenor: Some(tenor.to_owned()),
            maturity_date: None,
            day_count,
            compounding: Some(compounding),
            currency: None,
            as_of,
            period: None,
            exchange_referenced: false,
            slot: tenor_slot(tenor),
            published_ms: None,
            unit: None,
        }
    }
}

/// One category's positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PosObs {
    /// Contract market code (the instrument symbol).
    pub code: String,
    /// Report family.
    pub report: PositioningReport,
    /// Futures-and-options combined.
    pub combined: bool,
    /// Report date.
    pub as_of: Date,
    /// Trader category.
    pub category: PositioningCategory,
    /// Long positions.
    pub long: u64,
    /// Short positions.
    pub short: u64,
    /// Spreading positions.
    pub spreading: Option<u64>,
    /// Open interest.
    pub open_interest: u64,
}

impl PosObs {
    /// Order within the market on this date: family, form, category.
    #[must_use]
    pub fn slot(&self) -> u32 {
        let family = match self.report {
            PositioningReport::Legacy => 0,
            PositioningReport::Disaggregated => 1,
            PositioningReport::Tff => 2,
            PositioningReport::Supplemental => 3,
        };
        family * 100 + u32::from(self.combined) * 50 + self.category as u32
    }
}

/// A parsed observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Obs {
    /// A reference value.
    Ref(RefObs),
    /// Positions.
    Pos(PosObs),
}

impl Obs {
    /// The date the observation is for.
    #[must_use]
    pub fn as_of(&self) -> Date {
        match self {
            Obs::Ref(r) => r.as_of,
            Obs::Pos(p) => p.as_of,
        }
    }
}

/// Slot of a tenor: its approximate length in days (`ON` 0, `TN` 1), so
/// tenors order naturally and never collide.
#[must_use]
pub fn tenor_slot(tenor: &str) -> u32 {
    match tenor {
        "ON" => 0,
        "TN" => 1,
        t => {
            let (n, unit) = t.split_at(t.len().saturating_sub(1));
            let n: u32 = n.parse().unwrap_or(0);
            n.saturating_mul(match unit {
                "D" => 1,
                "W" => 7,
                "M" => 30,
                "Y" => 365,
                _ => 0,
            })
        }
    }
}

/// How records of one batch are stamped.
#[derive(Debug, Clone, Copy)]
pub enum Stamp {
    /// Live capture: the fetch time, and the response's publication time
    /// when known.
    Live {
        /// Fetch completion time.
        fetched_at: UnixNanos,
        /// `Last-Modified` of the response, Unix milliseconds, if
        /// plausible.
        last_modified_ms: Option<i64>,
    },
    /// History import: publication estimated per source.
    Backfill,
}

/// Errors building records.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// A series name or code that cannot form an instrument ID.
    #[error("bad instrument for {0:?}: {1}")]
    Instrument(String, mesh_types::IdError),
    /// The record fails validation (a parser bug).
    #[error("invalid record: {0}")]
    Invalid(#[from] mesh_types::ValidationError),
    /// More than 99 revisions of one value.
    #[error("too many revisions of {0}")]
    TooManyRevisions(String),
}

/// Estimated publication time of a backfilled value, Unix milliseconds.
///
/// The admitted history files do not say when each value was first
/// published, so the documented release schedule is used, in UTC (US
/// daylight time is not applied; the estimate is within an hour):
///
/// | Source | Estimate |
/// |---|---|
/// | COT | Friday after the Tuesday report date, 20:30 |
/// | Treasury rates | the date, 23:00 |
/// | Treasury auctions | the auction date, 18:00 |
/// | NY Fed | next weekday, 13:00 (14:00 for EFFR and OBFR) |
/// | H.15 | next weekday, 21:15 |
/// | H.10 | next Monday after the week, 21:15 |
/// | USDA LMR and dairy | the report date, 20:00 (stated publication time first) |
/// | ERCOT day-ahead | the day before delivery, 19:30 |
/// | ERCOT real-time | the end of the interval |
/// | EIA | by family: [`crate::sources::eia::backfill_published_ms`] |
/// | BLS | the 15th of the month after the reference month, 13:30 (8:30 a.m. ET) |
#[must_use]
pub fn backfill_published_ms(source: SourceId, obs: &Obs) -> i64 {
    const H: i64 = 3_600_000;
    let d = obs.as_of();
    match (source, obs) {
        (_, Obs::Ref(r)) if r.published_ms.is_some() => r.published_ms.unwrap_or_default(),
        (SourceId::Cot, _) => {
            d.add_days(i64::from((4 + 7 - d.weekday()) % 7)).start_ms() + 20 * H + 30 * 60_000
        }
        (SourceId::TreasuryRates, _) => d.start_ms() + 23 * H,
        (SourceId::TreasuryAuctions, _) => d.start_ms() + 18 * H,
        (SourceId::NyFed, Obs::Ref(r)) => {
            let late = r.series.starts_with("EFFR") || r.series.starts_with("OBFR");
            d.next_weekday().start_ms() + if late { 14 * H } else { 13 * H }
        }
        (SourceId::FrbH15, _) => d.next_weekday().start_ms() + 21 * H + 15 * 60_000,
        (SourceId::FrbH10, _) => {
            let monday = d.add_days(i64::from(7 - d.weekday()));
            monday.start_ms() + 21 * H + 15 * 60_000
        }
        (SourceId::Eia, Obs::Ref(r)) => crate::sources::eia::backfill_published_ms(&r.series, d),
        (SourceId::Bls, _) => {
            let next = if d.m == 12 {
                Date::new(d.y + 1, 1, 15)
            } else {
                Date::new(d.y, d.m + 1, 15)
            };
            next.unwrap_or(d).start_ms() + 13 * H + 30 * 60_000
        }
        (SourceId::Ercot, Obs::Ref(r)) => match r.period {
            Some((_, end)) if r.series.starts_with("RT") => end,
            _ => d.add_days(-1).start_ms() + 19 * H + 30 * 60_000,
        },
        _ => d.start_ms() + 20 * H,
    }
}

/// Turns observations into records for one source.
#[derive(Debug)]
pub struct Builder {
    source: SourceId,
}

impl Builder {
    /// A builder for `source`.
    #[must_use]
    pub fn new(source: SourceId) -> Self {
        Self { source }
    }

    /// Builds records from `obs`, skipping values already emitted
    /// unchanged and numbering revisions, in a deterministic order
    /// (instrument, block, slot). Returns the records and the number of
    /// duplicates skipped.
    ///
    /// # Errors
    ///
    /// Fails if an observation cannot form a valid record.
    pub fn build(
        &self,
        mut obs: Vec<Obs>,
        stamp: Stamp,
        store: &mut RevisionStore,
    ) -> Result<(Vec<Record>, usize), BuildError> {
        // Stable order: revisions follow source order for equal keys.
        obs.sort_by_cached_key(sort_key);
        let venue = self.source.venue();
        let admission = self.source.admission();
        let mut out = Vec::with_capacity(obs.len());
        let mut dups = 0usize;
        for o in obs {
            let (key, fingerprint) = revision_key(&o);
            let revision = match store.observe(&key, &fingerprint, o.as_of()) {
                Outcome::Duplicate => {
                    dups += 1;
                    continue;
                }
                Outcome::Emit(r) => r,
            };
            if revision > 99 {
                return Err(BuildError::TooManyRevisions(key));
            }
            let (ts_source_ms, ts_ingest_ns, mut flags) = match stamp {
                Stamp::Live {
                    fetched_at,
                    last_modified_ms,
                } => {
                    let published = match &o {
                        Obs::Ref(r) => r.published_ms,
                        Obs::Pos(_) => None,
                    };
                    let ts = published
                        .or(last_modified_ms)
                        .unwrap_or(fetched_at.to_millis().0);
                    (ts, fetched_at, Flags::NONE)
                }
                Stamp::Backfill => {
                    let ts = backfill_published_ms(self.source, &o);
                    (
                        ts,
                        UnixNanos(ts.saturating_mul(1_000_000)),
                        Flags::NONE.with(Flag::Backfill),
                    )
                }
            };
            let record = match o {
                Obs::Ref(r) => {
                    if r.exchange_referenced {
                        flags.insert(Flag::ExchangeReferenced);
                    }
                    let instrument = num::instrument(&venue, REFERENCE_CLASS, &r.series)
                        .map_err(|e| BuildError::Instrument(r.series.clone(), e))?;
                    let t = r
                        .period
                        .map_or(r.as_of.start_ms(), |p| p.0)
                        .div_euclid(1000);
                    let block = u64::try_from(t).unwrap_or(0) * 100 + u64::from(revision);
                    Record::ReferenceValue(ReferenceValue {
                        prov: Provenance::new(
                            instrument,
                            venue.clone(),
                            Seq::new(block, r.slot),
                            UnixMillis(ts_source_ms),
                            ts_ingest_ns,
                            admission.clone(),
                        )
                        .with_flags(flags),
                        measure: r.measure,
                        value: r.value,
                        tenor: r.tenor,
                        maturity_date: r.maturity_date.map(|d| d.to_string()),
                        day_count: r.day_count,
                        compounding: r.compounding,
                        currency: r.currency,
                        as_of_date: r.as_of.to_string(),
                        revision,
                        period_start_ms: r.period.map(|p| UnixMillis(p.0)),
                        period_end_ms: r.period.map(|p| UnixMillis(p.1)),
                        unit: r.unit,
                    })
                }
                Obs::Pos(p) => {
                    if p.combined {
                        flags.insert(Flag::ExchangeReferenced);
                    }
                    let instrument = num::instrument(&venue, POSITIONING_CLASS, &p.code)
                        .map_err(|e| BuildError::Instrument(p.code.clone(), e))?;
                    let t = p.as_of.start_ms().div_euclid(1000);
                    let block = u64::try_from(t).unwrap_or(0) * 100 + u64::from(revision);
                    Record::Positioning(Positioning {
                        prov: Provenance::new(
                            instrument,
                            venue.clone(),
                            Seq::new(block, p.slot()),
                            UnixMillis(ts_source_ms),
                            ts_ingest_ns,
                            admission.clone(),
                        )
                        .with_flags(flags),
                        report: p.report,
                        combined: p.combined,
                        contract_market_code: p.code,
                        as_of_date: p.as_of.to_string(),
                        category: p.category,
                        long: p.long,
                        short: p.short,
                        spreading: p.spreading,
                        open_interest: p.open_interest,
                        revision,
                    })
                }
            };
            record.validate()?;
            out.push(record);
        }
        Ok((out, dups))
    }
}

fn sort_key(o: &Obs) -> (String, i64, u32) {
    match o {
        Obs::Ref(r) => (
            r.series.clone(),
            r.period.map_or(r.as_of.start_ms(), |p| p.0),
            r.slot,
        ),
        Obs::Pos(p) => (p.code.clone(), p.as_of.start_ms(), p.slot()),
    }
}

/// The revision key and value fingerprint of an observation.
#[must_use]
pub fn revision_key(o: &Obs) -> (String, String) {
    match o {
        Obs::Ref(r) => {
            let mut fingerprint = format!(
                "{:?}|{}|{:?}|{:?}|{}|{}",
                r.measure,
                num::normalized(r.value),
                r.day_count,
                r.compounding,
                r.currency.as_deref().unwrap_or(""),
                r.period.map(|p| p.1.to_string()).unwrap_or_default()
            );
            // Appended only for quantities, so the fingerprints of values
            // stored before units existed are unchanged (no spurious
            // revisions).
            if let Some(u) = r.unit {
                fingerprint.push('|');
                fingerprint.push_str(u.as_str());
            }
            (
                format!(
                    "ref|{}|{}|{}|{}|{}|{}",
                    r.series,
                    r.tenor.as_deref().unwrap_or(""),
                    r.maturity_date.map(|d| d.to_string()).unwrap_or_default(),
                    r.period.map(|p| p.0.to_string()).unwrap_or_default(),
                    r.as_of,
                    r.slot
                ),
                fingerprint,
            )
        }
        Obs::Pos(p) => (
            format!(
                "pos|{}|{:?}|{}|{:?}|{}",
                p.code, p.report, p.combined, p.category, p.as_of
            ),
            format!(
                "{}|{}|{:?}|{}",
                p.long, p.short, p.spreading, p.open_interest
            ),
        ),
    }
}
