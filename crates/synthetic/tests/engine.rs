//! Engine behaviour on synthetic scenarios: calendars reaching records,
//! convergence at expiry, continuity across rolls, swap-anchor
//! interpolation, composite robustness, staleness and short-rate months.

#![allow(clippy::unwrap_used)]

mod common;

use common::{
    DAY, HOUR, crude_anchor, fixture_dir, grade, overnight, px, quote, reference, registry,
    to_jsonl, trade,
};
use mesh_types::{
    CarrySource, Compounding, DayCount, Grade, LiquidityLabel, Record, ReferenceMeasure,
    SwapAction, SyntheticMonth, SyntheticQuality,
};
use synthetic::Engine;
use synthetic::batch::{Grid, run};
use synthetic::calendar::{Calendar, CalendarId};
use synthetic::date::{Date, ny_to_utc_ms};

fn d(s: &str) -> Date {
    Date::parse(s).unwrap()
}

/// Rates inputs for valuations up to `until`: overnight fixings at
/// `rate` from 150 days before, and a bill and par curve every business
/// day, all published by 21:00 UTC on the business day after (fixings) or
/// the same day (curves).
fn rates(until: Date, rate: f64) -> Vec<Record> {
    let govt = Calendar::new(CalendarId::UsGovernmentSecurities);
    let mut v = Vec::new();
    let mut x = until.add_days(-150);
    while x <= until {
        if govt.is_business_day(x) {
            let publish = govt.next_business_day(x).utc_midnight_ms() + 13 * HOUR;
            v.push(overnight(publish, rate, &x.to_string(), 0));
            v.extend(curve_on(x, rate));
        }
        x = x.add_days(1);
    }
    v
}

fn curve_on(x: Date, rate: f64) -> Vec<Record> {
    let ts = x.utc_midnight_ms() + 21 * HOUR;
    let mut v = Vec::new();
    for (i, tenor) in ["4W", "13W", "26W", "52W"].into_iter().enumerate() {
        v.push(reference(
            "example-rates:ref:BILLS",
            ts,
            u32::try_from(i).unwrap(),
            ReferenceMeasure::Rate,
            rate,
            tenor,
            Compounding::Discount,
            Some(DayCount::Act360),
            &x.to_string(),
            0,
        ));
    }
    for (i, tenor) in ["2Y", "5Y", "10Y"].into_iter().enumerate() {
        v.push(reference(
            "example-rates:ref:PAR",
            ts,
            u32::try_from(i).unwrap(),
            ReferenceMeasure::ParYield,
            rate,
            tenor,
            Compounding::SemiAnnual,
            None,
            &x.to_string(),
            0,
        ));
    }
    v
}

const IDX: [&str; 3] = [
    "example-a:perp:IDX",
    "example-b:perp:IDX",
    "example-c:perp:IDX",
];

/// Index trades every `step` from `from` to `to` on the given sources.
fn index_trades(from: i64, to: i64, step: i64, price: impl Fn(i64, usize) -> f64) -> Vec<Record> {
    let mut v = Vec::new();
    for (i, inst) in IDX.iter().enumerate() {
        v.push(grade(
            inst,
            from - HOUR,
            if i == 0 { Grade::A } else { Grade::B },
        ));
    }
    let mut t = from;
    let mut id = 1;
    while t <= to {
        for (i, inst) in IDX.iter().enumerate() {
            v.push(trade(inst, t, price(t, i), 2, id));
            id += 1;
        }
        t += step;
    }
    v
}

fn months_of<'a>(out: &'a [SyntheticMonth], exposure: &str) -> Vec<&'a SyntheticMonth> {
    out.iter().filter(|m| m.exposure == exposure).collect()
}

fn f(p: mesh_types::Price) -> f64 {
    synthetic::engine::price_f64(p)
}

/// Pushes the inputs published by `as_of` (as a live feed would) and
/// values.
fn value_at(inputs: &[Record], as_of: i64) -> Vec<SyntheticMonth> {
    let mut e = Engine::new(registry());
    for r in inputs.iter().filter(|r| r.prov().ts_source_ms.0 <= as_of) {
        e.push(r);
    }
    e.value(as_of, u64::try_from(as_of / 1000).unwrap()).records
}

