//! Bars built from trades.
//!
//! - [`time`]: time bars (`Bar`, method `bars@1`) for any
//!   [`mesh_types::BarInterval`], aligned to the Unix epoch
//!   (UTC), with no bar for an interval without trades.
//! - [`mod@rollup`]: 5m, 15m, 1h and 1d bars rolled up from 1m bars, equal to
//!   computing them directly from the trades, and the [`BarEngine`] that
//!   streams them.
//! - [`activity`]: tick, volume and range bars ([`ActivityBar`]).

pub mod activity;
pub mod rollup;
pub mod time;

use mesh_types::{
    AggressorSide, Bar, BarInterval, DerivedHeader, Flags, Provenance, Seq, Trade, UnixMillis,
};

pub use activity::{ActivityBar, ActivityBarBuilder, ActivitySpec, activity_bars};
pub use rollup::{BarEngine, BarEngineConfig, rollup};
pub use time::{TimeBarBuilder, time_bars};

use crate::error::InputError;
use crate::fixed::{add, inc, price, same_scale, size, union};

/// Open, high, low, close and volumes of a run of trades, as mantissas.
#[derive(Debug, Clone)]
pub(crate) struct Ohlcv {
    pub(crate) open: i64,
    pub(crate) high: i64,
    pub(crate) low: i64,
    pub(crate) close: i64,
    pub(crate) volume: i64,
    pub(crate) buy: i64,
    pub(crate) sell: i64,
    pub(crate) count: u32,
    pub(crate) price_scale: u8,
    pub(crate) size_scale: u8,
    pub(crate) first_seq: Seq,
    pub(crate) first_ms: UnixMillis,
    pub(crate) last: Provenance,
    pub(crate) flags: Flags,
}

impl Ohlcv {
    pub(crate) fn new(t: &Trade) -> Self {
        let p = t.price.mantissa();
        let s = t.size.mantissa();
        Self {
            open: p,
            high: p,
            low: p,
            close: p,
            volume: s,
            buy: if t.aggressor == AggressorSide::Buy {
                s
            } else {
                0
            },
            sell: if t.aggressor == AggressorSide::Sell {
                s
            } else {
                0
            },
            count: 1,
            price_scale: t.price.scale(),
            size_scale: t.size.scale(),
            first_seq: t.prov.seq,
            first_ms: t.prov.ts_source_ms,
            last: t.prov.clone(),
            flags: t.prov.flags,
        }
    }

    pub(crate) fn push(&mut self, t: &Trade) -> Result<(), InputError> {
        same_scale("price", self.price_scale, t.price.scale())?;
        same_scale("size", self.size_scale, t.size.scale())?;
        let p = t.price.mantissa();
        let s = t.size.mantissa();
        self.high = self.high.max(p);
        self.low = self.low.min(p);
        self.close = p;
        self.volume = add(self.volume, s)?;
        match t.aggressor {
            AggressorSide::Buy => self.buy = add(self.buy, s)?,
            AggressorSide::Sell => self.sell = add(self.sell, s)?,
            AggressorSide::Unknown => {}
        }
        self.count = inc(self.count)?;
        self.flags = union(self.flags, t.prov.flags);
        self.last = t.prov.clone();
        Ok(())
    }

    /// Appends a later run.
    pub(crate) fn merge(&mut self, later: &Ohlcv) -> Result<(), InputError> {
        same_scale("price", self.price_scale, later.price_scale)?;
        same_scale("size", self.size_scale, later.size_scale)?;
        self.high = self.high.max(later.high);
        self.low = self.low.min(later.low);
        self.close = later.close;
        self.volume = add(self.volume, later.volume)?;
        self.buy = add(self.buy, later.buy)?;
        self.sell = add(self.sell, later.sell)?;
        self.count = self
            .count
            .checked_add(later.count)
            .ok_or(InputError::Overflow)?;
        self.flags = union(self.flags, later.flags);
        self.last = later.last.clone();
        Ok(())
    }

    /// The run of a bar (first `seq` and time are not recorded in a bar:
    /// the bar's own `seq` and start stand in).
    pub(crate) fn from_bar(b: &Bar) -> Self {
        Self {
            open: b.open.mantissa(),
            high: b.high.mantissa(),
            low: b.low.mantissa(),
            close: b.close.mantissa(),
            volume: b.volume.mantissa(),
            buy: b.buy_volume.mantissa(),
            sell: b.sell_volume.mantissa(),
            count: b.trade_count,
            price_scale: b.open.scale(),
            size_scale: b.volume.scale(),
            first_seq: b.prov.seq,
            first_ms: b.start_ms,
            last: b.prov.clone(),
            flags: b.prov.flags,
        }
    }

    /// Provenance of a derived record over this run: the last input's,
    /// with the union of every input's flags and no publish time.
    pub(crate) fn prov(&self) -> Provenance {
        let mut p = self.last.clone();
        p.flags = self.flags;
        p.ts_publish_ns = None;
        p
    }

    pub(crate) fn to_bar(
        &self,
        interval: BarInterval,
        start_ms: UnixMillis,
        derived: DerivedHeader,
    ) -> Result<Bar, InputError> {
        let ps = self.price_scale;
        let ss = self.size_scale;
        Ok(Bar {
            prov: self.prov(),
            derived,
            interval,
            start_ms,
            open: price(self.open, ps)?,
            high: price(self.high, ps)?,
            low: price(self.low, ps)?,
            close: price(self.close, ps)?,
            volume: size(self.volume, ss)?,
            buy_volume: size(self.buy, ss)?,
            sell_volume: size(self.sell, ss)?,
            trade_count: self.count,
        })
    }
}
