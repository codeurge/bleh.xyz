//! EIA (ADM-0021) and BLS (ADM-0022) readers on synthetic fixtures
//! (fixtures/synthetic/reference/eia and bls, every value invented): the
//! documented bulk layouts, the allowlists, the excluded families and
//! surveys, the `exchange_referenced` flag, revisions, the area selection,
//! backfill units and the User-Agent contact. No test touches the network.
#![allow(clippy::unwrap_used)]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use clap::Parser;
use common::{fixture, flagged, live, records, records_with, symbols, target, value_of, zip_of};
use mesh_types::{Flag, Record, ReferenceMeasure};
use reference_pollers::backfill::{Fetch, Options, run_source};
use reference_pollers::config::Args;
use reference_pollers::metrics::Metrics;
use reference_pollers::obs::Stamp;
use reference_pollers::revision::RevisionStore;
use reference_pollers::source::SourceId;
use reference_pollers::sources::bls::{self, Areas, Survey};
use reference_pollers::sources::eia::{self, Dataset};
use reference_pollers::sources::{Bases, reader};
use reference_pollers::time::Date;

fn rv(r: &Record) -> &mesh_types::ReferenceValue {
    match r {
        Record::ReferenceValue(v) => v,
        other => panic!("not a reference value: {other:?}"),
    }
}

fn one<'a>(recs: &'a [Record], symbol: &str, as_of: &str) -> &'a Record {
    let v: Vec<&Record> = recs
        .iter()
        .filter(|r| r.prov().instrument.symbol() == symbol && rv(r).as_of_date == as_of)
        .collect();
    assert_eq!(v.len(), 1, "{symbol} {as_of}");
    v[0]
}

// ----- EIA ---------------------------------------------------------------

#[test]
fn eia_reads_only_admitted_prices_and_flags_monthly_surveys() {
    let p = eia::parse_bulk(Dataset::Pet, &fixture("eia/PET.txt"), eia::Scope::all()).unwrap();
    // Excluded families (wholesale heating oil, WTI spot, a spot series
    // credited to a vendor, NYMEX futures) and the allowlisted series that
    // now credits a vendor.
    assert_eq!(p.skipped.get("excluded"), Some(&5), "{:?}", p.skipped);
    assert_eq!(p.skipped.get("out_of_scope"), Some(&1));
    assert_eq!(p.skipped.get("unsupported_volume"), Some(&2));
    // A null and an NA value; a malformed value and unexpected units.
    assert_eq!(p.skipped.get("missing"), Some(&2));
    assert_eq!(p.skipped.get("malformed"), Some(&2));
    assert!(
        p.unknown
            .contains("vendor_credit:PET.EMM_EPMRR_PTE_NUS_DPG.W")
    );
    assert!(p.unknown.contains("units:PET.EMM_EPM0_PTE_NUS_DPG.W"));
    let recs = records(SourceId::Eia, p, live());
    assert_eq!(
        symbols(&recs),
        [
            "PET.EMA_EPMR_PTR_NUS_DPG.M",
            "PET.EMD_EPD2D_PTE_NUS_DPG.W",
            "PET.EMM_EPMR_PTE_NUS_DPG.W",
            "PET.F000000__3.M",
            "PET.W_EPD2F_PRS_NUS_DPG.W",
        ]
    );
    for r in &recs {
        let s = r.prov().instrument.symbol();
        assert!(!eia::is_excluded_family(s), "{s}");
        assert!(eia::family(s).is_some(), "{s}");
        assert_eq!(r.prov().instrument.as_str(), format!("eia:ref:{s}"));
        let v = rv(r);
        assert_eq!(v.measure, ReferenceMeasure::Price);
        assert_eq!(v.currency.as_deref(), Some("USD"));
        // Monthly survey prices carry exchange_referenced; weekly retail
        // and residential prices do not.
        assert_eq!(flagged(r), s.ends_with(".M"), "{s}");
    }
    let regular = one(&recs, "PET.EMM_EPMR_PTE_NUS_DPG.W", "2026-11-09");
    assert_eq!(value_of(regular), "3.111");
    // ts_source_ms is the series' last_updated (2026-11-10 09:52:01 -05:00).
    let updated = Date::new(2026, 11, 10).unwrap().start_ms() + (14 * 3600 + 52 * 60 + 1) * 1000;
    assert_eq!(regular.prov().ts_source_ms.0, updated);
    // Exact text of quoted values.
    assert_eq!(
        value_of(one(&recs, "PET.EMD_EPD2D_PTE_NUS_DPG.W", "2026-11-09")),
        "3.950"
    );
    let monthly = one(&recs, "PET.EMA_EPMR_PTR_NUS_DPG.M", "2026-09-01");
    assert_eq!(value_of(monthly), "2.801");
    assert_eq!(
        value_of(one(&recs, "PET.F000000__3.M", "2026-08-01")),
        "61.25"
    );
    // The gap is not filled.
    assert!(recs.iter().all(|r| !(rv(r).as_of_date == "2026-10-26"
        && r.prov().instrument.symbol() == "PET.EMM_EPMR_PTE_NUS_DPG.W")));
}