#[test]
fn juneteenth_moves_the_june_index_expiry() {
    let as_of = d("2026-06-10").utc_midnight_ms() + 15 * HOUR;
    let mut inputs = rates(d("2026-06-10"), 4.0);
    inputs.extend(index_trades(as_of - 2 * HOUR, as_of, HOUR, |_, _| 5000.0));
    let out = value_at(&inputs, as_of);
    let idx = months_of(&out, "us-large-cap-index");
    let june = idx.iter().find(|m| m.month == "2026-06").unwrap();
    assert_eq!(june.last_trade_date, "2026-06-18");
    assert_eq!(june.expiry_ms.0, ny_to_utc_ms(d("2026-06-18"), 9, 30));
    // 10 June is on or after the roll date (eight business days before
    // the 18th is 8 June): September is the front month.
    assert_eq!(june.rank, 0);
    assert!(june.quality.contains(&SyntheticQuality::Rolling));
    let sep = idx.iter().find(|m| m.month == "2026-09").unwrap();
    assert_eq!(sep.rank, 1);
    assert_eq!(sep.last_trade_date, "2026-09-18");
}

#[test]
fn fair_value_converges_to_spot_at_expiry() {
    let expiry = ny_to_utc_ms(d("2026-03-20"), 9, 30);
    let mut inputs = rates(d("2026-03-20"), 4.5);
    inputs.extend(index_trades(
        expiry - 3 * DAY,
        expiry,
        5 * 60_000,
        |_, i| 5000.0 + [0.0, 0.5, -0.5][i],
    ));
    let mut last_gap = f64::INFINITY;
    for minutes_before in [3 * 24 * 60, 24 * 60, 60, 5, 0] {
        let as_of = expiry - minutes_before * 60_000;
        let out = value_at(&inputs, as_of);
        let march = months_of(&out, "us-large-cap-index")
            .into_iter()
            .find(|m| m.month == "2026-03")
            .unwrap();
        let gap = (f(march.fair_value) - f(march.spot.unwrap())).abs();
        assert!(gap <= last_gap, "{minutes_before} min: gap {gap} grew");
        last_gap = gap;
        if minutes_before == 0 {
            assert_eq!(march.fair_value, march.spot.unwrap());
            assert_eq!(march.year_fraction.to_string(), "0.00000000");
            assert!(march.quality.contains(&SyntheticQuality::Expiring));
        }
    }
    // One minute after expiry the month is gone.
    let out = value_at(&inputs, expiry + 60_000);
    assert!(
        months_of(&out, "us-large-cap-index")
            .iter()
            .all(|m| m.month != "2026-03")
    );
}

#[test]
fn months_are_continuous_across_the_roll() {
    // Constant spot and rates; hourly values across the March roll (10
    // March 2026, 00:00 New York = 04:00 UTC).
    let from = d("2026-03-09").utc_midnight_ms();
    let to = d("2026-03-11").utc_midnight_ms();
    let mut inputs = rates(d("2026-03-11"), 4.0);
    inputs.extend(index_trades(from - HOUR, to, 30 * 60_000, |_, _| 5000.0));
    let mut e = Engine::new(registry());
    let out = run(
        &mut e,
        inputs,
        Grid {
            step_ms: HOUR,
            from_ms: Some(from),
            to_ms: Some(to),
        },
    )
    .unwrap();
    let idx = months_of(&out.records, "us-large-cap-index");
    let roll = d("2026-03-10").utc_midnight_ms() + 4 * HOUR;
    for month in ["2026-03", "2026-06"] {
        let series: Vec<&&SyntheticMonth> = idx.iter().filter(|m| m.month == month).collect();
        assert_eq!(series.len(), 49, "{month}");
        for w in series.windows(2) {
            let jump = (f(w[1].fair_value) - f(w[0].fair_value)).abs();
            // Carry decays by about 5000 * 2.7% / 8760 = 0.015 an hour;
            // anything more is a discontinuity. Rate curve updates at
            // 13:00 and 21:00 UTC are allowed a little more.
            assert!(jump <= 0.05, "{month} at {}: jump {jump}", w[1].as_of_ms.0);
        }
    }
    // The front switches exactly at the roll, and the old front stays as
    // rank 0 until expiry.
    for m in &idx {
        let front = if m.as_of_ms.0 < roll {
            "2026-03"
        } else {
            "2026-06"
        };
        if m.rank == 1 {
            assert_eq!(m.month, front, "at {}", m.as_of_ms.0);
        }
        if m.month == "2026-03" && m.as_of_ms.0 >= roll {
            assert_eq!(m.rank, 0);
        }
    }
    // At the roll, the continuous front series moves by exactly the
    // calendar spread at that instant.
    let at = |t: i64, month: &str| {
        idx.iter()
            .find(|m| m.as_of_ms.0 == t && m.month == month)
            .map(|m| f(m.fair_value))
            .unwrap()
    };
    let before = at(roll - HOUR, "2026-03");
    let after = at(roll, "2026-06");
    let spread = at(roll, "2026-06") - at(roll, "2026-03");
    assert!(((after - before) - spread).abs() < 0.03);
}

