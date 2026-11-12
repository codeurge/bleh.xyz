//! Parser tests over synthetic DTCC slice and cumulative files.
//!
//! Synthetic data only (see `tests/common/mod.rs`).

#![allow(clippy::unwrap_used)]

mod common;

use std::io::Cursor;

use common::{DISSEMINATION_TS, Report, base_trade, zip_entries};
use mesh_types::{
    AssetClass, Flag, Record, SwapAction, SwapPrice, SwapPrint, UnixMillis, UnixNanos, allowlist,
};
use swap_pollers::parse::{
    FileContext, ParseStats, ReportKind, parse_csv_reader, parse_report_file, parse_zip,
};
use swap_pollers::repository::{DtccAsset, DtccBucket, Regime, Repository};

fn ctx(regime: Regime, asset: DtccAsset) -> FileContext {
    FileContext {
        bucket: DtccBucket::new(regime, asset),
        repository: Repository::DtccDdr,
        kind: ReportKind::Slice,
        batch: 34_980_001,
        fetched_at: Some(UnixNanos(1_794_499_600_000_000_000)),
        listing_dissemination: None,
        backfill: false,
    }
}

fn parse(report: &Report, ctx: &FileContext) -> (Vec<SwapPrint>, ParseStats) {
    let mut out = Vec::new();
    let mut sink = |p| {
        out.push(p);
        Ok(())
    };
    let stats = parse_csv_reader(report.csv().as_bytes(), ctx, &mut sink).unwrap();
    (out, stats)
}

fn ms(s: &str) -> UnixMillis {
    swap_pollers::values::timestamp(s).unwrap().unwrap()
}

fn rate(s: &str) -> Option<SwapPrice> {
    Some(SwapPrice::Rate {
        value: mesh_types::Price::parse_exact(s).unwrap(),
    })
}
fn spread(s: &str) -> Option<SwapPrice> {
    Some(SwapPrice::Spread {
        value: mesh_types::Price::parse_exact(s).unwrap(),
    })
}
fn price(s: &str) -> Option<SwapPrice> {
    Some(SwapPrice::Price {
        value: mesh_types::Price::parse_exact(s).unwrap(),
    })
}

/// Every emitted record validates, passes the field allowlist and
/// round-trips through JSON.
fn check_all(prints: &[SwapPrint]) {
    for p in prints {
        let r = Record::SwapPrint(p.clone());
        r.validate().unwrap();
        allowlist::check_record(&r).unwrap();
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Record>(&json).unwrap(), r);
    }
}

fn with(
    mut base: Vec<(&'static str, &'static str)>,
    extra: &[(&'static str, &'static str)],
) -> Vec<(&'static str, &'static str)> {
    base.extend_from_slice(extra);
    base
}

