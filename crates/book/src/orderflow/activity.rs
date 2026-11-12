//! Order activity (`order-activity@1`): order lifetimes and cancel/replace
//! ratios per time interval, from book events.
//!
//! Each event is counted in the interval containing its block time. For
//! every interval with events, one [`OrderActivity`]:
//!
//! - counts of `add`, `modify`, `cancel`, partial `fill` (size left) and
//!   full `fill` (size zero) events, and the added, cancelled and filled
//!   sizes (filled size is the resting size before the fill minus
//!   `new_size`; cancelled size is the resting size before the cancel);
//! - `cancel_to_add_bps = floor(cancels * 10_000 / adds)` and
//!   `cancel_replace_to_fill_bps = floor((cancels + modifies) * 10_000 /
//!   (partial fills + full fills))`, absent when the divisor is zero;
//! - lifetimes of orders removed in the interval (full fill or cancel):
//!   `lifetime = removal block time - placement time`, where placement time
//!   is the snapshot's `ts_order_ms` or the block time of the `add`. Min,
//!   nearest-rank median and 90th percentile (`sorted[ceil(p * n / 100) -
//!   1]`), max, and the number of cancels with a lifetime below
//!   `fleeting_ms`.
//!
//! The inputs hash covers the interval's book events in `seq` order. The
//! sizes and lifetimes also depend on the book before the interval, which
//! is fixed by the archive (nearest checkpoint plus events).

/// Record types, defined in `mesh-types`.
pub use mesh_types::{LifetimeStats, OrderActivity};

use mesh_types::{BarInterval, BookAction, ContentHasher, Flags, Provenance, UnixMillis};
use serde::{Deserialize, Serialize};

use crate::book::OrderView;
use crate::error::InputError;
use crate::fixed::{add, ratio_bps, size, union};
use crate::inputs::{Cursor, EventIn, header};
use crate::methods;

/// Order activity settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderActivityConfig {
    /// Interval of each record.
    pub interval: BarInterval,
    /// Cancels of orders younger than this count as fleeting.
    pub fleeting_ms: i64,
}

impl Default for OrderActivityConfig {
    /// One-minute intervals; fleeting below one second.
    fn default() -> Self {
        Self {
            interval: BarInterval::M1,
            fleeting_ms: 1_000,
        }
    }
}

#[derive(Debug, Clone)]
struct Open {
    start: UnixMillis,
    adds: u64,
    modifies: u64,
    cancels: u64,
    partial: u64,
    full: u64,
    added: i64,
    cancelled: i64,
    filled: i64,
    fleeting: u64,
    lifetimes: Vec<i64>,
    hasher: ContentHasher,
    last: Provenance,
    flags: Flags,
}

/// Streams order activity.
#[derive(Debug, Clone)]
pub struct OrderActivityBuilder {
    config: OrderActivityConfig,
    size_scale: Option<u8>,
    open: Option<Open>,
    cursor: Cursor,
}

impl OrderActivityBuilder {
    /// A builder for `config`.
    #[must_use]
    pub fn new(config: OrderActivityConfig) -> Self {
        Self {
            config,
            size_scale: None,
            open: None,
            cursor: Cursor::default(),
        }
    }

