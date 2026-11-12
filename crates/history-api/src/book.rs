//! Order-level book reconstruction: book at time T is the nearest
//! checkpoint snapshot at or before T plus every book event after it up to
//! T (observation-v0 § 2.4: events with a `seq` greater than the
//! snapshot's end-of-block marker apply on top of it).
//!
//! The book keeps price-time priority (orders at one price in arrival
//! order; a modify keeps its place) and an aggregated level view, so the
//! Databento-shaped MBP schemas can read the top levels after every event
//! cheaply.

use std::collections::{BTreeMap, HashMap};

use mesh_types::{
    BookAction, BookEvent, BookSide, BookSnapshot, Level, OrderId, Price, Provenance, RestingOrder,
    Seq, Size, SnapshotDepth, UnixMillis,
};
use serde::Serialize;

use crate::error::{ApiError, ApiResult};

#[derive(Debug, Clone, Copy)]
struct Slot {
    oid: u64,
    size: i64,
    ts_ms: i64,
}

#[derive(Debug, Default, Clone)]
struct Side {
    /// Price mantissa to orders in queue order.
    queues: BTreeMap<i64, Vec<Slot>>,
    /// Price mantissa to (total size, order count).
    levels: BTreeMap<i64, (i64, u32)>,
}

impl Side {
    fn add(&mut self, price: i64, slot: Slot) {
        self.queues.entry(price).or_default().push(slot);
        let l = self.levels.entry(price).or_insert((0, 0));
        l.0 += slot.size;
        l.1 += 1;
    }

    fn remove(&mut self, price: i64, oid: u64) -> Option<Slot> {
        let q = self.queues.get_mut(&price)?;
        let pos = q.iter().position(|s| s.oid == oid)?;
        let slot = q.remove(pos);
        if q.is_empty() {
            self.queues.remove(&price);
        }
        if let Some(l) = self.levels.get_mut(&price) {
            l.0 -= slot.size;
            l.1 = l.1.saturating_sub(1);
            if l.1 == 0 {
                self.levels.remove(&price);
            }
        }
        Some(slot)
    }

    fn resize(&mut self, price: i64, oid: u64, new_size: i64) -> Option<i64> {
        let slot = self
            .queues
            .get_mut(&price)?
            .iter_mut()
            .find(|s| s.oid == oid)?;
        let old = slot.size;
        slot.size = new_size;
        if let Some(l) = self.levels.get_mut(&price) {
            l.0 += new_size - old;
        }
        Some(old)
    }
}

/// Counters for events that did not fit the book (unknown order IDs and
/// similar). The book never invents data to make them fit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Inconsistencies {
    /// Modify, fill or cancel for an order not in the book.
    pub unknown_order: u64,
    /// Add for an order ID already in the book.
    pub duplicate_add: u64,
}

impl Inconsistencies {
    /// Whether anything was counted.
    #[must_use]
    pub const fn any(&self) -> bool {
        self.unknown_order > 0 || self.duplicate_add > 0
    }
}

/// What an applied event did, for MBO/MBP encoders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Applied {
    /// Size before the event (`None` for an add or an unknown order).
    pub before: Option<i64>,
    /// Size after the event (0 when removed).
    pub after: i64,
    /// The event fitted the book.
    pub consistent: bool,
}

/// An order-level book.
#[derive(Debug, Clone, Default)]
pub struct L4Book {
    bids: Side,
    asks: Side,
    index: HashMap<u64, (BookSide, i64)>,
    price_scale: u8,
    size_scale: u8,
    /// Last applied position.
    pub last_seq: Seq,
    /// Block time of the last applied position.
    pub last_ts_ms: i64,
    /// Ingest time of the last applied event (0 before any event).
    pub last_ingest_ns: i64,
    /// Events applied since the checkpoint.
    pub events_applied: u64,
    /// Events that did not fit.
    pub inconsistencies: Inconsistencies,
}

impl L4Book {
    /// An empty book with the instrument's scales.
    #[must_use]
    pub fn empty(price_scale: u8, size_scale: u8) -> Self {
        Self {
            price_scale,
            size_scale,
            ..Self::default()
        }
    }