#[test]
fn rates_swaps_and_swaptions() {
    let exec = "2026-11-12T14:05:07Z";
    let dissem = "2026-11-12T14:05:09Z";
    let report = Report::standard()
        // SOFR OIS, capped notional, block election.
        .row(&with(
            base_trade("9000000000000000001", "IR", exec, dissem),
            &[
                ("Effective Date", "2026-11-16"),
                ("Expiration Date", "2031-11-16"),
                ("Block trade election indicator", "TRUE"),
                ("Notional amount-Leg 1", "1,000,000,000+"),
                ("Notional amount-Leg 2", "1,000,000,000+"),
                ("Notional currency-Leg 1", "USD"),
                ("Notional currency-Leg 2", "USD"),
                ("Fixed rate-Leg 1", "0.0375"),
                ("Unique Product Identifier", "QZSYN0000001"),
                ("UPI FISN", "NA/Swap OIS USD"),
                ("UPI Underlier Name", "USD-SOFR-OIS Compound"),
            ],
        ))
        // Fixed rate on leg 2 only, negative.
        .row(&with(
            base_trade("9000000000000000002", "IR", exec, dissem),
            &[
                ("Effective Date", "2026-11-16"),
                ("Expiration Date", "2027-05-16"),
                ("Notional amount-Leg 1", "25,000,000"),
                ("Notional currency-Leg 1", "JPY"),
                ("Fixed rate-Leg 2", "-0.00125"),
                ("Unique Product Identifier", "QZSYN0000002"),
            ],
        ))
        // Swaption: strike in decimal notation is the price (CFTC).
        .row(&with(
            base_trade("9000000000000000003", "IR", exec, dissem),
            &[
                ("Effective Date", "2026-11-16"),
                ("Expiration Date", "2027-11-16"),
                ("Notional amount-Leg 1", "50,000,000"),
                ("Notional currency-Leg 1", "USD"),
                ("Fixed rate-Leg 1", "0.04"),
                ("Strike Price", "0.0425"),
                ("Strike price notation", "3"),
                ("Option Type", "CALL"),
                ("Option Style", "EURO"),
                ("Option Premium Amount", "1,250,000.00"),
                ("Option Premium Currency", "USD"),
                ("Unique Product Identifier", "QZSYN0000003"),
            ],
        ))
        // Basis swap: spread only, in basis points.
        .row(&with(
            base_trade("9000000000000000004", "IR", exec, dissem),
            &[
                ("Notional amount-Leg 1", "100,000,000"),
                ("Notional currency-Leg 1", "USD"),
                ("Spread-Leg 2", "12.5"),
                ("Spread notation-Leg 2", "4"),
                ("Unique Product Identifier", "QZSYN0000004"),
            ],
        ));
    let (p, stats) = parse(&report, &ctx(Regime::CftcPart43, DtccAsset::Rates));
    assert_eq!(stats.rows, 4);
    assert_eq!(stats.records, 4);
    assert_eq!(stats.row_errors, 0);
    check_all(&p);

    let ois = &p[0];
    assert_eq!(ois.asset_class, AssetClass::InterestRate);
    assert_eq!(ois.action, SwapAction::New);
    assert_eq!(ois.price, rate("3.75"));
    assert_eq!(ois.tenor.as_deref(), Some("5Y"));
    assert_eq!(ois.expiry.as_deref(), Some("2031-11-16"));
    assert_eq!(ois.upi.as_deref(), Some("QZSYN0000001"));
    assert_eq!(ois.product.as_deref(), Some("NA/Swap OIS USD"));
    assert_eq!(ois.underlier.as_deref(), Some("USD-SOFR-OIS Compound"));
    let n = ois.notional.as_ref().unwrap();
    assert_eq!(n.amount.to_string(), "1000000000");
    assert_eq!(n.currency, "USD");
    assert!(ois.prov.flags.contains(Flag::SizeCapped));
    assert!(ois.prov.flags.contains(Flag::BlockTradeDelayed));
    assert_eq!(
        ois.prov.instrument.as_str(),
        "cftc-part43:swap:QZSYN0000001"
    );
    assert_eq!(ois.prov.venue.as_str(), "cftc-part43");
    assert_eq!(ois.prov.admission.as_str(), "ADM-0002");
    assert_eq!(ois.repository.as_str(), "dtcc-ddr");
    assert_eq!(ois.ts_execution_ms, ms(exec));
    assert_eq!(ois.ts_dissemination_ms, ms(dissem));
    assert_eq!(ois.prov.ts_source_ms, ms(dissem));
    assert_eq!(ois.prov.ts_ingest_ns, UnixNanos(1_794_499_600_000_000_000));
    assert_eq!(ois.prov.seq.block_number, 34_980_001);
    assert_eq!(ois.prov.seq.event_index, 0);

    assert_eq!(p[1].price, rate("-0.125"));
    assert_eq!(p[1].tenor.as_deref(), Some("6M"));
    assert!(!p[1].prov.flags.contains(Flag::SizeCapped));
    assert_eq!(p[2].price, rate("4.25"));
    assert_eq!(p[3].price, spread("12.5"));
}

