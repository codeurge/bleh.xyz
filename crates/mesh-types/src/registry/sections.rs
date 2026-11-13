//! Registry sections beyond instruments (`registry-v0`, see
//! `docs/specs/registry-v0.md`): exposures with their venue mappings,
//! session calendars, roll schedules, the reference-series catalogue and
//! liquidity grades.
//!
//! Every entry is a fact written fresh for the registry. Names are neutral
//! ("US large-cap index front"); an exchange contract or a named index may
//! appear only as the economic reference in `tracks`, stated as a fact.
//! Nothing here is filled from exchange-licensed data: calendars and roll
//! rules are public rules (holiday law, published contract terms) or are
//! left for the operator to fill.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::analytics::{Grade, LiquidityLabel};
use crate::decimal::Price;
use crate::flags::Flags;
use crate::ids::{AdmissionId, InstrumentId, MethodId};
use crate::record::RecordKind;
use crate::time::UnixMillis;

use super::RegistryError;

/// Version tag of the registry document.
pub const REGISTRY_VERSION: &str = "registry-v0";

/// Identifier of an exposure, calendar or roll schedule:
/// `[a-z0-9][a-z0-9_-]{0,63}`. It never contains `:`, so it cannot be
/// confused with an [`InstrumentId`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EntryId(String);

impl EntryId {
    /// Validates an entry ID.
    ///
    /// # Errors
    ///
    /// Fails if `s` does not match the grammar.
    pub fn new(s: &str) -> Result<Self, RegistryError> {
        let b = s.as_bytes();
        let ok = !b.is_empty()
            && b.len() <= 64
            && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
            && b.iter()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'_');
        if ok {
            Ok(Self(s.to_owned()))
        } else {
            Err(invalid(format!("invalid entry id {s:?}")))
        }
    }

    /// The ID as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for EntryId {
    type Error = RegistryError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(&s)
    }
}

impl From<EntryId> for String {
    fn from(id: EntryId) -> Self {
        id.0
    }
}

impl std::fmt::Display for EntryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn invalid(msg: String) -> RegistryError {
    RegistryError::Invalid(msg)
}

/// Broad class of an exposure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExposureClass {
    /// Crypto asset.
    Crypto,
    /// Equity index.
    EquityIndex,
    /// Single equity.
    Equity,
    /// Crude oil, refined products, natural gas, power.
    Energy,
    /// Precious and base metals.
    Metal,
    /// Grains, oilseeds, softs.
    Agriculture,
    /// Cattle, hogs, dairy.
    Livestock,
    /// Short-term interest rate.
    ShortRate,
    /// Longer-term rates and yields.
    Rates,
    /// Foreign exchange.
    Fx,
    /// Anything else (say what in `tracks`).
    Other,
}

/// What point on the curve an exposure is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExposureForm {
    /// The cash or spot level; no expiry.
    Spot,
    /// The front contract month, changing on the roll schedule.
    Front,
    /// Each listed contract month separately.
    Months,
}

/// How a venue market relates to an exposure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingRelation {
    /// The market follows the cash or spot level.
    TracksSpot,
    /// The market follows the front contract month.
    TracksFrontMonth,
    /// Related but not the same reference (for example another grade or
    /// region); never used as a price reference for the exposure.
    Proxy,
}

/// One venue market giving an exposure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VenueMapping {
    /// The venue market.
    pub instrument: InstrumentId,
    /// How it relates to the exposure.
    pub relation: MappingRelation,
    /// Exposure units per unit of the market's price (default 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiplier: Option<Price>,
    /// First date the mapping holds, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Last date the mapping holds, `YYYY-MM-DD`; absent while current.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

/// An economic exposure with a neutral name, and the venue markets that
/// give it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exposure {
    /// Neutral, stable ID (`us-large-cap-index-front`).
    pub id: EntryId,
    /// Neutral display name ("US large-cap index front").
    pub name: String,
    /// The economic reference, stated as a fact ("Tracks the front
    /// quarterly US large-cap equity index futures month").
    pub tracks: String,
    /// Broad class.
    pub class: ExposureClass,
    /// Spot, front month or every month.
    pub form: ExposureForm,
    /// Quote currency or unit of account (`USD`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    /// Unit of the price ("index points", "US dollars per barrel").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Session calendar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<EntryId>,
    /// Roll schedule; required for `front` and `months`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roll: Option<EntryId>,
    /// Venue markets giving the exposure.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub venue_mappings: Vec<VenueMapping>,
    /// Free-text note (provenance of the facts, caveats).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Day of the week.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Weekday {
    /// Monday.
    Mon,
    /// Tuesday.
    Tue,
    /// Wednesday.
    Wed,
    /// Thursday.
    Thu,
    /// Friday.
    Fri,
    /// Saturday.
    Sat,
    /// Sunday.
    Sun,
}