    /// Starts from an L4 snapshot.
    ///
    /// # Errors
    ///
    /// `unsupported` for an L2 snapshot: order-level events cannot apply on
    /// top of aggregated levels.
    pub fn from_snapshot(s: &BookSnapshot) -> ApiResult<Self> {
        let SnapshotDepth::L4 { bids, asks } = &s.depth else {
            return Err(ApiError::new(
                crate::error::ErrorCode::Unsupported,
                "the checkpoint is an L2 snapshot; order-level events cannot be applied to it",
            ));
        };
        let scale_src = bids.first().or_else(|| asks.first());
        let mut book = Self::empty(
            scale_src.map_or(0, |o| o.price.scale()),
            scale_src.map_or(0, |o| o.size.scale()),
        );
        // Bids are best first; queue order within a price is preserved.
        for (side, orders) in [(BookSide::Bid, bids), (BookSide::Ask, asks)] {
            for o in orders {
                book.insert(
                    side,
                    o.price.mantissa(),
                    o.order_id.0,
                    o.size.mantissa(),
                    o.ts_order_ms.0,
                );
            }
        }
        book.last_seq = s.prov.seq;
        book.last_ts_ms = s.prov.ts_source_ms.0;
        Ok(book)
    }

    fn side_mut(&mut self, side: BookSide) -> &mut Side {
        match side {
            BookSide::Bid => &mut self.bids,
            BookSide::Ask => &mut self.asks,
        }
    }

    fn side(&self, side: BookSide) -> &Side {
        match side {
            BookSide::Bid => &self.bids,
            BookSide::Ask => &self.asks,
        }
    }

    fn insert(&mut self, side: BookSide, price: i64, oid: u64, size: i64, ts_ms: i64) {
        self.index.insert(oid, (side, price));
        self.side_mut(side).add(price, Slot { oid, size, ts_ms });
    }

    /// Size of a resting order.
    #[must_use]
    pub fn order_size(&self, oid: u64) -> Option<i64> {
        let (side, price) = self.index.get(&oid)?;
        self.side(*side)
            .queues
            .get(price)?
            .iter()
            .find(|s| s.oid == oid)
            .map(|s| s.size)
    }

    /// Applies one event. Unknown orders are counted, not invented.
    pub fn apply(&mut self, e: &BookEvent) -> Applied {
        if self.price_scale == 0 && self.size_scale == 0 && self.index.is_empty() {
            self.price_scale = e.price.scale();
            if let Some(s) = e.action.size_after() {
                self.size_scale = s.scale();
            }
        }
        self.last_seq = e.prov.seq;
        self.last_ts_ms = e.prov.ts_source_ms.0;
        self.last_ingest_ns = e.prov.ts_ingest_ns.0;
        self.events_applied += 1;
        let oid = e.order_id.0;
        let located = self.index.get(&oid).copied();
        match e.action {
            BookAction::Add { size } => {
                if located.is_some() {
                    self.inconsistencies.duplicate_add += 1;
                    return Applied {
                        before: None,
                        after: size.mantissa(),
                        consistent: false,
                    };
                }
                self.insert(
                    e.side,
                    e.price.mantissa(),
                    oid,
                    size.mantissa(),
                    e.prov.ts_source_ms.0,
                );
                Applied {
                    before: None,
                    after: size.mantissa(),
                    consistent: true,
                }
            }
            BookAction::Modify { new_size, .. } | BookAction::Fill { new_size, .. } => {
                let Some((side, price)) = located else {
                    self.inconsistencies.unknown_order += 1;
                    return Applied {
                        before: None,
                        after: new_size.mantissa(),
                        consistent: false,
                    };
                };
                if new_size.mantissa() == 0 {
                    let before = self.side_mut(side).remove(price, oid).map(|s| s.size);
                    self.index.remove(&oid);
                    Applied {
                        before,
                        after: 0,
                        consistent: true,
                    }
                } else {
                    let before = self.side_mut(side).resize(price, oid, new_size.mantissa());
                    Applied {
                        before,
                        after: new_size.mantissa(),
                        consistent: true,
                    }
                }
            }
            BookAction::Cancel {} => {
                let Some((side, price)) = located else {
                    self.inconsistencies.unknown_order += 1;
                    return Applied {
                        before: None,
                        after: 0,
                        consistent: false,
                    };
                };
                let before = self.side_mut(side).remove(price, oid).map(|s| s.size);
                self.index.remove(&oid);
                Applied {
                    before,
                    after: 0,
                    consistent: true,
                }
            }
        }
    }