#[test]
fn credit_spreads_prices_and_coupons() {
    let exec = "2026-11-12T15:00:00Z";
    let report = Report::standard()
        .row(&with(
            base_trade("8000000000000000001", "CR", exec, exec),
            &[
                ("Notional amount-Leg 1", "10,000,000"),
                ("Notional currency-Leg 1", "USD"),
                ("Fixed rate-Leg 1", "0.01"),
                ("Spread-Leg 1", "0.0062"),
                ("Spread notation-Leg 1", "3"),
                ("Unique Product Identifier", "QZSYNCDS0001"),
                ("UPI FISN", "NA/CDS Corp SN Sr"),
            ],
        ))
        .row(&with(
            base_trade("8000000000000000002", "CR", exec, exec),
            &[
                ("Notional amount-Leg 1", "5,000,000"),
                ("Notional currency-Leg 1", "EUR"),
                ("Spread-Leg 1", "125"),
                ("Spread notation-Leg 1", "4"),
                ("Unique Product Identifier", "QZSYNCDS0002"),
            ],
        ))
        // Price-quoted index trade.
        .row(&with(
            base_trade("8000000000000000003", "CR", exec, exec),
            &[
                ("Notional amount-Leg 1", "20,000,000"),
                ("Notional currency-Leg 1", "USD"),
                ("Fixed rate-Leg 1", "0.05"),
                ("Price", "101.25"),
                ("Price notation", "1"),
                ("Unique Product Identifier", "QZSYNCDX0003"),
            ],
        ))
        // Only the coupon is known.
        .row(&with(
            base_trade("8000000000000000004", "CR", exec, exec),
            &[
                ("Fixed rate-Leg 1", "0.01"),
                ("Unique Product Identifier", "QZSYNCDS0004"),
            ],
        ));
    let (p, stats) = parse(&report, &ctx(Regime::SecSbsr, DtccAsset::Credits));
    assert_eq!(stats.records, 4);
    check_all(&p);
    assert_eq!(p[0].price, spread("62"));
    assert_eq!(p[0].prov.venue.as_str(), "sec-sbsr");
    assert_eq!(p[0].prov.admission.as_str(), "ADM-0004");
    assert_eq!(p[1].price, spread("125"));
    assert_eq!(p[2].price, price("101.25"));
    assert_eq!(p[3].price, rate("1"));
    assert_eq!(p[3].notional, None);
}

#[test]
fn equity_prices_and_financing_spreads() {
    let exec = "2026-11-12T15:30:00Z";
    let report = Report::standard()
        .row(&with(
            base_trade("7000000000000000001", "EQ", exec, exec),
            &[
                ("Notional amount-Leg 1", "2,469,000"),
                ("Notional currency-Leg 1", "USD"),
                ("Price", "1,234.5"),
                ("Price notation", "1"),
                ("Price currency", "USD"),
                ("Price unit of measure", "SHAS"),
                ("Unique Product Identifier", "QZSYNEQS0001"),
                ("UPI FISN", "NA/Swaps Sgle Stk Tot Rtn"),
                ("Underlier ID-Leg 1", "SYNTHETIC.EQ1"),
            ],
        ))
        .row(&with(
            base_trade("7000000000000000002", "EQ", exec, exec),
            &[
                ("Notional amount-Leg 1", "10,000,000"),
                ("Notional currency-Leg 1", "USD"),
                ("Spread-Leg 2", "0.0045"),
                ("Spread notation-Leg 2", "3"),
                ("Unique Product Identifier", "QZSYNEQS0002"),
            ],
        ))
        // An equity option reported to the SEC: strike is not admitted
        // under ADM-0004, so only the price is used.
        .row(&with(
            base_trade("7000000000000000003", "EQ", exec, exec),
            &[
                ("Strike Price", "250"),
                ("Strike price notation", "1"),
                ("Option Type", "PUTO"),
                ("Price", "12.5"),
                ("Price notation", "1"),
                ("Unique Product Identifier", "QZSYNEQO0003"),
            ],
        ));
    let (p, stats) = parse(&report, &ctx(Regime::SecSbsr, DtccAsset::Equities));
    assert_eq!(stats.records, 3);
    check_all(&p);
    assert_eq!(p[0].price, price("1234.5"));
    assert_eq!(p[0].underlier.as_deref(), Some("SYNTHETIC.EQ1"));
    assert_eq!(p[1].price, spread("45"));
    assert_eq!(p[2].price, price("12.5"));
}

