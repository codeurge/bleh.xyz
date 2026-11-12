//! Reconciliation: comparing the live book with a fresh snapshot.
//!
//! [`reconcile`] compares an [`OrderBook`] with a [`BookSnapshot`] of the
//! same instrument and returns a [`ReconcileReport`]. Against an L4
//! snapshot it compares order by order (missing, extra and changed orders,
//! and queue order at each price) and level by level; against an L2
//! snapshot, level by level only. [`resync`] reports and then replaces the
//! book with the snapshot.
//!
//! The report lists differences in a fixed order (bids before asks, best
//! price first, queue order within a price), so two runs produce the same
//! report.

use std::collections::{BTreeMap, HashMap};

use mesh_types::{
    BookSide, BookSnapshot, Level, OrderId, Price, Seq, Size, SnapshotDepth, UnixMillis,
};
use serde::{Deserialize, Serialize};

use crate::book::{OrderBook, OrderView};
use crate::error::BookError;

/// What reconciliation compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcileOptions {
    /// Compare order placement times (`ts_order_ms`). The book takes them
    /// from the snapshot or from the block time of the `add`.
    pub compare_order_time: bool,
    /// Compare queue order at each price.
    pub compare_queue_order: bool,
}

impl Default for ReconcileOptions {
    fn default() -> Self {
        Self {
            compare_order_time: true,
            compare_queue_order: true,
        }
    }
}

/// One order as listed in a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderState {
    /// Order ID.
    pub order_id: OrderId,
    /// Side.
    pub side: BookSide,
    /// Price.
    pub price: Price,
    /// Size.
    pub size: Size,
    /// Placement time.
    pub ts_order_ms: UnixMillis,
}

impl From<OrderView> for OrderState {
    fn from(o: OrderView) -> Self {
        Self {
            order_id: o.order_id,
            side: o.side,
            price: o.price,
            size: o.size,
            ts_order_ms: o.ts_order_ms,
        }
    }
}

/// An order present in both with different attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderDiff {
    /// The order in the book.
    pub book: OrderState,
    /// The order in the snapshot.
    pub snapshot: OrderState,
}

/// A price whose level differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LevelDiff {
    /// Side.
    pub side: BookSide,
    /// Price.
    pub price: Price,
    /// The level in the book, if any.
    pub book: Option<Level>,
    /// The level in the snapshot, if any.
    pub snapshot: Option<Level>,
}

/// A price where both hold the same orders in a different queue order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueDiff {
    /// Side.
    pub side: BookSide,
    /// Price.
    pub price: Price,
    /// Order IDs in the book's queue order.
    pub book: Vec<OrderId>,
    /// Order IDs in the snapshot's queue order.
    pub snapshot: Vec<OrderId>,
}

/// The result of comparing a book with a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcileReport {
    /// The snapshot's position.
    pub snapshot_seq: Seq,
    /// The book's position when compared.
    pub book_seq: Option<Seq>,
    /// Orders in the snapshot but not in the book (L4 only).
    pub missing: Vec<OrderState>,
    /// Orders in the book but not in the snapshot (L4 only).
    pub extra: Vec<OrderState>,
    /// Orders in both with a different side, price, size or time (L4
    /// only).
    pub changed: Vec<OrderDiff>,
    /// Prices with the same orders in a different queue order (L4 only).
    pub queue: Vec<QueueDiff>,
    /// Prices whose aggregated level differs.
    pub levels: Vec<LevelDiff>,
}

impl ReconcileReport {
    /// True if the book matches the snapshot.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty()
            && self.extra.is_empty()
            && self.changed.is_empty()
            && self.queue.is_empty()
            && self.levels.is_empty()
    }

    /// Number of differences of every kind.
    #[must_use]
    pub fn difference_count(&self) -> usize {
        self.missing.len()
            + self.extra.len()
            + self.changed.len()
            + self.queue.len()
            + self.levels.len()
    }
}

