//! Parsing of every documented format on synthetic fixtures: values,
//! conventions, malformed and edge rows, excluded fields (one test per
//! source) and the `exchange_referenced` flag.
#![allow(clippy::unwrap_used)]

mod common;

use common::{fixture, flagged, live, records, symbols, value_of, zip_of};
use mesh_types::{Compounding, DayCount, Record, ReferenceMeasure};
use reference_pollers::obs::Stamp;
use reference_pollers::source::SourceId;
use reference_pollers::sources::cot::{CotReader, Form};
use reference_pollers::sources::{Bases, Reader, dairy, ercot, feeder, frb, lmr, nyfed, treasury};
use reference_pollers::xlsx;

fn rv(r: &Record) -> &mesh_types::ReferenceValue {
    match r {
        Record::ReferenceValue(v) => v,
        other => panic!("not a reference value: {other:?}"),
    }
}

fn find<'a>(recs: &'a [Record], symbol: &str, as_of: &str) -> Vec<&'a Record> {
    recs.iter()
        .filter(|r| r.prov().instrument.symbol() == symbol)
        .filter(|r| match r {
            Record::ReferenceValue(v) => v.as_of_date == as_of,
            Record::Positioning(p) => p.as_of_date == as_of,
            _ => false,
        })
        .collect()
}

// ----- New York Fed ------------------------------------------------------

#[test]
fn nyfed_rates_parse_with_conventions() {
    let p = nyfed::parse_rates(&fixture("nyfed/secured_all.json")).unwrap();
    assert!(p.unknown.contains("exampleUndocumentedKey"));
    let recs = records(SourceId::NyFed, p, live());
    let sofr = find(&recs, "SOFR", "2026-03-16");
    assert_eq!(sofr.len(), 1);
    let v = rv(sofr[0]);
    assert_eq!(v.measure, ReferenceMeasure::Rate);
    assert_eq!(v.tenor.as_deref(), Some("ON"));
    assert_eq!(v.compounding, Some(Compounding::Simple));
    assert_eq!(v.day_count, Some(DayCount::Act360));
    assert!(!flagged(sofr[0]));
    for s in [
        "SOFR.P1", "SOFR.P25", "SOFR.P75", "SOFR.P99", "BGCR", "TGCR.P99",
    ] {
        assert_eq!(find(&recs, s, "2026-03-16").len(), 1, "{s}");
    }
    // seq: block = midnight seconds * 100 + revision, index = slot.
    let block = 1_773_619_200u64 * 100;
    assert_eq!(sofr[0].prov().seq.block_number, block);

    let effr = records(
        SourceId::NyFed,
        nyfed::parse_rates(&fixture("nyfed/effr.json")).unwrap(),
        live(),
    );
    assert_eq!(
        value_of(find(&effr, "EFFR.TARGET_TO", "2026-03-16")[0]),
        "4.5"
    );
    let sofrai = records(
        SourceId::NyFed,
        nyfed::parse_rates(&fixture("nyfed/sofrai.json")).unwrap(),
        live(),
    );
    let idx = rv(find(&sofrai, "SOFR.INDEX", "2026-03-16")[0]);
    assert_eq!(idx.measure, ReferenceMeasure::IndexLevel);
    assert_eq!(idx.compounding, None);
    assert_eq!(
        rv(find(&sofrai, "SOFR.AVG180", "2026-03-16")[0])
            .tenor
            .as_deref(),
        Some("180D")
    );
}

#[test]
fn nyfed_malformed_rows_are_skipped_and_volumes_never_emitted() {
    let p = nyfed::parse_rates(&fixture("nyfed/obfr.json")).unwrap();
    assert_eq!(p.skipped.get("malformed"), Some(&1));
    assert!(p.skipped.get("unsupported_volume").copied().unwrap_or(0) > 0);
    let recs = records(SourceId::NyFed, p, live());
    for r in &recs {
        let s = r.prov().instrument.symbol().to_ascii_lowercase();
        assert!(!s.contains("volume") && !s.contains("undocumented"), "{s}");
    }
    assert!(nyfed::parse_rates(b"{\"other\":[]}").is_err());
    assert!(nyfed::parse_rates(b"not json").is_err());
}

// ----- Treasury ----------------------------------------------------------

