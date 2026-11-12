//! Deterministic generator of **synthetic** Hyperliquid node-format files.
//!
//! The output follows `docs/specs/hyperliquid-node-format.md`: per-block
//! lines (`--batch-by-block` shape) for fills, order statuses and raw book
//! diffs, a miscellaneous-events stream that readers must ignore, a
//! periodic L4 snapshot, a registry of the synthetic markets and an
//! `expected.json` with the ground truth a correct reader and book builder
//! must reproduce. Every value is invented by a seeded simulation; nothing
//! comes from real market data.
//!
//! Coverage: two fills per trade, partial fills, adds, in-place modifies,
//! cancels, IOC remainders, book diffs consistent with order statuses,
//! block boundaries that cross an hourly file boundary, empty blocks, an L4
//! snapshot with a trigger order, a market liquidation (kept, liquidation
//! object dropped), a backstop liquidation and a settlement fill pair
//! (dropped: no taker), excluded fields on every fill and order, and a
//! second scenario with a missing block (sequence gap).
//!
//! The simulation uses its own SplitMix64 generator, so output depends only
//! on the seed and this code, never on a dependency's RNG version.

use std::collections::BTreeMap;

use mesh_types::{AdmissionId, Flag, Flags, InstrumentSpec, MarketClass, Registry, UnixNanos};
use serde_json::{Map, Value, json};

/// Default seed for the committed golden set.
pub const DEFAULT_SEED: u64 = 42;
/// Default number of blocks per scenario.
pub const DEFAULT_BLOCKS: u64 = 60;
/// Height of the first synthetic block.
pub const FIRST_BLOCK: u64 = 1_000_000;
/// Block time of the first block: 2026-01-01T00:59:57.000Z, so the run
/// crosses from hour 0 into hour 1.
pub const START_MS: i64 = 1_767_229_197_000;

/// Generator settings.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// RNG seed.
    pub seed: u64,
    /// Blocks per scenario (at least 45, so every special case fits).
    pub blocks: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            seed: DEFAULT_SEED,
            blocks: DEFAULT_BLOCKS,
        }
    }
}

/// Error from generation.
#[derive(Debug)]
pub enum SynthError {
    /// The configuration cannot produce every scenario.
    Config(&'static str),
    /// Building the synthetic registry failed.
    Registry(String),
    /// JSON serialization failed.
    Json(serde_json::Error),
}

impl std::fmt::Display for SynthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(m) => write!(f, "configuration: {m}"),
            Self::Registry(m) => write!(f, "registry: {m}"),
            Self::Json(e) => write!(f, "json: {e}"),
        }
    }
}

impl std::error::Error for SynthError {}

impl From<serde_json::Error> for SynthError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// Generated files: relative path to contents, in path order.
pub type FileSet = BTreeMap<String, Vec<u8>>;

/// SplitMix64: tiny, fast, and fully specified, so output is stable.
#[derive(Debug, Clone)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    /// Seeds the generator.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// Next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform-ish integer in `0..n` (`n > 0`; modulo bias is irrelevant
    /// for fixtures).
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }

    /// Integer in `lo..=hi`.
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        let span = u64::try_from(hi - lo + 1).unwrap_or(1);
        lo + i64::try_from(self.below(span)).unwrap_or(0)
    }
}

/// Formats `mantissa * 10^-scale` with exactly `scale` fractional digits
/// (the mesh's canonical form).
#[must_use]
pub fn fixed(mantissa: i64, scale: u8) -> String {
    let sign = if mantissa < 0 { "-" } else { "" };
    let abs = mantissa.unsigned_abs();
    if scale == 0 {
        return format!("{sign}{abs}");
    }
    let pow = 10u64.pow(u32::from(scale));
    format!(
        "{sign}{}.{:0w$}",
        abs / pow,
        abs % pow,
        w = usize::from(scale)
    )
}

/// Formats like the venue: trailing fractional zeros (and a bare point)
/// removed, so readers must zero-fill to the instrument scale.
#[must_use]
pub fn venue_num(mantissa: i64, scale: u8) -> String {
    let s = fixed(mantissa, scale);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Bid,
    Ask,
}

impl Side {
    fn letter(self) -> &'static str {
        match self {
            Side::Bid => "B",
            Side::Ask => "A",
        }
    }
    fn word(self) -> &'static str {
        match self {
            Side::Bid => "Bid",
            Side::Ask => "Ask",
        }
    }
    fn mesh(self) -> &'static str {
        match self {
            Side::Bid => "bid",
            Side::Ask => "ask",
        }
    }
    fn opposite(self) -> Self {
        match self {
            Side::Bid => Side::Ask,
            Side::Ask => Side::Bid,
        }
    }
}

#[derive(Debug, Clone)]
struct MarketDef {
    coin: &'static str,
    class: MarketClass,
    sz_decimals: u8,
    price_scale: u8,
    tick: i64,
    lot: i64,
    ref_px: i64,
    min_px: i64,
    max_px: i64,
    hip3: bool,
}

