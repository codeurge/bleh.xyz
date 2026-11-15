//! A prototype emulated matching engine.
//!
//! Price-time priority per asset, with the order types Hyperliquid
//! documents for limit orders:
//!
//! | Input | Behaviour |
//! |---|---|
//! | `Gtc` | Matches against the opposite side while it crosses, then rests |
//! | `Ioc`, `FrontendMarket` | Matches, then the remainder is dropped (it never rests, so no book event) |
//! | `Alo` | Rejected if it would cross on arrival; otherwise rests |
//! | Trigger orders | Not emulated: triggers fire on the mark price, an excluded oracle input. Counted |
//! | Reduce only | Matched like any order; positions are not tracked, so the venue's reduce-only rejection is not emulated. Counted |
//! | Self-trade | The resting order of the same account is canceled and matching continues ("expire maker", per Hyperliquid's docs) |
//! | `cancel`, `cancelByCloid` | Remove the resting order; unknown orders are counted |
//! | `modify`, `batchModify` | Cancel the old order and place the replacement with a new order ID at the back of the queue. A replacement that would cross is rejected and the old order is kept (modify without `always_place` must be post-only). **Unverified** against the venue |
//! | `scheduleCancel`, other actions | Not emulated. Counted |
//!
//! Trades execute at the resting order's price. Order IDs come from the
//! action's responses when given, otherwise from a counter. Trade IDs come
//! from a counter: the venue's `tid` is a hash whose construction is not
//! documented.
//!
//! The matcher emits [`MatchEvent`]s in processing order. Accounts never
//! leave the matcher.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, VecDeque};

use hl_node_reader::assumptions::venue_decimal;
use mesh_types::{AggressorSide, BookSide, InstrumentId, Price, Size};
use serde::Serialize;

use super::cmds::{Action, OrderRef, OrderType, OrderWire, ReplayBlock, Tif};

/// Instrument and scales for one asset index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetSpec {
    /// Mesh instrument.
    pub instrument: InstrumentId,
    /// Price scale.
    pub price_scale: u8,
    /// Size scale.
    pub size_scale: u8,
}

/// A change to a resting order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookChange {
    /// New resting order with this size (mantissa).
    Add {
        /// Size.
        size: i64,
    },
    /// Filled from `orig` to `new` (0 means removed).
    Fill {
        /// Size before.
        orig: i64,
        /// Size after.
        new: i64,
    },
    /// Removed without a fill.
    Cancel,
}

/// What the matcher emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchEvent {
    /// A resting-order change.
    Book {
        /// Asset index.
        asset: u32,
        /// Order ID.
        oid: u64,
        /// Side.
        side: BookSide,
        /// Limit price (mantissa).
        px: i64,
        /// Change.
        change: BookChange,
    },
    /// An execution.
    Trade {
        /// Asset index.
        asset: u32,
        /// Price (mantissa; the maker's price).
        px: i64,
        /// Size (mantissa).
        sz: i64,
        /// Taker side.
        aggressor: AggressorSide,
        /// Resting order.
        maker_oid: u64,
        /// Incoming order.
        taker_oid: u64,
        /// Emulator trade ID.
        tid: u64,
    },
}

/// Counters of what was and was not emulated.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReplayStats {
    /// Blocks applied.
    pub blocks: u64,
    /// Orders received.
    pub orders: u64,
    /// Orders rejected (unknown asset, bad price or size).
    pub rejected: u64,
    /// Post-only orders rejected because they would cross.
    pub alo_rejected: u64,
    /// IOC orders with an unfilled remainder.
    pub ioc_remainders: u64,
    /// Trigger orders (not emulated).
    pub triggers_not_emulated: u64,
    /// Reduce-only orders matched without a position check.
    pub reduce_only_unchecked: u64,
    /// Resting orders canceled by self-trade prevention.
    pub self_trade_cancels: u64,
    /// Cancels of orders not resting.
    pub cancel_missing: u64,
    /// Modifies of orders not resting.
    pub modify_missing: u64,
    /// Modifies whose replacement would cross.
    pub modify_rejected: u64,
    /// Actions with no emulated effect (`scheduleCancel`, others).
    pub not_emulated: u64,
    /// Trades emitted.
    pub trades: u64,
    /// Book events emitted.
    pub book_events: u64,
}