/// Compares `book` with `snapshot`.
///
/// # Errors
///
/// Fails if the snapshot is invalid or for another instrument, or uses
/// other scales than the book.
pub fn reconcile(
    book: &OrderBook,
    snapshot: &BookSnapshot,
    options: ReconcileOptions,
) -> Result<ReconcileReport, BookError> {
    snapshot.validate().map_err(BookError::InvalidSnapshot)?;
    if snapshot.prov.instrument != *book.instrument() {
        return Err(BookError::InstrumentMismatch {
            expected: book.instrument().clone(),
            found: snapshot.prov.instrument.clone(),
        });
    }
    check_scales(book, snapshot)?;
    let mut report = ReconcileReport {
        snapshot_seq: snapshot.prov.seq,
        book_seq: book.seq(),
        missing: Vec::new(),
        extra: Vec::new(),
        changed: Vec::new(),
        queue: Vec::new(),
        levels: Vec::new(),
    };
    let l2 = snapshot.depth.to_l2()?;
    let SnapshotDepth::L2 {
        bids: snap_bids,
        asks: snap_asks,
    } = &l2
    else {
        return Ok(report);
    };
    if let SnapshotDepth::L4 { bids, asks } = &snapshot.depth {
        let mut snap_orders: HashMap<OrderId, OrderState> = HashMap::new();
        for (side, orders) in [(BookSide::Bid, bids), (BookSide::Ask, asks)] {
            for o in orders {
                snap_orders.insert(
                    o.order_id,
                    OrderState {
                        order_id: o.order_id,
                        side,
                        price: o.price,
                        size: o.size,
                        ts_order_ms: o.ts_order_ms,
                    },
                );
            }
        }
        // Missing and changed, in snapshot order.
        for (side, orders) in [(BookSide::Bid, bids), (BookSide::Ask, asks)] {
            for o in orders {
                let Some(s) = snap_orders.get(&o.order_id).copied() else {
                    continue;
                };
                debug_assert_eq!(s.side, side);
                match book.order(o.order_id) {
                    None => report.missing.push(s),
                    Some(b) => {
                        let b = OrderState::from(b);
                        let differs = b.side != s.side
                            || b.price != s.price
                            || b.size != s.size
                            || (options.compare_order_time && b.ts_order_ms != s.ts_order_ms);
                        if differs {
                            report.changed.push(OrderDiff {
                                book: b,
                                snapshot: s,
                            });
                        }
                    }
                }
            }
        }
        // Extra, in book order.
        for side in [BookSide::Bid, BookSide::Ask] {
            for o in book.orders_on(side) {
                if !snap_orders.contains_key(&o.order_id) {
                    report.extra.push(o.into());
                }
            }
        }
        if options.compare_queue_order {
            for (side, orders) in [(BookSide::Bid, bids), (BookSide::Ask, asks)] {
                queue_diffs(book, side, orders, &mut report.queue);
            }
        }
    }
    for (side, snap_levels) in [(BookSide::Bid, snap_bids), (BookSide::Ask, snap_asks)] {
        level_diffs(book, side, snap_levels, &mut report.levels)?;
    }
    Ok(report)
}

/// Reconciles and then replaces the book with the snapshot (an L4
/// snapshot is required for the replacement).
///
/// # Errors
///
/// Fails as [`reconcile`] does, or if the snapshot is L2. The book is
/// unchanged on error.
pub fn resync(
    book: &mut OrderBook,
    snapshot: &BookSnapshot,
    options: ReconcileOptions,
) -> Result<ReconcileReport, BookError> {
    let report = reconcile(book, snapshot, options)?;
    book.reset_to(snapshot)?;
    Ok(report)
}