fn markets() -> Vec<MarketDef> {
    vec![
        // Core perpetual: szDecimals 3, price scale 6 - 3 = 3.
        MarketDef {
            coin: "SYN",
            class: MarketClass::Perp,
            sz_decimals: 3,
            price_scale: 3,
            tick: 50,
            lot: 10,
            ref_px: 100_000,
            min_px: 50_000,
            max_px: 150_000,
            hip3: false,
        },
        // HIP-3 deployer market in the fictional "syn" namespace:
        // szDecimals 2, price scale 6 - 2 = 4.
        MarketDef {
            coin: "syn:IDX",
            class: MarketClass::Perp,
            sz_decimals: 2,
            price_scale: 4,
            tick: 50,
            lot: 1,
            ref_px: 500_000,
            min_px: 250_000,
            max_px: 750_000,
            hip3: true,
        },
        // HIP-4 outcome market. The symbol format and price scale are
        // assumptions (to verify): price is a probability in (0, 1).
        MarketDef {
            coin: "#9001",
            class: MarketClass::Outcome,
            sz_decimals: 0,
            price_scale: 4,
            tick: 10,
            lot: 1,
            ref_px: 5_000,
            min_px: 10,
            max_px: 9_990,
            hip3: false,
        },
    ]
}

#[derive(Debug, Clone)]
struct Order {
    oid: u64,
    user: u64,
    side: Side,
    px: i64,
    sz: i64,
    orig_sz: i64,
    ts_ms: i64,
    tif: &'static str,
    cloid: Option<String>,
}

#[derive(Debug, Default, Clone)]
struct Book {
    bids: Vec<Order>,
    asks: Vec<Order>,
}

impl Book {
    fn side(&self, s: Side) -> &Vec<Order> {
        match s {
            Side::Bid => &self.bids,
            Side::Ask => &self.asks,
        }
    }
    fn side_mut(&mut self, s: Side) -> &mut Vec<Order> {
        match s {
            Side::Bid => &mut self.bids,
            Side::Ask => &mut self.asks,
        }
    }
    fn insert(&mut self, o: Order) {
        let side = o.side;
        let v = self.side_mut(side);
        // Price priority, then time priority (after equal prices).
        let pos = v
            .iter()
            .position(|r| match side {
                Side::Bid => r.px < o.px,
                Side::Ask => r.px > o.px,
            })
            .unwrap_or(v.len());
        v.insert(pos, o);
    }
    fn best(&self, s: Side) -> Option<i64> {
        self.side(s).first().map(|o| o.px)
    }
}

#[derive(Debug, Default)]
struct BlockOut {
    fills: Vec<Value>,
    statuses: Vec<Value>,
    diffs: Vec<Value>,
    misc: Vec<Value>,
}

#[derive(Debug, Default)]
struct Truth {
    trades: Vec<Value>,
    book_events: Vec<Value>,
    dropped: BTreeMap<&'static str, u64>,
    liquidation_trades_kept: u64,
}

struct Gen {
    rng: SplitMix64,
    markets: Vec<MarketDef>,
    books: Vec<Book>,
    truth: Vec<Truth>,
    next_oid: u64,
    height: u64,
    t_ms: i64,
    record: bool,
    trade_idx: Vec<u32>,
    event_idx: Vec<u32>,
    out: BlockOut,
}

fn user_addr(i: u64) -> String {
    format!("0x{:040x}", 0x5157_0000_u64 + i)
}

fn iso_ms(t_ms: i64) -> String {
    UnixNanos(t_ms.saturating_mul(1_000_000)).format_rfc3339(3, false)
}

impl Gen {
    fn hash(&mut self) -> String {
        let mut s = String::from("0x");
        for _ in 0..4 {
            s.push_str(&format!("{:016x}", self.rng.next_u64()));
        }
        s
    }

    fn new_oid(&mut self) -> u64 {
        self.next_oid += 1 + self.rng.below(3);
        self.next_oid
    }

    fn order_json(&self, m: usize, o: &Order) -> Value {
        let d = &self.markets[m];
        json!({
            "coin": d.coin,
            "side": o.side.letter(),
            "limitPx": venue_num(o.px, d.price_scale),
            "sz": venue_num(o.sz, d.sz_decimals),
            "oid": o.oid,
            "timestamp": o.ts_ms,
            "triggerCondition": "N/A",
            "isTrigger": false,
            "triggerPx": "0.0",
            "children": [],
            "isPositionTpsl": false,
            "reduceOnly": false,
            "orderType": "Limit",
            "origSz": venue_num(o.orig_sz, d.sz_decimals),
            "tif": o.tif,
            "cloid": o.cloid,
        })
    }