#[derive(Debug, Clone, Copy)]
struct Resting {
    oid: u64,
    account: u64,
    px: i64,
    sz: i64,
    ts_ms: i64,
}

#[derive(Debug, Default)]
struct Book {
    bids: BTreeMap<Reverse<i64>, VecDeque<Resting>>,
    asks: BTreeMap<i64, VecDeque<Resting>>,
}

impl Book {
    fn best(&self, side: BookSide) -> Option<i64> {
        match side {
            BookSide::Bid => self.bids.keys().next().map(|r| r.0),
            BookSide::Ask => self.asks.keys().next().copied(),
        }
    }

    fn queue_mut(&mut self, side: BookSide, px: i64) -> Option<&mut VecDeque<Resting>> {
        match side {
            BookSide::Bid => self.bids.get_mut(&Reverse(px)),
            BookSide::Ask => self.asks.get_mut(&px),
        }
    }

    fn drop_if_empty(&mut self, side: BookSide, px: i64) {
        match side {
            BookSide::Bid => {
                if self.bids.get(&Reverse(px)).is_some_and(VecDeque::is_empty) {
                    self.bids.remove(&Reverse(px));
                }
            }
            BookSide::Ask => {
                if self.asks.get(&px).is_some_and(VecDeque::is_empty) {
                    self.asks.remove(&px);
                }
            }
        }
    }

    fn push(&mut self, side: BookSide, r: Resting) {
        match side {
            BookSide::Bid => self.bids.entry(Reverse(r.px)).or_default().push_back(r),
            BookSide::Ask => self.asks.entry(r.px).or_default().push_back(r),
        }
    }

    fn remove(&mut self, side: BookSide, px: i64, oid: u64) -> Option<Resting> {
        let q = self.queue_mut(side, px)?;
        let i = q.iter().position(|r| r.oid == oid)?;
        let r = q.remove(i);
        self.drop_if_empty(side, px);
        r
    }

    fn side_levels(&self, side: BookSide) -> Vec<(i64, i64, u32)> {
        let agg = |(px, q): (i64, &VecDeque<Resting>)| {
            (
                px,
                q.iter().map(|r| r.sz).sum(),
                u32::try_from(q.len()).unwrap_or(u32::MAX),
            )
        };
        match side {
            BookSide::Bid => self.bids.iter().map(|(p, q)| agg((p.0, q))).collect(),
            BookSide::Ask => self.asks.iter().map(|(p, q)| agg((*p, q))).collect(),
        }
    }

    /// Best price on `side` ignoring order `skip`.
    fn best_excluding(&self, side: BookSide, skip: u64) -> Option<i64> {
        let first = |q: &VecDeque<Resting>| q.iter().find(|r| r.oid != skip).map(|r| r.px);
        match side {
            BookSide::Bid => self.bids.values().find_map(first),
            BookSide::Ask => self.asks.values().find_map(first),
        }
    }

    fn side_orders(&self, side: BookSide) -> Vec<(u64, i64, i64, i64)> {
        let flat = |q: &VecDeque<Resting>| {
            q.iter()
                .map(|r| (r.oid, r.px, r.sz, r.ts_ms))
                .collect::<Vec<_>>()
        };
        match side {
            BookSide::Bid => self.bids.values().flat_map(flat).collect(),
            BookSide::Ask => self.asks.values().flat_map(flat).collect(),
        }
    }
}

/// Where an order rests: asset, side, price, owner and client order ID.
#[derive(Debug, Clone)]
struct Locator {
    asset: u32,
    side: BookSide,
    px: i64,
    account: u64,
    cloid: Option<String>,
}

