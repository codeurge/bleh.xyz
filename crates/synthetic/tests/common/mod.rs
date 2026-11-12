//! Shared helpers: synthetic record builders and the fixture generator.
//!
//! Every value here is invented. The venues (`example-a`, `example-b`,
//! `example-c`, `example-rates`, `example-swaps`), markets, rates, prices
//! and admission IDs (`ADM-9001` to `ADM-9003`, which name no admission
//! record) are fictional.

#![allow(dead_code, unreachable_pub, clippy::unwrap_used)]

use std::path::PathBuf;

use mesh_types::{
    AdmissionId, AggressorSide, AssetClass, Compounding, ContentHash, DayCount, DerivedHeader,
    Grade, InstrumentId, LiquidityGrade, LiquidityLabel, MethodId, Notional, Price, Provenance,
    QueueLevel, QueuePositions, Record, ReferenceMeasure, ReferenceValue, Seq, Sha256Digest, Size,
    SwapAction, SwapEventType, SwapPrice, SwapPrint, Trade, TradeId, UnixMillis, UnixNanos,
    VenueId,
};
use synthetic::Registry;

/// 2026-03-09T00:00:00Z.
pub const T_START: i64 = 1_773_014_400_000;
/// 2026-03-21T00:00:00Z.
pub const T_END: i64 = T_START + 12 * DAY;
pub const HOUR: i64 = 3_600_000;
pub const DAY: i64 = 86_400_000;

pub const SPOT_ADM: &str = "ADM-9001";
pub const RATES_ADM: &str = "ADM-9002";
pub const SWAPS_ADM: &str = "ADM-9003";

pub fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic/synthetic-months")
}

pub fn registry() -> Registry {
    Registry::load(&fixture_dir().join("registry.json")).unwrap()
}

pub fn prov(instrument: &str, ts: i64, block: u64, index: u32, adm: &str) -> Provenance {
    let inst = InstrumentId::new(instrument).unwrap();
    let venue = VenueId::new(inst.venue_part()).unwrap();
    Provenance::new(
        inst,
        venue,
        Seq::new(block, index),
        UnixMillis(ts),
        UnixNanos(ts * 1_000_000 + 250_000),
        AdmissionId::new(adm).unwrap(),
    )
}

/// A price with exactly `scale` digits (half away from zero).
pub fn px(x: f64, scale: u8) -> Price {
    synthetic::engine::to_price(x, scale).unwrap()
}

pub fn trade(instrument: &str, ts: i64, price: f64, scale: u8, id: u64) -> Record {
    let block = u64::try_from(ts / 1000).unwrap();
    Trade {
        prov: prov(instrument, ts, block, 0, SPOT_ADM),
        price: px(price, scale),
        size: Size::parse_exact("1.0").unwrap(),
        aggressor: AggressorSide::Buy,
        trade_id: TradeId(id),
    }
    .into()
}

pub fn quote(instrument: &str, ts: i64, bid: f64, ask: f64, scale: u8) -> Record {
    let block = u64::try_from(ts / 1000).unwrap();
    let mut p = prov(instrument, ts, block, 0, SPOT_ADM);
    p.seq = Seq::end_of_block(block);
    let level = |price: f64| QueueLevel {
        price: px(price, scale),
        total_size: Size::parse_exact("1.0").unwrap(),
        order_count: 1,
        orders: Vec::new(),
        truncated: true,
    };
    QueuePositions {
        prov: p,
        derived: header("queue-position@1"),
        bid: Some(level(bid)),
        ask: Some(level(ask)),
    }
    .into()
}

fn header(method: &str) -> DerivedHeader {
    DerivedHeader {
        method: MethodId::new(method).unwrap(),
        inputs_hash: ContentHash(Sha256Digest([1; 32])),
        input_count: 1,
    }
}