    fn status(&mut self, m: usize, o: &Order, status: &str) {
        let v = json!({
            "time": iso_ms(self.t_ms),
            "user": user_addr(o.user),
            "status": status,
            "order": self.order_json(m, o),
        });
        self.out.statuses.push(v);
    }

    fn book_event(&mut self, m: usize, o: &Order, diff: Value, mut truth: Map<String, Value>) {
        let d = &self.markets[m];
        self.out.diffs.push(json!({
            "user": user_addr(o.user),
            "oid": o.oid,
            "coin": d.coin,
            "side": o.side.word(),
            "px": venue_num(o.px, d.price_scale),
            "raw_book_diff": diff,
        }));
        if self.record {
            truth.insert("seq".into(), json!([self.height, self.event_idx[m]]));
            truth.insert("order_id".into(), json!(o.oid));
            truth.insert("side".into(), json!(o.side.mesh()));
            truth.insert("price".into(), json!(fixed(o.px, d.price_scale)));
            self.truth[m].book_events.push(Value::Object(truth));
        }
        self.event_idx[m] += 1;
    }

    fn add_resting(&mut self, m: usize, o: Order) {
        let sz = self.markets[m].sz_decimals;
        let mut t = Map::new();
        t.insert("action".into(), json!("add"));
        t.insert("size".into(), json!(fixed(o.sz, sz)));
        self.book_event(m, &o, json!({"new": {"sz": venue_num(o.sz, sz)}}), t);
        self.status(m, &o, "open");
        self.books[m].insert(o);
    }

    fn passive_order(&mut self, m: usize, side: Side, user: u64) -> Order {
        let d = self.markets[m].clone();
        let book = &self.books[m];
        let away = self.rng.range(0, 4) * d.tick;
        let px = match side {
            Side::Bid => match (book.best(Side::Bid), book.best(Side::Ask)) {
                (_, Some(ask)) => ask - d.tick - away,
                (Some(bid), None) => bid - away,
                (None, None) => d.ref_px - d.tick - away,
            },
            Side::Ask => match (book.best(Side::Ask), book.best(Side::Bid)) {
                (_, Some(bid)) => bid + d.tick + away,
                (Some(ask), None) => ask + away,
                (None, None) => d.ref_px + d.tick + away,
            },
        }
        .clamp(d.min_px, d.max_px);
        let sz = d.lot * self.rng.range(1, 50);
        let oid = self.new_oid();
        let cloid = (self.rng.below(4) == 0).then(|| self.hash()[..34].to_owned());
        Order {
            oid,
            user,
            side,
            px,
            sz,
            orig_sz: sz,
            ts_ms: self.t_ms,
            tif: if self.rng.below(3) == 0 { "Alo" } else { "Gtc" },
            cloid,
        }
    }

    fn act_add(&mut self, m: usize) {
        let side = if self.rng.below(2) == 0 {
            Side::Bid
        } else {
            Side::Ask
        };
        let user = self.rng.below(6);
        let o = self.passive_order(m, side, user);
        // Never cross: a passive order priced through the touch is skipped.
        let opp = self.books[m].best(side.opposite());
        let crosses = match (side, opp) {
            (Side::Bid, Some(a)) => o.px >= a,
            (Side::Ask, Some(b)) => o.px <= b,
            _ => false,
        };
        if !crosses {
            self.add_resting(m, o);
        }
    }

    fn pick(&mut self, m: usize) -> Option<(Side, usize)> {
        let (nb, na) = (self.books[m].bids.len(), self.books[m].asks.len());
        if nb + na == 0 {
            return None;
        }
        let i = usize::try_from(self.rng.below(u64::try_from(nb + na).unwrap_or(1))).unwrap_or(0);
        Some(if i < nb {
            (Side::Bid, i)
        } else {
            (Side::Ask, i - nb)
        })
    }

    fn act_cancel(&mut self, m: usize) {
        let Some((side, i)) = self.pick(m) else {
            return;
        };
        let o = self.books[m].side_mut(side).remove(i);
        let mut t = Map::new();
        t.insert("action".into(), json!("cancel"));
        self.book_event(m, &o, json!("remove"), t);
        self.status(m, &o, "canceled");
    }

    fn act_modify(&mut self, m: usize) {
        let lot = self.markets[m].lot;
        let sz_dec = self.markets[m].sz_decimals;
        let Some((side, i)) = self.pick(m) else {
            return;
        };
        let old = self.books[m].side(side)[i].sz;
        let new = (old / 2 / lot) * lot;
        if new < lot || new >= old {
            return;
        }
        self.books[m].side_mut(side)[i].sz = new;
        let o = self.books[m].side(side)[i].clone();
        let mut t = Map::new();
        t.insert("action".into(), json!("modify"));
        t.insert("orig_size".into(), json!(fixed(old, sz_dec)));
        t.insert("new_size".into(), json!(fixed(new, sz_dec)));
        let diff =
            json!({"update": {"origSz": venue_num(old, sz_dec), "newSz": venue_num(new, sz_dec)}});
        self.book_event(m, &o, diff, t);
        self.status(m, &o, "open");
    }

