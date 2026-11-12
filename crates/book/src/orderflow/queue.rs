//! Queue positions of top-of-book orders (`queue-position@1`).
//!
//! After each block, for the best bid and best ask levels: the orders in
//! queue order (up to `max_orders`), each with its size, the total size
//! ahead of it, and its placement time. A [`QueuePositions`] record is
//! emitted only when this differs from the last one emitted.
//!
//! Positions are **estimates**: they assume strict price-time priority and
//! that an in-place `modify` keeps the order's place (the book's rule; see
//! [`crate::book`]). A venue that moves modified orders to the back would
//! make the true queue differ.
//!
//! The inputs hash covers the book events (and any snapshot loaded) since
//! the previous record, in block-major order (see [`crate::inputs`]).

use mesh_types::{
    BookSide, ContentHasher, DerivedHeader, OrderId, Price, Provenance, Size, UnixMillis,
};
use serde::{Deserialize, Serialize};

use crate::book::OrderBook;
use crate::error::InputError;
use crate::inputs::header;
use crate::methods;

/// Queue settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueConfig {
    /// Orders listed per side.
    pub max_orders: usize,
}

impl Default for QueueConfig {
    /// Fifty orders per side.
    fn default() -> Self {
        Self { max_orders: 50 }
    }
}

/// One order in a top-of-book queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntry {
    /// Order ID.
    pub order_id: OrderId,
    /// Resting size.
    pub size: Size,
    /// Total size of the orders ahead of it at this price.
    pub size_ahead: Size,
    /// Placement time.
    pub ts_order_ms: UnixMillis,
}

/// The queue at one best price.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueLevel {
    /// Price.
    pub price: Price,
    /// Total resting size.
    pub total_size: Size,
    /// Number of orders.
    pub order_count: u32,
    /// The first orders, in queue order.
    pub orders: Vec<QueueEntry>,
    /// True if orders were left out (more than `max_orders`).
    pub truncated: bool,
}

/// Top-of-book queues after a block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuePositions {
    /// End of the block (`seq` is an end-of-block marker).
    pub prov: Provenance,
    /// `queue-position@1`; inputs are the book events since the previous
    /// record.
    pub derived: DerivedHeader,
    /// Best bid queue.
    pub bid: Option<QueueLevel>,
    /// Best ask queue.
    pub ask: Option<QueueLevel>,
}

/// The best level's queue on `side`.
///
/// # Errors
///
/// Fails only if a value cannot be represented.
pub fn top_queue(
    book: &OrderBook,
    side: BookSide,
    max_orders: usize,
) -> Result<Option<QueueLevel>, InputError> {
    let Some(level) = book.level_refs(side).next() else {
        return Ok(None);
    };
    let bad = |e: crate::BookError| InputError::Inconsistent(e.to_string());
    let mut orders = Vec::new();
    let mut ahead: i64 = 0;
    for id in level.order_ids().take(max_orders) {
        let Some(o) = book.order(id) else { continue };
        orders.push(QueueEntry {
            order_id: id,
            size: o.size,
            size_ahead: book.size_of(ahead).map_err(bad)?,
            ts_order_ms: o.ts_order_ms,
        });
        ahead = ahead
            .checked_add(o.size.mantissa())
            .ok_or(InputError::Overflow)?;
    }
    Ok(Some(QueueLevel {
        price: book.price_of(level.price_mantissa()).map_err(bad)?,
        total_size: book.size_of(level.size_mantissa()).map_err(bad)?,
        order_count: u32::try_from(level.order_count()).unwrap_or(u32::MAX),
        truncated: level.order_count() > orders.len(),
        orders,
    }))
}

/// Emits top-of-book queues when they change.
#[derive(Debug, Clone)]
pub struct QueueTracker {
    config: QueueConfig,
    last: Option<(Option<QueueLevel>, Option<QueueLevel>)>,
    hasher: ContentHasher,
}

