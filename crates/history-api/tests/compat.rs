//! Compatibility layers: Databento-shaped DBN, TradingView UDF, CCXT shim.

#![allow(clippy::unwrap_used, missing_docs)]

mod common;

use std::collections::{BTreeMap, HashMap};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::*;
use dbn::decode::{DbnDecoder, DbnMetadata, DecodeRecord};
use dbn::{MboMsg, Mbp10Msg, OhlcvMsg, TradeMsg};
use history_api::http::databento::dbn_instrument_id;
use mesh_types::{BarInterval, Record, RecordKind, SnapshotDepth};
use tower::ServiceExt;

fn pow9(scale: u8) -> i64 {
    10i64.pow(9 - u32::from(scale))
}

#[tokio::test]
async fn dbn_trades_round_trip() {
    let app = app();
    let (s0, e0) = (start(), start() + 2 * HOUR);
    let (s, h, body) = get_raw(
        &app,
        &format!(
            "/databento/v0/timeseries.get_range?dataset=BLEH.HL&symbols=hyperliquid:perp:SYN,hyperliquid:perp:syn:IDX&schema=trades&start={s0}&end={e0}"
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert_eq!(h["content-type"], "application/octet-stream");
    assert_eq!(h["x-bleh-size-units"], "lots");
    let scales: BTreeMap<String, u8> =
        serde_json::from_str(h["x-bleh-size-scales"].to_str().unwrap()).unwrap();
    assert_eq!(scales["hyperliquid:perp:SYN"], 3);
    let mut dec = DbnDecoder::new(body.as_slice()).unwrap();
    let md = dec.metadata().clone();
    assert_eq!(md.dataset, "BLEH.HL");
    assert_eq!(md.mappings.len(), 2);
    let ids: HashMap<u32, mesh_types::InstrumentId> = [syn(), idx()]
        .into_iter()
        .map(|i| (dbn_instrument_id(&i), i))
        .collect();
    for m in &md.mappings {
        let id: u32 = m.intervals[0].symbol.parse().unwrap();
        assert_eq!(ids[&id].as_str(), m.raw_symbol);
    }
    let mut got: HashMap<String, Vec<TradeMsg>> = HashMap::new();
    let mut last_ts = 0;
    while let Some(t) = dec.decode_record::<TradeMsg>().unwrap() {
        assert!(t.hd.ts_event >= last_ts, "sorted by ts_event");
        last_ts = t.hd.ts_event;
        assert_eq!(t.hd.publisher_id, 45_001);
        got.entry(ids[&t.hd.instrument_id].to_string())
            .or_default()
            .push(t.clone());
    }
    for id in [syn(), idx()] {
        let want = all(RecordKind::Trade, &id);
        let want: Vec<&mesh_types::Trade> = want
            .iter()
            .filter_map(|r| match r {
                Record::Trade(t) if t.prov.ts_source_ms.0 >= s0 && t.prov.ts_source_ms.0 < e0 => {
                    Some(t)
                }
                _ => None,
            })
            .collect();
        let g = &got[id.as_str()];
        assert_eq!(g.len(), want.len(), "{id}");
        for (d, w) in g.iter().zip(&want) {
            assert_eq!(d.price, w.price.mantissa() * pow9(w.price.scale()));
            assert_eq!(i64::from(d.size), w.size.mantissa());
            assert_eq!(
                d.hd.ts_event,
                u64::try_from(w.prov.ts_source_ms.0 * 1_000_000).unwrap()
            );
            assert_eq!(d.ts_recv, u64::try_from(w.prov.ts_ingest_ns.0).unwrap());
            let side = match w.aggressor {
                mesh_types::AggressorSide::Buy => b'B',
                mesh_types::AggressorSide::Sell => b'A',
                mesh_types::AggressorSide::Unknown => b'N',
            };
            assert_eq!(d.side as u8, side);
            assert_eq!(d.action as u8, b'T');
        }
    }
}

#[tokio::test]
async fn dbn_mbo_rebuilds_the_book() {
    let app = app();
    let (s0, e0) = (start() + 10 * 60_000, start() + 50 * 60_000);
    let (s, _, body) = get_raw(
        &app,
        &format!(
            "/databento/v0/timeseries.get_range?dataset=BLEH.HL&symbols=hyperliquid:perp:SYN&schema=mbo&start={s0}&end={e0}&compression=zstd"
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let mut dec = DbnDecoder::with_zstd(body.as_slice()).unwrap();
    // Databento-style book: cancels subtract their size.
    let mut book: HashMap<u64, (u8, i64, i64)> = HashMap::new();
    let mut n = 0;
    let mut snapshot_seen = false;
    while let Some(m) = dec.decode_record::<MboMsg>().unwrap() {
        n += 1;
        let size = i64::from(m.size);
        match m.action as u8 {
            b'R' => {
                assert!(m.flags.is_snapshot());
                snapshot_seen = true;
                book.clear();
            }
            b'A' => {
                book.insert(m.order_id, (m.side as u8, m.price, size));
            }
            b'M' => {
                let e = book.get_mut(&m.order_id).unwrap();
                e.2 = size;
            }
            b'C' => {
                let e = book.get_mut(&m.order_id).unwrap();
                e.2 -= size;
                if e.2 <= 0 {
                    book.remove(&m.order_id);
                }
            }
            b'F' | b'T' => {}
            other => panic!("unexpected action {other}"),
        }
    }
    assert!(snapshot_seen && n > 50);
    // Compare with the API's book at the end of the range.
    let (_, v) = get_json(
        &app,
        &format!("/v0/book?instrument=hyperliquid:perp:SYN&at={}", e0 - 1),
    )
    .await;
    let Record::BookSnapshot(snap) = serde_json::from_value(v["book"].clone()).unwrap() else {
        panic!()
    };
    let SnapshotDepth::L4 { bids, asks } = snap.depth else {
        panic!()
    };
    let mut want: HashMap<u64, (u8, i64, i64)> = HashMap::new();
    for (side, orders) in [(b'B', bids), (b'A', asks)] {
        for o in orders {
            want.insert(
                o.order_id.0,
                (
                    side,
                    o.price.mantissa() * pow9(o.price.scale()),
                    o.size.mantissa(),
                ),
            );
        }
    }
    assert_eq!(book, want);
}

#[tokio::test]
async fn dbn_mbp10_levels_match_the_book() {
    let app = app();
    let (s0, e0) = (start() + 5 * 60_000, start() + 25 * 60_000);
    let (s, _, body) = get_raw(
        &app,
        &format!(
            "/databento/v0/timeseries.get_range?dataset=BLEH.HL&symbols=hyperliquid:perp:syn:IDX&schema=mbp-10&start={s0}&end={e0}"
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let mut dec = DbnDecoder::new(body.as_slice()).unwrap();
    let mut last: Option<Mbp10Msg> = None;
    while let Some(m) = dec.decode_record::<Mbp10Msg>().unwrap() {
        assert!(m.depth < 10);
        last = Some(m.clone());
    }
    let last = last.unwrap();
    let (_, v) = get_json(
        &app,
        &format!(
            "/v0/book?instrument=hyperliquid:perp:syn:IDX&at={}&depth=l2&levels=10",
            e0 - 1
        ),
    )
    .await;
    let Record::BookSnapshot(snap) = serde_json::from_value(v["book"].clone()).unwrap() else {
        panic!()
    };
    let SnapshotDepth::L2 { bids, asks } = snap.depth else {
        panic!()
    };
    for i in 0..10 {
        let lv = &last.levels[i];
        if let Some(b) = bids.get(i) {
            assert_eq!(lv.bid_px, b.price.mantissa() * pow9(b.price.scale()));
            assert_eq!(i64::from(lv.bid_sz), b.size.mantissa());
            assert_eq!(lv.bid_ct, b.order_count);
        } else {
            assert_eq!(lv.bid_px, dbn::UNDEF_PRICE);
        }
        if let Some(a) = asks.get(i) {
            assert_eq!(lv.ask_px, a.price.mantissa() * pow9(a.price.scale()));
            assert_eq!(i64::from(lv.ask_sz), a.size.mantissa());
        }
    }
}

#[tokio::test]
async fn dbn_ohlcv_json_csv_and_limits() {
    let app = app();
    let (s0, e0) = (start(), start() + 3 * HOUR);
    let (s, _, body) = get_raw(
        &app,
        &format!(
            "/databento/v0/timeseries.get_range?dataset=BLEH.HL&symbols=hyperliquid:perp:SYN&schema=ohlcv-1m&start={s0}&end={e0}"
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let mut dec = DbnDecoder::new(body.as_slice()).unwrap();
    let mut n = 0;
    while let Some(b) = dec.decode_record::<OhlcvMsg>().unwrap() {
        assert!(b.high >= b.low);
        n += 1;
    }
    let want = all(RecordKind::Bar, &syn())
        .iter()
        .filter(|r| matches!(r, Record::Bar(b) if b.interval == BarInterval::M1 && b.start_ms.0 >= s0 && b.start_ms.0 < e0))
        .count();
    assert_eq!(n, want);

    let (s, h, body) = get_raw(
        &app,
        &format!(
            "/databento/v0/timeseries.get_range?dataset=BLEH.HL&symbols=hyperliquid:perp:SYN&schema=trades&start={s0}&end={e0}&encoding=json&limit=7&pretty_px=true"
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h["content-type"], "application/json");
    let text = String::from_utf8(body).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 7);
    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert!(first["price"].is_string(), "{first}");

    let (s, h, body) = get_raw(
        &app,
        &format!(
            "/databento/v0/timeseries.get_range?dataset=BLEH.HL&symbols=hyperliquid:perp:SYN&schema=trades&start={s0}&end={e0}&encoding=csv&limit=3&size_units=whole"
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h["x-bleh-size-units"], "whole");
    assert_eq!(String::from_utf8(body).unwrap().lines().count(), 4);
}

#[tokio::test]
async fn dbn_post_form_and_errors() {
    let app = app();
    let body = format!(
        "dataset=BLEH.HL&symbols=hyperliquid%3Aperp%3ASYN&schema=trades&start={}&limit=2",
        start()
    );
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/databento/v0/timeseries.get_range")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let mut dec = DbnDecoder::new(bytes.as_ref()).unwrap();
    let mut n = 0;
    while dec.decode_record::<TradeMsg>().unwrap().is_some() {
        n += 1;
    }
    assert_eq!(n, 2);

    let s0 = start();
    for (q, status, code) in [
        (
            "dataset=NOPE&schema=trades",
            StatusCode::NOT_FOUND,
            "not_found",
        ),
        (
            "dataset=BLEH.HL&schema=tbbo&symbols=hyperliquid:perp:SYN",
            StatusCode::BAD_REQUEST,
            "unsupported",
        ),
        (
            "dataset=BLEH.HL&schema=bogus&symbols=hyperliquid:perp:SYN",
            StatusCode::BAD_REQUEST,
            "unsupported",
        ),
        (
            "dataset=BLEH.HL&schema=trades&symbols=hyperliquid:perp:SYN&stype_in=parent",
            StatusCode::BAD_REQUEST,
            "unsupported",
        ),
        (
            "dataset=BLEH.HL&schema=trades&symbols=other:perp:X",
            StatusCode::BAD_REQUEST,
            "bad_request",
        ),
    ] {
        let (s, v) = get_json(
            &app,
            &format!("/databento/v0/timeseries.get_range?{q}&start={s0}"),
        )
        .await;
        assert_eq!(s, status, "{q}: {v}");
        assert_eq!(v["error"]["code"], code, "{q}");
    }
    let (_, v) = get_json(&app, "/databento/v0/metadata.list_datasets").await;
    assert_eq!(v, serde_json::json!(["BLEH.HL"]));
    let (_, v) = get_json(&app, "/databento/v0/metadata.list_schemas?dataset=BLEH.HL").await;
    assert!(v.as_array().unwrap().iter().any(|s| s == "mbp-10"));
    let (_, v) = get_json(&app, "/databento/v0/metadata.list_publishers").await;
    assert_eq!(v[0]["publisher_id"], 45_001);
}

#[tokio::test]
async fn udf_datafeed() {
    let app = app();
    let (_, v) = get_json(&app, "/udf/config").await;
    assert_eq!(v["supports_search"], true);
    assert!(
        v["supported_resolutions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "1D")
    );
    let (_, v) = get_json(&app, "/udf/symbols?symbol=hyperliquid:perp:SYN").await;
    assert_eq!(v["pricescale"], 1000);
    assert_eq!(v["session"], "24x7");
    let (_, v) = get_json(&app, "/udf/symbols?symbol=hyperliquid:perp:NOPE").await;
    assert_eq!(v["s"], "error");
    let (_, v) = get_json(&app, "/udf/search?query=idx&limit=5").await;
    assert_eq!(v[0]["symbol"], "hyperliquid:perp:syn:IDX");

    let from = start() / 1000;
    let to = from + 3600 - 1;
    let (_, v) = get_json(
        &app,
        &format!("/udf/history?symbol=hyperliquid:perp:SYN&resolution=5&from={from}&to={to}"),
    )
    .await;
    assert_eq!(v["s"], "ok", "{v}");
    let bars: Vec<mesh_types::Bar> = all(RecordKind::Bar, &syn())
        .into_iter()
        .filter_map(|r| match r {
            Record::Bar(b)
                if b.interval == BarInterval::M5
                    && b.start_ms.0 >= from * 1000
                    && b.start_ms.0 <= to * 1000 =>
            {
                Some(b)
            }
            _ => None,
        })
        .collect();
    assert_eq!(v["t"].as_array().unwrap().len(), bars.len());
    assert_eq!(v["t"][0], bars[0].start_ms.0 / 1000);
    let o0 = v["o"][0].as_f64().unwrap();
    assert!((o0 - bars[0].open.to_string().parse::<f64>().unwrap()).abs() < 1e-9);

    let (_, v) = get_json(
        &app,
        &format!(
            "/udf/history?symbol=hyperliquid:perp:SYN&resolution=60&from=0&to={}&countback=2",
            from + 3 * 3600
        ),
    )
    .await;
    assert_eq!(v["t"].as_array().unwrap().len(), 2);

    let later = from + 10 * 86_400;
    let (_, v) = get_json(
        &app,
        &format!(
            "/udf/history?symbol=hyperliquid:perp:SYN&resolution=60&from={later}&to={}",
            later + 3600
        ),
    )
    .await;
    assert_eq!(v["s"], "no_data");
    assert!(v["nextTime"].as_i64().unwrap() < later);

    let (_, v) = get_json(
        &app,
        "/udf/history?symbol=hyperliquid:perp:SYN&resolution=7&from=0&to=1",
    )
    .await;
    assert_eq!(v["s"], "error");
    let (s, _, body) = get_raw(&app, "/udf/time").await;
    assert_eq!(s, StatusCode::OK);
    assert!(String::from_utf8(body).unwrap().parse::<i64>().unwrap() > 1_700_000_000);
}

#[tokio::test]
async fn ccxt_shim() {
    let app = app();
    let (_, v) = get_json(&app, "/ccxt").await;
    assert_eq!(v["has"]["fetchOHLCV"], true);
    let (_, v) = get_json(&app, "/ccxt/markets").await;
    assert_eq!(v.as_array().unwrap().len(), 2);
    let m = v
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "hyperliquid:perp:SYN")
        .unwrap();
    assert_eq!(m["type"], "swap");
    assert!((m["precision"]["price"].as_f64().unwrap() - 0.001).abs() < 1e-12);

    let trades = all(RecordKind::Trade, &syn());
    let (_, v) = get_json(
        &app,
        &format!(
            "/ccxt/trades?symbol=hyperliquid:perp:SYN&since={}&limit=5",
            start()
        ),
    )
    .await;
    let got = v.as_array().unwrap();
    assert_eq!(got.len(), 5);
    for (g, w) in got.iter().zip(&trades) {
        let Record::Trade(w) = w else { panic!() };
        assert_eq!(g["id"], w.trade_id.0.to_string());
        assert_eq!(g["timestamp"], w.prov.ts_source_ms.0);
        assert_eq!(
            g["info"],
            serde_json::to_value(Record::Trade(w.clone())).unwrap()
        );
    }
    // Most recent without since.
    let (_, v) = get_json(&app, "/ccxt/trades?symbol=hyperliquid:perp:SYN&limit=4").await;
    let got = v.as_array().unwrap();
    assert_eq!(got.len(), 4);
    let Record::Trade(last) = trades.last().unwrap() else {
        panic!()
    };
    assert_eq!(got[3]["id"], last.trade_id.0.to_string());

    let (_, v) = get_json(
        &app,
        &format!(
            "/ccxt/ohlcv?symbol=hyperliquid:perp:SYN&timeframe=15m&since={}&limit=3",
            start()
        ),
    )
    .await;
    let rows = v.as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].as_array().unwrap().len(), 6);
    #[allow(clippy::cast_precision_loss)]
    let s = start() as f64;
    assert!((rows[0][0].as_f64().unwrap() - s).abs() < 1.0);
    let (_, v) = get_json(
        &app,
        "/ccxt/ohlcv?symbol=hyperliquid:perp:SYN&timeframe=1h&limit=2",
    )
    .await;
    assert_eq!(v.as_array().unwrap().len(), 2);

    let t = start() + HOUR;
    let (s, v) = get_json(
        &app,
        &format!("/ccxt/orderbook?symbol=hyperliquid:perp:SYN&limit=5&timestamp={t}"),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let bids = v["bids"].as_array().unwrap();
    assert!(bids.len() <= 5 && !bids.is_empty());
    assert!(bids[0][0].as_f64().unwrap() > bids[1][0].as_f64().unwrap());
    assert!(v["asks"][0][0].as_f64().unwrap() > bids[0][0].as_f64().unwrap());
    assert!(v["nonce"].is_u64());
    let (_, v) = get_json(&app, "/ccxt/time").await;
    assert!(v.as_i64().unwrap() > 1_700_000_000_000);
}