#[test]
fn fx_forwards_and_options() {
    let exec = "2026-11-12T08:00:00Z";
    let report = Report::standard()
        .row(&with(
            base_trade("6000000000000000001", "FX", exec, exec),
            &[
                ("Expiration Date", "2026-12-16"),
                ("Notional amount-Leg 1", "5,000,000"),
                ("Notional currency-Leg 1", "USD"),
                ("Notional amount-Leg 2", "415,617,000"),
                ("Notional currency-Leg 2", "INR"),
                ("Exchange rate", "83.1234"),
                ("Exchange rate basis", "USD/INR"),
                ("Unique Product Identifier", "QZSYNNDF0001"),
                ("UPI FISN", "NA/Fwd NDF INR USD"),
            ],
        ))
        .row(&with(
            base_trade("6000000000000000002", "FX", exec, exec),
            &[
                ("Notional amount-Leg 1", "10,000,000"),
                ("Notional currency-Leg 1", "EUR"),
                ("Strike Price", "1.0850"),
                ("Strike price notation", "1"),
                ("Option Type", "CALL"),
                ("Option Premium Amount", "85,000"),
                ("Option Premium Currency", "USD"),
                ("Unique Product Identifier", "QZSYNFXO0002"),
            ],
        ));
    let (p, stats) = parse(&report, &ctx(Regime::CftcPart43, DtccAsset::Forex));
    assert_eq!(stats.records, 2);
    check_all(&p);
    assert_eq!(p[0].asset_class, AssetClass::ForeignExchange);
    assert_eq!(p[0].price, price("83.1234"));
    assert_eq!(p[0].expiry.as_deref(), Some("2026-12-16"));
    assert_eq!(p[1].price, price("1.085"));
}

#[test]
fn commodities_without_upi_and_capped_quantities() {
    let exec = "2026-11-12T16:45:00Z";
    let report = Report::standard()
        .row(&with(
            base_trade("5000000000000000001", "CO", exec, exec),
            &[
                ("Product name", "Commodity:Energy:NatGas:Swap:Cash"),
                ("Expiration Date", "2027-01-31"),
                ("Total notional quantity-Leg 1", "50,000+"),
                ("Quantity unit of measure-Leg 1", "MMBT"),
                ("Price", "3.215"),
                ("Price notation", "1"),
                ("Price currency", "USD"),
                ("Underlier ID-Leg 1", "SYN HUB A"),
            ],
        ))
        // Basis swap with a monetary spread.
        .row(&with(
            base_trade("5000000000000000002", "CO", exec, exec),
            &[
                ("Product name", "Commodity:Energy:NatGas:Swap:Cash"),
                ("Notional amount-Leg 1", "1,000,000"),
                ("Notional currency-Leg 1", "USD"),
                ("Spread-Leg 1", "-0.25"),
                ("Spread notation-Leg 1", "1"),
                ("Underlier ID-Leg 1", "SYN HUB A"),
            ],
        ))
        // Commodity option: strike is the price.
        .row(&with(
            base_trade("5000000000000000003", "CO", exec, exec),
            &[
                ("Product name", "Commodity:Energy:Oil:Option:Cash"),
                ("Strike Price", "75.5"),
                ("Strike price notation", "1"),
                ("Option Type", "PUTO"),
                ("Option Premium Amount", "120,000"),
                ("Option Premium Currency", "USD"),
                ("Underlier ID-Leg 1", "SYN CRUDE"),
            ],
        ));
    let (p, stats) = parse(&report, &ctx(Regime::CftcPart43, DtccAsset::Commodities));
    assert_eq!(stats.records, 3);
    check_all(&p);
    assert_eq!(p[0].asset_class, AssetClass::Commodity);
    assert_eq!(
        p[0].prov.instrument.as_str(),
        "cftc-part43:swap:Commodity:Energy:NatGas:Swap:Cash:SYN-HUB-A"
    );
    assert_eq!(
        p[0].product.as_deref(),
        Some("Commodity:Energy:NatGas:Swap:Cash")
    );
    assert_eq!(p[0].price, price("3.215"));
    assert!(p[0].prov.flags.contains(Flag::SizeCapped));
    assert_eq!(p[0].notional, None);
    assert_eq!(p[1].price, price("-0.25"));
    assert_eq!(p[2].price, price("75.5"));
    // Same instrument, same batch: indices increase per instrument.
    assert_eq!(p[0].prov.seq.event_index, 0);
    assert_eq!(p[1].prov.seq.event_index, 1);
    assert_eq!(p[2].prov.seq.event_index, 0);
}