#[test]
fn eia_natural_gas_from_the_zip_through_the_reader() {
    let r = reader(SourceId::Eia, &Bases::default());
    let live_targets = r.live_targets(Date::new(2026, 11, 14).unwrap());
    assert_eq!(live_targets.len(), 2);
    assert!(live_targets[0].url.ends_with("/opendata/bulk/PET.zip"));
    let ng = live_targets
        .iter()
        .find(|t| t.url.ends_with("/opendata/bulk/NG.zip"))
        .unwrap();
    let body = zip_of(&[("NG.txt", &fixture("eia/NG.txt"))]);
    let p = r.parse(ng, &body).unwrap();
    // Henry Hub spot, and a petroleum series in the gas file.
    assert_eq!(p.skipped.get("excluded"), Some(&2), "{:?}", p.skipped);
    assert_eq!(p.skipped.get("out_of_scope"), Some(&1));
    assert_eq!(p.skipped.get("unsupported_volume"), Some(&1));
    assert_eq!(p.skipped.get("missing"), Some(&1));
    let recs = records(SourceId::Eia, p, live());
    assert_eq!(symbols(&recs), ["NG.N3010US3.M", "NG.N3050US3.M"]);
    assert_eq!(value_of(one(&recs, "NG.N3050US3.M", "2026-08-01")), "4.1");
    assert_eq!(value_of(one(&recs, "NG.N3010US3.M", "2026-07-01")), "15.01");
    assert!(recs.iter().all(flagged));
}

#[test]
fn eia_backfill_unit_keeps_its_year_with_estimated_publication() {
    let r = reader(SourceId::Eia, &Bases::default());
    let t = r
        .backfill_targets(1995)
        .into_iter()
        .find(|t| t.url.ends_with("PET.zip"))
        .unwrap();
    let p = r.parse(&t, &fixture("eia/PET.txt")).unwrap();
    let recs = records(SourceId::Eia, p, Stamp::Backfill);
    assert_eq!(recs.len(), 1);
    let v = rv(&recs[0]);
    assert_eq!(v.as_of_date, "1995-01-02");
    assert_eq!(value_of(&recs[0]), "1.111");
    assert!(recs[0].prov().flags.contains(Flag::Backfill));
    // Retail prices: the survey Monday, 22:00 UTC (never last_updated).
    assert_eq!(
        recs[0].prov().ts_source_ms.0,
        Date::new(1995, 1, 2).unwrap().start_ms() + 22 * 3_600_000
    );
}

#[test]
fn eia_rejects_files_without_series() {
    assert!(eia::parse_bulk(Dataset::Pet, b"", eia::Scope::all()).is_err());
    let categories = br#"{"category_id":"1","name":"x","childseries":[]}"#;
    assert!(eia::parse_bulk(Dataset::Pet, categories, eia::Scope::all()).is_err());
    let p = eia::parse_bulk(Dataset::Pet, b"not json\n", eia::Scope::all()).unwrap();
    assert!(p.unknown.contains("bulk_line_shape"));
    assert!(p.obs.is_empty());
    let r = reader(SourceId::Eia, &Bases::default());
    assert!(r.parse(&target("bulk:XX:live:1:2"), b"").is_err());
}

// ----- BLS ---------------------------------------------------------------

fn us() -> bls::Scope {
    bls::Scope {
        areas: Areas::parse(bls::DEFAULT_AREAS),
        ..bls::Scope::all()
    }
}

