//! An order-level (L4) book kept from a snapshot plus book events.
//!
//! The book follows observation-v0 § 2.3–2.4: orders rest at their limit
//! price in time priority; `add` inserts at the back of its price level,
//! `modify` and partial `fill` change the size in place (keeping queue
//! position), a `fill` to zero and `cancel` remove the order. Any event that
//! does not fit the book (unknown order, duplicate add, price or side
//! mismatch) is an error: the caller treats it as a gap and resynchronizes.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap};

use mesh_types::{
    BlockBatch, BookAction, BookEvent, BookSide, BookSnapshot, DecimalError, InstrumentId, Level,
    OrderId, Price, Provenance, Record, RecordKind, RestingOrder, Seq, Size, SnapshotDepth,
    UnixMillis,
};

use crate::protocol::Depth;

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
}