fn check_scales(book: &OrderBook, snapshot: &BookSnapshot) -> Result<(), BookError> {
    let first = match &snapshot.depth {
        SnapshotDepth::L4 { bids, asks } => bids
            .first()
            .or(asks.first())
            .map(|o| (o.price.scale(), o.size.scale())),
        SnapshotDepth::L2 { bids, asks } => bids
            .first()
            .or(asks.first())
            .map(|l| (l.price.scale(), l.size.scale())),
    };
    if let Some((ps, ss)) = first {
        for (field, mine, theirs) in [
            ("price", book.price_scale(), ps),
            ("size", book.size_scale(), ss),
        ] {
            if let Some(m) = mine
                && m != theirs
            {
                return Err(BookError::ScaleMismatch {
                    field,
                    expected: m,
                    found: theirs,
                });
            }
        }
    }
    Ok(())
}

fn queue_diffs(
    book: &OrderBook,
    side: BookSide,
    orders: &[mesh_types::RestingOrder],
    out: &mut Vec<QueueDiff>,
) {
    let mut by_price: BTreeMap<Price, Vec<OrderId>> = BTreeMap::new();
    let mut order: Vec<Price> = Vec::new();
    for o in orders {
        let e = by_price.entry(o.price).or_default();
        if e.is_empty() {
            order.push(o.price);
        }
        e.push(o.order_id);
    }
    for price in order {
        let snap = by_price.remove(&price).unwrap_or_default();
        let mine: Vec<OrderId> = book
            .queue_at(side, price)
            .into_iter()
            .map(|o| o.order_id)
            .collect();
        let mut a = mine.clone();
        let mut b = snap.clone();
        a.sort_unstable();
        b.sort_unstable();
        if a == b && mine != snap {
            out.push(QueueDiff {
                side,
                price,
                book: mine,
                snapshot: snap,
            });
        }
    }
}