/// A named public holiday rule set (defined in
/// `docs/specs/method-synthetic-months-v1.md` § 5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HolidayRules {
    /// No holidays (continuous venues).
    None,
    /// US exchange trading days.
    UsTrading,
    /// US government securities (overnight-rate fixing) days.
    UsGovernmentSecurities,
}

/// A trading or publication session in the calendar's time zone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    /// Session name (`regular`, `fixing`).
    pub name: String,
    /// Opening wall time, `HH:MM`.
    pub open: String,
    /// Closing wall time, `HH:MM` (`24:00` for midnight). A close at or
    /// before the open falls on the next day.
    pub close: String,
}

/// A day that closes early.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EarlyClose {
    /// Date, `YYYY-MM-DD`.
    pub date: String,
    /// Closing wall time, `HH:MM`.
    pub close: String,
}

/// A session calendar: which days are open and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Calendar {
    /// ID (`us_trading`, `continuous`).
    pub id: EntryId,
    /// IANA time zone of the session times (`America/New_York`, `UTC`).
    pub time_zone: String,
    /// Days of the week that are open before holidays.
    pub weekdays: Vec<Weekday>,
    /// Public holiday rules.
    pub holidays: HolidayRules,
    /// Sessions on an open day. Empty: whole days only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<Session>,
    /// Extra closed dates (unscheduled closures), `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closures: Vec<String>,
    /// Holidays that are open in a given year, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub openings: Vec<String>,
    /// Early closes (operator-filled).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub early_closes: Vec<EarlyClose>,
    /// Free-text note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Rule for a contract month's last trading day. The same vocabulary as
/// `synthetic-months@1` (method spec § 5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "rule", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExpiryRule {
    /// The `n`-th `weekday` of the month `month_offset` after the contract
    /// month, then `business_days_before` business days earlier (with 0, a
    /// non-business day moves to the preceding business day).
    NthWeekday {
        /// Which occurrence, from 1.
        n: u32,
        /// Day of the week.
        weekday: Weekday,
        /// Months after the contract month.
        month_offset: i32,
        /// Business days to step back.
        business_days_before: u32,
    },
    /// Calendar day `day` of the month `month_offset` after the contract
    /// month (preceding business day if not one), then
    /// `business_days_before` business days earlier.
    BeforeDay {
        /// Calendar day of the month.
        day: u32,
        /// Months after the contract month.
        month_offset: i32,
        /// Business days to step back.
        business_days_before: u32,
    },
    /// The `n`-th last business day of the month `month_offset` after the
    /// contract month.
    NthLastBusinessDay {
        /// Which occurrence from the end, from 1.
        n: u32,
        /// Months after the contract month.
        month_offset: i32,
    },
}

/// What a roll is counted back from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollReference {
    /// The last trading day.
    LastTrade,
    /// The first calendar day of the contract month.
    MonthStart,
}

/// Fixed-days roll rule (`synthetic-months@1` § 6). Volume-based rolls
/// would need exchange volume and are not available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollRule {
    /// Business days before the reference day.
    pub business_days_before: u32,
    /// The reference day.
    pub reference: RollReference,
}

/// Contract months, their last trading days and when the front month
/// rolls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollSchedule {
    /// ID (`quarterly-third-friday`).
    pub id: EntryId,
    /// Business-day calendar.
    pub calendar: EntryId,
    /// Contract months (1 to 12, ascending).
    pub cycle: Vec<u8>,
    /// Last trading day.
    pub expiry: ExpiryRule,
    /// Wall time on the last trading day in the calendar's time zone,
    /// `HH:MM`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlement_time: Option<String>,
    /// Roll rule.
    pub roll: RollRule,
    /// Free-text note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A method in force over a date range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MethodPeriod {
    /// First date, `YYYY-MM-DD`.
    pub from: String,
    /// Last date, or `None` while current.
    pub to: Option<String>,
    /// The method as the source describes it.
    pub method: String,
}

