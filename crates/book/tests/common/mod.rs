//! Shared test helpers: a deterministic, **synthetic** market simulator
//! that emits consistent trades and book events, and record builders.
//! Every value is invented by a seeded generator (hl-synth's SplitMix64).

#![allow(dead_code, clippy::unwrap_used, unreachable_pub)]

use std::collections::BTreeMap;

use hl_synth::SplitMix64;
use mesh_types::{
    AdmissionId, AggressorSide, BlockBatch, BookAction, BookEvent, BookSide, BookSnapshot,
    InstrumentId, OrderId, Price, Provenance, Record, RecordKind, RestingOrder, Seq, Size,
    SnapshotDepth, Trade, TradeId, UnixMillis, UnixNanos, VenueId,
};

/// 2026-01-01T00:00:00Z.
pub const BASE_MS: i64 = 1_767_225_600_000;
pub const PRICE_SCALE: u8 = 1;
pub const SIZE_SCALE: u8 = 3;

pub fn inst() -> InstrumentId {
    InstrumentId::new("hyperliquid:perp:SYN").unwrap()
}

pub fn prov(block: u64, index: u32, ts_ms: i64) -> Provenance {
    Provenance::new(
        inst(),
        VenueId::new("hyperliquid").unwrap(),
        Seq::new(block, index),
        UnixMillis(ts_ms),
        UnixNanos(ts_ms * 1_000_000 + 3_000_000),
        AdmissionId::new("ADM-0001").unwrap(),
    )
}

pub fn price(m: i64) -> Price {
    Price::new(m, PRICE_SCALE).unwrap()
}

pub fn size(m: i64) -> Size {
    Size::new(m, SIZE_SCALE).unwrap()
}

/// One block of simulated records.
#[derive(Debug, Clone)]
pub struct SimBlock {
    pub block_number: u64,
    pub block_time: UnixMillis,
    pub trades: Vec<Trade>,
    pub events: Vec<BookEvent>,
}

#[derive(Debug, Clone)]
struct SimOrder {
    side: BookSide,
    price: i64,
    size: i64,
    ts: i64,
}

/// A synthetic market: random adds, cancels, modifies and market orders
/// that sweep the book, producing fills and trades consistently.
#[derive(Debug, Clone)]
pub struct Sim {
    rng: SplitMix64,
    orders: BTreeMap<u64, SimOrder>,
    bids: BTreeMap<i64, Vec<u64>>,
    asks: BTreeMap<i64, Vec<u64>>,
    next_oid: u64,
    next_tid: u64,
    pub block: u64,
    pub time: i64,
    mid: i64,
    /// Actions per block are drawn from `0..=max_actions`.
    pub max_actions: u64,
}

