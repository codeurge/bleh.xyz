//! Revisions and corrections: a changed value for a date already emitted
//! becomes the next revision, an unchanged one is not emitted again, and
//! revision state survives a restart.
#![allow(clippy::unwrap_used)]

mod common;

use common::{fixture, live, records_with, zip_of};
use mesh_types::Record;
use reference_pollers::revision::RevisionStore;
use reference_pollers::source::SourceId;
use reference_pollers::sources::{dairy, ercot, frb, lmr, nyfed};
use reference_pollers::time::Date;

fn revision(r: &Record) -> u32 {
    match r {
        Record::ReferenceValue(v) => v.revision,
        Record::Positioning(p) => p.revision,
        _ => u32::MAX,
    }
}

#[test]
fn nyfed_same_day_revision() {
    let mut store = RevisionStore::memory();
    let first = records_with(
        SourceId::NyFed,
        nyfed::parse_rates(&fixture("nyfed/secured_all.json")).unwrap(),
        live(),
        &mut store,
    );
    assert!(first.iter().all(|r| revision(r) == 0));
    // Re-reading the same data emits nothing.
    let again = records_with(
        SourceId::NyFed,
        nyfed::parse_rates(&fixture("nyfed/secured_all.json")).unwrap(),
        live(),
        &mut store,
    );
    assert!(again.is_empty());
    // The 2:30 p.m. revision changes one SOFR value.
    let revised = records_with(
        SourceId::NyFed,
        nyfed::parse_rates(&fixture("nyfed/secured_all_revised.json")).unwrap(),
        live(),
        &mut store,
    );
    assert_eq!(revised.len(), 1);
    let Record::ReferenceValue(v) = &revised[0] else {
        unreachable!()
    };
    assert_eq!(
        (
            v.prov.instrument.symbol(),
            v.as_of_date.as_str(),
            v.revision
        ),
        ("SOFR", "2026-03-16", 1)
    );
    let orig = first
        .iter()
        .find(|r| {
            r.prov().instrument.symbol() == "SOFR"
                && matches!(r, Record::ReferenceValue(x) if x.as_of_date == "2026-03-16")
        })
        .unwrap();
    // The revision sorts after the original and never shares its seq.
    assert_eq!(v.prov.seq.block_number, orig.prov().seq.block_number + 1);
}

#[test]
fn h15_revision() {
    let mut store = RevisionStore::memory();
    let a = records_with(
        SourceId::FrbH15,
        frb::parse_release(frb::Release::H15, &fixture("frb/H15_data.xml")).unwrap(),
        live(),
        &mut store,
    );
    assert!(!a.is_empty());
    let b = records_with(
        SourceId::FrbH15,
        frb::parse_release(frb::Release::H15, &fixture("frb/H15_data_revised.xml")).unwrap(),
        live(),
        &mut store,
    );
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].prov().instrument.symbol(), "RIFSPFF_N.B");
    assert_eq!(revision(&b[0]), 1);
}

#[test]
fn lmr_correction() {
    let mut store = RevisionStore::memory();
    records_with(
        SourceId::UsdaLmr,
        lmr::parse_swine(&fixture("lmr/2511.json")).unwrap(),
        live(),
        &mut store,
    );
    let c = records_with(
        SourceId::UsdaLmr,
        lmr::parse_swine(&fixture("lmr/2511_corrected.json")).unwrap(),
        live(),
        &mut store,
    );
    assert_eq!(c.len(), 1);
    assert_eq!(
        c[0].prov().instrument.symbol(),
        "LM_HG201.NEGOTIATED.base_price"
    );
    assert_eq!(revision(&c[0]), 1);
    assert_eq!(common::value_of(&c[0]), "90.35");
}

#[test]
fn ndpsr_four_week_revision_via_final_section() {
    let mut store = RevisionStore::memory();
    let w1 = records_with(
        SourceId::UsdaDairy,
        dairy::parse_ndpsr(&fixture("dairy/2993_week1.json")).unwrap(),
        live(),
        &mut store,
    );
    assert_eq!(w1.len(), 1);
    let w2 = records_with(
        SourceId::UsdaDairy,
        dairy::parse_ndpsr(&fixture("dairy/2993.json")).unwrap(),
        live(),
        &mut store,
    );
    let butter_0228: Vec<&Record> = w2
        .iter()
        .filter(|r| matches!(r, Record::ReferenceValue(v) if v.as_of_date == "2026-02-28"))
        .collect();
    assert_eq!(butter_0228.len(), 1);
    assert_eq!(revision(butter_0228[0]), 1);
    assert_eq!(common::value_of(butter_0228[0]), "2.5401");
    assert!(common::flagged(butter_0228[0]));
}

#[test]
fn ercot_price_correction() {
    let mut store = RevisionStore::memory();
    let a = records_with(
        SourceId::Ercot,
        ercot::parse_document(12331, &zip_of(&[("a.csv", &fixture("ercot/dam_spp.csv"))])).unwrap(),
        live(),
        &mut store,
    );
    assert_eq!(a.len(), 48);
    let b = records_with(
        SourceId::Ercot,
        ercot::parse_document(12331, &fixture("ercot/dam_spp_corrected.csv")).unwrap(),
        live(),
        &mut store,
    );
    assert_eq!(b.len(), 1);
    assert_eq!(revision(&b[0]), 1);
    assert_eq!(common::value_of(&b[0]), "31.25");
}

#[test]
fn revision_state_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let today = Date::new(2026, 3, 16).unwrap();
    {
        let mut store = RevisionStore::open(dir.path(), 120, today).unwrap();
        records_with(
            SourceId::NyFed,
            nyfed::parse_rates(&fixture("nyfed/secured_all.json")).unwrap(),
            live(),
            &mut store,
        );
        store.flush(today).unwrap();
    }
    let mut store = RevisionStore::open(dir.path(), 120, today).unwrap();
    let again = records_with(
        SourceId::NyFed,
        nyfed::parse_rates(&fixture("nyfed/secured_all.json")).unwrap(),
        live(),
        &mut store,
    );
    assert!(again.is_empty());
    let revised = records_with(
        SourceId::NyFed,
        nyfed::parse_rates(&fixture("nyfed/secured_all_revised.json")).unwrap(),
        live(),
        &mut store,
    );
    assert_eq!(revised.len(), 1);
}
