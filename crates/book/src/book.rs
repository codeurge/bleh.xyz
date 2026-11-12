//! The order-level (L4) book.
//!
//! An [`OrderBook`] holds every resting order of one instrument: order ID,
//! side, price, size, placement time and queue priority. It starts empty or
//! from an L4 [`BookSnapshot`] and applies [`BookEvent`]s in `seq` order.
//!
//! # Structure
//!
//! - `orders`: order ID to entry, for O(1) lookup by ID.
//! - One `BTreeMap` of price levels per side, keyed by price mantissa (one
//!   price scale per book). Each level keeps its total size and its queue:
//!   a `BTreeMap` from priority to order ID, so the queue iterates in time
//!   priority and an order is removed in O(log n).
//!
//! Priorities come from one counter per book: orders in a snapshot get
//! priorities in snapshot order (which is queue order at each price), and
//! each `add` takes the next one. A `modify` keeps the order's priority:
//! the source reports it as an in-place update (see
//! `docs/specs/hyperliquid-node-format.md` § 6.1).
//!
//! # Consistency
//!
//! [`OrderBook::apply`] is atomic: it checks the event against the book and
//! returns a typed [`BookError`] without changing anything, or applies it.
//! Checks: instrument, `seq` order, the event's own validation, scales,
//! unknown or duplicate order IDs, side and price agreement, `orig_size`
//! agreement, and fills that would not reduce the size. A crossed or locked
//! book is checked at block boundaries ([`OrderBook::check_crossed`],
//! [`OrderBook::apply_block`]), because the source does not order events
//! within a block in a way that excludes a transient cross.

use std::collections::{BTreeMap, HashMap};

use mesh_types::{
    BookAction, BookEvent, BookSide, BookSnapshot, InstrumentId, Level, OrderId, Price, Provenance,
    RestingOrder, Seq, Size, SnapshotDepth, UnixMillis,
};
use serde::{Deserialize, Serialize};

use crate::error::BookError;

/// A resting order as seen through the book.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderView {
    /// Venue order ID.
    pub order_id: OrderId,
    /// Side.
    pub side: BookSide,
    /// Limit price.
    pub price: Price,
    /// Resting size.
    pub size: Size,
    /// Placement time: the snapshot's `ts_order_ms`, or the block time of
    /// the `add`.
    pub ts_order_ms: UnixMillis,
}

/// Best bid and best ask levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TopOfBook {
    /// Best (highest) bid level.
    pub bid: Option<Level>,
    /// Best (lowest) ask level.
    pub ask: Option<Level>,
}

impl TopOfBook {
    /// Best ask minus best bid.
    #[must_use]
    pub fn spread(&self) -> Option<Price> {
        let (b, a) = (self.bid?.price, self.ask?.price);
        let m = a.mantissa().checked_sub(b.mantissa())?;
        Price::new(m, b.scale()).ok()
    }

    /// Midpoint, exactly, at one more fractional digit than the book's
    /// price scale: `(bid + ask) / 2`. `None` without both sides or at the
    /// maximum scale.
    #[must_use]
    pub fn mid(&self) -> Option<Price> {
        let (b, a) = (self.bid?.price, self.ask?.price);
        let m = b.mantissa().checked_add(a.mantissa())?.checked_mul(5)?;
        Price::new(m, b.scale().checked_add(1)?).ok()
    }
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    side: BookSide,
    price: i64,
    size: i64,
    ts_order_ms: UnixMillis,
    priority: u64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct LevelQueue {
    total: i64,
    queue: BTreeMap<u64, OrderId>,
}

/// One price level, borrowed from the book.
#[derive(Debug, Clone, Copy)]
pub struct LevelRef<'a> {
    price: i64,
    level: &'a LevelQueue,
}

impl LevelRef<'_> {
    /// Price mantissa at the book's price scale.
    #[must_use]
    pub fn price_mantissa(&self) -> i64 {
        self.price
    }

    /// Total size mantissa at the book's size scale.
    #[must_use]
    pub fn size_mantissa(&self) -> i64 {
        self.level.total
    }

    /// Number of orders.
    #[must_use]
    pub fn order_count(&self) -> usize {
        self.level.queue.len()
    }

    /// Order IDs in queue (time priority) order.
    pub fn order_ids(&self) -> impl Iterator<Item = OrderId> + '_ {
        self.level.queue.values().copied()
    }
}