    /// Aggregated levels, best first: `(price, size, count)`.
    #[must_use]
    pub fn levels(&self, side: BookSide, n: usize) -> Vec<(i64, i64, u32)> {
        let s = self.side(side);
        let map = |(p, (sz, c)): (&i64, &(i64, u32))| (*p, *sz, *c);
        match side {
            BookSide::Bid => s.levels.iter().rev().take(n).map(map).collect(),
            BookSide::Ask => s.levels.iter().take(n).map(map).collect(),
        }
    }

    /// Number of resting orders.
    #[must_use]
    pub fn order_count(&self) -> usize {
        self.index.len()
    }

    /// Resting orders, best first and in queue order:
    /// `(side, order_id, price, size, ts_ms)`.
    #[must_use]
    pub fn orders(&self) -> Vec<(BookSide, u64, i64, i64, i64)> {
        let mut out = Vec::with_capacity(self.index.len());
        for (p, q) in self.bids.queues.iter().rev() {
            out.extend(
                q.iter()
                    .map(|s| (BookSide::Bid, s.oid, *p, s.size, s.ts_ms)),
            );
        }
        for (p, q) in &self.asks.queues {
            out.extend(
                q.iter()
                    .map(|s| (BookSide::Ask, s.oid, *p, s.size, s.ts_ms)),
            );
        }
        out
    }

    /// The book as an L4 or L2 snapshot depth, limited to `max_levels`
    /// price levels per side (all when `None`).
    ///
    /// # Errors
    ///
    /// `internal` if a stored value no longer fits the decimal type.
    pub fn depth(&self, l2: bool, max_levels: Option<usize>) -> ApiResult<SnapshotDepth> {
        let (ps, ss) = (self.price_scale, self.size_scale);
        let px = |m: i64| Price::new(m, ps).map_err(ApiError::internal);
        let sz = |m: i64| Size::new(m, ss).map_err(ApiError::internal);
        let n = max_levels.unwrap_or(usize::MAX);
        if l2 {
            let conv = |side| -> ApiResult<Vec<Level>> {
                self.levels(side, n)
                    .into_iter()
                    .map(|(p, s, c)| {
                        Ok(Level {
                            price: px(p)?,
                            size: sz(s)?,
                            order_count: c,
                        })
                    })
                    .collect()
            };
            return Ok(SnapshotDepth::L2 {
                bids: conv(BookSide::Bid)?,
                asks: conv(BookSide::Ask)?,
            });
        }
        let conv = |side: BookSide| -> ApiResult<Vec<RestingOrder>> {
            let s = self.side(side);
            let iter: Box<dyn Iterator<Item = (&i64, &Vec<Slot>)>> = match side {
                BookSide::Bid => Box::new(s.queues.iter().rev()),
                BookSide::Ask => Box::new(s.queues.iter()),
            };
            let mut out = Vec::new();
            for (p, q) in iter.take(n) {
                for slot in q {
                    out.push(RestingOrder {
                        order_id: OrderId(slot.oid),
                        price: px(*p)?,
                        size: sz(slot.size)?,
                        ts_order_ms: UnixMillis(slot.ts_ms),
                    });
                }
            }
            Ok(out)
        };
        Ok(SnapshotDepth::L4 {
            bids: conv(BookSide::Bid)?,
            asks: conv(BookSide::Ask)?,
        })
    }