#[test]
fn treasury_par_curve_tenors_and_edge_rows() {
    let p = treasury::parse_feed(
        "daily_treasury_yield_curve",
        &fixture("treasury/daily_treasury_yield_curve_2026.xml"),
    )
    .unwrap();
    // A new tenor is read and reported.
    assert!(p.unknown.contains("BC_9MONTH"));
    let recs = records(SourceId::TreasuryRates, p, live());
    let day = |d| -> Vec<String> {
        find(&recs, "par_yield_curve", d)
            .iter()
            .map(|r| rv(r).tenor.clone().unwrap())
            .collect()
    };
    let last = day("2026-03-16");
    assert_eq!(last.len(), 15, "14 tenors and the new 9M");
    assert!(
        last.contains(&"6W".to_owned())
            && last.contains(&"9M".to_owned())
            && last.contains(&"30Y".to_owned())
    );
    // The first date has no 1.5-month element: no value, no record.
    assert!(!day("2026-03-02").contains(&"6W".to_owned()));
    let v = rv(find(&recs, "par_yield_curve", "2026-03-02")[0]);
    assert_eq!(v.measure, ReferenceMeasure::ParYield);
    assert_eq!(v.compounding, Some(Compounding::SemiAnnual));
    // Tenors on one date get distinct slots (event indexes).
    let mut idx: Vec<u32> = find(&recs, "par_yield_curve", "2026-03-16")
        .iter()
        .map(|r| r.prov().seq.event_index)
        .collect();
    idx.sort_unstable();
    idx.dedup();
    assert_eq!(idx.len(), 15);
}

#[test]
fn treasury_excluded_fields_are_never_emitted() {
    let p = treasury::parse_feed(
        "daily_treasury_bill_rates",
        &fixture("treasury/daily_treasury_bill_rates_2026.xml"),
    )
    .unwrap();
    // CS_* averages are not in ADM-0006's field list: dropped as excluded.
    assert_eq!(p.skipped.get("excluded"), Some(&(14 * 11)));
    assert_eq!(p.skipped.get("malformed"), Some(&1));
    let recs = records(SourceId::TreasuryRates, p, live());
    assert_eq!(
        symbols(&recs),
        vec!["bill_rates_coupon_equivalent", "bill_rates_discount"]
    );
    let disc = find(&recs, "bill_rates_discount", "2026-03-16");
    assert_eq!(disc.len(), 7);
    let v = rv(disc[0]);
    assert_eq!(v.compounding, Some(Compounding::Discount));
    assert!(v.maturity_date.is_some());
    // The malformed 4-week discount rate of the first date is skipped only.
    assert_eq!(find(&recs, "bill_rates_discount", "2026-03-02").len(), 6);
    let ce52 = find(&recs, "bill_rates_coupon_equivalent", "2026-03-16")
        .into_iter()
        .find(|r| rv(r).tenor.as_deref() == Some("52W"))
        .unwrap();
    assert_eq!(rv(ce52).compounding, Some(Compounding::SemiAnnual));
    // The display duplicate, composites and extrapolation factors never
    // appear.
    let lt = treasury::parse_feed(
        "daily_treasury_long_term_rate",
        &fixture("treasury/daily_treasury_long_term_rate_2026.xml"),
    )
    .unwrap();
    assert!(lt.skipped.get("unsupported_no_tenor").copied().unwrap_or(0) > 0);
    let lt = records(SourceId::TreasuryRates, lt, live());
    assert_eq!(symbols(&lt), vec!["cmt_20y"]);
    let real = records(
        SourceId::TreasuryRates,
        treasury::parse_feed(
            "daily_treasury_real_yield_curve",
            &fixture("treasury/daily_treasury_real_yield_curve_2026.xml"),
        )
        .unwrap(),
        live(),
    );
    assert_eq!(find(&real, "real_par_yield_curve", "2026-03-16").len(), 5);
    for r in recs.iter().chain(&lt).chain(&real) {
        assert!(!flagged(r));
        assert!(!r.prov().instrument.as_str().contains("CS_"));
    }
    assert!(treasury::parse_feed("daily_treasury_yield_curve", b"<feed><entry></feed>").is_err());
}