#[test]
fn bls_cpi_index_levels_with_exclusions() {
    let p = bls::parse_data(Survey::Cu, &fixture("bls/cu.data.0.Current"), &us()).unwrap();
    assert!(p.unknown.is_empty(), "{:?}", p.unknown);
    // A PPI series is not admitted.
    assert_eq!(p.skipped.get("excluded"), Some(&1), "{:?}", p.skipped);
    // The annual average (M13) and a semiannual period (S01).
    assert_eq!(p.skipped.get("other_frequency"), Some(&2));
    // The unpublished month (footnote X) is a gap.
    assert_eq!(p.skipped.get("missing"), Some(&1));
    // M14, a bad year, a bad seasonal code.
    assert_eq!(p.skipped.get("malformed"), Some(&3));
    // An area outside the default selection.
    assert_eq!(p.skipped.get("out_of_scope"), Some(&1));
    let recs = records(SourceId::Bls, p, live());
    assert_eq!(
        symbols(&recs),
        ["CUSR0000SA0", "CUUR0000SA0", "CUUR0000SA0L1E"]
    );
    let all_items = one(&recs, "CUUR0000SA0", "2026-09-01");
    assert_eq!(all_items.prov().instrument.as_str(), "bls:ref:CUUR0000SA0");
    let v = rv(all_items);
    assert_eq!(v.measure, ReferenceMeasure::IndexLevel);
    assert_eq!(v.currency, None);
    assert_eq!(value_of(all_items), "322.456");
    assert!(!flagged(all_items));
    assert_eq!(value_of(one(&recs, "CUUR0000SA0", "1996-12-01")), "158.600");

    let every = bls::parse_data(
        Survey::Cu,
        &fixture("bls/cu.data.0.Current"),
        &bls::Scope::all(),
    )
    .unwrap();
    assert!(!every.skipped.contains_key("out_of_scope"));
    let recs = records(SourceId::Bls, every, live());
    assert!(symbols(&recs).contains(&"CUURS49ASA0".to_owned()));
}

#[test]
fn bls_revisions_of_seasonal_factors_and_chained_cpi() {
    let mut store = RevisionStore::memory();
    let first = records_with(
        SourceId::Bls,
        bls::parse_data(Survey::Cu, &fixture("bls/cu.data.0.Current"), &us()).unwrap(),
        live(),
        &mut store,
    );
    assert!(first.iter().all(|r| rv(r).revision == 0));
    let revised = records_with(
        SourceId::Bls,
        bls::parse_data(Survey::Cu, &fixture("bls/cu.data.0.Current.revised"), &us()).unwrap(),
        live(),
        &mut store,
    );
    assert_eq!(revised.len(), 1);
    let v = rv(&revised[0]);
    assert_eq!(revised[0].prov().instrument.symbol(), "CUSR0000SA0");
    assert_eq!((v.as_of_date.as_str(), v.revision), ("2026-08-01", 1));
    assert_eq!(value_of(&revised[0]), "321.600");

    let mut store = RevisionStore::memory();
    let initial = records_with(
        SourceId::Bls,
        bls::parse_data(Survey::Su, &fixture("bls/su.data.0.Current"), &us()).unwrap(),
        live(),
        &mut store,
    );
    assert_eq!(initial.len(), 3);
    let interim = records_with(
        SourceId::Bls,
        bls::parse_data(Survey::Su, &fixture("bls/su.data.0.Current.revised"), &us()).unwrap(),
        live(),
        &mut store,
    );
    assert_eq!(interim.len(), 1);
    assert_eq!(rv(&interim[0]).revision, 1);
    assert_eq!(value_of(&interim[0]), "172.401");
}