impl Sim {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: SplitMix64::new(seed),
            orders: BTreeMap::new(),
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            next_oid: 1,
            next_tid: 1,
            block: 1_000_000,
            time: BASE_MS,
            mid: 10_000,
            max_actions: 8,
        }
    }

    pub fn order_count(&self) -> usize {
        self.orders.len()
    }

    fn best_bid(&self) -> Option<i64> {
        self.bids.keys().next_back().copied()
    }

    fn best_ask(&self) -> Option<i64> {
        self.asks.keys().next().copied()
    }

    fn side_map(&mut self, side: BookSide) -> &mut BTreeMap<i64, Vec<u64>> {
        match side {
            BookSide::Bid => &mut self.bids,
            BookSide::Ask => &mut self.asks,
        }
    }

    fn remove(&mut self, oid: u64) {
        if let Some(o) = self.orders.remove(&oid) {
            let m = self.side_map(o.side);
            if let Some(q) = m.get_mut(&o.price) {
                q.retain(|x| *x != oid);
                if q.is_empty() {
                    m.remove(&o.price);
                }
            }
        }
    }

    fn random_order(&mut self) -> Option<u64> {
        if self.orders.is_empty() {
            return None;
        }
        let n = self.rng.below(self.orders.len() as u64) as usize;
        self.orders.keys().nth(n).copied()
    }

    /// Adds `n` resting orders on each side around the mid (for big
    /// books), as one block.
    pub fn seed_book(&mut self, n: usize) -> SimBlock {
        self.block += 1;
        self.time += 1_000;
        let mut events = Vec::new();
        for i in 0..n {
            for side in [BookSide::Bid, BookSide::Ask] {
                let off = 1 + self.rng.range(0, (n as i64 / 4).clamp(10, 5_000));
                let p = match side {
                    BookSide::Bid => self.mid - off,
                    BookSide::Ask => self.mid + off,
                };
                let s = self.rng.range(1, 5_000);
                events.push(self.add(side, p, s, events.len()));
            }
            let _ = i;
        }
        SimBlock {
            block_number: self.block,
            block_time: UnixMillis(self.time),
            trades: Vec::new(),
            events,
        }
    }

    fn add(&mut self, side: BookSide, p: i64, s: i64, index: usize) -> BookEvent {
        let oid = self.next_oid;
        self.next_oid += 1;
        self.orders.insert(
            oid,
            SimOrder {
                side,
                price: p,
                size: s,
                ts: self.time,
            },
        );
        self.side_map(side).entry(p).or_default().push(oid);
        BookEvent {
            prov: prov(self.block, index as u32, self.time),
            order_id: OrderId(oid),
            side,
            price: price(p),
            action: BookAction::Add { size: size(s) },
        }
    }

    /// Simulates the next block (possibly empty).
    pub fn step(&mut self) -> SimBlock {
        self.block += 1;
        self.time += self.rng.range(200, 1_500);
        let mut events: Vec<BookEvent> = Vec::new();
        let mut trades: Vec<Trade> = Vec::new();
        let n = self.rng.below(self.max_actions + 1);
        for _ in 0..n {
            let r = self.rng.below(100);
            if r < 45 {
                let side = if self.rng.below(2) == 0 {
                    BookSide::Bid
                } else {
                    BookSide::Ask
                };
                let off = self.rng.range(1, 20);
                let p = match side {
                    BookSide::Bid => {
                        let p = self.mid - off;
                        self.best_ask().map_or(p, |a| p.min(a - 1))
                    }
                    BookSide::Ask => {
                        let p = self.mid + off;
                        self.best_bid().map_or(p, |b| p.max(b + 1))
                    }
                };
                let s = self.rng.range(1, 5_000);
                let idx = events.len();
                events.push(self.add(side, p, s, idx));
            } else if r < 65 {
                if let Some(oid) = self.random_order() {
                    let o = self.orders[&oid].clone();
                    events.push(BookEvent {
                        prov: prov(self.block, events.len() as u32, self.time),
                        order_id: OrderId(oid),
                        side: o.side,
                        price: price(o.price),
                        action: BookAction::Cancel {},
                    });
                    self.remove(oid);
                }
            } else if r < 75 {
                if let Some(oid) = self.random_order() {
                    let o = self.orders[&oid].clone();
                    if o.size > 1 {
                        let new = self.rng.range(1, o.size - 1);
                        events.push(BookEvent {
                            prov: prov(self.block, events.len() as u32, self.time),
                            order_id: OrderId(oid),
                            side: o.side,
                            price: price(o.price),
                            action: BookAction::Modify {
                                orig_size: Some(size(o.size)),
                                new_size: size(new),
                            },
                        });
                        if let Some(x) = self.orders.get_mut(&oid) {
                            x.size = new;
                        }
                    }
                }
            } else if r < 95 {
                let aggressor = if self.rng.below(2) == 0 {
                    AggressorSide::Buy
                } else {
                    AggressorSide::Sell
                };
                let mut qty = self.rng.range(1, 9_000);
                while qty > 0 {
                    let best = match aggressor {
                        AggressorSide::Buy => self.best_ask(),
                        _ => self.best_bid(),
                    };
                    let Some(level) = best else { break };
                    let side = match aggressor {
                        AggressorSide::Buy => BookSide::Ask,
                        _ => BookSide::Bid,
                    };
                    let oid = self.side_map(side)[&level][0];
                    let o = self.orders[&oid].clone();
                    let f = qty.min(o.size);
                    qty -= f;
                    let left = o.size - f;
                    events.push(BookEvent {
                        prov: prov(self.block, events.len() as u32, self.time),
                        order_id: OrderId(oid),
                        side,
                        price: price(o.price),
                        action: BookAction::Fill {
                            orig_size: if left > 0 { Some(size(o.size)) } else { None },
                            new_size: size(left),
                        },
                    });
                    trades.push(Trade {
                        prov: prov(self.block, trades.len() as u32, self.time),
                        price: price(o.price),
                        size: size(f),
                        aggressor,
                        trade_id: TradeId(self.next_tid),
                    });
                    self.next_tid += 1;
                    if left == 0 {
                        self.remove(oid);
                    } else if let Some(x) = self.orders.get_mut(&oid) {
                        x.size = left;
                    }
                }
            } else {
                self.mid += self.rng.range(-3, 3);
                if let (Some(b), Some(a)) = (self.best_bid(), self.best_ask())
                    && a - b >= 2
                {
                    self.mid = self.mid.clamp(b + 1, a - 1);
                }
            }
        }
        SimBlock {
            block_number: self.block,
            block_time: UnixMillis(self.time),
            trades,
            events,
        }
    }

    /// The simulator's own book as an L4 snapshot after the current block.
    pub fn snapshot(&self) -> BookSnapshot {
        let mut p = prov(self.block, 0, self.time);
        p.seq = Seq::end_of_block(self.block);
        let side = |m: &BTreeMap<i64, Vec<u64>>, rev: bool| -> Vec<RestingOrder> {
            let levels: Vec<(&i64, &Vec<u64>)> = if rev {
                m.iter().rev().collect()
            } else {
                m.iter().collect()
            };
            levels
                .into_iter()
                .flat_map(|(_, q)| q.iter())
                .map(|oid| {
                    let o = &self.orders[oid];
                    RestingOrder {
                        order_id: OrderId(*oid),
                        price: price(o.price),
                        size: size(o.size),
                        ts_order_ms: UnixMillis(o.ts),
                    }
                })
                .collect()
        };
        BookSnapshot {
            prov: p,
            depth: SnapshotDepth::L4 {
                bids: side(&self.bids, true),
                asks: side(&self.asks, false),
            },
        }
    }
}