#[test]
fn treasury_auctions_pages_and_exclusions() {
    let reader = treasury::AuctionsReader::new(&Bases::default());
    let t = &reader.backfill_targets(2026)[0];
    let p1 = reader
        .parse(t, &fixture("fiscaldata/auctions_page1.json"))
        .unwrap();
    assert_eq!(p1.follow.len(), 1, "links.next is followed");
    assert!(p1.follow[0].url.contains("page%5Bnumber%5D=2"));
    assert!(p1.skipped.get("unsupported_amount").copied().unwrap_or(0) > 0);
    let p2 = reader
        .parse(&p1.follow[0], &fixture("fiscaldata/auctions_page2.json"))
        .unwrap();
    assert!(p2.follow.is_empty());
    assert_eq!(p2.skipped.get("malformed"), Some(&1));
    let mut recs = records(SourceId::TreasuryAuctions, p1, live());
    recs.extend(records(SourceId::TreasuryAuctions, p2, live()));
    let syms = symbols(&recs);
    assert!(syms.contains(&"auction.BILL.13_WEEK.high_discnt_rate".to_owned()));
    assert!(syms.contains(&"auction.TIPS.10_YEAR.high_yield".to_owned()));
    assert!(syms.contains(&"auction.NOTE.10_YEAR.int_rate".to_owned()));
    let inv52 = recs
        .iter()
        .find(|r| r.prov().instrument.symbol() == "auction.BILL.52_WEEK.high_investment_rate")
        .unwrap();
    assert_eq!(rv(inv52).compounding, Some(Compounding::SemiAnnual));
    let price = recs
        .iter()
        .find(|r| r.prov().instrument.symbol() == "auction.BILL.13_WEEK.high_price")
        .unwrap();
    assert_eq!(rv(price).currency.as_deref(), Some("USD"));
    assert_eq!(rv(price).maturity_date.as_deref(), Some("2026-06-11"));
    for s in &syms {
        for banned in [
            "offering",
            "tender",
            "accepted",
            "bid_to_cover",
            "soma",
            "pdf",
            "xml",
            "cusip",
        ] {
            assert!(!s.to_ascii_lowercase().contains(banned), "{s}");
        }
    }
}

// ----- Board H.15 and H.10 ------------------------------------------------

#[test]
fn h15_allowlist_and_exclusions() {
    let xml = fixture("frb/H15_data.xml");
    let zipped = zip_of(&[("H15_struct.xml", b"<x/>"), ("H15_data.xml", &xml)]);
    let p = frb::parse_release(frb::Release::H15, &zipped).unwrap();
    assert_eq!(p.skipped.get("excluded"), Some(&4));
    assert_eq!(p.skipped.get("unsupported_no_tenor"), Some(&1));
    assert_eq!(p.skipped.get("other_frequency"), Some(&1));
    assert_eq!(p.skipped.get("missing"), Some(&1), "ND is no data");
    assert!(p.unknown.contains("RIFNEWSERIES_N"));
    let recs = records(SourceId::FrbH15, p, live());
    assert_eq!(
        symbols(&recs),
        vec![
            "RIFLGFCY05_XII_N.B",
            "RIFLGFCY10_N.B",
            "RIFSGFSW04_N.B",
            "RIFSPFF_N.B",
            "RIFSPPNAAD30_N.B",
            "RIFSRP_F02_N.B"
        ]
    );
    for r in &recs {
        let s = r.prov().instrument.symbol();
        for banned in [
            "RIFLDIY",
            "RILSPDEP",
            "RIMLPAAAR",
            "RIFSPPCM",
            "RIFSPBLP",
            "RIFNEWSERIES",
        ] {
            assert!(!s.contains(banned), "{s}");
        }
    }
    let cmt = rv(find(&recs, "RIFLGFCY10_N.B", "2026-03-09")[0]);
    assert_eq!(
        (cmt.measure, cmt.tenor.as_deref()),
        (ReferenceMeasure::ParYield, Some("10Y"))
    );
    let bill = rv(find(&recs, "RIFSGFSW04_N.B", "2026-03-09")[0]);
    assert_eq!(bill.compounding, Some(Compounding::Discount));
    let cp = rv(find(&recs, "RIFSPPNAAD30_N.B", "2026-03-09")[0]);
    assert_eq!(cp.tenor.as_deref(), Some("30D"));
}