/// L2 levels `(price, size, order_count)` as mantissas, best first.
pub type Levels = Vec<(i64, i64, u32)>;

/// L4 orders `(oid, price, size, ts_ms)` as mantissas, best first, queue
/// order within a price.
pub type Orders = Vec<(u64, i64, i64, i64)>;

/// The emulated matcher.
#[derive(Debug)]
pub struct Matcher {
    assets: BTreeMap<u32, AssetSpec>,
    books: BTreeMap<u32, Book>,
    index: HashMap<u64, Locator>,
    cloids: HashMap<(u64, String), u64>,
    next_oid: u64,
    next_tid: u64,
    /// Counters.
    pub stats: ReplayStats,
}

const fn opposite(side: BookSide) -> BookSide {
    match side {
        BookSide::Bid => BookSide::Ask,
        BookSide::Ask => BookSide::Bid,
    }
}

const fn crosses(side: BookSide, px: i64, best_opposite: i64) -> bool {
    match side {
        BookSide::Bid => px >= best_opposite,
        BookSide::Ask => px <= best_opposite,
    }
}

impl Matcher {
    /// A matcher for the given assets. Generated order IDs start at
    /// `first_oid`.
    #[must_use]
    pub fn new(assets: BTreeMap<u32, AssetSpec>, first_oid: u64) -> Self {
        Self {
            assets,
            books: BTreeMap::new(),
            index: HashMap::new(),
            cloids: HashMap::new(),
            next_oid: first_oid,
            next_tid: 1,
            stats: ReplayStats::default(),
        }
    }

    /// The asset table.
    #[must_use]
    pub fn assets(&self) -> &BTreeMap<u32, AssetSpec> {
        &self.assets
    }

    fn take_oid(&mut self, given: Option<u64>) -> u64 {
        match given {
            Some(o) => {
                self.next_oid = self.next_oid.max(o + 1);
                o
            }
            None => {
                let o = self.next_oid;
                self.next_oid += 1;
                o
            }
        }
    }

    /// Parses price and size at the asset's scales.
    fn scaled(&self, o: &OrderWire) -> Option<(i64, i64)> {
        let spec = self.assets.get(&o.a)?;
        let px = Price::parse_with_scale(&venue_decimal(&o.p), spec.price_scale).ok()?;
        let sz = Size::parse_with_scale(&venue_decimal(&o.s), spec.size_scale).ok()?;
        (px.mantissa() > 0 && sz.mantissa() > 0).then_some((px.mantissa(), sz.mantissa()))
    }

    fn would_cross(&self, asset: u32, side: BookSide, px: i64) -> bool {
        self.books
            .get(&asset)
            .and_then(|b| b.best(opposite(side)))
            .is_some_and(|best| crosses(side, px, best))
    }

    fn emit_book(
        &mut self,
        out: &mut Vec<MatchEvent>,
        asset: u32,
        oid: u64,
        side: BookSide,
        px: i64,
        change: BookChange,
    ) {
        self.stats.book_events += 1;
        out.push(MatchEvent::Book {
            asset,
            oid,
            side,
            px,
            change,
        });
    }

    fn cancel_oid(&mut self, oid: u64, asset: Option<u32>, out: &mut Vec<MatchEvent>) -> bool {
        let Some(loc) = self.index.get(&oid).cloned() else {
            return false;
        };
        if asset.is_some_and(|a| a != loc.asset) {
            return false;
        }
        let removed = self
            .books
            .get_mut(&loc.asset)
            .and_then(|b| b.remove(loc.side, loc.px, oid));
        self.forget(oid);
        if removed.is_some() {
            self.emit_book(out, loc.asset, oid, loc.side, loc.px, BookChange::Cancel);
            true
        } else {
            false
        }
    }