    fn tid(buyer_oid: u64, seller_oid: u64) -> u64 {
        let mut r = SplitMix64::new(buyer_oid.rotate_left(17) ^ seller_oid);
        r.next_u64() & ((1u64 << 50) - 1)
    }

    #[allow(clippy::too_many_arguments)]
    fn fill_json(
        &mut self,
        m: usize,
        user: u64,
        side: Side,
        oid: u64,
        crossed: bool,
        px: i64,
        qty: i64,
        tid: u64,
        dir: &str,
        liquidation: Option<Value>,
    ) -> Value {
        let d = self.markets[m].clone();
        let mut fill = json!({
            "coin": d.coin,
            "px": venue_num(px, d.price_scale),
            "sz": venue_num(qty, d.sz_decimals),
            "side": side.letter(),
            "time": self.t_ms,
            "startPosition": "0.0",
            "dir": dir,
            "closedPnl": "0.0",
            "hash": self.hash(),
            "oid": oid,
            "crossed": crossed,
            "fee": if crossed { "0.0045" } else { "-0.0003" },
            "tid": tid,
            "feeToken": "USDC",
            "twapId": null,
        });
        if let Some(obj) = fill.as_object_mut() {
            if d.hip3 {
                obj.insert("deployerFee".into(), json!("0.0001"));
            }
            if let Some(l) = liquidation {
                obj.insert("liquidation".into(), l);
            }
        }
        json!([user_addr(user), fill])
    }

    fn record_trade(&mut self, m: usize, px: i64, qty: i64, aggressor: Side, tid: u64) {
        if self.record {
            let d = &self.markets[m];
            let v = json!({
                "seq": [self.height, self.trade_idx[m]],
                "ts_source_ms": self.t_ms,
                "trade_id": tid,
                "price": fixed(px, d.price_scale),
                "size": fixed(qty, d.sz_decimals),
                "aggressor": if aggressor == Side::Bid { "buy" } else { "sell" },
            });
            self.truth[m].trades.push(v);
        }
        self.trade_idx[m] += 1;
    }

    /// A marketable order: matches against the opposite side in priority
    /// order, emitting two fills per match, book diffs and statuses.
    fn act_take(&mut self, m: usize, liquidation: bool) {
        let d = self.markets[m].clone();
        let side = if liquidation || self.rng.below(2) == 0 {
            Side::Ask
        } else {
            Side::Bid
        };
        let opp = side.opposite();
        let Some(best) = self.books[m].best(opp) else {
            return;
        };
        let through = self.rng.range(0, 3) * d.tick;
        let limit = match side {
            Side::Bid => best + through,
            Side::Ask => best - through,
        };
        let qty = d.lot * self.rng.range(1, 80);
        let user = if liquidation { 7 } else { self.rng.below(6) };
        let oid = self.new_oid();
        let ioc = liquidation || self.rng.below(3) == 0;
        let mut taker = Order {
            oid,
            user,
            side,
            px: limit,
            sz: qty,
            orig_sz: qty,
            ts_ms: self.t_ms,
            tif: if ioc { "Ioc" } else { "Gtc" },
            cloid: None,
        };
        let mark = d.ref_px;
        while taker.sz > 0 {
            let Some(maker) = self.books[m].side(opp).first().cloned() else {
                break;
            };
            let ok = match side {
                Side::Bid => maker.px <= limit,
                Side::Ask => maker.px >= limit,
            };
            if !ok {
                break;
            }
            let q = taker.sz.min(maker.sz);
            let (buyer, seller) = match side {
                Side::Bid => (taker.oid, maker.oid),
                Side::Ask => (maker.oid, taker.oid),
            };
            let tid = Self::tid(buyer, seller);
            let liq = liquidation.then(|| {
                json!({
                    "liquidatedUser": user_addr(user),
                    "markPx": venue_num(mark, d.price_scale),
                    "method": "market",
                })
            });
            let taker_dir = if side == Side::Bid {
                "Open Long"
            } else {
                "Open Short"
            };
            let maker_dir = if side == Side::Bid {
                "Open Short"
            } else {
                "Open Long"
            };
            let f1 = self.fill_json(
                m, taker.user, side, taker.oid, true, maker.px, q, tid, taker_dir, liq,
            );
            let f2 = self.fill_json(
                m, maker.user, opp, maker.oid, false, maker.px, q, tid, maker_dir, None,
            );
            self.out.fills.push(f1);
            self.out.fills.push(f2);
            self.record_trade(m, maker.px, q, side, tid);
            if liquidation && self.record {
                self.truth[m].liquidation_trades_kept += 1;
            }
            taker.sz -= q;
            let remaining = maker.sz - q;
            let mut t = Map::new();
            t.insert("action".into(), json!("fill"));
            t.insert("new_size".into(), json!(fixed(remaining, d.sz_decimals)));
            if remaining == 0 {
                let filled = self.books[m].side_mut(opp).remove(0);
                self.book_event(m, &filled, json!("remove"), t);
                let mut done = filled;
                done.sz = 0;
                self.status(m, &done, "filled");
            } else {
                self.books[m].side_mut(opp)[0].sz = remaining;
                let o = self.books[m].side(opp)[0].clone();
                t.insert("orig_size".into(), json!(fixed(maker.sz, d.sz_decimals)));
                let diff = json!({"update": {
                    "origSz": venue_num(maker.sz, d.sz_decimals),
                    "newSz": venue_num(remaining, d.sz_decimals),
                }});
                self.book_event(m, &o, diff, t);
            }
        }
        if taker.sz == 0 {
            self.status(m, &taker, "filled");
        } else if ioc {
            self.status(m, &taker, "canceled");
        } else {
            self.add_resting(m, taker);
        }
    }