#[test]
fn h10_allowlist_and_exclusions() {
    let p = frb::parse_release(frb::Release::H10, &fixture("frb/H10_data.xml")).unwrap();
    assert_eq!(p.skipped.get("excluded"), Some(&1), "dollar index");
    assert!(p.unknown.contains("RXI_N.B.ZZ"));
    let recs = records(SourceId::FrbH10, p, live());
    assert_eq!(
        symbols(&recs),
        vec!["RXI@US_N.B.EU", "RXI_N.B.CA", "RXI_N.B.JA"]
    );
    assert_eq!(
        rv(find(&recs, "RXI@US_N.B.EU", "2026-03-09")[0])
            .currency
            .as_deref(),
        Some("USD")
    );
    let jpy = rv(find(&recs, "RXI_N.B.JA", "2026-03-09")[0]);
    assert_eq!(
        (jpy.measure, jpy.currency.as_deref()),
        (ReferenceMeasure::Price, Some("JPY"))
    );
    assert!(
        recs.iter()
            .all(|r| !r.prov().instrument.symbol().starts_with("JRX"))
    );
}

// ----- COT -----------------------------------------------------------------

#[test]
fn cot_annual_weekly_and_flags() {
    let reader = CotReader::new(&Bases::default());
    // Weekly files have no header: refused until the annual file is read.
    assert!(
        reader
            .parse_weekly(Form::LegacyFut, &fixture("cot/deafut_weekly.txt"))
            .is_err()
    );
    let annual = zip_of(&[("annual.txt", &fixture("cot/legacy_fut_2025.txt"))]);
    let p = reader.parse_annual(Form::LegacyFut, &annual).unwrap();
    assert_eq!(p.skipped.get("malformed"), Some(&1), "one bad count");
    let recs = records(SourceId::Cot, p, Stamp::Backfill);
    // 6 rows x 4 categories, plus 3 categories of the row with a bad cell.
    assert_eq!(recs.len(), 27);
    let Record::Positioning(nc) = find(&recs, "000001", "2025-12-30")
        .into_iter()
        .find(|r| matches!(r, Record::Positioning(p) if p.category == mesh_types::PositioningCategory::Noncommercial))
        .unwrap()
    else {
        unreachable!()
    };
    assert_eq!(
        (nc.long, nc.short, nc.spreading, nc.open_interest),
        (3000, 1500, Some(300), 10000)
    );
    assert!(!nc.combined && !nc.prov.flags.contains(mesh_types::Flag::ExchangeReferenced));
    assert!(nc.prov.flags.contains(mesh_types::Flag::Backfill));
    assert_eq!(symbols(&recs), vec!["000001", "000003", "00000B"]);
    // Weekly now parses with the annual header; a different width fails.
    let w = reader
        .parse_weekly(Form::LegacyFut, &fixture("cot/deafut_weekly.txt"))
        .unwrap();
    assert_eq!(w.obs.len(), 4);
    assert!(
        reader
            .parse_weekly(Form::LegacyFut, b"\"A\",\"260113\",\"2026-01-13\"\n")
            .is_err()
    );

    // Combined forms carry exchange_referenced, always.
    for (form, file) in [
        (Form::LegacyCom, "cot/legacy_com_2026.txt"),
        (Form::DisaggCom, "cot/disagg_com_2026.txt"),
        (Form::Cit, "cot/cit_2026.txt"),
    ] {
        let p = reader.parse_annual(form, &fixture(file)).unwrap();
        let recs = records(SourceId::Cot, p, live());
        assert!(!recs.is_empty(), "{form:?}");
        assert!(recs.iter().all(flagged), "{form:?}");
    }
    let dis = records(
        SourceId::Cot,
        reader
            .parse_annual(Form::DisaggCom, &fixture("cot/disagg_com_2026.txt"))
            .unwrap(),
        live(),
    );
    let mm = dis
        .iter()
        .find_map(|r| match r {
            Record::Positioning(p)
                if p.category == mesh_types::PositioningCategory::ManagedMoney =>
            {
                Some(p)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(
        (mm.long, mm.short, mm.spreading),
        (5000, 3000, Some(800)),
        "doubled underscores normalize"
    );
}

#[test]
fn cot_emits_only_positioning_fields() {
    // COT has no excluded fields; the guard is that nothing beyond the
    // positioning kind's fields (no names, changes, percentages, trader
    // counts or concentration ratios) reaches a record.
    let reader = CotReader::new(&Bases::default());
    let recs = records(
        SourceId::Cot,
        reader
            .parse_annual(Form::LegacyFut, &fixture("cot/legacy_fut_2025.txt"))
            .unwrap(),
        live(),
    );
    for r in &recs {
        let v = serde_json::to_value(r).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            vec![
                "as_of_date",
                "category",
                "combined",
                "contract_market_code",
                "kind",
                "long",
                "open_interest",
                "prov",
                "report",
                "revision",
                "short",
                "spreading"
            ]
        );
        assert!(
            !v.to_string().contains("SYNTHETIC COMMODITY"),
            "market names stay source metadata"
        );
    }
    // A header that lacks a category column is refused, not guessed.
    assert!(
        reader
            .parse_annual(Form::TffFut, &fixture("cot/legacy_fut_2025.txt"))
            .is_err()
    );
}

// ----- USDA LMR --------------------------------------------------------------

#[test]
fn lmr_swine_purchase_types_and_flags() {
    let p = lmr::parse_swine(&fixture("lmr/2511.json")).unwrap();
    assert_eq!(
        p.skipped.get("excluded"),
        Some(&6),
        "five excluded types and one unknown"
    );
    assert_eq!(
        p.skipped.get("out_of_scope"),
        Some(&1),
        "Carcass Measurements"
    );
    assert!(
        p.unknown
            .contains("purchase_type:Prod. Sold Example New Type")
    );
    let recs = records(SourceId::UsdaLmr, p, live());
    assert_eq!(recs.len(), 16);
    for r in &recs {
        let s = r.prov().instrument.symbol();
        assert_eq!(flagged(r), !s.starts_with("LM_HG201.NEGOTIATED."), "{s}");
        for banned in ["OTHER", "PACKER", "ALL", "wtd_avg", "EXAMPLE"] {
            assert!(!s.contains(banned), "{s}");
        }
    }
    let base = rv(find(&recs, "LM_HG201.NEGOTIATED.base_price", "2026-03-10")[0]);
    assert_eq!(
        base.value.to_string(),
        "90.10",
        "carried exactly as published"
    );
    // Published 08:30 Central (13:30 UTC in daylight time).
    assert_eq!(
        find(&recs, "LM_HG201.NEGOTIATED.base_price", "2026-03-10")[0]
            .prov()
            .ts_source_ms
            .0,
        1_773_149_400_000
    );
    assert!(
        lmr::parse_swine(b"\"No Results Found.\"")
            .unwrap()
            .obs
            .is_empty()
    );
}

#[test]
fn lmr_cattle_negotiated_detail() {
    let p = lmr::parse_cattle(&fixture("lmr/2466.json")).unwrap();
    assert_eq!(p.skipped.get("out_of_scope"), Some(&1));
    let recs = records(SourceId::UsdaLmr, p, live());
    assert_eq!(recs.len(), 6);
    assert!(recs.iter().all(|r| !flagged(r)));
    assert!(symbols(&recs).contains(
        &"LM_CT100.STEER.LIVE_FOB.OVER_80_CHOICE.1350_1600.weighted_avg_price".to_owned()
    ));
}

// ----- USDA dairy -------------------------------------------------------------

#[test]
fn dairy_prices_flagged_and_exchange_quotes_excluded() {
    let p = dairy::parse_ndpsr(&fixture("dairy/2993.json")).unwrap();
    assert_eq!(p.skipped.get("excluded"), Some(&1), "the CME-named field");
    assert!(p.skipped.get("unsupported_volume").copied().unwrap_or(0) >= 6);
    // The Final section comes last, after the initial values.
    assert!(
        matches!(p.obs.last(), Some(reference_pollers::obs::Obs::Ref(r)) if r.as_of.to_string() == "2026-02-28")
    );
    let recs = records(SourceId::UsdaDairy, p, live());
    assert_eq!(recs.len(), 7);
    assert!(
        recs.iter().all(flagged),
        "every NDPSR price is exchange_referenced"
    );
    for r in &recs {
        assert!(
            !r.prov()
                .instrument
                .symbol()
                .to_ascii_lowercase()
                .contains("cme")
        );
        assert!(!r.prov().instrument.symbol().contains("Sales"));
    }
    let c = dairy::parse_class(&fixture("dairy/2991.json")).unwrap();
    assert_eq!(c.skipped.get("excluded"), Some(&1));
    assert!(c.unknown.contains("example_undocumented_Price"));
    let crecs = records(SourceId::UsdaDairy, c, live());
    assert_eq!(crecs.len(), 18);
    assert!(crecs.iter().all(flagged));
    assert!(crecs.iter().all(|r| rv(r).as_of_date == "2026-02-01"));
    assert!(
        crecs
            .iter()
            .all(|r| !r.prov().instrument.symbol().contains("undocumented"))
    );
}

// ----- USDA feeder cattle -------------------------------------------------------

#[test]
fn feeder_rows_kept_narrative_and_basis_dropped() {
    let p = feeder::parse_summary(&fixture("feeder/sj_ls850.txt")).unwrap();
    assert_eq!(
        p.skipped.get("excluded"),
        Some(&2),
        "basis and futures rows"
    );
    let recs = records(SourceId::UsdaFeeder, p, live());
    assert_eq!(recs.len(), 12);
    let syms = symbols(&recs);
    assert!(syms.contains(&"LS850.STEERS.MEDIUM_AND_LARGE_1.500_600.avg_price".to_owned()));
    assert!(syms.contains(&"LS850.STEERS.MEDIUM_AND_LARGE_1.600_700.FLESHY.price_low".to_owned()));
    assert!(syms.contains(&"LS850.HEIFERS.MEDIUM_AND_LARGE_1_2.400_500.price_high".to_owned()));
    for s in &syms {
        assert!(
            !s.contains("BASIS") && !s.contains("FUTURES") && !s.contains("700_800"),
            "{s}"
        );
    }
    assert!(
        recs.iter()
            .all(|r| rv(r).as_of_date == "2026-03-06" && !flagged(r))
    );
    assert!(feeder::parse_summary(b"no date here\n").is_err());
}

// ----- ERCOT ----------------------------------------------------------------------

#[test]
fn ercot_listing_csv_and_dst() {
    let reader = ercot::ErcotReader::new(&Bases::default());
    let listing = &reader.live_targets(reference_pollers::time::Date::new(2026, 3, 10).unwrap())[0];
    assert!(
        listing
            .url
            .contains("www.ercot.com/misapp/GetReports.do?reportTypeId=12331")
    );
    assert!(!listing.url.contains("api.ercot.com"));
    let p = reader
        .parse(listing, &fixture("ercot/listing_12331.html"))
        .unwrap();
    assert_eq!(p.follow.len(), 1, "XML duplicate skipped");
    let doc = &p.follow[0];
    assert!(doc.once && doc.url.contains("doclookupId=900001"));
    let zipped = zip_of(&[("cdr.DAMSPNP4190.csv", &fixture("ercot/dam_spp.csv"))]);
    let d = reader.parse(doc, &zipped).unwrap();
    assert_eq!(d.skipped.get("malformed"), Some(&1), "hour ending 25");
    let recs = records(SourceId::Ercot, d, live());
    assert_eq!(recs.len(), 48);
    let he1 = rv(find(&recs, "DAM_SPP.HB_NORTH", "2026-03-10")
        .into_iter()
        .min_by_key(|r| rv(r).period_start_ms)
        .unwrap());
    // HE01 on 2026-03-10 (CDT) starts 00:00 local = 05:00 UTC.
    let start = 1_773_100_800_000 + 5 * 3_600_000;
    assert_eq!(he1.period_start_ms.unwrap().0, start);
    assert_eq!(he1.period_end_ms.unwrap().0 - start, 3_600_000);
    assert_eq!(he1.currency.as_deref(), Some("USD"));

    // The repeated hour when daylight time ends is a distinct interval.
    let fall = records(
        SourceId::Ercot,
        ercot::parse_document(12331, &fixture("ercot/dam_spp_fallback.csv")).unwrap(),
        live(),
    );
    assert_eq!(fall.len(), 4);
    let mut starts: Vec<i64> = fall
        .iter()
        .map(|r| rv(r).period_start_ms.unwrap().0)
        .collect();
    starts.sort_unstable();
    assert!(
        starts.windows(2).all(|w| w[1] - w[0] == 3_600_000),
        "{starts:?}"
    );
    assert!(
        fall.iter().any(|r| value_of(r) == "-5.50"),
        "negative prices kept"
    );

    // 15-minute real-time prices; an unknown column is reported.
    let rt = ercot::parse_document(12301, &fixture("ercot/rt_spp.csv")).unwrap();
    assert!(rt.unknown.contains("12301:examplenewcolumn"));
    let rt = records(SourceId::Ercot, rt, live());
    assert_eq!(rt.len(), 8);
    assert!(rt.iter().all(|r| {
        let v = rv(r);
        v.period_end_ms.unwrap().0 - v.period_start_ms.unwrap().0 == 900_000
    }));
}

#[test]
fn ercot_historical_workbook() {
    let rows: Vec<Vec<String>> = [
        vec![
            "Delivery Date",
            "Hour Ending",
            "Repeated Hour Flag",
            "Settlement Point",
            "Settlement Point Price",
        ],
        vec!["01/02/2025", "01:00", "N", "HB_NORTH", "21.50"],
        vec!["01/02/2025", "02:00", "N", "HB_NORTH", "20.75"],
        vec!["01/02/2025", "01:00", "N", "LZ_WEST", "19.00"],
    ]
    .iter()
    .map(|r| r.iter().map(|c| (*c).to_owned()).collect())
    .collect();
    let wb = xlsx::write_minimal(&[rows]).unwrap();
    let doc = zip_of(&[("DAMLZHBSPP_2025.xlsx", &wb)]);
    let p = ercot::parse_document(13060, &doc).unwrap();
    assert!(p.unknown.is_empty(), "{:?}", p.unknown);
    let recs = records(SourceId::Ercot, p, Stamp::Backfill);
    assert_eq!(symbols(&recs), vec!["DAM_SPP.HB_NORTH", "DAM_SPP.LZ_WEST"]);
    assert!(
        recs.iter()
            .all(|r| r.prov().flags.contains(mesh_types::Flag::Backfill))
    );
    // Day-ahead backfill is dated the day before delivery, 19:30 UTC.
    assert_eq!(recs[0].prov().ts_source_ms.0, 1_735_759_800_000);
}

#[test]
fn every_source_reads_only_admitted_hosts() {
    let today = reference_pollers::time::Date::new(2026, 3, 10).unwrap();
    for id in SourceId::ALL {
        let r = reference_pollers::sources::reader(id, &Bases::default());
        let mut urls: Vec<String> = r.live_targets(today).into_iter().map(|t| t.url).collect();
        urls.extend(r.backfill_targets(2025).into_iter().map(|t| t.url));
        for u in urls {
            let host = u
                .trim_start_matches("https://")
                .split('/')
                .next()
                .unwrap()
                .to_owned();
            let allowed = match id {
                SourceId::Cot => host == "www.cftc.gov",
                SourceId::TreasuryRates => host == "home.treasury.gov",
                SourceId::TreasuryAuctions => host == "api.fiscaldata.treasury.gov",
                SourceId::NyFed => host == "markets.newyorkfed.org",
                SourceId::FrbH15 | SourceId::FrbH10 => host == "www.federalreserve.gov",
                SourceId::UsdaLmr | SourceId::UsdaDairy => host == "mpr.datamart.ams.usda.gov",
                SourceId::UsdaFeeder => host == "www.ams.usda.gov" && u.contains("/mnreports/"),
                SourceId::Ercot => host == "www.ercot.com",
                // Bulk files only: the APIs are for checks (ADM-0021,
                // ADM-0022), and only the four admitted BLS surveys.
                SourceId::Eia => host == "www.eia.gov" && u.contains("/opendata/bulk/"),
                SourceId::Bls => {
                    host == "download.bls.gov"
                        && ["cu", "cw", "su", "ap"]
                            .iter()
                            .any(|d| u.contains(&format!("/pub/time.series/{d}/{d}.data.")))
                }
            };
            assert!(allowed, "{id}: {u}");
            assert!(
                !u.contains("treasurydirect")
                    && !u.contains("api.ercot.com")
                    && !u.contains("marsapi")
                    && !u.contains("api.eia.gov")
                    && !u.contains("api.bls.gov"),
                "{u}"
            );
        }
    }
}
