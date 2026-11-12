//! Books kept from a snapshot plus deltas.
//!
//! [`L4Book`] is the order-level book kept from an L4 snapshot plus book
//! events. [`L2Book`] is the level book an `l2` subscriber keeps from an L2
//! snapshot plus the gateway's `levels` messages, which the gateway derives
//! from its own L4 book with [`L4Book::apply_batch_levels`].
//!
//! The book follows observation-v0 § 2.3–2.4: orders rest at their limit
//! price in time priority; `add` inserts at the back of its price level,
//! `modify` and partial `fill` change the size in place (keeping queue
//! position), a `fill` to zero and `cancel` remove the order. Any event that
//! does not fit the book (unknown order, duplicate add, price or side
//! mismatch) is an error: the caller treats it as a gap and resynchronizes.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use mesh_types::{
    BlockBatch, BookAction, BookEvent, BookSide, BookSnapshot, DecimalError, InstrumentId, Level,
    OrderId, Price, Provenance, Record, RecordKind, RestingOrder, Seq, Size, SnapshotDepth,
    UnixMillis,
};

use crate::protocol::{Depth, Levels};

/// An event that does not fit the book.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BookError {
    /// `add` for an order already resting.
    #[error("order {0} already rests")]
    Duplicate(u64),
    /// Change to an order that is not resting.
    #[error("order {0} is not in the book")]
    Unknown(u64),
    /// The event's price or side differs from the resting order's.
    #[error("order {0}: price or side differs from the resting order")]
    Mismatch(u64),
    /// A size that must be positive is not.
    #[error("order {0}: non-positive size")]
    BadSize(u64),
    /// Record for another instrument.
    #[error("record for another instrument")]
    WrongInstrument,
    /// Not a book event batch or not an L4 snapshot.
    #[error("not an L4 book snapshot or book_event batch")]
    WrongKind,
    /// Batch is not after the book's block.
    #[error("batch block {block} is not after the book's block {book}")]
    Stale {
        /// Batch block.
        block: u64,
        /// Book block.
        book: u64,
    },
    /// Aggregating a level overflowed.
    #[error("level size overflow")]
    Overflow,
}

impl From<DecimalError> for BookError {
    fn from(_: DecimalError) -> Self {
        Self::Overflow
    }
}

/// The price levels a block changed, best first on each side. A level
/// with size zero and `order_count` 0 is gone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LevelChanges {
    /// Changed bid levels, highest price first.
    pub bids: Vec<Level>,
    /// Changed ask levels, lowest price first.
    pub asks: Vec<Level>,
}

impl LevelChanges {
    /// True if no level changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bids.is_empty() && self.asks.is_empty()
    }
}