#[test]
fn bls_average_prices_and_other_surveys() {
    let p = bls::parse_data(Survey::Ap, &fixture("bls/ap.data.0.Current"), &us()).unwrap();
    assert_eq!(p.skipped.get("missing"), Some(&1), "{:?}", p.skipped);
    assert_eq!(p.skipped.get("out_of_scope"), Some(&1));
    let recs = records(SourceId::Bls, p, live());
    assert_eq!(symbols(&recs), ["APU0000709112", "APU000074714"]);
    let milk = rv(one(&recs, "APU0000709112", "2026-09-01"));
    assert_eq!(milk.measure, ReferenceMeasure::Price);
    assert_eq!(milk.currency.as_deref(), Some("USD"));

    let cw = records(
        SourceId::Bls,
        bls::parse_data(Survey::Cw, &fixture("bls/cw.data.0.Current"), &us()).unwrap(),
        live(),
    );
    assert_eq!(symbols(&cw), ["CWSR0000SA0", "CWUR0000SA0"]);
    // A CPI-U file read as CPI-W: every series is another survey's.
    let wrong = bls::parse_data(Survey::Cw, &fixture("bls/cu.data.0.Current"), &us()).unwrap();
    assert!(wrong.obs.is_empty());
    // An undocumented column is reported; a missing one is an error.
    let odd = b"series_id\tyear\tperiod\tvalue\tfootnote_codes\textra\nCUUR0000SA0\t2026\tM09\t1.000\t\tx\n";
    let p = bls::parse_data(Survey::Cu, odd, &us()).unwrap();
    assert!(p.unknown.contains("column:extra"));
    assert_eq!(p.obs.len(), 1);
    assert!(bls::parse_data(Survey::Cu, b"series_id\tyear\tvalue\n", &us()).is_err());
    // Published files end lines with CRLF (the repository stores the
    // fixtures with LF).
    let crlf = b"series_id        \tyear\tperiod\t       value\tfootnote_codes\r\nCUUR0000SA0      \t2026\tM09\t     322.456\t\r\n";
    let p = bls::parse_data(Survey::Cu, crlf, &us()).unwrap();
    assert!(p.unknown.is_empty() && p.skipped.is_empty(), "{p:?}");
    assert_eq!(p.obs.len(), 1);
    // Space-separated, as the layout files describe.
    let spaced = b"series_id year period value footnote_codes\nCUUR0000SA0 2026 M09 1.000\n";
    assert_eq!(
        bls::parse_data(Survey::Cu, spaced, &us())
            .unwrap()
            .obs
            .len(),
        1
    );
}

#[test]
fn bls_targets_and_publication_estimate() {
    let r = reader(SourceId::Bls, &Bases::default());
    let live_targets = r.live_targets(Date::new(2026, 11, 14).unwrap());
    let urls: Vec<&str> = live_targets.iter().map(|t| t.url.as_str()).collect();
    assert_eq!(
        urls,
        [
            "https://download.bls.gov/pub/time.series/cu/cu.data.0.Current",
            "https://download.bls.gov/pub/time.series/cw/cw.data.0.Current",
            "https://download.bls.gov/pub/time.series/su/su.data.0.Current",
            "https://download.bls.gov/pub/time.series/ap/ap.data.0.Current",
        ]
    );
    let old: Vec<String> = r
        .backfill_targets(1990)
        .into_iter()
        .map(|t| t.url)
        .collect();
    assert!(old.iter().any(|u| u.ends_with("cu/cu.data.1.AllItems")));
    assert!(
        old.iter()
            .any(|u| u.ends_with("cw/cw.data.11.USFoodBeverage"))
    );
    assert!(old.iter().any(|u| u.ends_with("ap/ap.data.3.Food")));
    // Area-only partitions are not read for the U.S. city average, and
    // C-CPI-U has no values before December 1999.
    assert!(
        !old.iter()
            .any(|u| u.contains("Asize") || u.contains("/su/"))
    );
    assert!(old.iter().all(|u| !u.ends_with("data.0.Current")));
    let all = reader(
        SourceId::Bls,
        &Bases::parse(&["bls-areas=all".into()]).unwrap(),
    );
    assert!(
        all.backfill_targets(1990)
            .iter()
            .any(|t| t.url.contains("AsizeWest"))
    );
    let recent: BTreeSet<String> = r
        .backfill_targets(2026)
        .into_iter()
        .map(|t| t.url)
        .collect();
    assert_eq!(recent.len(), 4);
    assert!(recent.iter().all(|u| u.ends_with("data.0.Current")));

    // CPI is released around the middle of the next month at 8:30 a.m. ET.
    let p = bls::parse_data(Survey::Cu, &fixture("bls/cu.data.0.Current"), &us()).unwrap();
    let recs = records(SourceId::Bls, p, Stamp::Backfill);
    let sep = one(&recs, "CUUR0000SA0", "2026-09-01");
    assert_eq!(
        sep.prov().ts_source_ms.0,
        Date::new(2026, 10, 15).unwrap().start_ms() + 13 * 3_600_000 + 30 * 60_000
    );
    let dec = one(&recs, "CUUR0000SA0", "1996-12-01");
    assert_eq!(
        dec.prov().ts_source_ms.0,
        Date::new(1997, 1, 15).unwrap().start_ms() + 13 * 3_600_000 + 30 * 60_000
    );
}