#[test]
fn swap_anchors_interpolate_log_linearly() {
    let as_of = d("2026-04-01").utc_midnight_ms() + 18 * HOUR;
    let mut inputs = rates(d("2026-04-01"), 4.0);
    for (i, inst) in ["example-a:perp:OIL", "example-b:perp:OIL"]
        .iter()
        .enumerate()
    {
        inputs.push(grade(inst, as_of - 2 * HOUR, Grade::B));
        inputs.push(trade(
            inst,
            as_of - HOUR,
            70.0,
            2,
            u64::try_from(i).unwrap(),
        ));
    }
    let a_jun = 68.0;
    let a_jul = 66.5;
    let t0 = as_of - 10 * HOUR;
    inputs.push(crude_anchor("A1", t0, 0, "2026-06-30", a_jun));
    inputs.push(crude_anchor(
        "A2",
        t0 + 60_000,
        0,
        "2026-06-30",
        a_jun + 0.2,
    ));
    inputs.push(crude_anchor(
        "A3",
        t0 + 120_000,
        0,
        "2026-06-30",
        a_jun - 0.2,
    ));
    inputs.push(crude_anchor("A4", t0, 1, "2026-07-31", a_jul));
    // A print reported in error, then cancelled: ignored.
    inputs.push(crude_anchor("BAD", t0 + 180_000, 0, "2026-07-31", 99.0));
    let mut cancel = crude_anchor("BAD-X", t0 + 240_000, 0, "2026-07-31", 99.0);
    if let Record::SwapPrint(p) = &mut cancel {
        p.action = SwapAction::Cancel;
        p.original_dissemination_id = Some("BAD".into());
    }
    inputs.push(cancel);
    let out = value_at(&inputs, as_of);
    let oil = months_of(&out, "wti-crude");
    // Contract 2026-07 expires 3 business days before 25 June (22 June),
    // between the mid-June and mid-July anchors.
    let jul = oil.iter().find(|m| m.month == "2026-07").unwrap();
    assert_eq!(jul.last_trade_date, "2026-06-22");
    let mid = |a: &str, b: &str| {
        let s = d(a).utc_midnight_ms();
        s + (d(b).utc_midnight_ms() - s) / 2
    };
    let t1 = mid("2026-06-01", "2026-07-01");
    let t2 = mid("2026-07-01", "2026-08-01");
    #[allow(clippy::cast_precision_loss)]
    let x = (jul.expiry_ms.0 - t1) as f64 / (t2 - t1) as f64;
    let want = a_jun.powf(1.0 - x) * a_jul.powf(x);
    assert!(
        (f(jul.fair_value) - want).abs() <= 0.006,
        "{} vs {want}",
        jul.fair_value
    );
    assert!(jul.quality.contains(&SyntheticQuality::AnchorInterpolated));
    assert_eq!(jul.carry_source, CarrySource::SwapAnchor);
    assert_eq!(jul.anchors, 2);
    // The front month (May, tracked by the spot) is the spot.
    let front = oil.iter().find(|m| m.rank == 1).unwrap();
    assert_eq!(front.month, "2026-05");
    assert_eq!(f(front.fair_value), 70.0);
    assert_eq!(front.carry_source, CarrySource::None);
    assert!(!front.quality.contains(&SyntheticQuality::CarryUnobserved));
    // Past the last anchor: extrapolated with financing.
    let aug = oil.iter().find(|m| m.month == "2026-08");
    assert!(aug.is_none_or(|m| m.quality.contains(&SyntheticQuality::AnchorExtrapolated)));
}

#[test]
fn energy_without_anchors_is_financing_only() {
    let as_of = d("2026-04-01").utc_midnight_ms() + 18 * HOUR;
    let mut inputs = rates(d("2026-04-01"), 4.0);
    inputs.push(trade("example-a:perp:GAS", as_of - HOUR, 3.2, 3, 1));
    let out = value_at(&inputs, as_of);
    let gas = months_of(&out, "us-natural-gas");
    assert_eq!(gas.len(), 2);
    let second = gas.iter().find(|m| m.rank == 2).unwrap();
    assert!(second.quality.contains(&SyntheticQuality::CarryUnobserved));
    assert!(second.quality.contains(&SyntheticQuality::SpotSingleSource));
    assert!(second.quality.contains(&SyntheticQuality::SpotTape));
    assert_eq!(second.carry_source, CarrySource::None);
    assert!(f(second.fair_value) > 3.2);
    // Ungraded single source: grade D, a tape.
    assert_eq!(second.grade, Some(Grade::D));
    assert_eq!(second.label, LiquidityLabel::Tape);
}