    /// Drops an order from the indexes.
    fn forget(&mut self, oid: u64) {
        if let Some(loc) = self.index.remove(&oid)
            && let Some(c) = loc.cloid
        {
            self.cloids.remove(&(loc.account, c));
        }
    }

    /// Places one order. `force_post_only` applies the modify rule.
    #[allow(clippy::too_many_arguments)]
    fn place(
        &mut self,
        account: u64,
        o: &OrderWire,
        given_oid: Option<u64>,
        ts_ms: i64,
        force_post_only: bool,
        out: &mut Vec<MatchEvent>,
    ) {
        self.stats.orders += 1;
        let tif = match &o.t {
            OrderType::Trigger { .. } => {
                self.stats.triggers_not_emulated += 1;
                return;
            }
            OrderType::Limit { tif } => *tif,
        };
        let Some((px, mut remaining)) = self.scaled(o) else {
            self.stats.rejected += 1;
            return;
        };
        if o.r {
            self.stats.reduce_only_unchecked += 1;
        }
        let side = if o.b { BookSide::Bid } else { BookSide::Ask };
        let post_only = force_post_only || tif == Tif::Alo;
        if post_only && self.would_cross(o.a, side, px) {
            self.stats.alo_rejected += 1;
            return;
        }
        let oid = self.take_oid(given_oid);
        let asset = o.a;
        let aggressor = if o.b {
            AggressorSide::Buy
        } else {
            AggressorSide::Sell
        };

        // Match against the opposite side.
        while remaining > 0 {
            let book = self.books.entry(asset).or_default();
            let Some(best) = book.best(opposite(side)) else {
                break;
            };
            if !crosses(side, px, best) {
                break;
            }
            let Some(maker) = book
                .queue_mut(opposite(side), best)
                .and_then(|q| q.front().copied())
            else {
                break;
            };
            if maker.account == account {
                // Self-trade prevention: expire the maker.
                self.stats.self_trade_cancels += 1;
                self.cancel_oid(maker.oid, None, out);
                continue;
            }
            let q = remaining.min(maker.sz);
            let new = maker.sz - q;
            remaining -= q;
            if let Some(front) = book
                .queue_mut(opposite(side), best)
                .and_then(VecDeque::front_mut)
            {
                front.sz = new;
            }
            if new == 0 {
                if let Some(queue) = book.queue_mut(opposite(side), best) {
                    queue.pop_front();
                }
                book.drop_if_empty(opposite(side), best);
                self.forget(maker.oid);
            }
            let tid = self.next_tid;
            self.next_tid += 1;
            self.stats.trades += 1;
            out.push(MatchEvent::Trade {
                asset,
                px: maker.px,
                sz: q,
                aggressor,
                maker_oid: maker.oid,
                taker_oid: oid,
                tid,
            });
            self.emit_book(
                out,
                asset,
                maker.oid,
                opposite(side),
                maker.px,
                BookChange::Fill {
                    orig: maker.sz,
                    new,
                },
            );
        }

        if remaining == 0 {
            return;
        }
        match tif {
            Tif::Ioc | Tif::FrontendMarket if !force_post_only => {
                self.stats.ioc_remainders += 1;
            }
            _ => {
                self.books.entry(asset).or_default().push(
                    side,
                    Resting {
                        oid,
                        account,
                        px,
                        sz: remaining,
                        ts_ms,
                    },
                );
                self.index.insert(
                    oid,
                    Locator {
                        asset,
                        side,
                        px,
                        account,
                        cloid: o.c.clone(),
                    },
                );
                if let Some(c) = &o.c {
                    self.cloids.insert((account, c.clone()), oid);
                }
                self.emit_book(
                    out,
                    asset,
                    oid,
                    side,
                    px,
                    BookChange::Add { size: remaining },
                );
            }
        }
    }

    fn resolve(&self, account: u64, r: &OrderRef) -> Option<u64> {
        match r {
            OrderRef::Oid(o) => Some(*o),
            OrderRef::Cloid(c) => self.cloids.get(&(account, c.clone())).copied(),
        }
    }