#[test]
fn cancels_corrections_and_lifecycle_events() {
    let exec = "2026-11-12T10:00:00Z";
    let later = "2026-11-12T10:30:00Z";
    let upi = ("Unique Product Identifier", "QZSYN0000009");
    let fixed = ("Fixed rate-Leg 1", "0.039");
    let report = Report::standard()
        .row(&with(base_trade("100", "IR", exec, exec), &[upi, fixed]))
        // Correction of 100.
        .row(&with(
            base_trade("101", "IR", exec, later),
            &[
                ("Action type", "CORR"),
                ("Event type", ""),
                ("Original Dissemination Identifier", "100"),
                upi,
                ("Fixed rate-Leg 1", "0.0391"),
            ],
        ))
        // Error (cancel) of 100.
        .row(&with(
            base_trade("102", "IR", exec, later),
            &[
                ("Action type", "EROR"),
                ("Event type", ""),
                ("Original Dissemination Identifier", "100"),
                upi,
            ],
        ))
        // Correction without the original ID: rejected.
        .row(&with(
            base_trade("103", "IR", exec, later),
            &[("Action type", "CORR"), upi],
        ))
        // Lifecycle actions observation v0 cannot represent: skipped.
        .row(&with(
            base_trade("104", "IR", exec, later),
            &[
                ("Action type", "MODI"),
                ("Original Dissemination Identifier", "100"),
                upi,
            ],
        ))
        .row(&with(
            base_trade("105", "IR", exec, later),
            &[("Action type", "TERM"), ("Event type", "ETRM"), upi],
        ))
        .row(&with(
            base_trade("106", "IR", exec, later),
            &[("Action type", "REVI"), upi],
        ))
        // New message for a non-trade event (clearing): skipped.
        .row(&with(
            base_trade("107", "IR", exec, later),
            &[("Event type", "CLRG"), upi, fixed],
        ))
        // Missing action type: rejected.
        .row(&with(
            base_trade("108", "IR", exec, later),
            &[("Action type", ""), upi],
        ));
    let (p, stats) = parse(&report, &ctx(Regime::CftcPart43, DtccAsset::Rates));
    check_all(&p);
    assert_eq!(stats.rows, 9);
    assert_eq!(stats.records, 3);
    assert_eq!(stats.row_errors, 2);
    assert_eq!(stats.skipped.get("lifecycle_action"), Some(&3));
    assert_eq!(stats.skipped.get("non_trade_event"), Some(&1));
    assert_eq!(p[1].action, SwapAction::Correct);
    assert_eq!(p[1].original_dissemination_id.as_deref(), Some("100"));
    assert_eq!(p[1].price, rate("3.91"));
    assert_eq!(p[2].action, SwapAction::Cancel);
    assert_eq!(p[2].original_dissemination_id.as_deref(), Some("100"));
    assert_eq!(p[2].ts_dissemination_ms, ms(later));
}