fn level_diffs(
    book: &OrderBook,
    side: BookSide,
    snap: &[Level],
    out: &mut Vec<LevelDiff>,
) -> Result<(), BookError> {
    let mine = book.levels(side, usize::MAX)?;
    // Both lists are best first; merge them in that order.
    let better = |a: Price, b: Price| match side {
        BookSide::Bid => a > b,
        BookSide::Ask => a < b,
    };
    let (mut i, mut j) = (0, 0);
    while i < mine.len() || j < snap.len() {
        let (m, s) = (mine.get(i), snap.get(j));
        match (m, s) {
            (Some(m), Some(s)) if m.price == s.price => {
                if m != s {
                    out.push(LevelDiff {
                        side,
                        price: m.price,
                        book: Some(*m),
                        snapshot: Some(*s),
                    });
                }
                i += 1;
                j += 1;
            }
            (Some(m), Some(s)) if better(m.price, s.price) => {
                out.push(LevelDiff {
                    side,
                    price: m.price,
                    book: Some(*m),
                    snapshot: None,
                });
                i += 1;
            }
            (Some(m), None) => {
                out.push(LevelDiff {
                    side,
                    price: m.price,
                    book: Some(*m),
                    snapshot: None,
                });
                i += 1;
            }
            (_, Some(s)) => {
                out.push(LevelDiff {
                    side,
                    price: s.price,
                    book: None,
                    snapshot: Some(*s),
                });
                j += 1;
            }
            (None, None) => break,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use mesh_types::RestingOrder;

    fn ro(id: u64, price: &str, size: &str, block: u64) -> RestingOrder {
        RestingOrder {
            order_id: OrderId(id),
            price: px(price),
            size: sz(size),
            ts_order_ms: UnixMillis(block_ms(block)),
        }
    }

    fn built() -> OrderBook {
        let mut b = OrderBook::new(inst());
        let evs = [
            add(1, 0, 1, BookSide::Bid, "100.0", "1.00"),
            add(1, 1, 2, BookSide::Bid, "100.0", "2.00"),
            add(1, 2, 3, BookSide::Bid, "99.0", "1.00"),
            add(1, 3, 4, BookSide::Ask, "101.0", "0.50"),
        ];
        b.apply_block(1, UnixMillis(block_ms(1)), &evs).unwrap();
        b
    }

    fn snap(bids: Vec<RestingOrder>, asks: Vec<RestingOrder>) -> BookSnapshot {
        BookSnapshot {
            prov: prov_eob(1, 1_000),
            depth: SnapshotDepth::L4 { bids, asks },
        }
    }

    #[test]
    fn clean_when_equal() {
        let b = built();
        let s = b.to_snapshot(prov_eob(1, 1_000)).unwrap();
        let r = reconcile(&b, &s, ReconcileOptions::default()).unwrap();
        assert!(r.is_clean(), "{r:?}");
        let l2 = b.to_l2_snapshot(prov_eob(1, 1_000), None).unwrap();
        assert!(
            reconcile(&b, &l2, ReconcileOptions::default())
                .unwrap()
                .is_clean()
        );
    }

    #[test]
    fn reports_every_difference_and_resyncs() {
        let mut b = built();
        // Snapshot: order 1 resized, 2 gone (extra in the book), 5 new
        // (missing from the book), 3 with another time, asks unchanged.
        let s = snap(
            vec![
                ro(1, "100.0", "0.50", 1),
                ro(5, "99.5", "1.00", 1),
                ro(3, "99.0", "1.00", 0),
            ],
            vec![ro(4, "101.0", "0.50", 1)],
        );
        let r = reconcile(&b, &s, ReconcileOptions::default()).unwrap();
        assert_eq!(
            r.missing.iter().map(|o| o.order_id.0).collect::<Vec<_>>(),
            [5]
        );
        assert_eq!(
            r.extra.iter().map(|o| o.order_id.0).collect::<Vec<_>>(),
            [2]
        );
        assert_eq!(
            r.changed
                .iter()
                .map(|d| d.book.order_id.0)
                .collect::<Vec<_>>(),
            [1, 3]
        );
        // Levels: 100.0 differs (3.00 vs 0.50), 99.5 only in the snapshot.
        assert_eq!(r.levels.len(), 2);
        assert_eq!(r.levels[0].price, px("100.0"));
        assert_eq!(r.levels[1].book, None);
        let no_time = ReconcileOptions {
            compare_order_time: false,
            ..ReconcileOptions::default()
        };
        assert_eq!(reconcile(&b, &s, no_time).unwrap().changed.len(), 1);

        let report = resync(&mut b, &s, ReconcileOptions::default()).unwrap();
        assert_eq!(report, r);
        assert!(
            reconcile(&b, &s, ReconcileOptions::default())
                .unwrap()
                .is_clean()
        );
        assert_eq!(b.seq(), Some(Seq::end_of_block(1)));
    }

    #[test]
    fn queue_order_difference() {
        let b = built();
        let s = snap(
            vec![
                ro(2, "100.0", "2.00", 1),
                ro(1, "100.0", "1.00", 1),
                ro(3, "99.0", "1.00", 1),
            ],
            vec![ro(4, "101.0", "0.50", 1)],
        );
        let r = reconcile(&b, &s, ReconcileOptions::default()).unwrap();
        assert_eq!(r.queue.len(), 1);
        assert_eq!(r.difference_count(), 1);
        let opts = ReconcileOptions {
            compare_queue_order: false,
            ..ReconcileOptions::default()
        };
        assert!(reconcile(&b, &s, opts).unwrap().is_clean());
    }

    #[test]
    fn rejects_foreign_or_rescaled_snapshots() {
        let b = built();
        let mut s = snap(vec![ro(1, "100.00", "1.00", 1)], vec![]);
        assert!(matches!(
            reconcile(&b, &s, ReconcileOptions::default()),
            Err(BookError::ScaleMismatch { field: "price", .. })
        ));
        s = snap(vec![], vec![]);
        s.prov.instrument = mesh_types::InstrumentId::new("hyperliquid:perp:OTHER").unwrap();
        assert!(matches!(
            reconcile(&b, &s, ReconcileOptions::default()),
            Err(BookError::InstrumentMismatch { .. })
        ));
    }
}