#[test]
fn composite_tolerates_removing_any_one_source() {
    let as_of = d("2026-03-12").utc_midnight_ms() + 15 * HOUR;
    let mut inputs = rates(d("2026-03-12"), 4.0);
    inputs.extend(index_trades(as_of - 3 * HOUR, as_of, HOUR, |_, i| {
        [5000.0, 5001.5, 4999.0][i]
    }));
    let full = value_at(&inputs, as_of);
    let full_jun = months_of(&full, "us-large-cap-index")
        .into_iter()
        .find(|m| m.month == "2026-06")
        .unwrap();
    assert_eq!(full_jun.spot_sources, 3);
    for removed in IDX {
        let rest: Vec<Record> = inputs
            .iter()
            .filter(|r| r.prov().instrument.as_str() != removed)
            .cloned()
            .collect();
        let out = value_at(&rest, as_of);
        let jun = months_of(&out, "us-large-cap-index")
            .into_iter()
            .find(|m| m.month == "2026-06")
            .unwrap();
        assert_eq!(jun.spot_sources, 2);
        let bps = (f(jun.fair_value) / f(full_jun.fair_value) - 1.0).abs() * 10_000.0;
        assert!(bps < 5.0, "without {removed}: {bps} bps");
        assert_ne!(jun.derived.inputs_hash, full_jun.derived.inputs_hash);
    }
}

#[test]
fn book_mids_count_as_observations() {
    let as_of = d("2026-03-12").utc_midnight_ms() + 15 * HOUR;
    let mut inputs = rates(d("2026-03-12"), 4.0);
    for (i, inst) in IDX.iter().enumerate() {
        inputs.push(grade(inst, as_of - 2 * HOUR, Grade::A));
        inputs.push(trade(
            inst,
            as_of - 50 * 60_000,
            4990.0,
            2,
            u64::try_from(i).unwrap(),
        ));
        // A newer top of book wins over the older trade.
        inputs.push(quote(inst, as_of - 10 * 60_000, 4999.5, 5000.5, 2));
    }
    let out = value_at(&inputs, as_of);
    let m = months_of(&out, "us-large-cap-index")[0];
    assert_eq!(m.spot, Some(px(5000.0, 2)));
    assert_eq!(m.spot_ms.unwrap().0, as_of - 10 * 60_000);
}

#[test]
fn staleness_is_flagged() {
    let as_of = d("2026-03-12").utc_midnight_ms() + 15 * HOUR;
    // Rates stop on 5 March: four fixing days before the 12th.
    let mut inputs = rates(d("2026-03-05"), 4.0);
    // Spot stopped five hours ago (fresh limit three hours).
    inputs.extend(index_trades(
        as_of - 8 * HOUR,
        as_of - 5 * HOUR,
        HOUR,
        |_, _| 5000.0,
    ));
    let out = value_at(&inputs, as_of);
    let m = months_of(&out, "us-large-cap-index")[0];
    assert!(m.quality.contains(&SyntheticQuality::RatesStale));
    assert!(m.quality.contains(&SyntheticQuality::SpotStale));
    assert!(m.quality.contains(&SyntheticQuality::SpotSingleSource));
    assert_eq!(m.spot_sources, 1);
    assert_eq!(m.grade, Some(Grade::D));
    assert_eq!(m.label, LiquidityLabel::Tape);
    // Past the hard limit (one day) there is no spot and no value.
    let out = value_at(&inputs, as_of + 2 * DAY);
    assert!(months_of(&out, "us-large-cap-index").is_empty());
    // Anchors older than 48 hours are stale.
    let mut inputs = rates(d("2026-04-01"), 4.0);
    let t = d("2026-04-01").utc_midnight_ms() + 18 * HOUR;
    inputs.push(trade("example-a:perp:OIL", t - HOUR, 70.0, 2, 1));
    inputs.push(crude_anchor("OLD", t - 72 * HOUR, 0, "2026-06-30", 68.0));
    let out = value_at(&inputs, t);
    let jun = months_of(&out, "wti-crude")
        .into_iter()
        .find(|m| m.anchors > 0)
        .unwrap();
    assert!(jun.quality.contains(&SyntheticQuality::AnchorStale));
}