    /// A fill pair with no taker (both `crossed = false`) that is not an
    /// order-book trade. Readers drop it.
    fn act_no_taker_pair(&mut self, m: usize, reason: &'static str) {
        let d = self.markets[m].clone();
        let px = d.ref_px;
        let qty = d.lot * 5;
        let (a, b) = (self.new_oid(), self.new_oid());
        let tid = Self::tid(a, b);
        let (liq_a, liq_b, dir) = if reason == "backstop_liquidation" {
            let l = json!({"liquidatedUser": user_addr(7), "markPx": venue_num(px, d.price_scale), "method": "backstop"});
            (Some(l.clone()), Some(l), "Liquidated Isolated Long")
        } else {
            (None, None, "Settlement")
        };
        let f1 = self.fill_json(m, 7, Side::Ask, a, false, px, qty, tid, dir, liq_a);
        let f2 = self.fill_json(m, 6, Side::Bid, b, false, px, qty, tid, dir, liq_b);
        self.out.fills.push(f1);
        self.out.fills.push(f2);
        if self.record {
            *self.truth[m].dropped.entry(reason).or_default() += 1;
        }
    }

    fn misc_event(&mut self, inner: Value) {
        let h = self.hash();
        self.out
            .misc
            .push(json!({"time": iso_ms(self.t_ms), "hash": h, "inner": inner}));
    }

    fn book_truth(&self, m: usize) -> Value {
        let d = &self.markets[m];
        let side = |orders: &Vec<Order>| -> Vec<Value> {
            orders
                .iter()
                .map(|o| {
                    json!({
                        "order_id": o.oid,
                        "price": fixed(o.px, d.price_scale),
                        "size": fixed(o.sz, d.sz_decimals),
                        "ts_order_ms": o.ts_ms,
                    })
                })
                .collect()
        };
        json!({"bids": side(&self.books[m].bids), "asks": side(&self.books[m].asks)})
    }

    fn l4_snapshot(&self) -> Value {
        let mut coins = Vec::new();
        for (m, d) in self.markets.iter().enumerate() {
            let with_user = |o: &Order| -> Value {
                let mut v = self.order_json(m, o);
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("user".into(), json!(user_addr(o.user)));
                }
                v
            };
            let mut bids: Vec<Value> = self.books[m].bids.iter().map(with_user).collect();
            let asks: Vec<Value> = self.books[m].asks.iter().map(with_user).collect();
            if m == 0 {
                // A resting trigger order: not on the book, readers drop it.
                bids.push(json!({
                    "coin": d.coin, "side": "B",
                    "limitPx": venue_num(d.min_px, d.price_scale),
                    "sz": venue_num(d.lot, d.sz_decimals), "oid": 1,
                    "timestamp": START_MS, "triggerCondition": "Price below trigger",
                    "isTrigger": true, "triggerPx": venue_num(d.min_px, d.price_scale),
                    "children": [], "isPositionTpsl": false, "reduceOnly": true,
                    "orderType": "Stop Market", "origSz": venue_num(d.lot, d.sz_decimals),
                    "tif": null, "cloid": null, "user": user_addr(5),
                }));
            }
            coins.push(json!([d.coin, [bids, asks]]));
        }
        json!({"height": self.height, "snapshot": coins})
    }
}

/// A scenario: the base run, or the same run with one block missing.
#[derive(Debug, Clone, Copy)]
struct Scenario {
    name: &'static str,
    missing_block: Option<u64>,
}

fn block_line(height: u64, block_ms: i64, local_ns: i64, events: Vec<Value>) -> Value {
    json!({
        "local_time": UnixNanos(local_ns).format_rfc3339(9, false),
        "block_time": iso_ms(block_ms),
        "block_number": height,
        "events": events,
    })
}

