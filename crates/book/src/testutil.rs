//! Helpers for unit tests: records built directly, with synthetic values.

use mesh_types::{
    AdmissionId, AggressorSide, BookAction, BookEvent, BookSide, InstrumentId, OrderId, Price,
    Provenance, Seq, Size, Trade, TradeId, UnixMillis, UnixNanos, VenueId,
};

/// 2026-01-01T00:00:00Z.
pub(crate) const BASE_MS: i64 = 1_767_225_600_000;

pub(crate) fn inst() -> InstrumentId {
    InstrumentId::new("hyperliquid:perp:SYN").unwrap()
}

pub(crate) fn px(s: &str) -> Price {
    Price::parse_exact(s).unwrap()
}

pub(crate) fn sz(s: &str) -> Size {
    Size::parse_exact(s).unwrap()
}

pub(crate) fn ms(offset: i64) -> UnixMillis {
    UnixMillis(BASE_MS + offset)
}

/// Block time of block `b` in unit tests: one block per second.
pub(crate) fn block_ms(b: u64) -> i64 {
    BASE_MS + i64::try_from(b).unwrap() * 1_000
}

pub(crate) fn prov(block: u64, index: u32, ts_ms: i64) -> Provenance {
    Provenance::new(
        inst(),
        VenueId::new("hyperliquid").unwrap(),
        Seq::new(block, index),
        UnixMillis(ts_ms),
        UnixNanos(ts_ms * 1_000_000 + 4_000_000),
        AdmissionId::new("ADM-0001").unwrap(),
    )
}

pub(crate) fn prov_eob(block: u64, offset_ms: i64) -> Provenance {
    let mut p = prov(block, 0, BASE_MS + offset_ms);
    p.seq = Seq::end_of_block(block);
    p
}

pub(crate) fn event(
    block: u64,
    index: u32,
    oid: u64,
    side: BookSide,
    price: &str,
    action: BookAction,
) -> BookEvent {
    BookEvent {
        prov: prov(block, index, block_ms(block)),
        order_id: OrderId(oid),
        side,
        price: px(price),
        action,
    }
}

pub(crate) fn add(
    block: u64,
    index: u32,
    oid: u64,
    side: BookSide,
    price: &str,
    size: &str,
) -> BookEvent {
    event(
        block,
        index,
        oid,
        side,
        price,
        BookAction::Add { size: sz(size) },
    )
}

pub(crate) fn modify(
    block: u64,
    index: u32,
    oid: u64,
    side: BookSide,
    price: &str,
    orig: Option<&str>,
    new: &str,
) -> BookEvent {
    event(
        block,
        index,
        oid,
        side,
        price,
        BookAction::Modify {
            orig_size: orig.map(sz),
            new_size: sz(new),
        },
    )
}

pub(crate) fn fill(
    block: u64,
    index: u32,
    oid: u64,
    side: BookSide,
    price: &str,
    orig: Option<&str>,
    new: &str,
) -> BookEvent {
    event(
        block,
        index,
        oid,
        side,
        price,
        BookAction::Fill {
            orig_size: orig.map(sz),
            new_size: sz(new),
        },
    )
}

pub(crate) fn cancel(block: u64, index: u32, oid: u64, side: BookSide, price: &str) -> BookEvent {
    event(block, index, oid, side, price, BookAction::Cancel {})
}

pub(crate) fn trade(
    block: u64,
    index: u32,
    ts_offset_ms: i64,
    price: &str,
    size: &str,
    aggressor: AggressorSide,
) -> Trade {
    Trade {
        prov: prov(block, index, BASE_MS + ts_offset_ms),
        price: px(price),
        size: sz(size),
        aggressor,
        trade_id: TradeId(block * 1_000 + u64::from(index)),
    }
}
