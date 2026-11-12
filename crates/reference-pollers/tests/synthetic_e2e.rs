//! End to end on synthetic fixtures: Treasury and New York Fed files are
//! parsed into `reference_value` records exactly as the backfill emits
//! them, and the synthetic-month engine, with the reference-poller curve
//! preset, values the short-rate months from them.
#![allow(clippy::unwrap_used)]

mod common;

use common::{fixture, records};
use mesh_types::{Record, SyntheticFamily};
use reference_pollers::obs::Stamp;
use reference_pollers::source::SourceId;
use reference_pollers::sources::{nyfed, treasury};
use synthetic::batch::{Grid, run};
use synthetic::registry::CurveConfig;
use synthetic::{Engine, Registry};

fn rate_records() -> Vec<Record> {
    let mut v = records(
        SourceId::NyFed,
        nyfed::parse_rates(&fixture("nyfed/secured_all.json")).unwrap(),
        Stamp::Backfill,
    );
    for (feed, file) in [
        (
            "daily_treasury_yield_curve",
            "treasury/daily_treasury_yield_curve_2026.xml",
        ),
        (
            "daily_treasury_bill_rates",
            "treasury/daily_treasury_bill_rates_2026.xml",
        ),
    ] {
        v.extend(records(
            SourceId::TreasuryRates,
            treasury::parse_feed(feed, &fixture(file)).unwrap(),
            Stamp::Backfill,
        ));
    }
    v
}

fn registry() -> Registry {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/synthetic/synthetic-months/registry.json"
    );
    let mut r = Registry::load(std::path::Path::new(path))
        .unwrap()
        .with_curve(CurveConfig::reference_pollers())
        .unwrap();
    // Short-rate months need rates only; the registry's synthetic spot
    // entries are left as they are and simply not valued here.
    r.exposures
        .retain(|e| e.family == SyntheticFamily::ShortRate);
    r.check().unwrap();
    r
}

fn grid() -> Grid {
    // Daily at 20:00 UTC, 2026-03-10 to 2026-03-16.
    Grid {
        step_ms: 86_400_000,
        from_ms: Some(1_773_172_800_000),
        to_ms: Some(1_773_691_200_000),
    }
}

#[test]
fn rates_from_pollers_value_short_rate_months() {
    let inputs = rate_records();
    let mut engine = Engine::new(registry());
    assert!(
        inputs.iter().any(|r| engine.wants(r)),
        "the preset reads the pollers' instruments"
    );
    let out = run(&mut engine, inputs.clone(), grid()).unwrap();
    assert!(!out.records.is_empty(), "{:?}", out.notices);
    for m in &out.records {
        m.validate_like_record();
        let adms: Vec<&str> = m.admissions.iter().map(|a| a.as_str()).collect();
        assert!(
            adms.contains(&"ADM-0006") && adms.contains(&"ADM-0007"),
            "{adms:?}"
        );
        let fr: f64 = m.financing_rate.to_string().parse().unwrap();
        assert!((3.0..5.0).contains(&fr), "financing rate {fr}");
        let fv: f64 = m.fair_value.to_string().parse().unwrap();
        assert!((94.0..97.0).contains(&fv), "fair value {fv}");
        // Backfilled rates carry estimated publication times: no input is
        // newer than the valuation.
        assert!(m.rates_ms.0 <= m.as_of_ms.0);
        assert!(m.prov.flags.contains(mesh_types::Flag::Backfill));
    }
    let months: std::collections::BTreeSet<_> = out
        .records
        .iter()
        .map(|m| (m.exposure.clone(), m.month.clone()))
        .collect();
    assert!(months.contains(&("sofr-average-month".to_owned(), "2026-03".to_owned())));
    assert!(months.contains(&("sofr-compounded-quarter".to_owned(), "2026-03".to_owned())));

    // Deterministic: the same inputs give the same records.
    let mut engine2 = Engine::new(registry());
    let again = run(&mut engine2, inputs, grid()).unwrap();
    assert_eq!(again.records, out.records);
}

#[test]
fn a_revised_fixing_moves_the_month() {
    let base = rate_records();
    let mut revised = base.clone();
    let mut store = reference_pollers::revision::RevisionStore::memory();
    // Seed the store with the originals, then build the revision.
    let _ = common::records_with(
        SourceId::NyFed,
        nyfed::parse_rates(&fixture("nyfed/secured_all.json")).unwrap(),
        Stamp::Backfill,
        &mut store,
    );
    let rev = common::records_with(
        SourceId::NyFed,
        nyfed::parse_rates(&fixture("nyfed/secured_all_revised.json")).unwrap(),
        Stamp::Backfill,
        &mut store,
    );
    assert_eq!(rev.len(), 1);
    revised.extend(rev);
    // Value the day after the revised fixing is published.
    let g = Grid {
        step_ms: 86_400_000,
        from_ms: Some(1_773_777_600_000),
        to_ms: Some(1_773_777_600_000),
    };
    let a = run(&mut Engine::new(registry()), base, g).unwrap();
    let b = run(&mut Engine::new(registry()), revised, g).unwrap();
    let pick = |o: &synthetic::batch::Output| {
        o.records
            .iter()
            .find(|m| m.exposure == "sofr-average-month" && m.month == "2026-03")
            .map(|m| m.fair_value.to_string())
            .unwrap()
    };
    assert_ne!(
        pick(&a),
        pick(&b),
        "the higher revision replaces the original fixing"
    );
}

trait ValidateLike {
    fn validate_like_record(&self);
}

impl ValidateLike for mesh_types::SyntheticMonth {
    fn validate_like_record(&self) {
        let r = Record::SyntheticMonth(self.clone());
        r.validate().unwrap();
        mesh_types::allowlist::check_record(&r).unwrap();
    }
}