#[tokio::test]
async fn bls_backfill_reads_partitions_then_the_current_file_once() {
    let fetched: Arc<Mutex<BTreeMap<String, u32>>> = Arc::default();
    let log = fetched.clone();
    let fetch = Fetch::Map(Arc::new(move |url: &str| -> Option<Bytes> {
        *log.lock().unwrap().entry(url.to_owned()).or_default() += 1;
        let name = url.rsplit('/').next().unwrap_or("");
        match name {
            "cu.data.1.AllItems" | "cu.data.0.Current" | "su.data.0.Current"
            | "cw.data.0.Current" | "ap.data.0.Current" => {
                Some(Bytes::from(fixture(&format!("bls/{name}"))))
            }
            _ => None,
        }
    }));
    let root = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let r = reader(SourceId::Bls, &Bases::default());
    let opts = Options {
        archive_root: root.path().to_path_buf(),
        state_dir: state.path().to_path_buf(),
        from_year: Some(1990),
        to_year: 2026,
        current_year: 2026,
    };
    let units = run_source(r.as_ref(), &fetch, &opts, &Metrics::new())
        .await
        .unwrap();
    let records_of = |y: i32| units.iter().find(|u| u.year == y).unwrap().records;
    // 1990: the all-items partition, U.S. city average only.
    assert_eq!(records_of(1990), 2);
    assert_eq!(records_of(1996), 0);
    // 2026: CPI-U 5, CPI-W 2, C-CPI-U 2, average prices 2.
    assert_eq!(records_of(2026), 11);
    // Every file that exists was fetched once for the whole run, although
    // the all-items partition serves seven units.
    let counts = fetched.lock().unwrap();
    for name in [
        "cu.data.1.AllItems",
        "cu.data.0.Current",
        "ap.data.0.Current",
    ] {
        let n: u32 = counts
            .iter()
            .filter(|(u, _)| u.ends_with(&format!("/{name}")))
            .map(|(_, n)| *n)
            .sum();
        assert_eq!(n, 1, "{name}: {counts:?}");
    }
    let notices = std::fs::read_to_string(root.path().join("_notices.json")).unwrap();
    assert!(notices.contains("BLS.gov cannot vouch"));
}

// ----- Configuration -------------------------------------------------------

#[test]
fn user_agent_carries_the_project_contact() {
    let a = Args::parse_from([
        "reference-pollers",
        "--user-agent",
        "bleh.xyz reference-pollers",
    ]);
    assert_eq!(
        a.full_user_agent(),
        "bleh.xyz reference-pollers (data@bleh.xyz)"
    );
    let named = Args::parse_from([
        "reference-pollers",
        "--user-agent",
        "x (+https://bleh.xyz; data@bleh.xyz)",
    ]);
    assert_eq!(
        named.full_user_agent(),
        "x (+https://bleh.xyz; data@bleh.xyz)"
    );
    let none = Args::parse_from(["reference-pollers", "--user-agent", "x", "--contact", ""]);
    assert_eq!(none.full_user_agent(), "x");
    let b = Args::parse_from(["reference-pollers", "--bls-areas", "all"])
        .bases()
        .unwrap();
    assert_eq!(b.get("bls-areas", ""), "all");
    assert!(
        Args::parse_from(["reference-pollers"])
            .selected()
            .unwrap()
            .contains(&SourceId::Bls)
    );
}

// ----- Live polling ----------------------------------------------------------

mod live_poll {
    use std::sync::Arc;

