//! Synthetic records and helpers shared by the unit tests. Every value is
//! invented.

use mesh_types::derived::{DerivedHeader, Footprint, FootprintLevel, ProfileLevel, VolumeProfile};
use mesh_types::swap::{AssetClass, Notional, SwapAction, SwapPrice, SwapPrint};
use mesh_types::{
    AdmissionId, AggressorSide, Bar, BarInterval, BookAction, BookEvent, BookSide, BookSnapshot,
    ContentHash, Flag, Flags, InstrumentId, Level, MethodId, OrderId, Price, Provenance, Record,
    RestingOrder, Seq, Sha256Digest, Size, SnapshotDepth, Trade, TradeId, UnixMillis, UnixNanos,
    VenueId,
};

/// 2026-01-01T00:00:00Z.
pub(crate) const DAY_MS: i64 = 1_767_225_600_000;

pub(crate) fn px(s: &str) -> Price {
    Price::parse_exact(s).unwrap()
}

pub(crate) fn sz(s: &str) -> Size {
    Size::parse_exact(s).unwrap()
}

pub(crate) fn prov(instrument: &str, block: u64, index: u32, ts_ms: i64) -> Provenance {
    let venue = instrument.split(':').next().unwrap();
    Provenance::new(
        InstrumentId::new(instrument).unwrap(),
        VenueId::new(venue).unwrap(),
        Seq::new(block, index),
        UnixMillis(ts_ms),
        UnixNanos(ts_ms * 1_000_000 + 4_000_000),
        AdmissionId::new("ADM-0001").unwrap(),
    )
}

pub(crate) fn trade(instrument: &str, block: u64, index: u32, ts_ms: i64) -> Record {
    Record::Trade(Trade {
        prov: prov(instrument, block, index, ts_ms),
        price: px("100.250"),
        size: sz("0.5"),
        aggressor: if index.is_multiple_of(2) {
            AggressorSide::Buy
        } else {
            AggressorSide::Sell
        },
        trade_id: TradeId(block * 1000 + u64::from(index)),
    })
}

pub(crate) fn book_event(block: u64, index: u32, action: BookAction) -> Record {
    Record::BookEvent(BookEvent {
        prov: prov("hyperliquid:perp:SYN", block, index, DAY_MS + 1000),
        order_id: OrderId(10 + u64::from(index)),
        side: if index.is_multiple_of(2) {
            BookSide::Bid
        } else {
            BookSide::Ask
        },
        price: px("99.950"),
        action,
    })
}

fn order(id: u64, price: &str, size: &str) -> RestingOrder {
    RestingOrder {
        order_id: OrderId(id),
        price: px(price),
        size: sz(size),
        ts_order_ms: UnixMillis(DAY_MS - 5000),
    }
}

pub(crate) fn header(count: u64) -> DerivedHeader {
    DerivedHeader {
        method: MethodId::new("bars@1").unwrap(),
        inputs_hash: ContentHash(Sha256Digest([7; 32])),
        input_count: count,
    }
}