fn hour_path(stream: &str, block_ms: i64) -> String {
    let t = UnixNanos(block_ms.saturating_mul(1_000_000)).format_rfc3339(0, false);
    // t = YYYY-MM-DDTHH:MM:SS
    let date: String = t[..10].chars().filter(char::is_ascii_digit).collect();
    let hour: u32 = t[11..13].parse().unwrap_or(0);
    format!("{stream}/hourly/{date}/{hour}")
}

fn run(cfg: Config, sc: Scenario, files: &mut FileSet) -> Result<(), SynthError> {
    let markets = markets();
    let n = markets.len();
    let mut g = Gen {
        rng: SplitMix64::new(cfg.seed),
        books: vec![Book::default(); n],
        truth: (0..n).map(|_| Truth::default()).collect(),
        markets,
        next_oid: 100_000,
        height: FIRST_BLOCK,
        t_ms: START_MS,
        record: true,
        trade_idx: vec![0; n],
        event_idx: vec![0; n],
        out: BlockOut::default(),
    };
    let snapshot_height = FIRST_BLOCK + 40;
    let last = FIRST_BLOCK + cfg.blocks - 1;
    let mut streams: BTreeMap<String, String> = BTreeMap::new();
    let mut at_snapshot = vec![Value::Null; n];
    for t in &mut g.truth {
        t.dropped.insert("backstop_liquidation", 0);
        t.dropped.insert("settlement", 0);
    }

    for height in FIRST_BLOCK..=last {
        g.height = height;
        if height > FIRST_BLOCK {
            g.t_ms += g.rng.range(60, 110);
        }
        let local_ns = g.t_ms * 1_000_000 + g.rng.range(5_000_000, 40_000_000);
        g.record = sc.missing_block != Some(height);
        g.trade_idx = vec![0; n];
        g.event_idx = vec![0; n];
        g.out = BlockOut::default();
        let k = height - FIRST_BLOCK;

        if k == 0 {
            for m in 0..n {
                for i in 0..5 {
                    for side in [Side::Bid, Side::Ask] {
                        let mut o = g.passive_order(m, side, i);
                        let d = &g.markets[m];
                        o.px = match side {
                            Side::Bid => d.ref_px - d.tick * (i64::try_from(i).unwrap_or(0) + 1),
                            Side::Ask => d.ref_px + d.tick * (i64::try_from(i).unwrap_or(0) + 1),
                        };
                        g.add_resting(m, o);
                    }
                }
            }
        } else {
            // Interleave actions across markets, as transactions would be.
            let mut actions: Vec<(usize, u64)> = Vec::new();
            for m in 0..n {
                // Every fifth block is empty for the outcome market.
                if m == 2 && k.is_multiple_of(5) {
                    continue;
                }
                for _ in 0..g.rng.below(5) {
                    let roll = g.rng.below(100);
                    actions.push((m, roll));
                }
            }
            for i in (1..actions.len()).rev() {
                let j =
                    usize::try_from(g.rng.below(u64::try_from(i + 1).unwrap_or(1))).unwrap_or(0);
                actions.swap(i, j);
            }
            for (m, roll) in actions {
                match roll {
                    0..=34 => g.act_add(m),
                    35..=54 => g.act_cancel(m),
                    55..=64 => g.act_modify(m),
                    _ => g.act_take(m, false),
                }
            }
            // Keep both sides populated.
            for m in 0..n {
                for side in [Side::Bid, Side::Ask] {
                    if g.books[m].side(side).len() < 3 {
                        let o = g.passive_order(m, side, 5);
                        g.add_resting(m, o);
                    }
                }
            }
            match k {
                10 => g.misc_event(json!({"Funding": {
                    "coin": "SYN", "usdc": "-0.12", "szi": "1.5",
                    "fundingRate": "0.0000125", "nSamples": null}})),
                20 => {
                    g.act_take(0, true);
                    g.misc_event(json!({"LedgerUpdate": {
                        "users": [user_addr(7)],
                        "delta": {"type": "liquidation", "liquidatedNtlPos": "500.0",
                                  "accountValue": "20.0", "leverageType": "Cross",
                                  "liquidatedPositions": [{"coin": "SYN", "szi": "-5.0"}]}}}));
                }
                25 => g.act_no_taker_pair(0, "backstop_liquidation"),
                _ => {}
            }
            if height == last {
                g.act_no_taker_pair(1, "settlement");
            }
        }

        if g.record {
            let out = std::mem::take(&mut g.out);
            for (stream, events) in [
                ("node_fills_by_block", out.fills),
                ("node_order_statuses_by_block", out.statuses),
                ("node_raw_book_diffs_by_block", out.diffs),
                ("misc_events_by_block", out.misc),
            ] {
                let line = serde_json::to_string(&block_line(height, g.t_ms, local_ns, events))?;
                let buf = streams.entry(hour_path(stream, g.t_ms)).or_default();
                buf.push_str(&line);
                buf.push('\n');
            }
        }
        if height == snapshot_height {
            let snap = serde_json::to_string_pretty(&g.l4_snapshot())?;
            files.insert(
                format!("{}/l4_snapshots/{height}.json", sc.name),
                format!("{snap}\n").into_bytes(),
            );
            for (m, slot) in at_snapshot.iter_mut().enumerate() {
                *slot = g.book_truth(m);
            }
        }
    }
    for (path, content) in streams {
        files.insert(format!("{}/{path}", sc.name), content.into_bytes());
    }

    let mut coins = Map::new();
    for (m, at_snapshot) in at_snapshot.into_iter().enumerate() {
        let d = &g.markets[m];
        let t = &g.truth[m];
        coins.insert(
            d.coin.to_owned(),
            json!({
                "instrument": instrument_id(d),
                "price_scale": d.price_scale,
                "size_scale": d.sz_decimals,
                "trades": t.trades,
                "book_events": t.book_events,
                "dropped_fill_pairs": t.dropped,
                "liquidation_trades_kept": t.liquidation_trades_kept,
                "book_at_snapshot": at_snapshot,
                "final_book": g.book_truth(m),
            }),
        );
    }
    let expected = json!({
        "synthetic": true,
        "generator": "hl-synth",
        "seed": cfg.seed,
        "scenario": sc.name,
        "first_block": FIRST_BLOCK,
        "last_block": last,
        "missing_blocks": sc.missing_block.into_iter().collect::<Vec<_>>(),
        "snapshot_height": snapshot_height,
        "admission": "ADM-0001",
        "coins": coins,
    });
    let text = serde_json::to_string_pretty(&expected)?;
    files.insert(
        format!("{}/expected.json", sc.name),
        format!("{text}\n").into_bytes(),
    );
    Ok(())
}