#[test]
fn odd_timestamps_and_formats() {
    let upi = ("Unique Product Identifier", "QZSYN0000010");
    let report = Report::standard()
        .row(&with(
            base_trade("1", "IR", "2026-11-12 14:05:00", "2026-11-12 14:05:02"),
            &[upi],
        ))
        .row(&with(
            base_trade(
                "2",
                "IR",
                "2026-11-12T14:05:00.123456Z",
                "2026-11-12T14:05:02.5Z",
            ),
            &[upi],
        ))
        .row(&with(
            base_trade(
                "3",
                "IR",
                "2026-11-12T09:05:00-05:00",
                "2026-11-12T15:05:02+01:00",
            ),
            &[upi],
        ))
        .row(&with(
            base_trade("4", "IR", "11/12/2026 14:05:00", "2026-11-12T14:05:02"),
            &[upi],
        ))
        .row(&with(
            base_trade("5", "IR", "not a time", "2026-11-12T14:05:02Z"),
            &[upi],
        ))
        // Bad optional cells drop only that value.
        .row(&with(
            base_trade("6", "IR", "2026-11-12T14:05:00Z", "2026-11-12T14:05:02Z"),
            &[
                upi,
                ("Expiration Date", "someday"),
                ("Fixed rate-Leg 1", "abc"),
            ],
        ))
        // The all-nines "unknown" sentinel is not a price.
        .row(&with(
            base_trade("7", "IR", "2026-11-12T14:05:00Z", "2026-11-12T14:05:02Z"),
            &[
                upi,
                ("Price", "99999.9999999999999"),
                ("Price notation", "1"),
            ],
        ))
        // Exponent notation is expanded exactly.
        .row(&with(
            base_trade("8", "IR", "2026-11-12T14:05:00Z", "2026-11-12T14:05:02Z"),
            &[upi, ("Fixed rate-Leg 1", "3.75E-2")],
        ));
    let (p, stats) = parse(&report, &ctx(Regime::CftcPart43, DtccAsset::Rates));
    check_all(&p);
    assert_eq!(stats.records, 7);
    assert_eq!(stats.row_errors, 1);
    let t = ms("2026-11-12T14:05:00Z");
    assert_eq!(p[0].ts_execution_ms, t);
    assert_eq!(p[0].ts_dissemination_ms, ms("2026-11-12T14:05:02Z"));
    assert_eq!(p[1].ts_execution_ms, UnixMillis(t.0 + 123));
    assert_eq!(p[1].ts_dissemination_ms, UnixMillis(t.0 + 2_500));
    assert_eq!(p[2].ts_execution_ms, t);
    assert_eq!(p[2].ts_dissemination_ms, ms("2026-11-12T14:05:02Z"));
    assert_eq!(p[3].ts_execution_ms, t);
    assert_eq!(p[4].dissemination_id, "6");
    assert_eq!(p[4].expiry, None);
    assert_eq!(p[4].price, None);
    assert_eq!(stats.field_errors.get("Expiration Date"), Some(&1));
    assert_eq!(stats.field_errors.get("Fixed rate-Leg 1"), Some(&1));
    assert_eq!(p[5].price, None);
    assert_eq!(p[6].price, rate("3.75"));
}

#[test]
fn unknown_columns_are_counted_not_fatal() {
    let report = Report::standard()
        .with_column("Brand New Column")
        .with_column("Another-New Field")
        .row(&with(
            base_trade("1", "IR", "2026-11-12T14:05:00Z", "2026-11-12T14:05:02Z"),
            &[
                ("Unique Product Identifier", "QZSYN0000011"),
                ("Brand New Column", "whatever"),
                ("Fixed rate-Leg 1", "0.03"),
            ],
        ));
    let (p, stats) = parse(&report, &ctx(Regime::CftcPart43, DtccAsset::Rates));
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].price, rate("3"));
    assert_eq!(
        stats.unknown_columns,
        vec!["brandnewcolumn".to_string(), "anothernewfield".to_string()]
    );
}