    /// Records an event that the book accepted. `before` is the order as
    /// the book held it before the event (`None` for an `add`).
    ///
    /// # Errors
    ///
    /// Fails on out-of-order input or overflow.
    pub fn observe(
        &mut self,
        e: EventIn<'_>,
        before: Option<&OrderView>,
        out: &mut Vec<OrderActivity>,
    ) -> Result<(), InputError> {
        let ev = e.event;
        self.cursor.check(&ev.prov)?;
        if let Some(s) = ev.action.size_after().or(before.map(|b| b.size)) {
            self.size_scale.get_or_insert(s.scale());
        }
        let start = self.config.interval.align(ev.prov.ts_source_ms);
        if self.open.as_ref().is_none_or(|o| o.start != start) {
            self.close(out)?;
            self.open = Some(Open {
                start,
                adds: 0,
                modifies: 0,
                cancels: 0,
                partial: 0,
                full: 0,
                added: 0,
                cancelled: 0,
                filled: 0,
                fleeting: 0,
                lifetimes: Vec::new(),
                hasher: ContentHasher::new(),
                last: ev.prov.clone(),
                flags: Flags::NONE,
            });
        }
        let Some(o) = &mut self.open else {
            return Ok(());
        };
        let before_size = before.map_or(0, |b| b.size.mantissa());
        let lifetime = before.map(|b| (ev.prov.ts_source_ms.0 - b.ts_order_ms.0).max(0));
        match ev.action {
            BookAction::Add { size } => {
                o.adds += 1;
                o.added = add(o.added, size.mantissa())?;
            }
            BookAction::Modify { .. } => o.modifies += 1,
            BookAction::Fill { new_size, .. } => {
                o.filled = add(
                    o.filled,
                    before_size.saturating_sub(new_size.mantissa()).max(0),
                )?;
                if new_size.is_zero() {
                    o.full += 1;
                    if let Some(l) = lifetime {
                        o.lifetimes.push(l);
                    }
                } else {
                    o.partial += 1;
                }
            }
            BookAction::Cancel {} => {
                o.cancels += 1;
                o.cancelled = add(o.cancelled, before_size)?;
                if let Some(l) = lifetime {
                    o.lifetimes.push(l);
                    if l < self.config.fleeting_ms {
                        o.fleeting += 1;
                    }
                }
            }
        }
        o.hasher.update_canonical(e.canonical);
        o.last = ev.prov.clone();
        o.flags = union(o.flags, ev.prov.flags);
        Ok(())
    }

    /// Emits the open record if its interval ends at or before `ts`.
    ///
    /// # Errors
    ///
    /// Fails if a value cannot be represented.
    pub fn advance_to(
        &mut self,
        ts: UnixMillis,
        out: &mut Vec<OrderActivity>,
    ) -> Result<(), InputError> {
        self.cursor.advance(ts);
        if self
            .open
            .as_ref()
            .is_some_and(|o| o.start.0.saturating_add(self.config.interval.millis()) <= ts.0)
        {
            self.close(out)?;
        }
        Ok(())
    }

    /// Emits the open record, if any.
    ///
    /// # Errors
    ///
    /// Fails if a value cannot be represented.
    pub fn finish(&mut self, out: &mut Vec<OrderActivity>) -> Result<(), InputError> {
        self.close(out)
    }

    /// Drops the open interval (the book desynchronised: its sizes and
    /// lifetimes cannot be trusted).
    pub fn discard(&mut self) {
        self.open = None;
    }