/// The level that marks a removed price: size `0`, no orders.
fn removed(price: Price) -> Result<Level, DecimalError> {
    Ok(Level {
        price,
        size: Size::zero(0)?,
        order_count: 0,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Resting {
    side: BookSide,
    price: Price,
    size: Size,
    ts_order_ms: UnixMillis,
    queue: u64,
}

/// An L4 book for one instrument.
#[derive(Debug, Clone)]
pub struct L4Book {
    instrument: InstrumentId,
    bids: BTreeMap<(Reverse<Price>, u64), OrderId>,
    asks: BTreeMap<(Price, u64), OrderId>,
    orders: HashMap<OrderId, Resting>,
    next_queue: u64,
    last_block: Option<u64>,
}

impl PartialEq for L4Book {
    fn eq(&self, other: &Self) -> bool {
        self.instrument == other.instrument && self.depth_l4() == other.depth_l4()
    }
}

impl L4Book {
    /// An empty book that has seen no block.
    #[must_use]
    pub fn new(instrument: InstrumentId) -> Self {
        Self {
            instrument,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            orders: HashMap::new(),
            next_queue: 0,
            last_block: None,
        }
    }

    /// The book from an L4 snapshot; its block is the snapshot's.
    ///
    /// # Errors
    ///
    /// [`BookError::WrongKind`] for L2 snapshots, or duplicate orders.
    pub fn from_snapshot(snapshot: &BookSnapshot) -> Result<Self, BookError> {
        let SnapshotDepth::L4 { bids, asks } = &snapshot.depth else {
            return Err(BookError::WrongKind);
        };
        let mut book = Self::new(snapshot.prov.instrument.clone());
        for (side, orders) in [(BookSide::Bid, bids), (BookSide::Ask, asks)] {
            for o in orders {
                book.insert(o.order_id, side, o.price, o.size, o.ts_order_ms)?;
            }
        }
        book.last_block = Some(snapshot.prov.seq.block_number);
        Ok(book)
    }

    /// The book from a `book_snapshot` record.
    ///
    /// # Errors
    ///
    /// As [`L4Book::from_snapshot`]; [`BookError::WrongKind`] for other
    /// records.
    pub fn from_record(record: &Record) -> Result<Self, BookError> {
        match record {
            Record::BookSnapshot(s) => Self::from_snapshot(s),
            _ => Err(BookError::WrongKind),
        }
    }

    fn insert(
        &mut self,
        id: OrderId,
        side: BookSide,
        price: Price,
        size: Size,
        ts_order_ms: UnixMillis,
    ) -> Result<(), BookError> {
        if !size.is_positive() {
            return Err(BookError::BadSize(id.0));
        }
        if self.orders.contains_key(&id) {
            return Err(BookError::Duplicate(id.0));
        }
        let queue = self.next_queue;
        self.next_queue += 1;
        match side {
            BookSide::Bid => self.bids.insert((Reverse(price), queue), id),
            BookSide::Ask => self.asks.insert((price, queue), id),
        };
        self.orders.insert(
            id,
            Resting {
                side,
                price,
                size,
                ts_order_ms,
                queue,
            },
        );
        Ok(())
    }

    fn remove(&mut self, id: OrderId) -> Option<Resting> {
        let r = self.orders.remove(&id)?;
        match r.side {
            BookSide::Bid => self.bids.remove(&(Reverse(r.price), r.queue)),
            BookSide::Ask => self.asks.remove(&(r.price, r.queue)),
        };
        Some(r)
    }

    /// Applies one event.
    ///
    /// # Errors
    ///
    /// [`BookError`] if the event does not fit; the book is unchanged.
    pub fn apply(&mut self, ev: &BookEvent) -> Result<(), BookError> {
        if ev.prov.instrument != self.instrument {
            return Err(BookError::WrongInstrument);
        }
        let id = ev.order_id;
        if let BookAction::Add { size } = ev.action {
            return self.insert(id, ev.side, ev.price, size, ev.prov.ts_source_ms);
        }
        let r = *self.orders.get(&id).ok_or(BookError::Unknown(id.0))?;
        if r.side != ev.side || r.price != ev.price {
            return Err(BookError::Mismatch(id.0));
        }
        match ev.action {
            BookAction::Add { .. } => Ok(()),
            BookAction::Modify { new_size, .. } => {
                if !new_size.is_positive() {
                    return Err(BookError::BadSize(id.0));
                }
                self.set_size(id, new_size);
                Ok(())
            }
            BookAction::Fill { new_size, .. } => {
                if new_size.is_zero() {
                    self.remove(id);
                } else if new_size.is_positive() {
                    self.set_size(id, new_size);
                } else {
                    return Err(BookError::BadSize(id.0));
                }
                Ok(())
            }
            BookAction::Cancel {} => {
                self.remove(id);
                Ok(())
            }
        }
    }

    fn set_size(&mut self, id: OrderId, size: Size) {
        if let Some(r) = self.orders.get_mut(&id) {
            r.size = size;
        }
    }

    /// Applies a `book_event` batch, all or nothing, and advances the book
    /// to the batch's block. Contiguity (`prev_block_number`) is the
    /// caller's check.
    ///
    /// # Errors
    ///
    /// [`BookError`]; on error the book is unchanged.
    pub fn apply_batch(&mut self, batch: &BlockBatch) -> Result<(), BookError> {
        if batch.kind != RecordKind::BookEvent {
            return Err(BookError::WrongKind);
        }
        if batch.instrument != self.instrument {
            return Err(BookError::WrongInstrument);
        }
        if let Some(b) = self.last_block
            && batch.block_number <= b
        {
            return Err(BookError::Stale {
                block: batch.block_number,
                book: b,
            });
        }
        let mut next = self.clone();
        for r in &batch.records {
            match r {
                Record::BookEvent(ev) => next.apply(ev)?,
                _ => return Err(BookError::WrongKind),
            }
        }
        next.last_block = Some(batch.block_number);
        *self = next;
        Ok(())
    }

    /// Applies a `book_event` batch like [`L4Book::apply_batch`] and
    /// returns the price levels it changed: for every price an event
    /// touched whose aggregate size or order count differs afterwards, the
    /// new level (size zero and `order_count` 0 when the level is gone).
    ///
    /// # Errors
    ///
    /// As [`L4Book::apply_batch`] (the book is then unchanged), or
    /// [`BookError::Overflow`] when a level's size overflows (the batch is
    /// then applied but its levels are unknown).
    pub fn apply_batch_levels(&mut self, batch: &BlockBatch) -> Result<LevelChanges, BookError> {
        let mut bid_prices = BTreeSet::new();
        let mut ask_prices = BTreeSet::new();
        for r in &batch.records {
            if let Record::BookEvent(ev) = r {
                match ev.side {
                    BookSide::Bid => bid_prices.insert(ev.price),
                    BookSide::Ask => ask_prices.insert(ev.price),
                };
            }
        }
        let before = |book: &Self, side, prices: &BTreeSet<Price>| {
            prices
                .iter()
                .map(|p| book.level(side, *p))
                .collect::<Result<Vec<_>, _>>()
        };
        let bids_before = before(self, BookSide::Bid, &bid_prices)?;
        let asks_before = before(self, BookSide::Ask, &ask_prices)?;
        self.apply_batch(batch)?;
        let changed = |side, prices: BTreeSet<Price>, old: Vec<Option<Level>>| {
            let mut out = Vec::new();
            for (price, old) in prices.into_iter().zip(old) {
                let new = self.level(side, price)?;
                if new != old {
                    out.push(match new {
                        Some(l) => l,
                        None => removed(price)?,
                    });
                }
            }
            Ok::<_, DecimalError>(out)
        };
        let mut bids = changed(BookSide::Bid, bid_prices, bids_before)?;
        bids.reverse(); // ascending price order; bids go best (highest) first
        let asks = changed(BookSide::Ask, ask_prices, asks_before)?;
        Ok(LevelChanges { bids, asks })
    }

    /// The aggregate level at `price` on `side`, if any order rests there.
    ///
    /// # Errors
    ///
    /// Size overflow.
    pub fn level(&self, side: BookSide, price: Price) -> Result<Option<Level>, DecimalError> {
        let ids: Vec<&OrderId> = match side {
            BookSide::Bid => self
                .bids
                .range((Reverse(price), 0)..=(Reverse(price), u64::MAX))
                .map(|(_, id)| id)
                .collect(),
            BookSide::Ask => self
                .asks
                .range((price, 0)..=(price, u64::MAX))
                .map(|(_, id)| id)
                .collect(),
        };
        let mut level: Option<Level> = None;
        for r in ids.into_iter().filter_map(|id| self.orders.get(id)) {
            level = Some(match level {
                None => Level {
                    price,
                    size: r.size,
                    order_count: 1,
                },
                Some(l) => Level {
                    price,
                    size: l.size.checked_add(r.size)?,
                    order_count: l.order_count.saturating_add(1),
                },
            });
        }
        Ok(level)
    }

    /// Instrument.
    #[must_use]
    pub fn instrument(&self) -> &InstrumentId {
        &self.instrument
    }

    /// The block the book reflects (state after it), if any.
    #[must_use]
    pub fn last_block(&self) -> Option<u64> {
        self.last_block
    }

    /// Marks the book as reflecting `block` (blocks without events for this
    /// instrument leave the book unchanged).
    pub fn advance_to(&mut self, block: u64) {
        if self.last_block.is_none_or(|b| b < block) {
            self.last_block = Some(block);
        }
    }

    /// Number of resting orders.
    #[must_use]
    pub fn len(&self) -> usize {
        self.orders.len()
    }

    /// True if no orders rest.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.orders.is_empty()
    }

    /// Best bid and ask.
    #[must_use]
    pub fn top(&self) -> (Option<Price>, Option<Price>) {
        (
            self.bids.keys().next().map(|(Reverse(p), _)| *p),
            self.asks.keys().next().map(|(p, _)| *p),
        )
    }

    fn side_orders<'a>(&'a self, ids: impl Iterator<Item = &'a OrderId>) -> Vec<RestingOrder> {
        ids.filter_map(|id| {
            self.orders.get(id).map(|r| RestingOrder {
                order_id: *id,
                price: r.price,
                size: r.size,
                ts_order_ms: r.ts_order_ms,
            })
        })
        .collect()
    }

    /// Every order, bids best first, asks best first, time priority within a
    /// price.
    #[must_use]
    pub fn depth_l4(&self) -> SnapshotDepth {
        SnapshotDepth::L4 {
            bids: self.side_orders(self.bids.values()),
            asks: self.side_orders(self.asks.values()),
        }
    }

    /// Aggregated levels.
    ///
    /// # Errors
    ///
    /// Size overflow.
    pub fn depth_l2(&self) -> Result<SnapshotDepth, DecimalError> {
        self.depth_l4().to_l2()
    }

    /// L2 levels of one side, best first.
    ///
    /// # Errors
    ///
    /// Size overflow.
    pub fn levels(&self, side: BookSide) -> Result<Vec<Level>, DecimalError> {
        match (self.depth_l2()?, side) {
            (SnapshotDepth::L2 { bids, .. }, BookSide::Bid) => Ok(bids),
            (SnapshotDepth::L2 { asks, .. }, BookSide::Ask) => Ok(asks),
            _ => Ok(Vec::new()),
        }
    }

    /// A snapshot record of the book at its block. `template` supplies the
    /// venue, admission, flags and timestamps; its `seq` is replaced by the
    /// end-of-block marker.
    ///
    /// # Errors
    ///
    /// Size overflow when aggregating to L2.
    pub fn snapshot(
        &self,
        template: &Provenance,
        depth: Depth,
    ) -> Result<BookSnapshot, DecimalError> {
        let mut prov = template.clone();
        prov.instrument = self.instrument.clone();
        prov.seq = Seq::end_of_block(self.last_block.unwrap_or(0));
        Ok(BookSnapshot {
            prov,
            depth: match depth {
                Depth::L4 => self.depth_l4(),
                Depth::L2 => self.depth_l2()?,
            },
        })
    }
}

/// A level (L2) book for one instrument: an L2 snapshot plus `levels`
/// deltas from the gateway. Equality compares the instrument and levels,
/// not the block (as for [`L4Book`]).
#[derive(Debug, Clone)]
pub struct L2Book {
    instrument: InstrumentId,
    bids: BTreeMap<Reverse<Price>, Level>,
    asks: BTreeMap<Price, Level>,
    last_block: Option<u64>,
}

impl PartialEq for L2Book {
    fn eq(&self, other: &Self) -> bool {
        self.instrument == other.instrument && self.bids == other.bids && self.asks == other.asks
    }
}

impl L2Book {
    /// An empty book that has seen no block.
    #[must_use]
    pub fn new(instrument: InstrumentId) -> Self {
        Self {
            instrument,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            last_block: None,
        }
    }

    /// The book from a snapshot (L2, or L4 aggregated); its block is the
    /// snapshot's.
    ///
    /// # Errors
    ///
    /// [`BookError::Overflow`] when aggregating an L4 snapshot.
    pub fn from_snapshot(snapshot: &BookSnapshot) -> Result<Self, BookError> {
        let SnapshotDepth::L2 { bids, asks } = snapshot.depth.to_l2()? else {
            return Err(BookError::WrongKind);
        };
        let mut book = Self::new(snapshot.prov.instrument.clone());
        book.set(bids, asks);
        book.last_block = Some(snapshot.prov.seq.block_number);
        Ok(book)
    }

    /// The book from a `book_snapshot` record.
    ///
    /// # Errors
    ///
    /// As [`L2Book::from_snapshot`]; [`BookError::WrongKind`] for other
    /// records.
    pub fn from_record(record: &Record) -> Result<Self, BookError> {
        match record {
            Record::BookSnapshot(s) => Self::from_snapshot(s),
            _ => Err(BookError::WrongKind),
        }
    }

    /// The levels of an L4 book, at the same block.
    ///
    /// # Errors
    ///
    /// [`BookError::Overflow`].
    pub fn from_l4(book: &L4Book) -> Result<Self, BookError> {
        let SnapshotDepth::L2 { bids, asks } = book.depth_l2()? else {
            return Err(BookError::WrongKind);
        };
        let mut out = Self::new(book.instrument().clone());
        out.set(bids, asks);
        out.last_block = book.last_block();
        Ok(out)
    }

    fn set(&mut self, bids: Vec<Level>, asks: Vec<Level>) {
        self.bids = bids
            .into_iter()
            .filter(|l| l.size.is_positive())
            .map(|l| (Reverse(l.price), l))
            .collect();
        self.asks = asks
            .into_iter()
            .filter(|l| l.size.is_positive())
            .map(|l| (l.price, l))
            .collect();
    }

    /// Applies one `levels` message, all or nothing, and advances the book
    /// to its block. Contiguity (`prev_block_number`) is the caller's
    /// check.
    ///
    /// # Errors
    ///
    /// [`BookError::WrongInstrument`], [`BookError::Stale`].
    pub fn apply_levels(&mut self, levels: &Levels) -> Result<(), BookError> {
        if levels.instrument != self.instrument {
            return Err(BookError::WrongInstrument);
        }
        if let Some(b) = self.last_block
            && levels.block_number <= b
        {
            return Err(BookError::Stale {
                block: levels.block_number,
                book: b,
            });
        }
        for l in &levels.bids {
            if l.size.is_positive() {
                self.bids.insert(Reverse(l.price), *l);
            } else {
                self.bids.remove(&Reverse(l.price));
            }
        }
        for l in &levels.asks {
            if l.size.is_positive() {
                self.asks.insert(l.price, *l);
            } else {
                self.asks.remove(&l.price);
            }
        }
        self.last_block = Some(levels.block_number);
        Ok(())
    }

    /// Instrument.
    #[must_use]
    pub fn instrument(&self) -> &InstrumentId {
        &self.instrument
    }

    /// The block the book reflects (state after it), if any.
    #[must_use]
    pub fn last_block(&self) -> Option<u64> {
        self.last_block
    }

    /// Best bid and ask.
    #[must_use]
    pub fn top(&self) -> (Option<Price>, Option<Price>) {
        (
            self.bids.keys().next().map(|Reverse(p)| *p),
            self.asks.keys().next().copied(),
        )
    }

    /// Levels of one side, best first.
    #[must_use]
    pub fn levels(&self, side: BookSide) -> Vec<Level> {
        match side {
            BookSide::Bid => self.bids.values().copied().collect(),
            BookSide::Ask => self.asks.values().copied().collect(),
        }
    }

    /// Both sides as snapshot depth.
    #[must_use]
    pub fn depth(&self) -> SnapshotDepth {
        SnapshotDepth::L2 {
            bids: self.levels(BookSide::Bid),
            asks: self.levels(BookSide::Ask),
        }
    }

    /// True if no level rests.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bids.is_empty() && self.asks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesh_types::{AdmissionId, UnixNanos, VenueId};

    fn prov(block: u64, i: u32) -> Provenance {
        Provenance::new(
            InstrumentId::new("hyperliquid:perp:BTC").unwrap(),
            VenueId::new("hyperliquid").unwrap(),
            Seq::new(block, i),
            UnixMillis(1000 + i64::try_from(block).unwrap()),
            UnixNanos(1),
            AdmissionId::new("ADM-0001").unwrap(),
        )
    }

    fn ev(block: u64, i: u32, id: u64, side: BookSide, px: &str, action: BookAction) -> Record {
        Record::BookEvent(BookEvent {
            prov: prov(block, i),
            order_id: OrderId(id),
            side,
            price: Price::parse_exact(px).unwrap(),
            action,
        })
    }

    fn sz(s: &str) -> Size {
        Size::parse_exact(s).unwrap()
    }

    fn batch(block: u64, prev: Option<u64>, records: Vec<Record>) -> BlockBatch {
        BlockBatch {
            instrument: InstrumentId::new("hyperliquid:perp:BTC").unwrap(),
            venue: VenueId::new("hyperliquid").unwrap(),
            kind: RecordKind::BookEvent,
            block_number: block,
            block_time_ms: UnixMillis(0),
            prev_block_number: prev,
            records,
        }
    }

    #[test]
    fn queue_priority_and_actions() {
        let mut book = L4Book::new(InstrumentId::new("hyperliquid:perp:BTC").unwrap());
        let add = |s| BookAction::Add { size: sz(s) };
        book.apply_batch(&batch(
            1,
            None,
            vec![
                ev(1, 0, 1, BookSide::Bid, "99.0", add("1.0")),
                ev(1, 1, 2, BookSide::Bid, "99.5", add("2.0")),
                ev(1, 2, 3, BookSide::Bid, "99.5", add("3.0")),
                ev(1, 3, 4, BookSide::Ask, "100.5", add("1.5")),
                ev(1, 4, 5, BookSide::Ask, "100.0", add("0.5")),
            ],
        ))
        .unwrap();
        assert_eq!(book.last_block(), Some(1));
        let (bid, ask) = book.top();
        assert_eq!(
            (bid.unwrap().to_string(), ask.unwrap().to_string()),
            ("99.5".into(), "100.0".into())
        );
        let SnapshotDepth::L4 { bids, asks } = book.depth_l4() else {
            panic!()
        };
        assert_eq!(
            bids.iter().map(|o| o.order_id.0).collect::<Vec<_>>(),
            vec![2, 3, 1]
        );
        assert_eq!(
            asks.iter().map(|o| o.order_id.0).collect::<Vec<_>>(),
            vec![5, 4]
        );

        // Modify keeps queue position; fills shrink and remove; cancel removes.
        book.apply_batch(&batch(
            2,
            Some(1),
            vec![
                ev(
                    2,
                    0,
                    2,
                    BookSide::Bid,
                    "99.5",
                    BookAction::Modify {
                        orig_size: Some(sz("2.0")),
                        new_size: sz("1.0"),
                    },
                ),
                ev(
                    2,
                    1,
                    5,
                    BookSide::Ask,
                    "100.0",
                    BookAction::Fill {
                        orig_size: None,
                        new_size: sz("0.0"),
                    },
                ),
                ev(
                    2,
                    2,
                    4,
                    BookSide::Ask,
                    "100.5",
                    BookAction::Fill {
                        orig_size: Some(sz("1.5")),
                        new_size: sz("1.0"),
                    },
                ),
                ev(2, 3, 1, BookSide::Bid, "99.0", BookAction::Cancel {}),
            ],
        ))
        .unwrap();
        let SnapshotDepth::L4 { bids, asks } = book.depth_l4() else {
            panic!()
        };
        assert_eq!(
            bids.iter()
                .map(|o| (o.order_id.0, o.size.to_string()))
                .collect::<Vec<_>>(),
            vec![(2, "1.0".into()), (3, "3.0".into())]
        );
        assert_eq!(asks.len(), 1);
        assert_eq!(book.len(), 3);
        let levels = book.levels(BookSide::Bid).unwrap();
        assert_eq!(
            (levels[0].size.to_string(), levels[0].order_count),
            ("4.0".into(), 2)
        );

        // Snapshot round trip.
        let snap = book.snapshot(&prov(2, 0), Depth::L4).unwrap();
        snap.validate().unwrap();
        assert_eq!(snap.prov.seq, Seq::end_of_block(2));
        let again = L4Book::from_snapshot(&snap).unwrap();
        assert_eq!(again, book);
        assert!(L4Book::from_snapshot(&book.snapshot(&prov(2, 0), Depth::L2).unwrap()).is_err());
    }

    #[test]
    fn inconsistent_events_leave_the_book_unchanged() {
        let mut book = L4Book::new(InstrumentId::new("hyperliquid:perp:BTC").unwrap());
        let add = BookAction::Add { size: sz("1.0") };
        book.apply_batch(&batch(
            1,
            None,
            vec![ev(1, 0, 1, BookSide::Bid, "99.0", add)],
        ))
        .unwrap();
        let before = book.clone();
        for (records, want) in [
            (
                vec![ev(2, 0, 9, BookSide::Bid, "99.0", BookAction::Cancel {})],
                BookError::Unknown(9),
            ),
            (
                vec![ev(2, 0, 1, BookSide::Bid, "99.0", add)],
                BookError::Duplicate(1),
            ),
            (
                vec![ev(2, 0, 1, BookSide::Ask, "99.0", BookAction::Cancel {})],
                BookError::Mismatch(1),
            ),
            (
                vec![
                    ev(2, 0, 2, BookSide::Bid, "98.0", add),
                    ev(2, 1, 1, BookSide::Bid, "98.5", BookAction::Cancel {}),
                ],
                BookError::Mismatch(1),
            ),
        ] {
            assert_eq!(book.apply_batch(&batch(2, Some(1), records)), Err(want));
            assert_eq!(book, before);
        }
        assert!(matches!(
            book.apply_batch(&batch(1, None, vec![])),
            Err(BookError::Stale { block: 1, book: 1 })
        ));
        book.advance_to(5);
        assert_eq!(book.last_block(), Some(5));
        book.advance_to(3);
        assert_eq!(book.last_block(), Some(5));
    }

    fn levels_msg(block: u64, prev: Option<u64>, ch: LevelChanges) -> Levels {
        Levels {
            instrument: InstrumentId::new("hyperliquid:perp:BTC").unwrap(),
            venue: VenueId::new("hyperliquid").unwrap(),
            block_number: block,
            block_time_ms: UnixMillis(0),
            prev_block_number: prev,
            bids: ch.bids,
            asks: ch.asks,
            ts_publish_ns: None,
        }
    }

    fn lv(px: &str, size: &str, n: u32) -> (String, String, u32) {
        (px.into(), size.into(), n)
    }

    fn show(levels: &[Level]) -> Vec<(String, String, u32)> {
        levels
            .iter()
            .map(|l| (l.price.to_string(), l.size.to_string(), l.order_count))
            .collect()
    }

    #[test]
    fn level_changes_follow_the_l4_book() {
        let add = |s| BookAction::Add { size: sz(s) };
        let mut l4 = L4Book::new(InstrumentId::new("hyperliquid:perp:BTC").unwrap());
        let ch = l4
            .apply_batch_levels(&batch(
                1,
                None,
                vec![
                    ev(1, 0, 1, BookSide::Bid, "99.0", add("1.0")),
                    ev(1, 1, 2, BookSide::Bid, "99.5", add("2.0")),
                    ev(1, 2, 3, BookSide::Bid, "99.5", add("3.0")),
                    ev(1, 3, 4, BookSide::Ask, "100.5", add("1.5")),
                    ev(1, 4, 5, BookSide::Ask, "100.0", add("0.5")),
                    // Added and cancelled within the block: no change.
                    ev(1, 5, 6, BookSide::Ask, "101.0", add("9.0")),
                    ev(1, 6, 6, BookSide::Ask, "101.0", BookAction::Cancel {}),
                ],
            ))
            .unwrap();
        assert_eq!(
            show(&ch.bids),
            vec![lv("99.5", "5.0", 2), lv("99.0", "1.0", 1)]
        );
        assert_eq!(
            show(&ch.asks),
            vec![lv("100.0", "0.5", 1), lv("100.5", "1.5", 1)]
        );
        let mut l2 = L2Book::new(l4.instrument().clone());
        l2.apply_levels(&levels_msg(1, None, ch)).unwrap();
        assert_eq!(l2, L2Book::from_l4(&l4).unwrap());

        // Partial fill, cancel emptying a level, modify back to the same
        // size (no change).
        let ch = l4
            .apply_batch_levels(&batch(
                3,
                Some(1),
                vec![
                    ev(
                        3,
                        0,
                        2,
                        BookSide::Bid,
                        "99.5",
                        BookAction::Fill {
                            orig_size: None,
                            new_size: sz("0.5"),
                        },
                    ),
                    ev(3, 1, 1, BookSide::Bid, "99.0", BookAction::Cancel {}),
                    ev(
                        3,
                        2,
                        4,
                        BookSide::Ask,
                        "100.5",
                        BookAction::Modify {
                            orig_size: None,
                            new_size: sz("2.0"),
                        },
                    ),
                    ev(
                        3,
                        3,
                        4,
                        BookSide::Ask,
                        "100.5",
                        BookAction::Modify {
                            orig_size: None,
                            new_size: sz("1.5"),
                        },
                    ),
                ],
            ))
            .unwrap();
        assert_eq!(
            show(&ch.bids),
            vec![lv("99.5", "3.5", 2), lv("99.0", "0", 0)]
        );
        assert!(ch.asks.is_empty());
        let msg = levels_msg(3, Some(1), ch);
        l2.apply_levels(&msg).unwrap();
        assert_eq!(l2, L2Book::from_l4(&l4).unwrap());
        assert_eq!(l2.last_block(), Some(3));
        assert_eq!(
            l2.top(),
            (
                Some(Price::parse_exact("99.5").unwrap()),
                Some(Price::parse_exact("100.0").unwrap())
            )
        );
        // Stale or foreign levels are refused and change nothing.
        let before = l2.clone();
        assert!(matches!(
            l2.apply_levels(&msg),
            Err(BookError::Stale { block: 3, book: 3 })
        ));
        assert_eq!(l2, before);

        // A failing batch leaves the L4 book unchanged.
        let snapshot = l4.clone();
        assert!(
            l4.apply_batch_levels(&batch(
                4,
                Some(3),
                vec![ev(4, 0, 77, BookSide::Bid, "1.0", BookAction::Cancel {})]
            ))
            .is_err()
        );
        assert_eq!(l4, snapshot);

        // An L2 snapshot of the L4 book gives the same L2 book.
        let snap = l4.snapshot(&prov(3, 0), Depth::L2).unwrap();
        assert_eq!(L2Book::from_snapshot(&snap).unwrap(), l2);
        assert!(!l2.is_empty());
        assert_eq!(show(&l2.levels(BookSide::Ask)).len(), 2);
    }
}