pub fn grade(instrument: &str, ts: i64, g: Grade) -> Record {
    let block = u64::try_from(ts / 1000).unwrap();
    let mut p = prov(instrument, ts, block, 0, SPOT_ADM);
    p.seq = Seq::end_of_block(block);
    let label = if matches!(g, Grade::A | Grade::B) {
        LiquidityLabel::Benchmark
    } else {
        LiquidityLabel::Tape
    };
    LiquidityGrade {
        prov: p,
        derived: header("liquidity-grade@1"),
        as_of_ms: UnixMillis(ts),
        window_ms: 300_000,
        samples: 300,
        spread_bps: Some(Size::parse_exact("2.00").unwrap()),
        depth_bps: 50,
        depth_bid: Size::parse_exact("100000").unwrap(),
        depth_ask: Size::parse_exact("100000").unwrap(),
        trades: 100,
        trades_per_minute: Size::parse_exact("20.00").unwrap(),
        grade: g,
        label,
    }
    .into()
}

#[allow(clippy::too_many_arguments)]
pub fn reference(
    instrument: &str,
    ts: i64,
    index: u32,
    measure: ReferenceMeasure,
    value: f64,
    tenor: &str,
    compounding: Compounding,
    day_count: Option<DayCount>,
    as_of_date: &str,
    revision: u32,
) -> Record {
    let block = u64::try_from(ts / 1000).unwrap();
    ReferenceValue {
        prov: prov(instrument, ts, block, index, RATES_ADM),
        measure,
        value: px(value, 3),
        tenor: Some(tenor.into()),
        maturity_date: None,
        day_count,
        compounding: Some(compounding),
        currency: None,
        as_of_date: as_of_date.into(),
        revision,
    }
    .into()
}