impl QueueTracker {
    /// A tracker for `config`.
    #[must_use]
    pub fn new(config: QueueConfig) -> Self {
        Self {
            config,
            last: None,
            hasher: ContentHasher::new(),
        }
    }

    /// Adds an input record's canonical encoding (book event or snapshot),
    /// in block-major order.
    pub fn on_input(&mut self, canonical: &[u8]) {
        self.hasher.update_canonical(canonical);
    }

    /// After a block: emits a record if the queues changed. `prov` is the
    /// block's end-of-block provenance.
    ///
    /// # Errors
    ///
    /// Fails only if a value cannot be represented.
    pub fn after_block(
        &mut self,
        book: &OrderBook,
        prov: &Provenance,
        out: &mut Vec<QueuePositions>,
    ) -> Result<(), InputError> {
        let bid = top_queue(book, BookSide::Bid, self.config.max_orders)?;
        let ask = top_queue(book, BookSide::Ask, self.config.max_orders)?;
        if self
            .last
            .as_ref()
            .is_some_and(|(b, a)| *b == bid && *a == ask)
        {
            return Ok(());
        }
        let hasher = std::mem::take(&mut self.hasher);
        out.push(QueuePositions {
            prov: prov.clone(),
            derived: header(methods::QUEUE_POSITION, hasher)?,
            bid: bid.clone(),
            ask: ask.clone(),
        });
        self.last = Some((bid, ask));
        Ok(())
    }

    /// Forgets the last record and inputs (after a desync).
    pub fn reset(&mut self) {
        self.last = None;
        self.hasher = ContentHasher::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inputs::canonical_event;
    use crate::testutil::*;
    use mesh_types::BookSide::*;

    #[test]
    fn positions_and_change_detection() {
        let mut book = OrderBook::new(inst());
        let mut q = QueueTracker::new(QueueConfig { max_orders: 2 });
        let mut out = Vec::new();
        let b1 = [
            add(1, 0, 1, Bid, "100.0", "1.00"),
            add(1, 1, 2, Bid, "100.0", "2.00"),
            add(1, 2, 3, Bid, "100.0", "0.50"),
            add(1, 3, 4, Ask, "101.0", "1.00"),
        ];
        for e in &b1 {
            q.on_input(&canonical_event(e).unwrap());
        }
        book.apply_block(1, ms(1_000), &b1).unwrap();
        q.after_block(&book, &prov_eob(1, 1_000), &mut out).unwrap();
        assert_eq!(out.len(), 1);
        let bid = out[0].bid.as_ref().unwrap();
        assert_eq!(bid.orders.len(), 2);
        assert!(bid.truncated);
        assert_eq!(bid.order_count, 3);
        assert_eq!(bid.orders[1].size_ahead, sz("1.00"));
        assert_eq!(bid.total_size, sz("3.50"));
        assert_eq!(out[0].derived.input_count, 4);

        // A change away from the top: no record.
        let b2 = [add(2, 0, 5, Bid, "99.0", "1.00")];
        q.on_input(&canonical_event(&b2[0]).unwrap());
        book.apply_block(2, ms(2_000), &b2).unwrap();
        q.after_block(&book, &prov_eob(2, 2_000), &mut out).unwrap();
        assert_eq!(out.len(), 1);

        // A change at the top: a record covering both blocks' events.
        let b3 = [cancel(3, 0, 1, Bid, "100.0")];
        q.on_input(&canonical_event(&b3[0]).unwrap());
        book.apply_block(3, ms(3_000), &b3).unwrap();
        q.after_block(&book, &prov_eob(3, 3_000), &mut out).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].derived.input_count, 2);
        let bid = out[1].bid.as_ref().unwrap();
        assert_eq!(bid.orders[0].order_id, OrderId(2));
        assert_eq!(bid.orders[0].size_ahead, sz("0.00"));
        assert!(!bid.truncated);
    }
}