#[test]
fn appendix_a_layout_without_dissemination_timestamp() {
    let exec = "2026-11-12T14:05:00Z";
    let mut row = base_trade("1", "IR", exec, "");
    row.retain(|(k, _)| *k != DISSEMINATION_TS);
    row.push(("Unique Product Identifier", "QZSYN0000012"));
    row.push(("Event timestamp", "2026-11-12T14:05:01Z"));
    row.retain(|(k, v)| !(*k == "Event timestamp" && *v == exec));
    let report = Report::appendix_a().row(&row);

    // Listing dissemination time is used when present.
    let mut c = ctx(Regime::CftcPart43, DtccAsset::Rates);
    c.listing_dissemination = Some(ms("2026-11-12T14:06:00Z"));
    let (p, stats) = parse(&report, &c);
    assert_eq!(p[0].ts_dissemination_ms, ms("2026-11-12T14:06:00Z"));
    assert_eq!(stats.dissemination_fallbacks.get("listing"), Some(&1));

    // Otherwise the event timestamp; without a sidecar the ingest time is
    // the dissemination time, so output is a pure function of the file.
    c.listing_dissemination = None;
    c.fetched_at = None;
    let (p, stats) = parse(&report, &c);
    assert_eq!(p[0].ts_dissemination_ms, ms("2026-11-12T14:05:01Z"));
    assert_eq!(
        p[0].prov.ts_ingest_ns,
        ms("2026-11-12T14:05:01Z").to_nanos().unwrap()
    );
    assert_eq!(
        stats.dissemination_fallbacks.get("event_timestamp"),
        Some(&1)
    );
    check_all(&p);
}

#[test]
fn json_style_headers_bom_quoting_and_ragged_rows() {
    // Ticker-style camel-case headers normalize to the same fields.
    let csv = "\u{feff}disseminationIdentifier,actionType,eventType,executionTimestamp,disseminationTimestamp,assetClass,uniqueProductIdentifier,uniqueProductIdentifierShortName,notionalAmountLeg1,notionalCurrencyLeg1,fixedRateLeg1\r\n\
               42,NEWT,TRAD,2026-11-12T14:05:00Z,2026-11-12T14:05:02Z,IR,QZSYN0000013,\"NA/Swap, Fxd Flt USD\",\"5,000,000\",USD,0.0412\r\n\
               43,NEWT,TRAD,2026-11-12T14:05:00Z\r\n\
               \r\n";
    let mut out = Vec::new();
    let mut sink = |p| {
        out.push(p);
        Ok(())
    };
    let c = ctx(Regime::CftcPart43, DtccAsset::Rates);
    let stats = parse_csv_reader(csv.as_bytes(), &c, &mut sink).unwrap();
    assert_eq!(stats.rows, 2);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].product.as_deref(), Some("NA/Swap, Fxd Flt USD"));
    assert_eq!(
        out[0].notional.as_ref().unwrap().amount.to_string(),
        "5000000"
    );
    assert_eq!(out[0].price, rate("4.12"));
    // The short row has what it has; dissemination falls back.
    assert_eq!(out[1].dissemination_id, "43");
    assert_eq!(stats.dissemination_fallbacks.get("event_timestamp"), None);
    assert_eq!(
        stats.dissemination_fallbacks.get("execution_timestamp"),
        Some(&1)
    );
}

#[test]
fn missing_required_column_is_a_file_error() {
    let csv = "Action type,Price\r\nNEWT,1\r\n";
    let mut sink = |_| Ok(());
    let c = ctx(Regime::CftcPart43, DtccAsset::Rates);
    assert!(parse_csv_reader(csv.as_bytes(), &c, &mut sink).is_err());
}