pub fn overnight(ts: i64, rate: f64, as_of_date: &str, revision: u32) -> Record {
    reference(
        "example-rates:ref:OVERNIGHT",
        ts,
        0,
        ReferenceMeasure::Rate,
        rate,
        "ON",
        Compounding::Simple,
        Some(DayCount::Act360),
        as_of_date,
        revision,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn swap(
    id: &str,
    ts: i64,
    index: u32,
    class: AssetClass,
    underlier: &str,
    tenor: &str,
    expiry: Option<&str>,
    price: SwapPrice,
) -> Record {
    let block = u64::try_from(ts / 1000).unwrap();
    let sym = format!("{underlier}.{}", expiry.unwrap_or(tenor));
    let instrument = format!("example-swaps:swap:{sym}");
    let p = prov(&instrument, ts, block, index, SWAPS_ADM);
    SwapPrint {
        ts_dissemination_ms: p.ts_source_ms,
        prov: p,
        repository: VenueId::new("sdr-example").unwrap(),
        dissemination_id: id.into(),
        original_dissemination_id: None,
        action: SwapAction::New,
        asset_class: class,
        upi: None,
        product: Some("Synthetic:Example".into()),
        underlier: Some(underlier.into()),
        tenor: Some(tenor.into()),
        expiry: expiry.map(Into::into),
        price: Some(price),
        notional: Some(Notional {
            amount: Size::parse_exact("10000000").unwrap(),
            currency: "USD".into(),
        }),
        quantity: None,
        event_type: Some(SwapEventType::Trade),
        strike: None,
        option_premium: None,
        option_type: None,
        option_style: None,
        first_exercise_date: None,
        ts_execution_ms: UnixMillis(ts - 60_000),
    }
    .into()
}

pub fn ois(id: &str, ts: i64, index: u32, tenor: &str, rate: f64) -> Record {
    swap(
        id,
        ts,
        index,
        AssetClass::InterestRate,
        "EXAMPLE-OIS",
        tenor,
        None,
        SwapPrice::Rate { value: px(rate, 4) },
    )
}

pub fn crude_anchor(id: &str, ts: i64, index: u32, expiry: &str, price: f64) -> Record {
    swap(
        id,
        ts,
        index,
        AssetClass::Commodity,
        "EXAMPLE-CRUDE",
        "1M",
        Some(expiry),
        SwapPrice::Price {
            value: px(price, 2),
        },
    )
}

fn date_of(ms: i64) -> synthetic::date::Date {
    synthetic::date::Date::from_days(i32::try_from(ms.div_euclid(DAY)).unwrap())
}

/// A smooth invented wave.
fn wave(h: f64, period: f64) -> f64 {
    (2.0 * std::f64::consts::PI * h / period).sin()
}

/// Generates the fixture inputs: 12 days of invented spot trades on
/// eight markets, liquidity grades, an overnight rate with history,
/// bill and par curves, overnight-index swap prints and crude swap
/// anchors. Deterministic: no randomness.
pub fn fixture_inputs() -> Vec<Record> {
    let mut v: Vec<Record> = Vec::new();
    let govt =
        synthetic::calendar::Calendar::new(synthetic::calendar::CalendarId::UsGovernmentSecurities);
    let trading = synthetic::calendar::Calendar::new(synthetic::calendar::CalendarId::UsTrading);

    // Liquidity grades, one hour before the start.
    for (inst, g) in [
        ("example-a:perp:IDX", Grade::A),
        ("example-b:perp:IDX", Grade::B),
        ("example-c:perp:IDX", Grade::B),
        ("example-a:perp:OIL", Grade::B),
        ("example-b:perp:OIL", Grade::B),
        ("example-a:perp:GAS", Grade::C),
        ("example-a:perp:GOLD", Grade::A),
        ("example-b:perp:GOLD", Grade::A),
        ("example-a:perp:SILVER", Grade::B),
    ] {
        v.push(grade(inst, T_START - HOUR, g));
    }

    // Spot trades every two hours, staggered by source.
    let roll_ms = 1_773_705_600_000; // 2026-03-17T00:00:00Z, the crude roll.
    let mut id = 1u64;
    let mut k = 0i64;
    loop {
        let base = T_START + k * 2 * HOUR;
        if base >= T_END {
            break;
        }
        #[allow(clippy::cast_precision_loss)]
        let h = (base - T_START) as f64 / HOUR as f64;
        let idx = 5000.0 + 20.0 * wave(h, 50.0) + 0.02 * h;
        let oil = 70.0 + 0.8 * wave(h, 40.0) - if base >= roll_ms { 0.35 } else { 0.0 };
        let gas = 3.1 + 0.05 * wave(h, 30.0);
        let gold = 2900.0 + 5.0 * wave(h, 60.0);
        let silver = 32.5 + 0.2 * wave(h, 45.0);
        // Source c of the index prints one outlier on day 3.
        let c_idx = if k == 30 { idx * 1.03 } else { idx - 0.5 };
        let mut series: Vec<(&str, f64, u8)> = vec![
            ("example-a:perp:IDX", idx, 2),
            ("example-b:perp:IDX", idx + 0.75, 2),
            ("example-c:perp:IDX", c_idx, 2),
            ("example-a:perp:OIL", oil, 2),
            ("example-b:perp:OIL", oil + 0.04, 2),
            ("example-a:perp:GAS", gas, 3),
            ("example-a:perp:GOLD", gold, 1),
            ("example-b:perp:GOLD", gold + 0.4, 1),
        ];
        // The silver market stops trading on 15 March.
        if base < T_START + 6 * DAY {
            series.push(("example-a:perp:SILVER", silver, 3));
        }
        for (i, (inst, price, scale)) in series.into_iter().enumerate() {
            let ts = base + i64::try_from(i).unwrap() * 7 * 60_000;
            v.push(trade(inst, ts, price, scale, id));
            id += 1;
        }
        k += 1;
    }

    // Overnight fixings from 1 December 2025, published the next day at
    // 13:00 UTC; a cut on 28 January.
    let mut d = synthetic::date::Date::parse("2025-12-01").unwrap();
    let last = date_of(T_END);
    while d < last {
        if govt.is_business_day(d) {
            let rate = if d < synthetic::date::Date::parse("2026-01-29").unwrap() {
                4.30
            } else {
                4.05
            };
            let publish = govt.next_business_day(d).utc_midnight_ms() + 13 * HOUR;
            if publish < T_END {
                v.push(overnight(publish, rate, &d.to_string(), 0));
            }
        }
        d = d.add_days(1);
    }
    // One revised fixing.
    v.push(overnight(
        synthetic::date::Date::parse("2026-03-12")
            .unwrap()
            .utc_midnight_ms()
            + 15 * HOUR,
        4.06,
        "2026-03-11",
        1,
    ));

    // Bill and par curves each trading day at 21:00 UTC.
    let mut d = synthetic::date::Date::parse("2026-03-02").unwrap();
    while d < last {
        if govt.is_business_day(d) {
            let ts = d.utc_midnight_ms() + 21 * HOUR;
            for (i, (tenor, rate)) in [("4W", 4.10), ("13W", 4.05), ("26W", 3.95), ("52W", 3.80)]
                .into_iter()
                .enumerate()
            {
                v.push(reference(
                    "example-rates:ref:BILLS",
                    ts,
                    u32::try_from(i).unwrap(),
                    ReferenceMeasure::Rate,
                    rate,
                    tenor,
                    Compounding::Discount,
                    Some(DayCount::Act360),
                    &d.to_string(),
                    0,
                ));
            }
            for (i, (tenor, rate)) in [
                ("2Y", 3.75),
                ("3Y", 3.72),
                ("5Y", 3.80),
                ("7Y", 3.90),
                ("10Y", 4.05),
                ("30Y", 4.50),
            ]
            .into_iter()
            .enumerate()
            {
                v.push(reference(
                    "example-rates:ref:PAR",
                    ts,
                    u32::try_from(i).unwrap(),
                    ReferenceMeasure::ParYield,
                    rate,
                    tenor,
                    Compounding::SemiAnnual,
                    None,
                    &d.to_string(),
                    0,
                ));
            }
        }
        d = d.add_days(1);
    }

    // Overnight-index swap prints and crude anchors on trading days.
    let mut d = date_of(T_START - 3 * DAY);
    let mut n = 0u32;
    while d < last {
        if trading.is_business_day(d) {
            for (j, (tenor, rate)) in [
                ("1M", 4.08),
                ("3M", 4.02),
                ("6M", 3.93),
                ("1Y", 3.80),
                ("2Y", 3.62),
            ]
            .into_iter()
            .enumerate()
            {
                let ts = d.utc_midnight_ms() + 15 * HOUR + i64::try_from(j).unwrap() * 60_000;
                n += 1;
                v.push(ois(&format!("SYN-OIS-{n}"), ts, 0, tenor, rate));
                n += 1;
                v.push(ois(
                    &format!("SYN-OIS-{n}"),
                    ts + 30 * 60_000,
                    0,
                    tenor,
                    rate + 0.01,
                ));
            }
            for (j, (expiry, price)) in [
                ("2026-04-30", 69.60),
                ("2026-05-31", 69.10),
                ("2026-06-30", 68.50),
                ("2026-07-31", 68.00),
            ]
            .into_iter()
            .enumerate()
            {
                let ts = d.utc_midnight_ms() + 16 * HOUR + i64::try_from(j).unwrap() * 60_000;
                n += 1;
                v.push(crude_anchor(&format!("SYN-CRD-{n}"), ts, 0, expiry, price));
            }
        }
        d = d.add_days(1);
    }
    // A crude print reported in error and cancelled.
    let bad_ts = T_START + 3 * DAY + 17 * HOUR;
    v.push(crude_anchor("SYN-CRD-BAD", bad_ts, 0, "2026-05-31", 90.0));
    let mut cancel = crude_anchor("SYN-CRD-BAD-X", bad_ts + 10 * 60_000, 0, "2026-05-31", 90.0);
    if let Record::SwapPrint(p) = &mut cancel {
        p.action = SwapAction::Cancel;
        p.original_dissemination_id = Some("SYN-CRD-BAD".into());
    }
    v.push(cancel);

    v.sort_by(|a, b| {
        a.prov()
            .ts_source_ms
            .cmp(&b.prov().ts_source_ms)
            .then_with(|| a.canonical_cmp(b))
    });
    v
}

/// JSON lines of records.
pub fn to_jsonl<'a>(records: impl IntoIterator<Item = &'a Record>) -> String {
    let mut s = String::new();
    for r in records {
        s.push_str(&serde_json::to_string(r).unwrap());
        s.push('\n');
    }
    s
}