/// Simulates `n` blocks, dropping empty ones.
pub fn simulate(seed: u64, n: usize) -> (Sim, Vec<SimBlock>) {
    let mut sim = Sim::new(seed);
    let mut blocks = Vec::new();
    for _ in 0..n {
        let b = sim.step();
        if !b.trades.is_empty() || !b.events.is_empty() {
            blocks.push(b);
        }
    }
    (sim, blocks)
}

/// The bus batches of a block, linked to the previous batches per kind.
pub fn batches(
    b: &SimBlock,
    prev_trade: &mut Option<u64>,
    prev_event: &mut Option<u64>,
) -> Vec<BlockBatch> {
    let mut out = Vec::new();
    if !b.trades.is_empty() {
        out.push(BlockBatch {
            instrument: inst(),
            venue: VenueId::new("hyperliquid").unwrap(),
            kind: RecordKind::Trade,
            block_number: b.block_number,
            block_time_ms: b.block_time,
            prev_block_number: *prev_trade,
            records: b.trades.iter().cloned().map(Record::from).collect(),
        });
        *prev_trade = Some(b.block_number);
    }
    if !b.events.is_empty() {
        out.push(BlockBatch {
            instrument: inst(),
            venue: VenueId::new("hyperliquid").unwrap(),
            kind: RecordKind::BookEvent,
            block_number: b.block_number,
            block_time_ms: b.block_time,
            prev_block_number: *prev_event,
            records: b.events.iter().cloned().map(Record::from).collect(),
        });
        *prev_event = Some(b.block_number);
    }
    out
}

pub fn all_trades(blocks: &[SimBlock]) -> Vec<Trade> {
    blocks
        .iter()
        .flat_map(|b| b.trades.iter().cloned())
        .collect()
}

pub fn all_events(blocks: &[SimBlock]) -> Vec<BookEvent> {
    blocks
        .iter()
        .flat_map(|b| b.events.iter().cloned())
        .collect()
}

pub fn block_times(blocks: &[SimBlock]) -> BTreeMap<u64, UnixMillis> {
    blocks
        .iter()
        .map(|b| (b.block_number, b.block_time))
        .collect()
}