fn instrument_id(d: &MarketDef) -> String {
    format!("hyperliquid:{}:{}", d.class.as_str(), d.coin)
}

fn registry() -> Result<Registry, SynthError> {
    let adm = AdmissionId::new("ADM-0001").map_err(|e| SynthError::Registry(e.to_string()))?;
    let mut r = Registry::new();
    for d in markets() {
        let price_scale = (d.class == MarketClass::Outcome).then_some(d.price_scale);
        let mut spec =
            InstrumentSpec::hyperliquid(d.class, d.coin, d.sz_decimals, price_scale, adm.clone())
                .map_err(|e| SynthError::Registry(e.to_string()))?;
        if d.hip3 {
            spec = spec.with_flags(Flags::NONE.with(Flag::OracleAnchoredVenue));
        }
        if spec.price_scale != d.price_scale {
            return Err(SynthError::Registry(format!("{}: scale mismatch", d.coin)));
        }
        r.insert(spec)
            .map_err(|e| SynthError::Registry(e.to_string()))?;
    }
    Ok(r)
}

/// Generates the full golden set: `markets.json`, the `base` scenario and
/// the `gap` scenario (base with block `FIRST_BLOCK + 15` missing from
/// every stream).
///
/// # Errors
///
/// Fails if the configuration is too short or serialization fails.
pub fn generate(cfg: Config) -> Result<FileSet, SynthError> {
    if cfg.blocks < 45 {
        return Err(SynthError::Config("need at least 45 blocks"));
    }
    let mut files = FileSet::new();
    let reg = serde_json::to_string_pretty(&registry()?)?;
    files.insert("markets.json".into(), format!("{reg}\n").into_bytes());
    run(
        cfg,
        Scenario {
            name: "base",
            missing_block: None,
        },
        &mut files,
    )?;
    run(
        cfg,
        Scenario {
            name: "gap",
            missing_block: Some(FIRST_BLOCK + 15),
        },
        &mut files,
    )?;
    Ok(files)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use mesh_types::{Price, Size};

    #[test]
    fn committed_golden_set_is_current() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic/hl");
        for (rel, content) in generate(Config::default()).unwrap() {
            let on_disk = std::fs::read(root.join(&rel)).unwrap_or_default();
            assert!(
                on_disk == content,
                "fixtures/synthetic/hl/{rel} is stale; regenerate with `cargo run -p hl-synth`"
            );
        }
    }

    #[test]
    fn formatting_matches_mesh_types() {
        for (m, s) in [
            (0i64, 0u8),
            (5, 3),
            (-5, 2),
            (100_000, 3),
            (1, 4),
            (123_456_789, 4),
        ] {
            assert_eq!(fixed(m, s), Price::new(m, s).unwrap().to_string());
            let venue = venue_num(m, s);
            assert_eq!(
                Price::parse_with_scale(&venue, s).unwrap().mantissa(),
                m,
                "{venue}"
            );
        }
        assert_eq!(venue_num(100_000, 3), "100");
        assert_eq!(venue_num(100_250, 3), "100.25");
        assert_eq!(venue_num(0, 3), "0");
    }

    #[test]
    fn deterministic() {
        let a = generate(Config::default()).unwrap();
        let b = generate(Config::default()).unwrap();
        assert_eq!(a, b);
        let c = generate(Config {
            seed: 7,
            ..Config::default()
        })
        .unwrap();
        assert_ne!(a, c);
    }

    #[test]
    fn covers_the_required_cases() {
        let files = generate(Config::default()).unwrap();
        let expected: Value = serde_json::from_slice(&files["base/expected.json"]).unwrap();
        let coins = expected["coins"].as_object().unwrap();
        assert_eq!(coins.len(), 3);
        let mut actions = std::collections::BTreeSet::new();
        for c in coins.values() {
            assert!(!c["trades"].as_array().unwrap().is_empty());
            for e in c["book_events"].as_array().unwrap() {
                actions.insert(e["action"].as_str().unwrap().to_owned());
                if e["action"] == "fill" && e.get("orig_size").is_some() {
                    actions.insert("partial_fill".into());
                }
            }
        }
        for a in ["add", "modify", "fill", "partial_fill", "cancel"] {
            assert!(actions.contains(a), "missing {a}");
        }
        assert_eq!(
            coins["SYN"]["dropped_fill_pairs"]["backstop_liquidation"],
            1
        );
        assert_eq!(coins["syn:IDX"]["dropped_fill_pairs"]["settlement"], 1);
        assert!(coins["SYN"]["liquidation_trades_kept"].as_u64().unwrap() >= 1);
        // Files cross an hour boundary.
        assert!(files.contains_key("base/node_fills_by_block/hourly/20260101/0"));
        assert!(files.contains_key("base/node_fills_by_block/hourly/20260101/1"));
        // The gap scenario lacks exactly one block.
        let gap: Value = serde_json::from_slice(&files["gap/expected.json"]).unwrap();
        assert_eq!(gap["missing_blocks"][0], FIRST_BLOCK + 15);
        let count_lines = |prefix: &str| -> usize {
            files
                .iter()
                .filter(|(k, _)| k.starts_with(prefix))
                .map(|(_, v)| v.iter().filter(|&&b| b == b'\n').count())
                .sum()
        };
        assert_eq!(
            count_lines("base/node_raw_book_diffs_by_block/"),
            count_lines("gap/node_raw_book_diffs_by_block/") + 1
        );
        // Final books agree between scenarios (the gap is in the input, not the truth).
        assert_eq!(
            expected["coins"]["SYN"]["final_book"],
            gap["coins"]["SYN"]["final_book"]
        );
    }

    #[test]
    fn book_diffs_replay_to_the_expected_books() {
        // Independent check: applying every diff in order reproduces the
        // final book in expected.json, and statuses agree with diffs.
        let files = generate(Config::default()).unwrap();
        let expected: Value = serde_json::from_slice(&files["base/expected.json"]).unwrap();
        let mut books: BTreeMap<String, BTreeMap<u64, (String, String, String)>> = BTreeMap::new();
        for (path, content) in files
            .iter()
            .filter(|(k, _)| k.starts_with("base/node_raw_book_diffs_by_block/"))
        {
            let _ = path;
            for line in std::str::from_utf8(content).unwrap().lines() {
                let block: Value = serde_json::from_str(line).unwrap();
                for e in block["events"].as_array().unwrap() {
                    let coin = e["coin"].as_str().unwrap().to_owned();
                    let oid = e["oid"].as_u64().unwrap();
                    let book = books.entry(coin).or_default();
                    match &e["raw_book_diff"] {
                        Value::String(s) if s == "remove" => {
                            assert!(book.remove(&oid).is_some());
                        }
                        d if d.get("new").is_some() => {
                            let sz = d["new"]["sz"].as_str().unwrap().to_owned();
                            let side = e["side"].as_str().unwrap().to_owned();
                            let px = e["px"].as_str().unwrap().to_owned();
                            assert!(book.insert(oid, (side, px, sz)).is_none());
                        }
                        d => {
                            let entry = book.get_mut(&oid).unwrap();
                            assert_eq!(entry.2, d["update"]["origSz"].as_str().unwrap());
                            entry.2 = d["update"]["newSz"].as_str().unwrap().to_owned();
                        }
                    }
                }
            }
        }
        for (coin, c) in expected["coins"].as_object().unwrap() {
            let scale = u8::try_from(c["size_scale"].as_u64().unwrap()).unwrap();
            let fb = &c["final_book"];
            let n = fb["bids"].as_array().unwrap().len() + fb["asks"].as_array().unwrap().len();
            assert_eq!(books[coin].len(), n, "{coin}");
            for o in fb["bids"]
                .as_array()
                .unwrap()
                .iter()
                .chain(fb["asks"].as_array().unwrap())
            {
                let (_, _, sz) = &books[coin][&o["order_id"].as_u64().unwrap()];
                let want = Size::parse_exact(o["size"].as_str().unwrap()).unwrap();
                assert_eq!(Size::parse_with_scale(sz, scale).unwrap(), want);
            }
        }
    }
}