/// One or more records of every kind, all valid, with the oracle flag on
/// one and multiple instruments.
pub(crate) fn every_kind() -> Vec<Record> {
    let mut v = vec![
        trade("hyperliquid:perp:SYN", 5, 0, DAY_MS + 100),
        trade("hyperliquid:perp:SYN", 5, 1, DAY_MS + 100),
        trade("hyperliquid:perp:syn:IDX", 6, 0, DAY_MS + 200),
        book_event(5, 0, BookAction::Add { size: sz("1.000") }),
        book_event(
            5,
            1,
            BookAction::Modify {
                orig_size: Some(sz("1.000")),
                new_size: sz("0.500"),
            },
        ),
        book_event(
            5,
            2,
            BookAction::Fill {
                orig_size: None,
                new_size: sz("0.000"),
            },
        ),
        book_event(5, 3, BookAction::Cancel {}),
        book_event(
            6,
            0,
            BookAction::Fill {
                orig_size: Some(sz("2.000")),
                new_size: sz("1.500"),
            },
        ),
    ];
    if let Record::Trade(t) = &mut v[2] {
        t.prov.flags = Flags::NONE.with(Flag::OracleAnchoredVenue);
    }
    let mut sp = prov("hyperliquid:perp:SYN", 9, 0, DAY_MS + 3000);
    sp.seq = Seq::end_of_block(9);
    v.push(Record::BookSnapshot(BookSnapshot {
        prov: sp.clone(),
        depth: SnapshotDepth::L4 {
            bids: vec![
                order(1, "99.500", "1.000"),
                order(2, "99.500", "0.500"),
                order(3, "99.000", "2.000"),
            ],
            asks: vec![order(4, "100.500", "0.100")],
        },
    }));
    let mut sp2 = sp.clone();
    sp2.seq = Seq::end_of_block(10);
    v.push(Record::BookSnapshot(BookSnapshot {
        prov: sp2,
        depth: SnapshotDepth::L2 {
            bids: vec![Level {
                price: px("99.500"),
                size: sz("1.500"),
                order_count: 2,
            }],
            asks: vec![],
        },
    }));
    let mut sp3 = sp;
    sp3.seq = Seq::end_of_block(11);
    v.push(Record::BookSnapshot(BookSnapshot {
        prov: sp3,
        depth: SnapshotDepth::L4 {
            bids: vec![],
            asks: vec![],
        },
    }));
    v.push(Record::Bar(Bar {
        prov: prov("hyperliquid:perp:SYN", 12, 0, DAY_MS + 900),
        derived: header(3),
        interval: BarInterval::S1,
        start_ms: UnixMillis(DAY_MS),
        open: px("100.000"),
        high: px("101.000"),
        low: px("99.000"),
        close: px("100.500"),
        volume: sz("3.000"),
        buy_volume: sz("2.000"),
        sell_volume: sz("1.000"),
        trade_count: 3,
    }));
    v.push(Record::Footprint(Footprint {
        prov: prov("hyperliquid:perp:SYN", 12, 0, DAY_MS + 900),
        derived: DerivedHeader {
            method: MethodId::new("footprint@1").unwrap(),
            ..header(2)
        },
        interval: BarInterval::M1,
        start_ms: UnixMillis(DAY_MS),
        levels: vec![
            FootprintLevel {
                price: px("99.500"),
                bid_volume: sz("1.0"),
                ask_volume: sz("0.0"),
                trade_count: 1,
            },
            FootprintLevel {
                price: px("100.000"),
                bid_volume: sz("0.0"),
                ask_volume: sz("2.0"),
                trade_count: 1,
            },
        ],
    }));
    v.push(Record::VolumeProfile(VolumeProfile {
        prov: prov("hyperliquid:perp:SYN", 12, 0, DAY_MS + 900),
        derived: DerivedHeader {
            method: MethodId::new("volume-profile@1").unwrap(),
            ..header(3)
        },
        start_ms: UnixMillis(DAY_MS),
        end_ms: UnixMillis(DAY_MS + 86_400_000),
        bucket: px("0.500"),
        levels: vec![ProfileLevel {
            price: px("100.000"),
            volume: sz("3.000"),
            buy_volume: sz("2.000"),
            sell_volume: sz("1.000"),
        }],
    }));
    let swap_prov = |idx: u32| {
        let mut p = prov("cftc-part43:swap:USD-SOFR", 77, idx, DAY_MS + 5000);
        p.admission = AdmissionId::new("ADM-0002").unwrap();
        p
    };
    v.push(Record::SwapPrint(SwapPrint {
        prov: swap_prov(0),
        repository: VenueId::new("sdr-example").unwrap(),
        dissemination_id: "D-1".into(),
        original_dissemination_id: None,
        action: SwapAction::New,
        asset_class: AssetClass::InterestRate,
        upi: Some("QZ0000000001".into()),
        product: Some("IRS fixed-float".into()),
        underlier: Some("USD-SOFR".into()),
        tenor: Some("5Y".into()),
        expiry: Some("2031-01-02".into()),
        price: Some(SwapPrice::Rate { value: px("3.875") }),
        notional: Some(Notional {
            amount: sz("25000000.00"),
            currency: "USD".into(),
        }),
        ts_execution_ms: UnixMillis(DAY_MS + 4000),
        ts_dissemination_ms: UnixMillis(DAY_MS + 5000),
    }));
    v.push(Record::SwapPrint(SwapPrint {
        prov: swap_prov(1),
        repository: VenueId::new("sdr-example").unwrap(),
        dissemination_id: "D-2".into(),
        original_dissemination_id: Some("D-1".into()),
        action: SwapAction::Cancel,
        asset_class: AssetClass::InterestRate,
        upi: None,
        product: None,
        underlier: None,
        tenor: None,
        expiry: None,
        price: None,
        notional: None,
        ts_execution_ms: UnixMillis(DAY_MS + 4000),
        ts_dissemination_ms: UnixMillis(DAY_MS + 5000),
    }));
    v
}

/// A deterministic signing key for tests.
pub(crate) fn key(b: u8) -> mesh_types::SigningKey {
    mesh_types::SigningKey::from_bytes(&[b; 32]).unwrap()
}