/// A reference series family (the reference pollers' catalogue).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeriesEntry {
    /// Instrument, or a pattern with `<...>` placeholders.
    pub instrument: String,
    /// Record kind.
    pub kind: RecordKind,
    /// Source name.
    pub source: String,
    /// Admission.
    pub admission: AdmissionId,
    /// What the series is.
    pub description: String,
    /// Unit of the value.
    pub unit: String,
    /// Flags every record carries.
    pub flags: Flags,
    /// Notice travelling with the series.
    pub notice: String,
    /// Method periods, where the admission asks for them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<MethodPeriod>,
}

/// The latest liquidity grade of an instrument or exposure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiquidityEntry {
    /// Instrument ID or exposure ID.
    pub subject: String,
    /// Grade.
    pub grade: Grade,
    /// Benchmark or tape.
    pub label: LiquidityLabel,
    /// Time of the grade.
    pub as_of_ms: UnixMillis,
    /// Method that produced it (`liquidity-grade@1`).
    pub method: MethodId,
}

/// Everything in a registry document besides instruments.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sections {
    /// Exposures.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exposures: Vec<Exposure>,
    /// Session calendars.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calendars: Vec<Calendar>,
    /// Roll schedules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rolls: Vec<RollSchedule>,
    /// Reference series.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub series: Vec<SeriesEntry>,
    /// Liquidity grades.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub liquidity: Vec<LiquidityEntry>,
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        let t = s.get(r)?;
        t.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| t.parse().ok())?
    };
    matches!((num(0..4), num(5..7), num(8..10)), (Some(_), Some(m), Some(d)) if (1..=12).contains(&m) && (1..=31).contains(&d))
}

fn is_time(s: &str, allow_24: bool) -> bool {
    let b = s.as_bytes();
    if b.len() != 5
        || b[2] != b':'
        || !b
            .iter()
            .enumerate()
            .all(|(i, c)| i == 2 || c.is_ascii_digit())
    {
        return false;
    }
    let h = u32::from(b[0] - b'0') * 10 + u32::from(b[1] - b'0');
    let m = u32::from(b[3] - b'0') * 10 + u32::from(b[4] - b'0');
    (h < 24 && m < 60) || (allow_24 && h == 24 && m == 0)
}

fn check_text(what: &str, s: &str) -> Result<(), RegistryError> {
    if s.trim().is_empty() {
        return Err(invalid(format!("{what} is empty")));
    }
    Ok(())
}

fn check_date(what: &str, s: &str) -> Result<(), RegistryError> {
    if is_date(s) {
        Ok(())
    } else {
        Err(invalid(format!("{what}: {s:?} is not YYYY-MM-DD")))
    }
}

fn unique<'a>(what: &str, ids: impl Iterator<Item = &'a str>) -> Result<(), RegistryError> {
    let mut seen = BTreeSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(RegistryError::Duplicate(format!("{what} {id}")));
        }
    }
    Ok(())
}