    fn close(&mut self, out: &mut Vec<OrderActivity>) -> Result<(), InputError> {
        let Some(mut o) = self.open.take() else {
            return Ok(());
        };
        let ss = self.size_scale.unwrap_or(0);
        o.lifetimes.sort_unstable();
        let n = o.lifetimes.len();
        let rank = |p: usize| -> Option<i64> {
            if n == 0 {
                return None;
            }
            let r = (p * n).div_ceil(100).max(1);
            o.lifetimes.get(r - 1).copied()
        };
        let lifetimes = LifetimeStats {
            removed: o.full + o.cancels,
            filled: o.full,
            cancelled: o.cancels,
            fleeting_cancels: o.fleeting,
            min_ms: o.lifetimes.first().copied(),
            p50_ms: rank(50),
            p90_ms: rank(90),
            max_ms: o.lifetimes.last().copied(),
        };
        let mut prov = o.last;
        prov.flags = o.flags;
        prov.ts_publish_ns = None;
        out.push(OrderActivity {
            prov,
            derived: header(methods::ORDER_ACTIVITY, o.hasher)?,
            interval: self.config.interval,
            start_ms: o.start,
            adds: o.adds,
            modifies: o.modifies,
            cancels: o.cancels,
            partial_fills: o.partial,
            full_fills: o.full,
            added_size: size(o.added, ss)?,
            cancelled_size: size(o.cancelled, ss)?,
            filled_size: size(o.filled, ss)?,
            cancel_to_add_bps: ratio_bps(o.cancels, o.adds),
            cancel_replace_to_fill_bps: ratio_bps(o.cancels + o.modifies, o.partial + o.full),
            lifetimes,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::OrderBook;
    use crate::inputs::canonical_event;
    use crate::testutil::add;
    use crate::testutil::*;
    use mesh_types::{BookEvent, BookSide::*};

    fn run(events: &[BookEvent]) -> Vec<OrderActivity> {
        let mut book = OrderBook::new(inst());
        let mut b = OrderActivityBuilder::new(OrderActivityConfig::default());
        let mut out = Vec::new();
        for e in events {
            let before = book.order(e.order_id);
            book.apply(e).unwrap();
            let c = canonical_event(e).unwrap();
            b.observe(
                EventIn {
                    event: e,
                    canonical: &c,
                },
                before.as_ref(),
                &mut out,
            )
            .unwrap();
        }
        b.finish(&mut out).unwrap();
        out
    }

    #[test]
    fn counts_ratios_and_lifetimes() {
        // Block b has time BASE + b seconds.
        let evs = vec![
            add(1, 0, 1, Bid, "100.0", "1.00"),
            add(1, 1, 2, Bid, "99.0", "2.00"),
            add(1, 2, 3, Ask, "101.0", "1.00"),
            add(1, 3, 4, Ask, "102.0", "1.00"),
            modify(2, 0, 2, Bid, "99.0", Some("2.00"), "1.50"),
            fill(2, 1, 3, Ask, "101.0", Some("1.00"), "0.40"),
            cancel(3, 0, 4, Ask, "102.0"),
            fill(11, 0, 3, Ask, "101.0", None, "0.00"),
            cancel(21, 0, 1, Bid, "100.0"),
        ];
        let out = run(&evs);
        assert_eq!(out.len(), 1);
        let a = &out[0];
        assert_eq!((a.adds, a.modifies, a.cancels), (4, 1, 2));
        assert_eq!((a.partial_fills, a.full_fills), (1, 1));
        assert_eq!(a.added_size, sz("5.00"));
        assert_eq!(a.cancelled_size, sz("2.00"));
        assert_eq!(a.filled_size, sz("1.00"));
        assert_eq!(a.cancel_to_add_bps, Some(5_000));
        assert_eq!(a.cancel_replace_to_fill_bps, Some(15_000));
        // Lifetimes: order 4 2s, order 3 10s, order 1 20s.
        let l = a.lifetimes;
        assert_eq!((l.removed, l.filled, l.cancelled), (3, 1, 2));
        assert_eq!(l.min_ms, Some(2_000));
        assert_eq!(l.p50_ms, Some(10_000));
        assert_eq!(l.p90_ms, Some(20_000));
        assert_eq!(l.max_ms, Some(20_000));
        assert_eq!(l.fleeting_cancels, 0);
        assert_eq!(a.derived.input_count, 9);
    }

    #[test]
    fn intervals_split_and_fleeting() {
        let evs = vec![
            add(1, 0, 1, Bid, "100.0", "1.00"),
            cancel(1, 1, 1, Bid, "100.0"),
            add(70, 0, 2, Bid, "100.0", "1.00"),
        ];
        let out = run(&evs);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].lifetimes.fleeting_cancels, 1);
        assert_eq!(out[0].lifetimes.min_ms, Some(0));
        assert_eq!(out[1].cancel_to_add_bps, Some(0));
        assert_eq!(out[1].cancel_replace_to_fill_bps, None);
    }
}