#[test]
fn zip_archives_and_files_on_disk() {
    let a = Report::standard().row(&with(
        base_trade("1", "EQ", "2026-11-12T14:05:00Z", "2026-11-12T14:05:02Z"),
        &[
            ("Unique Product Identifier", "QZSYNEQS0100"),
            ("Price", "10"),
            ("Price notation", "1"),
        ],
    ));
    let b = Report::standard().row(&with(
        base_trade("2", "EQ", "2026-11-12T14:06:00Z", "2026-11-12T14:06:02Z"),
        &[
            ("Unique Product Identifier", "QZSYNEQS0100"),
            ("Price", "11"),
            ("Price notation", "1"),
        ],
    ));
    // Two CSV entries (and a non-CSV entry, ignored), parsed in name order
    // with indices continuing across entries.
    let zip = zip_entries(&[
        ("b.csv", b.csv().as_bytes()),
        ("readme.txt", b"synthetic"),
        ("a.csv", a.csv().as_bytes()),
    ]);
    let c = ctx(Regime::CftcPart43, DtccAsset::Equities);
    let mut out = Vec::new();
    let mut sink = |p| {
        out.push(p);
        Ok(())
    };
    let stats = parse_zip(Cursor::new(zip.clone()), &c, &mut sink).unwrap();
    assert_eq!(stats.records, 2);
    assert_eq!(out[0].dissemination_id, "1");
    assert_eq!(out[0].prov.seq.event_index, 0);
    assert_eq!(out[1].prov.seq.event_index, 1);

    let dir = tempfile::tempdir().unwrap();
    let zpath = dir.path().join("CFTC_SLICE_EQUITIES_2026_11_12_7.zip");
    std::fs::write(&zpath, &zip).unwrap();
    let cpath = dir.path().join("plain.csv");
    std::fs::write(&cpath, a.csv()).unwrap();
    let mut n = 0;
    let mut count = |_| {
        n += 1;
        Ok(())
    };
    parse_report_file(&zpath, &c, &mut count).unwrap();
    parse_report_file(&cpath, &c, &mut count).unwrap();
    assert_eq!(n, 3);

    let bad = dir.path().join("bad.zip");
    std::fs::write(&bad, b"PK\x03\x04 not really a zip").unwrap();
    let mut none = |_| Ok(());
    assert!(parse_report_file(&bad, &c, &mut none).is_err());
}

#[test]
fn deterministic_canonical_output() {
    let report = Report::standard().row(&with(
        base_trade("1", "IR", "2026-11-12T14:05:00Z", "2026-11-12T14:05:02Z"),
        &[
            ("Unique Product Identifier", "QZSYN0000014"),
            ("Fixed rate-Leg 1", "0.0375"),
        ],
    ));
    let c = ctx(Regime::CftcPart43, DtccAsset::Rates);
    let (a, _) = parse(&report, &c);
    let (b, _) = parse(&report, &c);
    let enc =
        |p: &SwapPrint| mesh_types::hash::canonical_record(&Record::SwapPrint(p.clone())).unwrap();
    assert_eq!(enc(&a[0]), enc(&b[0]));
    let text = String::from_utf8(enc(&a[0])).unwrap();
    assert!(text.starts_with("{\"action\":\"new\",\"asset_class\":\"interest_rate\""));
    assert!(text.contains("\"kind\":\"swap_print\""));
    assert!(text.contains("\"price\":{\"type\":\"rate\",\"value\":\"3.75\"}"));
}

#[test]
fn backfill_flag_from_context() {
    let report = Report::standard().row(&with(
        base_trade("1", "IR", "2026-11-12T14:05:00Z", "2026-11-12T14:05:02Z"),
        &[("Unique Product Identifier", "QZSYN0000015")],
    ));
    let mut c = ctx(Regime::CftcPart43, DtccAsset::Rates);
    c.kind = ReportKind::Cumulative;
    c.backfill = true;
    let (p, _) = parse(&report, &c);
    assert!(p[0].prov.flags.contains(Flag::Backfill));
    check_all(&p);
}