impl Sections {
    /// True if every section is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.exposures.is_empty()
            && self.calendars.is_empty()
            && self.rolls.is_empty()
            && self.series.is_empty()
            && self.liquidity.is_empty()
    }

    /// Checks formats, unique IDs and references. `is_instrument` says
    /// whether an instrument ID is registered (for liquidity subjects).
    ///
    /// # Errors
    ///
    /// Fails on the first problem found.
    pub fn validate(&self, is_instrument: impl Fn(&str) -> bool) -> Result<(), RegistryError> {
        unique("calendar", self.calendars.iter().map(|c| c.id.as_str()))?;
        unique("roll", self.rolls.iter().map(|r| r.id.as_str()))?;
        unique("exposure", self.exposures.iter().map(|e| e.id.as_str()))?;
        let calendar = |id: &EntryId, who: &str| {
            if self.calendars.iter().any(|c| c.id == *id) {
                Ok(())
            } else {
                Err(invalid(format!("{who}: unknown calendar {id}")))
            }
        };

        for c in &self.calendars {
            let who = format!("calendar {}", c.id);
            check_text(&format!("{who} time_zone"), &c.time_zone)?;
            if c.weekdays.is_empty() || !c.weekdays.windows(2).all(|w| w[0] < w[1]) {
                return Err(invalid(format!(
                    "{who}: weekdays must be non-empty and in order Mon to Sun"
                )));
            }
            for s in &c.sessions {
                check_text(&format!("{who} session name"), &s.name)?;
                if !is_time(&s.open, false) || !is_time(&s.close, true) {
                    return Err(invalid(format!(
                        "{who} session {}: times must be HH:MM",
                        s.name
                    )));
                }
            }
            for d in c.closures.iter().chain(&c.openings) {
                check_date(&who, d)?;
            }
            for e in &c.early_closes {
                check_date(&who, &e.date)?;
                if !is_time(&e.close, false) {
                    return Err(invalid(format!(
                        "{who} early close {}: time must be HH:MM",
                        e.date
                    )));
                }
            }
        }

        for r in &self.rolls {
            let who = format!("roll {}", r.id);
            calendar(&r.calendar, &who)?;
            if r.cycle.is_empty()
                || !r.cycle.iter().all(|m| (1..=12).contains(m))
                || !r.cycle.windows(2).all(|w| w[0] < w[1])
            {
                return Err(invalid(format!(
                    "{who}: cycle must be ascending months 1 to 12"
                )));
            }
            let n_ok = match r.expiry {
                ExpiryRule::NthWeekday { n, .. } => (1..=5).contains(&n),
                ExpiryRule::BeforeDay { day, .. } => (1..=31).contains(&day),
                ExpiryRule::NthLastBusinessDay { n, .. } => n >= 1,
            };
            if !n_ok {
                return Err(invalid(format!("{who}: expiry rule out of range")));
            }
            if let Some(t) = &r.settlement_time
                && !is_time(t, false)
            {
                return Err(invalid(format!("{who}: settlement_time must be HH:MM")));
            }
        }

        for e in &self.exposures {
            let who = format!("exposure {}", e.id);
            check_text(&format!("{who} name"), &e.name)?;
            check_text(&format!("{who} tracks"), &e.tracks)?;
            if let Some(c) = &e.calendar {
                calendar(c, &who)?;
            }
            match (&e.roll, e.form) {
                (Some(r), _) => {
                    if !self.rolls.iter().any(|x| x.id == *r) {
                        return Err(invalid(format!("{who}: unknown roll {r}")));
                    }
                }
                (None, ExposureForm::Front | ExposureForm::Months) => {
                    return Err(invalid(format!("{who}: form {:?} needs a roll", e.form)));
                }
                (None, ExposureForm::Spot) => {}
            }
            for m in &e.venue_mappings {
                if let Some(x) = &m.multiplier
                    && x.mantissa() <= 0
                {
                    return Err(invalid(format!(
                        "{who} {}: multiplier must be positive",
                        m.instrument
                    )));
                }
                for d in m.from.iter().chain(&m.to) {
                    check_date(&format!("{who} {}", m.instrument), d)?;
                }
                if let (Some(f), Some(t)) = (&m.from, &m.to)
                    && f > t
                {
                    return Err(invalid(format!("{who} {}: from after to", m.instrument)));
                }
            }
        }

        let series_keys: Vec<String> = self
            .series
            .iter()
            .map(|s| format!("{} ({})", s.instrument, s.kind.as_str()))
            .collect();
        unique("series", series_keys.iter().map(String::as_str))?;
        for s in &self.series {
            let who = format!("series {}", s.instrument);
            check_text(&who, &s.instrument)?;
            check_text(&format!("{who} notice"), &s.notice)?;
            for m in &s.methods {
                check_date(&who, &m.from)?;
                if let Some(t) = &m.to {
                    check_date(&who, t)?;
                }
            }
        }

        unique(
            "liquidity subject",
            self.liquidity.iter().map(|l| l.subject.as_str()),
        )?;
        for l in &self.liquidity {
            let known = if l.subject.contains(':') {
                is_instrument(&l.subject)
            } else {
                self.exposures.iter().any(|e| e.id.as_str() == l.subject)
            };
            if !known {
                return Err(invalid(format!(
                    "liquidity: {} is not a registered instrument or exposure",
                    l.subject
                )));
            }
        }
        Ok(())
    }
}