    use clap::Parser;
    use reference_pollers::config::Args;
    use reference_pollers::http::{Fetcher, HttpConfig};
    use reference_pollers::live::SourcePoller;
    use reference_pollers::metrics::Metrics;
    use reference_pollers::output::Emitter;
    use reference_pollers::raw::RawStore;
    use reference_pollers::source::SourceId;
    use reference_pollers::sources::{Bases, reader};
    use reference_pollers::time::Date;
    use tokio::sync::{Mutex, mpsc};
    use wiremock::matchers::{header_regex, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::common::{fixture, flagged, zip_of};

    /// 2026-11-14 12:00 UTC.
    const NOW: i64 = 1_794_657_600_000;

    async fn poller(
        id: SourceId,
        server: &MockServer,
        key: &str,
    ) -> (
        SourcePoller,
        mpsc::Receiver<mesh_types::Record>,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let metrics = Arc::new(Metrics::new());
        let args = Args::parse_from([
            "reference-pollers",
            "--user-agent",
            "bleh.xyz reference-pollers",
        ]);
        let fetcher = Fetcher::new(
            HttpConfig {
                min_interval: std::time::Duration::from_millis(1),
                max_retries: 0,
                ..args.http_config()
            },
            metrics.clone(),
        )
        .unwrap();
        let (tx, rx) = mpsc::channel(10_000);
        let emitter = Arc::new(Mutex::new(Emitter::new(None, Some(tx), metrics.clone())));
        let bases = Bases::parse(&[format!("{key}={}", server.uri())]).unwrap();
        let p = SourcePoller::open(
            reader(id, &bases),
            fetcher,
            RawStore::new(dir.path().join("raw")),
            &dir.path().join("state"),
            emitter,
            metrics,
            Date::of_ms(NOW),
        )
        .unwrap();
        (p, rx, dir)
    }

    #[tokio::test]
    async fn bls_live_cycle_sends_the_contact_and_dedupes() {
        let server = MockServer::start().await;
        for d in ["cu", "cw", "su", "ap"] {
            Mock::given(method("GET"))
                .and(path(format!("/{d}/{d}.data.0.Current")))
                .and(header_regex("user-agent", r"\(data@bleh\.xyz\)$"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("last-modified", "Tue, 10 Nov 2026 13:30:00 GMT")
                        .set_body_bytes(fixture(&format!("bls/{d}.data.0.Current"))),
                )
                .mount(&server)
                .await;
        }
        let (mut p, mut rx, _dir) = poller(SourceId::Bls, &server, "bls").await;
        let report = p.cycle(NOW).await.unwrap();
        assert_eq!(report.failures, 0, "{report:?}");
        let mut recs = Vec::new();
        while let Ok(r) = rx.try_recv() {
            recs.push(r);
        }
        // Inside the 1,900-day window: CPI-U 5, CPI-W 2, C-CPI-U 3,
        // average prices 2 (the 1996 value is outside it).
        assert_eq!(recs.len(), 12, "{report:?}");
        // ts_source_ms is the release time the files carry.
        let released = Date::new(2026, 11, 10).unwrap().start_ms() + 13 * 3_600_000 + 30 * 60_000;
        assert!(recs.iter().all(|r| r.prov().ts_source_ms.0 == released));
        let again = p.cycle(NOW + 60_000).await.unwrap();
        assert_eq!((again.records, again.changed), (0, 0));
    }

    #[tokio::test]
    async fn eia_live_cycle_reads_both_bulk_files() {
        let server = MockServer::start().await;
        for code in ["PET", "NG"] {
            Mock::given(method("GET"))
                .and(path(format!("/opendata/bulk/{code}.zip")))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(zip_of(&[(
                    &format!("{code}.txt"),
                    &fixture(&format!("eia/{code}.txt")),
                )])))
                .mount(&server)
                .await;
        }
        let (mut p, mut rx, _dir) = poller(SourceId::Eia, &server, "eia").await;
        let report = p.cycle(NOW).await.unwrap();
        assert_eq!(report.failures, 0, "{report:?}");
        let mut recs = Vec::new();
        while let Ok(r) = rx.try_recv() {
            recs.push(r);
        }
        // Within the 400-day window: regular gasoline 2, diesel 1,
        // residential heating oil 2, refiner gasoline 2, crude first
        // purchase 1, city gate 2, residential gas 1.
        assert_eq!(recs.len(), 11);
        assert_eq!(recs.iter().filter(|r| flagged(r)).count(), 6);
    }
}