    fn modify(
        &mut self,
        account: u64,
        r: &OrderRef,
        order: &OrderWire,
        new_oid: Option<u64>,
        ts_ms: i64,
        out: &mut Vec<MatchEvent>,
    ) {
        let target = self.resolve(account, r).filter(|o| {
            self.index
                .get(o)
                .is_some_and(|l| l.account == account && l.asset == order.a)
        });
        let Some(old) = target else {
            self.stats.modify_missing += 1;
            return;
        };
        let side = if order.b {
            BookSide::Bid
        } else {
            BookSide::Ask
        };
        // Decide on the replacement before touching the old order: a
        // replacement that would cross (ignoring the old order itself) is
        // rejected and the old order kept.
        let Some((px, _)) = self.scaled(order) else {
            self.stats.rejected += 1;
            return;
        };
        let crosses_now = self
            .books
            .get(&order.a)
            .and_then(|b| b.best_excluding(opposite(side), old))
            .is_some_and(|best| crosses(side, px, best));
        if crosses_now || matches!(order.t, OrderType::Trigger { .. }) {
            self.stats.modify_rejected += 1;
            return;
        }
        self.cancel_oid(old, Some(order.a), out);
        self.place(account, order, new_oid, ts_ms, true, out);
    }

    /// Applies one block and returns its events in processing order.
    pub fn apply_block(&mut self, block: &ReplayBlock) -> Vec<MatchEvent> {
        self.stats.blocks += 1;
        let mut out = Vec::new();
        let ts = block.block_time_ms;
        for sa in &block.actions {
            let acct = sa.account;
            match &sa.action {
                Action::Order { orders, oids, .. } => {
                    for (i, o) in orders.iter().enumerate() {
                        self.place(acct, o, oids.get(i).copied(), ts, false, &mut out);
                    }
                }
                Action::Cancel { cancels } => {
                    for c in cancels {
                        let owned = self.index.get(&c.o).is_some_and(|l| l.account == acct);
                        if !owned || !self.cancel_oid(c.o, Some(c.a), &mut out) {
                            self.stats.cancel_missing += 1;
                        }
                    }
                }
                Action::CancelByCloid { cancels } => {
                    for c in cancels {
                        let oid = self.cloids.get(&(acct, c.cloid.clone())).copied();
                        if !oid.is_some_and(|o| self.cancel_oid(o, Some(c.asset), &mut out)) {
                            self.stats.cancel_missing += 1;
                        }
                    }
                }
                Action::Modify {
                    oid,
                    order,
                    new_oid,
                } => self.modify(acct, oid, order, *new_oid, ts, &mut out),
                Action::BatchModify { modifies } => {
                    for m in modifies {
                        self.modify(acct, &m.oid, &m.order, m.new_oid, ts, &mut out);
                    }
                }
                Action::ScheduleCancel { .. } | Action::Other => self.stats.not_emulated += 1,
            }
        }
        out
    }

    /// L2 levels of an asset, best first: `(bids, asks)`.
    #[must_use]
    pub fn l2(&self, asset: u32) -> (Levels, Levels) {
        self.books.get(&asset).map_or_else(Default::default, |b| {
            (b.side_levels(BookSide::Bid), b.side_levels(BookSide::Ask))
        })
    }

    /// L4 orders of an asset, best first, queue order within a price:
    /// `(oid, px, sz, ts_ms)` per side.
    #[must_use]
    pub fn l4(&self, asset: u32) -> (Orders, Orders) {
        self.books.get(&asset).map_or_else(Default::default, |b| {
            (b.side_orders(BookSide::Bid), b.side_orders(BookSide::Ask))
        })
    }

    /// Assets with a book.
    pub fn book_assets(&self) -> impl Iterator<Item = u32> + '_ {
        self.books.keys().copied()
    }
}