/// An L4 order book for one instrument.
#[derive(Debug, Clone)]
pub struct OrderBook {
    instrument: InstrumentId,
    price_scale: Option<u8>,
    size_scale: Option<u8>,
    orders: HashMap<OrderId, Entry>,
    bids: BTreeMap<i64, LevelQueue>,
    asks: BTreeMap<i64, LevelQueue>,
    next_priority: u64,
    seq: Option<Seq>,
    ts_ms: Option<UnixMillis>,
}

impl PartialEq for OrderBook {
    /// Books are equal when they hold the same orders in the same queue
    /// order at the same position. Internal priority numbers do not count.
    fn eq(&self, other: &Self) -> bool {
        self.instrument == other.instrument
            && self.seq == other.seq
            && self.orders.len() == other.orders.len()
            && self.side_eq(other, BookSide::Bid)
            && self.side_eq(other, BookSide::Ask)
    }
}

impl Eq for OrderBook {}

impl OrderBook {
    /// An empty book. Scales are taken from the first order.
    #[must_use]
    pub fn new(instrument: InstrumentId) -> Self {
        Self {
            instrument,
            price_scale: None,
            size_scale: None,
            orders: HashMap::new(),
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            next_priority: 0,
            seq: None,
            ts_ms: None,
        }
    }

    /// An empty book with known scales.
    #[must_use]
    pub fn with_scales(instrument: InstrumentId, price_scale: u8, size_scale: u8) -> Self {
        let mut b = Self::new(instrument);
        b.price_scale = Some(price_scale);
        b.size_scale = Some(size_scale);
        b
    }

    /// Builds a book from an L4 snapshot. Its position becomes the
    /// snapshot's `seq` (end of block).
    ///
    /// # Errors
    ///
    /// Fails if the snapshot is invalid, L2, or uses mixed scales.
    pub fn from_snapshot(snapshot: &BookSnapshot) -> Result<Self, BookError> {
        snapshot.validate().map_err(BookError::InvalidSnapshot)?;
        let SnapshotDepth::L4 { bids, asks } = &snapshot.depth else {
            return Err(BookError::NotL4);
        };
        let mut book = Self::new(snapshot.prov.instrument.clone());
        for (side, orders) in [(BookSide::Bid, bids), (BookSide::Ask, asks)] {
            for o in orders {
                book.check_price_scale(o.price)?;
                book.check_size_scale(o.size)?;
                book.learn_scales(o.price, o.size);
                let priority = book.take_priority();
                book.insert_unchecked(
                    o.order_id,
                    Entry {
                        side,
                        price: o.price.mantissa(),
                        size: o.size.mantissa(),
                        ts_order_ms: o.ts_order_ms,
                        priority,
                    },
                )?;
            }
        }
        book.seq = Some(snapshot.prov.seq);
        book.ts_ms = Some(snapshot.prov.ts_source_ms);
        Ok(book)
    }

    /// Replaces the contents with a snapshot's (a resync). The instrument
    /// must match.
    ///
    /// # Errors
    ///
    /// Fails as [`OrderBook::from_snapshot`] does, or on another
    /// instrument. The book is unchanged on error.
    pub fn reset_to(&mut self, snapshot: &BookSnapshot) -> Result<(), BookError> {
        self.check_instrument(&snapshot.prov.instrument)?;
        let fresh = Self::from_snapshot(snapshot)?;
        *self = fresh;
        Ok(())
    }

    /// The instrument.
    #[must_use]
    pub fn instrument(&self) -> &InstrumentId {
        &self.instrument
    }

    /// Position: the `seq` of the last applied event or snapshot.
    #[must_use]
    pub fn seq(&self) -> Option<Seq> {
        self.seq
    }

    /// Source time of the last applied event or snapshot.
    #[must_use]
    pub fn ts_ms(&self) -> Option<UnixMillis> {
        self.ts_ms
    }

    /// Price scale, once known.
    #[must_use]
    pub fn price_scale(&self) -> Option<u8> {
        self.price_scale
    }

    /// Size scale, once known.
    #[must_use]
    pub fn size_scale(&self) -> Option<u8> {
        self.size_scale
    }