#[test]
fn short_rate_months_use_fixings_and_the_curve() {
    let as_of = d("2026-05-15").utc_midnight_ms() + 18 * HOUR;
    let mut inputs = rates(d("2026-05-15"), 4.0);
    let out = value_at(&inputs, as_of);
    let q = months_of(&out, "sofr-compounded-quarter");
    // The March quarter (18 March to 17 June) is in progress.
    let mar = q.iter().find(|m| m.month == "2026-03").unwrap();
    assert_eq!(mar.last_trade_date, "2026-06-16");
    let implied = f(mar.implied_rate.unwrap());
    // Fixings at 4% simple; the curve forward is slightly higher (bill
    // discount rates convert to higher simple rates).
    assert!((implied - 4.0).abs() < 0.05, "{implied}");
    assert!((f(mar.fair_value) - (100.0 - implied)).abs() < 1e-4);
    assert!(!mar.quality.contains(&SyntheticQuality::FixingsMissing));
    assert_eq!(mar.grade, None);
    assert_eq!(mar.spot, None);
    // Drop one past fixing: filled from the curve and flagged.
    inputs.retain(|r| match r {
        Record::ReferenceValue(v) => {
            v.as_of_date != "2026-04-15" || v.tenor.as_deref() != Some("ON")
        }
        _ => true,
    });
    let out = value_at(&inputs, as_of);
    let mar = months_of(&out, "sofr-compounded-quarter")
        .into_iter()
        .find(|m| m.month == "2026-03")
        .unwrap();
    assert!(mar.quality.contains(&SyntheticQuality::FixingsMissing));
    let avg = months_of(&out, "sofr-average-month");
    let may = avg.iter().find(|m| m.month == "2026-05").unwrap();
    assert_eq!(may.last_trade_date, "2026-05-29");
}

#[test]
fn future_inputs_never_leak_into_a_value() {
    let as_of = d("2026-03-12").utc_midnight_ms() + 15 * HOUR;
    let mut inputs = rates(d("2026-03-12"), 4.0);
    inputs.extend(index_trades(as_of - 3 * HOUR, as_of, HOUR, |_, _| 5000.0));
    let base = value_at(&inputs, as_of);
    // Push everything, plus later trades at another price and later
    // rates, before valuing: the value is unchanged.
    let mut future: Vec<Record> =
        index_trades(as_of + 60_000, as_of + 3 * HOUR, HOUR, |_, _| 6000.0);
    future.extend(rates(d("2026-03-14"), 9.0));
    future.retain(|r| r.prov().ts_source_ms.0 > as_of);
    let mut e = Engine::new(registry());
    for r in inputs.iter().chain(&future) {
        e.push(r);
    }
    let pushed_ahead = e.value(as_of, u64::try_from(as_of / 1000).unwrap()).records;
    assert_eq!(pushed_ahead, base);
}

#[test]
fn output_is_deterministic_and_order_independent() {
    let text = std::fs::read_to_string(fixture_dir().join("inputs.jsonl")).unwrap();
    let inputs = synthetic::batch::read_records(text.as_bytes()).unwrap();
    let grid = Grid {
        step_ms: 6 * HOUR,
        from_ms: Some(common::T_START + 6 * HOUR),
        to_ms: Some(common::T_START + 3 * DAY),
    };
    let a = run(&mut Engine::new(registry()), inputs.clone(), grid).unwrap();
    let mut reversed = inputs;
    reversed.reverse();
    let b = run(&mut Engine::new(registry()), reversed, grid).unwrap();
    assert_eq!(a, b);
    let recs: Vec<Record> = a.records.into_iter().map(Record::from).collect();
    assert_eq!(to_jsonl(&recs).lines().count(), recs.len());
    // The index outlier on day 3 is dropped from the composite.
    let text = std::fs::read_to_string(fixture_dir().join("inputs.jsonl")).unwrap();
    let inputs = synthetic::batch::read_records(text.as_bytes()).unwrap();
    let t = common::T_START + 60 * HOUR + 30 * 60_000;
    let out = value_at(&inputs, t);
    let m = months_of(&out, "us-large-cap-index")[0];
    assert!(
        m.quality.contains(&SyntheticQuality::SpotDispersed),
        "{:?}",
        m.quality
    );
    assert_eq!(m.spot_sources, 2);
}

#[test]
fn cli_reproduces_the_committed_output() {
    let dir = fixture_dir();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_synthetic"))
        .arg("--registry")
        .arg(dir.join("registry.json"))
        .arg("--input")
        .arg(dir.join("inputs.jsonl"))
        .args(["--step-secs", "86400"])
        .args(["--from-ms", &(common::T_START + 20 * HOUR).to_string()])
        .args(["--to-ms", &common::T_END.to_string()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let want = std::fs::read_to_string(dir.join("expected.jsonl")).unwrap();
    assert!(String::from_utf8(out.stdout).unwrap() == want);
}