    /// The book as a `book_snapshot` record positioned at the end of the
    /// last applied block.
    ///
    /// # Errors
    ///
    /// As [`L4Book::depth`].
    pub fn to_snapshot(
        &self,
        template: &Provenance,
        l2: bool,
        max_levels: Option<usize>,
    ) -> ApiResult<BookSnapshot> {
        let mut prov = template.clone();
        prov.seq = Seq::end_of_block(self.last_seq.block_number);
        prov.ts_source_ms = UnixMillis(self.last_ts_ms);
        if self.events_applied > 0 {
            prov.ts_ingest_ns = mesh_types::UnixNanos(self.last_ingest_ns);
        }
        prov.ts_publish_ns = None;
        Ok(BookSnapshot {
            prov,
            depth: self.depth(l2, max_levels)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesh_types::{AdmissionId, InstrumentId, UnixNanos, VenueId};

    fn prov(block: u64, idx: u32) -> Provenance {
        Provenance::new(
            InstrumentId::new("hyperliquid:perp:SYN").unwrap(),
            VenueId::new("hyperliquid").unwrap(),
            Seq::new(block, idx),
            UnixMillis(1_000 + i64::try_from(block).unwrap()),
            UnixNanos(1),
            AdmissionId::new("ADM-0001").unwrap(),
        )
    }

    fn ev(
        block: u64,
        idx: u32,
        oid: u64,
        side: BookSide,
        price: i64,
        action: BookAction,
    ) -> BookEvent {
        BookEvent {
            prov: prov(block, idx),
            order_id: OrderId(oid),
            side,
            price: Price::new(price, 2).unwrap(),
            action,
        }
    }

    fn s(m: i64) -> Size {
        Size::new(m, 1).unwrap()
    }

    #[test]
    fn applies_events_with_price_time_priority() {
        let mut b = L4Book::empty(2, 1);
        b.apply(&ev(
            1,
            0,
            1,
            BookSide::Bid,
            100,
            BookAction::Add { size: s(10) },
        ));
        b.apply(&ev(
            1,
            1,
            2,
            BookSide::Bid,
            100,
            BookAction::Add { size: s(5) },
        ));
        b.apply(&ev(
            1,
            2,
            3,
            BookSide::Bid,
            101,
            BookAction::Add { size: s(7) },
        ));
        b.apply(&ev(
            1,
            3,
            4,
            BookSide::Ask,
            105,
            BookAction::Add { size: s(2) },
        ));
        assert_eq!(b.levels(BookSide::Bid, 5), vec![(101, 7, 1), (100, 15, 2)]);
        // Modify keeps queue position.
        let a = b.apply(&ev(
            2,
            0,
            1,
            BookSide::Bid,
            100,
            BookAction::Modify {
                orig_size: None,
                new_size: s(3),
            },
        ));
        assert_eq!(a.before, Some(10));
        let orders = b.orders();
        assert_eq!(orders[1].1, 1);
        assert_eq!(orders[2].1, 2);
        // Partial and full fills.
        b.apply(&ev(
            2,
            1,
            3,
            BookSide::Bid,
            101,
            BookAction::Fill {
                orig_size: Some(s(7)),
                new_size: s(2),
            },
        ));
        assert_eq!(b.order_size(3), Some(2));
        let a = b.apply(&ev(
            2,
            2,
            3,
            BookSide::Bid,
            101,
            BookAction::Fill {
                orig_size: Some(s(2)),
                new_size: s(0),
            },
        ));
        assert_eq!((a.before, a.after), (Some(2), 0));
        assert_eq!(b.levels(BookSide::Bid, 1), vec![(100, 8, 2)]);
        // Cancel and unknown orders.
        b.apply(&ev(3, 0, 4, BookSide::Ask, 105, BookAction::Cancel {}));
        assert!(b.levels(BookSide::Ask, 5).is_empty());
        let a = b.apply(&ev(3, 1, 99, BookSide::Ask, 105, BookAction::Cancel {}));
        assert!(!a.consistent);
        assert_eq!(b.inconsistencies.unknown_order, 1);
        assert_eq!(b.events_applied, 9);
        assert_eq!(b.last_seq, Seq::new(3, 1));
    }

    #[test]
    fn snapshots_round_trip() {
        let mut b = L4Book::empty(2, 1);
        b.apply(&ev(
            1,
            0,
            1,
            BookSide::Bid,
            100,
            BookAction::Add { size: s(10) },
        ));
        b.apply(&ev(
            1,
            1,
            2,
            BookSide::Ask,
            110,
            BookAction::Add { size: s(4) },
        ));
        b.apply(&ev(
            1,
            2,
            3,
            BookSide::Ask,
            110,
            BookAction::Add { size: s(6) },
        ));
        let snap = b.to_snapshot(&prov(1, 0), false, None).unwrap();
        snap.validate().unwrap();
        assert_eq!(snap.prov.seq, Seq::end_of_block(1));
        let back = L4Book::from_snapshot(&snap).unwrap();
        assert_eq!(back.orders(), b.orders());
        let l2 = b.to_snapshot(&prov(1, 0), true, Some(1)).unwrap();
        l2.validate().unwrap();
        match &l2.depth {
            SnapshotDepth::L2 { asks, .. } => {
                assert_eq!(asks.len(), 1);
                assert_eq!(asks[0].order_count, 2);
                assert_eq!(asks[0].size, s(10));
            }
            SnapshotDepth::L4 { .. } => panic!("expected l2"),
        }
        assert!(L4Book::from_snapshot(&l2).is_err());
    }
}