    /// Number of resting orders.
    #[must_use]
    pub fn len(&self) -> usize {
        self.orders.len()
    }

    /// True if no order rests.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.orders.is_empty()
    }

    /// Number of price levels on a side.
    #[must_use]
    pub fn level_count(&self, side: BookSide) -> usize {
        self.side(side).len()
    }

    /// Moves the position forward without an event (for example to the
    /// end of a block). Ignored if `seq` is not after the position.
    pub fn advance(&mut self, seq: Seq, ts_ms: UnixMillis) {
        if self.seq.is_none_or(|s| seq > s) {
            self.seq = Some(seq);
            self.ts_ms = Some(ts_ms);
        }
    }

    /// Applies one event atomically.
    ///
    /// # Errors
    ///
    /// Returns the inconsistency; the book is unchanged.
    pub fn apply(&mut self, ev: &BookEvent) -> Result<(), BookError> {
        let seq = ev.prov.seq;
        self.check_instrument(&ev.prov.instrument)?;
        if let Some(last) = self.seq
            && seq <= last
        {
            return Err(BookError::OutOfOrder { last, got: seq });
        }
        ev.validate()
            .map_err(|source| BookError::InvalidEvent { seq, source })?;
        self.check_price_scale(ev.price)?;
        if let Some(s) = ev.action.size_after() {
            self.check_size_scale(s)?;
        }
        match ev.action {
            BookAction::Add { size } => {
                if self.orders.contains_key(&ev.order_id) {
                    return Err(BookError::DuplicateOrder {
                        order_id: ev.order_id,
                        seq,
                    });
                }
                let new_total = self
                    .side(ev.side)
                    .get(&ev.price.mantissa())
                    .map_or(Some(size.mantissa()), |l| {
                        l.total.checked_add(size.mantissa())
                    });
                if new_total.is_none() {
                    return Err(BookError::Overflow);
                }
                self.learn_scales(ev.price, size);
                let priority = self.take_priority();
                self.insert_unchecked(
                    ev.order_id,
                    Entry {
                        side: ev.side,
                        price: ev.price.mantissa(),
                        size: size.mantissa(),
                        ts_order_ms: ev.prov.ts_source_ms,
                        priority,
                    },
                )?;
            }
            BookAction::Modify {
                orig_size,
                new_size,
            } => {
                let e = self.existing(ev)?;
                self.check_orig(ev, &e, orig_size)?;
                self.set_size(ev.order_id, &e, new_size.mantissa())?;
            }
            BookAction::Fill {
                orig_size,
                new_size,
            } => {
                let e = self.existing(ev)?;
                self.check_orig(ev, &e, orig_size)?;
                if new_size.mantissa() >= e.size {
                    return Err(BookError::NonReducingFill {
                        order_id: ev.order_id,
                        book: self.size_of(e.size)?,
                        new_size,
                        seq,
                    });
                }
                if new_size.is_zero() {
                    self.remove(ev.order_id);
                } else {
                    self.set_size(ev.order_id, &e, new_size.mantissa())?;
                }
            }
            BookAction::Cancel {} => {
                self.existing(ev)?;
                self.remove(ev.order_id);
            }
        }
        self.seq = Some(seq);
        self.ts_ms = Some(ev.prov.ts_source_ms);
        Ok(())
    }

    /// Applies the events of one block in order, then checks that the book
    /// is not crossed and moves the position to the end of the block.
    ///
    /// # Errors
    ///
    /// Returns the index of the first failing event and its error, or
    /// `events.len()` with [`BookError::Crossed`]. Events before the
    /// failing one stay applied.
    pub fn apply_block(
        &mut self,
        block_number: u64,
        block_time: UnixMillis,
        events: &[BookEvent],
    ) -> Result<(), (usize, BookError)> {
        for (i, ev) in events.iter().enumerate() {
            self.apply(ev).map_err(|e| (i, e))?;
        }
        self.advance(Seq::end_of_block(block_number), block_time);
        self.check_crossed().map_err(|e| (events.len(), e))
    }

    /// Fails if the best bid is at or above the best ask.
    ///
    /// # Errors
    ///
    /// [`BookError::Crossed`].
    pub fn check_crossed(&self) -> Result<(), BookError> {
        if let (Some((&b, _)), Some((&a, _))) =
            (self.bids.last_key_value(), self.asks.first_key_value())
            && b >= a
        {
            return Err(BookError::Crossed {
                bid: self.price_of(b)?,
                ask: self.price_of(a)?,
                seq: self.seq.unwrap_or_default(),
            });
        }
        Ok(())
    }

    /// The order with this ID, if resting.
    #[must_use]
    pub fn order(&self, id: OrderId) -> Option<OrderView> {
        let e = self.orders.get(&id)?;
        self.view(id, e).ok()
    }

    /// Levels on a side, best first, borrowed (no allocation).
    pub fn level_refs(&self, side: BookSide) -> Box<dyn Iterator<Item = LevelRef<'_>> + '_> {
        let map = |(&price, level)| LevelRef { price, level };
        match side {
            BookSide::Bid => Box::new(self.bids.iter().rev().map(map)),
            BookSide::Ask => Box::new(self.asks.iter().map(map)),
        }
    }

    /// Up to `max` aggregated levels on a side, best first.
    ///
    /// # Errors
    ///
    /// Fails only if a value cannot be represented (never for a book built
    /// through this API).
    pub fn levels(&self, side: BookSide, max: usize) -> Result<Vec<Level>, BookError> {
        self.level_refs(side)
            .take(max)
            .map(|l| self.level_of(&l))
            .collect()
    }

    /// Best bid and best ask.
    #[must_use]
    pub fn top(&self) -> TopOfBook {
        let best = |side| {
            self.level_refs(side)
                .next()
                .and_then(|l| self.level_of(&l).ok())
        };
        TopOfBook {
            bid: best(BookSide::Bid),
            ask: best(BookSide::Ask),
        }
    }

    /// Orders at one price, in queue order.
    #[must_use]
    pub fn queue_at(&self, side: BookSide, price: Price) -> Vec<OrderView> {
        if self.price_scale.is_some_and(|s| s != price.scale()) {
            return Vec::new();
        }
        self.side(side)
            .get(&price.mantissa())
            .map(|l| l.queue.values().filter_map(|id| self.order(*id)).collect())
            .unwrap_or_default()
    }

    /// Every order on a side, best price first, queue order within a price.
    pub fn orders_on(&self, side: BookSide) -> impl Iterator<Item = OrderView> + '_ {
        self.level_refs(side)
            .flat_map(|l| l.level.queue.values())
            .filter_map(|id| self.order(*id))
    }

    /// The book as an L4 snapshot with the given provenance (whose `seq`
    /// should be an end-of-block marker).
    ///
    /// # Errors
    ///
    /// Fails only if a value cannot be represented.
    pub fn to_snapshot(&self, prov: Provenance) -> Result<BookSnapshot, BookError> {
        let side = |s| -> Result<Vec<RestingOrder>, BookError> {
            self.level_refs(s)
                .flat_map(|l| l.level.queue.values())
                .map(|id| {
                    let e = self.orders.get(id).ok_or(BookError::Overflow)?;
                    Ok(RestingOrder {
                        order_id: *id,
                        price: self.price_of(e.price)?,
                        size: self.size_of(e.size)?,
                        ts_order_ms: e.ts_order_ms,
                    })
                })
                .collect()
        };
        Ok(BookSnapshot {
            prov,
            depth: SnapshotDepth::L4 {
                bids: side(BookSide::Bid)?,
                asks: side(BookSide::Ask)?,
            },
        })
    }

    /// The book as an L2 snapshot, optionally limited to `depth` levels per
    /// side.
    ///
    /// # Errors
    ///
    /// Fails only if a value cannot be represented.
    pub fn to_l2_snapshot(
        &self,
        prov: Provenance,
        depth: Option<usize>,
    ) -> Result<BookSnapshot, BookError> {
        let max = depth.unwrap_or(usize::MAX);
        Ok(BookSnapshot {
            prov,
            depth: SnapshotDepth::L2 {
                bids: self.levels(BookSide::Bid, max)?,
                asks: self.levels(BookSide::Ask, max)?,
            },
        })
    }

    // ----- internals -----

    pub(crate) fn side(&self, side: BookSide) -> &BTreeMap<i64, LevelQueue> {
        match side {
            BookSide::Bid => &self.bids,
            BookSide::Ask => &self.asks,
        }
    }

    fn side_mut(&mut self, side: BookSide) -> &mut BTreeMap<i64, LevelQueue> {
        match side {
            BookSide::Bid => &mut self.bids,
            BookSide::Ask => &mut self.asks,
        }
    }

    fn side_eq(&self, other: &Self, side: BookSide) -> bool {
        let a = self.side(side);
        let b = other.side(side);
        a.len() == b.len()
            && a.iter().zip(b.iter()).all(|((pa, la), (pb, lb))| {
                pa == pb
                    && la.total == lb.total
                    && la.queue.len() == lb.queue.len()
                    && la.queue.values().zip(lb.queue.values()).all(|(ia, ib)| {
                        ia == ib
                            && match (self.orders.get(ia), other.orders.get(ib)) {
                                (Some(x), Some(y)) => {
                                    x.size == y.size && x.ts_order_ms == y.ts_order_ms
                                }
                                _ => false,
                            }
                    })
            })
    }

    fn take_priority(&mut self) -> u64 {
        let p = self.next_priority;
        self.next_priority = self.next_priority.wrapping_add(1);
        p
    }

    fn check_instrument(&self, found: &InstrumentId) -> Result<(), BookError> {
        if *found == self.instrument {
            Ok(())
        } else {
            Err(BookError::InstrumentMismatch {
                expected: self.instrument.clone(),
                found: found.clone(),
            })
        }
    }

    fn check_price_scale(&self, p: Price) -> Result<(), BookError> {
        match self.price_scale {
            Some(s) if s != p.scale() => Err(BookError::ScaleMismatch {
                field: "price",
                expected: s,
                found: p.scale(),
            }),
            _ => Ok(()),
        }
    }

    fn check_size_scale(&self, s: Size) -> Result<(), BookError> {
        match self.size_scale {
            Some(x) if x != s.scale() => Err(BookError::ScaleMismatch {
                field: "size",
                expected: x,
                found: s.scale(),
            }),
            _ => Ok(()),
        }
    }

    fn learn_scales(&mut self, p: Price, s: Size) {
        self.price_scale.get_or_insert(p.scale());
        self.size_scale.get_or_insert(s.scale());
    }

    pub(crate) fn price_of(&self, m: i64) -> Result<Price, BookError> {
        Ok(Price::new(m, self.price_scale.unwrap_or(0))?)
    }

    pub(crate) fn size_of(&self, m: i64) -> Result<Size, BookError> {
        Ok(Size::new(m, self.size_scale.unwrap_or(0))?)
    }

    fn level_of(&self, l: &LevelRef<'_>) -> Result<Level, BookError> {
        Ok(Level {
            price: self.price_of(l.price)?,
            size: self.size_of(l.level.total)?,
            order_count: u32::try_from(l.level.queue.len()).unwrap_or(u32::MAX),
        })
    }

    fn view(&self, id: OrderId, e: &Entry) -> Result<OrderView, BookError> {
        Ok(OrderView {
            order_id: id,
            side: e.side,
            price: self.price_of(e.price)?,
            size: self.size_of(e.size)?,
            ts_order_ms: e.ts_order_ms,
        })
    }

    /// The resting entry an event refers to, after checking side and price.
    fn existing(&self, ev: &BookEvent) -> Result<Entry, BookError> {
        let seq = ev.prov.seq;
        let e = *self
            .orders
            .get(&ev.order_id)
            .ok_or(BookError::UnknownOrder {
                order_id: ev.order_id,
                seq,
            })?;
        if e.side != ev.side {
            return Err(BookError::SideMismatch {
                order_id: ev.order_id,
                book: e.side,
                event: ev.side,
                seq,
            });
        }
        if e.price != ev.price.mantissa() {
            return Err(BookError::PriceMismatch {
                order_id: ev.order_id,
                book: self.price_of(e.price)?,
                event: ev.price,
                seq,
            });
        }
        Ok(e)
    }

    fn check_orig(&self, ev: &BookEvent, e: &Entry, orig: Option<Size>) -> Result<(), BookError> {
        if let Some(orig) = orig {
            self.check_size_scale(orig)?;
            if orig.mantissa() != e.size {
                return Err(BookError::OrigSizeMismatch {
                    order_id: ev.order_id,
                    book: self.size_of(e.size)?,
                    event: orig,
                    seq: ev.prov.seq,
                });
            }
        }
        Ok(())
    }

    fn insert_unchecked(&mut self, id: OrderId, e: Entry) -> Result<(), BookError> {
        let level = self.side_mut(e.side).entry(e.price).or_default();
        level.total = level.total.checked_add(e.size).ok_or(BookError::Overflow)?;
        level.queue.insert(e.priority, id);
        self.orders.insert(id, e);
        Ok(())
    }

    fn set_size(&mut self, id: OrderId, e: &Entry, new: i64) -> Result<(), BookError> {
        let side = self.side_mut(e.side);
        let Some(level) = side.get_mut(&e.price) else {
            return Err(BookError::Overflow);
        };
        let total = level
            .total
            .checked_sub(e.size)
            .and_then(|t| t.checked_add(new))
            .ok_or(BookError::Overflow)?;
        level.total = total;
        if let Some(entry) = self.orders.get_mut(&id) {
            entry.size = new;
        }
        Ok(())
    }

    fn remove(&mut self, id: OrderId) {
        let Some(e) = self.orders.remove(&id) else {
            return;
        };
        let side = self.side_mut(e.side);
        if let Some(level) = side.get_mut(&e.price) {
            level.queue.remove(&e.priority);
            level.total = level.total.saturating_sub(e.size);
            if level.queue.is_empty() {
                side.remove(&e.price);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn add_modify_fill_cancel() {
        let mut b = OrderBook::new(inst());
        b.apply(&add(1, 0, 1, BookSide::Bid, "100.0", "1.00"))
            .unwrap();
        b.apply(&add(1, 1, 2, BookSide::Bid, "100.0", "2.00"))
            .unwrap();
        b.apply(&add(1, 2, 3, BookSide::Ask, "101.0", "0.50"))
            .unwrap();
        let top = b.top();
        assert_eq!(top.bid.unwrap().size, sz("3.00"));
        assert_eq!(top.bid.unwrap().order_count, 2);
        assert_eq!(top.spread().unwrap(), px("1.0"));
        assert_eq!(top.mid().unwrap(), px("100.50"));

        b.apply(&modify(
            2,
            0,
            1,
            BookSide::Bid,
            "100.0",
            Some("1.00"),
            "0.40",
        ))
        .unwrap();
        b.apply(&fill(2, 1, 2, BookSide::Bid, "100.0", Some("2.00"), "0.50"))
            .unwrap();
        assert_eq!(b.top().bid.unwrap().size, sz("0.90"));
        // Queue priority is kept on modify: order 1 is still first.
        let q = b.queue_at(BookSide::Bid, px("100.0"));
        assert_eq!(q.iter().map(|o| o.order_id.0).collect::<Vec<_>>(), [1, 2]);

        b.apply(&fill(3, 0, 2, BookSide::Bid, "100.0", None, "0.00"))
            .unwrap();
        b.apply(&cancel(3, 1, 1, BookSide::Bid, "100.0")).unwrap();
        assert!(b.top().bid.is_none());
        assert_eq!(b.len(), 1);
        assert_eq!(b.level_count(BookSide::Bid), 0);
    }

    #[test]
    fn typed_inconsistencies_leave_book_unchanged() {
        let mut b = OrderBook::new(inst());
        b.apply(&add(1, 0, 1, BookSide::Bid, "100.0", "1.00"))
            .unwrap();
        let before = b.clone();
        let cases = [
            (
                cancel(2, 0, 9, BookSide::Bid, "100.0"),
                "unknown",
                BookError::UnknownOrder {
                    order_id: OrderId(9),
                    seq: Seq::new(2, 0),
                },
            ),
            (
                add(2, 0, 1, BookSide::Bid, "100.0", "1.00"),
                "dup",
                BookError::DuplicateOrder {
                    order_id: OrderId(1),
                    seq: Seq::new(2, 0),
                },
            ),
        ];
        for (ev, what, want) in cases {
            assert_eq!(b.apply(&ev), Err(want), "{what}");
            assert_eq!(b, before);
        }
        assert!(matches!(
            b.apply(&cancel(2, 0, 1, BookSide::Ask, "100.0")),
            Err(BookError::SideMismatch { .. })
        ));
        assert!(matches!(
            b.apply(&cancel(2, 0, 1, BookSide::Bid, "100.5")),
            Err(BookError::PriceMismatch { .. })
        ));
        assert!(matches!(
            b.apply(&modify(
                2,
                0,
                1,
                BookSide::Bid,
                "100.0",
                Some("2.00"),
                "0.50"
            )),
            Err(BookError::OrigSizeMismatch { .. })
        ));
        assert!(matches!(
            b.apply(&fill(2, 0, 1, BookSide::Bid, "100.0", None, "1.50")),
            Err(BookError::NonReducingFill { .. })
        ));
        assert!(matches!(
            b.apply(&fill(2, 0, 1, BookSide::Bid, "100.0", None, "1.00")),
            Err(BookError::NonReducingFill { .. })
        ));
        assert!(matches!(
            b.apply(&add(2, 0, 5, BookSide::Bid, "100.00", "1.00")),
            Err(BookError::ScaleMismatch { field: "price", .. })
        ));
        assert!(matches!(
            b.apply(&add(2, 0, 5, BookSide::Bid, "100.0", "1.0")),
            Err(BookError::ScaleMismatch { field: "size", .. })
        ));
        assert!(matches!(
            b.apply(&add(2, 0, 5, BookSide::Bid, "100.0", "0.00")),
            Err(BookError::InvalidEvent { .. })
        ));
        assert!(matches!(
            b.apply(&add(1, 0, 5, BookSide::Bid, "100.0", "1.00")),
            Err(BookError::OutOfOrder { .. })
        ));
        let mut other = add(2, 0, 5, BookSide::Bid, "100.0", "1.00");
        other.prov.instrument = InstrumentId::new("hyperliquid:perp:OTHER").unwrap();
        assert!(matches!(
            b.apply(&other),
            Err(BookError::InstrumentMismatch { .. })
        ));
        assert_eq!(b, before);
    }

    #[test]
    fn crossed_book_detected_at_block_end() {
        let mut b = OrderBook::new(inst());
        let evs = [
            add(1, 0, 1, BookSide::Bid, "100.0", "1.00"),
            add(1, 1, 2, BookSide::Ask, "100.0", "1.00"),
        ];
        let err = b.apply_block(1, ms(1_000), &evs).unwrap_err();
        assert_eq!(err.0, 2);
        assert!(matches!(err.1, BookError::Crossed { .. }));
    }

    #[test]
    fn snapshot_round_trip() {
        let mut b = OrderBook::new(inst());
        let evs = [
            add(1, 0, 1, BookSide::Bid, "100.0", "1.00"),
            add(1, 1, 2, BookSide::Bid, "99.0", "2.00"),
            add(1, 2, 3, BookSide::Bid, "100.0", "0.30"),
            add(1, 3, 4, BookSide::Ask, "101.0", "0.50"),
        ];
        b.apply_block(1, ms(1_000), &evs).unwrap();
        let snap = b.to_snapshot(prov_eob(1, 1_000)).unwrap();
        snap.validate().unwrap();
        let back = OrderBook::from_snapshot(&snap).unwrap();
        assert_eq!(back, b);
        let SnapshotDepth::L4 { bids, .. } = &snap.depth else {
            panic!()
        };
        assert_eq!(
            bids.iter().map(|o| o.order_id.0).collect::<Vec<_>>(),
            [1, 3, 2]
        );
        let l2 = b.to_l2_snapshot(prov_eob(1, 1_000), Some(1)).unwrap();
        let SnapshotDepth::L2 { bids, asks } = &l2.depth else {
            panic!()
        };
        assert_eq!(bids.len(), 1);
        assert_eq!(bids[0].size, sz("1.30"));
        assert_eq!(asks[0].order_count, 1);
        assert_eq!(l2.depth, snap.depth.to_l2().unwrap().truncated(1));
        assert_eq!(OrderBook::from_snapshot(&l2).unwrap_err(), BookError::NotL4);
    }

    trait Truncate {
        fn truncated(self, n: usize) -> Self;
    }
    impl Truncate for SnapshotDepth {
        fn truncated(self, n: usize) -> Self {
            match self {
                SnapshotDepth::L2 { mut bids, mut asks } => {
                    bids.truncate(n);
                    asks.truncate(n);
                    SnapshotDepth::L2 { bids, asks }
                }
                other => other,
            }
        }
    }
}
