//! Parser tests over synthetic DTCC slice and cumulative files.
//!
//! Synthetic data only (see `tests/common/mod.rs`).

#![allow(clippy::unwrap_used)]

mod common;

use std::io::Cursor;

use common::{DISSEMINATION_TS, Report, base_trade, zip_entries};
use mesh_types::{
    AssetClass, Flag, Notional, OptionStyle, OptionType, Quantity, Record, SwapAction,
    SwapEventType, SwapPrice, SwapPrint, UnixMillis, UnixNanos, allowlist,
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
fn spread_amount(s: &str) -> Option<SwapPrice> {
    Some(SwapPrice::SpreadAmount {
        value: mesh_types::Price::parse_exact(s).unwrap(),
    })
}
fn money(amount: &str, currency: &str) -> Option<Notional> {
    Some(Notional {
        amount: mesh_types::Size::parse_exact(amount).unwrap(),
        currency: currency.into(),
    })
}
fn qty(amount: &str, unit: &str) -> Option<Quantity> {
    Some(Quantity {
        amount: mesh_types::Size::parse_exact(amount).unwrap(),
        unit: unit.into(),
    })
}

/// True when no option term is set.
fn no_option_terms(p: &SwapPrint) -> bool {
    p.strike.is_none()
        && p.option_premium.is_none()
        && p.option_type.is_none()
        && p.option_style.is_none()
        && p.first_exercise_date.is_none()
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
        // Swaption: the strike (decimal notation) and premium have their
        // own fields (CFTC); the price is the underlying fixed rate.
        .row(&with(
            base_trade("9000000000000000003", "IR", exec, dissem),
            &[
                ("Effective Date", "2026-11-16"),
                ("Expiration Date", "2027-11-16"),
                ("First exercise date", "2027-11-12"),
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

    assert_eq!(ois.event_type, Some(SwapEventType::Trade));
    assert!(no_option_terms(ois));
    assert_eq!(ois.quantity, None);

    assert_eq!(p[1].price, rate("-0.125"));
    assert_eq!(p[1].tenor.as_deref(), Some("6M"));
    assert!(!p[1].prov.flags.contains(Flag::SizeCapped));

    let swaption = &p[2];
    assert_eq!(swaption.price, rate("4"));
    assert_eq!(swaption.strike, rate("4.25"));
    assert_eq!(swaption.option_type, Some(OptionType::Call));
    assert_eq!(swaption.option_style, Some(OptionStyle::European));
    assert_eq!(swaption.option_premium, money("1250000", "USD"));
    assert_eq!(swaption.first_exercise_date.as_deref(), Some("2027-11-12"));
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
        // An equity option reported to the SEC: option terms are not
        // admitted under ADM-0004, so only the price is used.
        .row(&with(
            base_trade("7000000000000000003", "EQ", exec, exec),
            &[
                ("Strike Price", "250"),
                ("Strike price notation", "1"),
                ("Option Type", "PUTO"),
                ("Option Style", "AMER"),
                ("Option Premium Amount", "1,000"),
                ("Option Premium Currency", "USD"),
                ("First exercise date", "2026-12-01"),
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
    for print in &p {
        assert!(no_option_terms(print), "{}", print.dissemination_id);
    }
    // Not read at all, so no field errors either.
    assert!(stats.field_errors.is_empty(), "{:?}", stats.field_errors);
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
                ("Option Style", "AMER"),
                ("Option Premium Amount", "85,000"),
                ("Option Premium Currency", "usd"),
                ("Unique Product Identifier", "QZSYNFXO0002"),
            ],
        ));
    let (p, stats) = parse(&report, &ctx(Regime::CftcPart43, DtccAsset::Forex));
    assert_eq!(stats.records, 2);
    check_all(&p);
    assert_eq!(p[0].asset_class, AssetClass::ForeignExchange);
    assert_eq!(p[0].price, price("83.1234"));
    assert_eq!(p[0].expiry.as_deref(), Some("2026-12-16"));
    assert!(no_option_terms(&p[0]));
    // The strike is no longer carried as the price.
    assert_eq!(p[1].price, None);
    assert_eq!(p[1].strike, price("1.085"));
    assert_eq!(p[1].option_type, Some(OptionType::Call));
    assert_eq!(p[1].option_style, Some(OptionStyle::American));
    assert_eq!(p[1].option_premium, money("85000", "USD"));
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
        // Commodity option: strike and premium in their own fields.
        .row(&with(
            base_trade("5000000000000000003", "CO", exec, exec),
            &[
                ("Product name", "Commodity:Energy:Oil:Option:Cash"),
                ("Strike Price", "75.5"),
                ("Strike price notation", "1"),
                ("Option Type", "PUTO"),
                ("Option Style", "ASIA"),
                ("Option Premium Amount", "120,000"),
                ("Option Premium Currency", "USD"),
                ("Total notional quantity-Leg 1", "100,000"),
                ("Quantity unit of measure-Leg 1", "BBL"),
                ("Underlier ID-Leg 1", "SYN CRUDE"),
            ],
        ))
        // Quantity on leg 2 only; leg 1 has a unit that is not a code.
        .row(&with(
            base_trade("5000000000000000004", "CO", exec, exec),
            &[
                ("Product name", "Commodity:Energy:Elec:Swap:Cash"),
                ("Total notional quantity-Leg 1", "1,200"),
                ("Quantity unit of measure-Leg 1", "MW h"),
                ("Total notional quantity-Leg 2", "2,400.5+"),
                ("Quantity unit of measure-Leg 2", "MWh"),
                ("Price", "45.10"),
                ("Price notation", "1"),
                ("Underlier ID-Leg 1", "SYN POWER"),
            ],
        ))
        // A quantity without a unit is dropped (and counted).
        .row(&with(
            base_trade("5000000000000000005", "CO", exec, exec),
            &[
                ("Product name", "Commodity:Energy:NatGas:Swap:Cash"),
                ("Total notional quantity-Leg 1", "10,000"),
                ("Price", "3.1"),
                ("Price notation", "1"),
                ("Underlier ID-Leg 1", "SYN HUB A"),
            ],
        ));
    let (p, stats) = parse(&report, &ctx(Regime::CftcPart43, DtccAsset::Commodities));
    assert_eq!(stats.records, 5);
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
    // The capped quantity is kept at the cap value, in its unit.
    assert_eq!(p[0].quantity, qty("50000", "MMBT"));
    // A monetary spread (notation 1) is a spread amount, not a price.
    assert_eq!(p[1].price, spread_amount("-0.25"));
    assert_eq!(p[1].quantity, None);
    assert_eq!(p[2].price, None);
    assert_eq!(p[2].strike, price("75.5"));
    assert_eq!(p[2].option_type, Some(OptionType::Put));
    assert_eq!(p[2].option_style, Some(OptionStyle::Asian));
    assert_eq!(p[2].option_premium, money("120000", "USD"));
    assert_eq!(p[2].quantity, qty("100000", "BBL"));
    assert_eq!(p[3].quantity, qty("2400.5", "MWh"));
    assert!(p[3].prov.flags.contains(Flag::SizeCapped));
    assert_eq!(p[4].quantity, None);
    assert_eq!(
        stats.field_errors.get("Quantity unit of measure-Leg 1"),
        Some(&2)
    );
    // Same instrument, same batch: indices increase per instrument.
    assert_eq!(p[0].prov.seq.event_index, 0);
    assert_eq!(p[1].prov.seq.event_index, 1);
    assert_eq!(p[2].prov.seq.event_index, 0);
    assert_eq!(p[4].prov.seq.event_index, 2);
}

#[test]
fn quantities_only_for_commodities_without_a_currency_notional() {
    let exec = "2026-11-12T16:45:00Z";
    let report = Report::standard()
        // A commodity swap with both: the currency notional wins.
        .row(&with(
            base_trade("5100000000000000001", "CO", exec, exec),
            &[
                ("Product name", "Commodity:Energy:NatGas:Swap:Cash"),
                ("Notional amount-Leg 1", "321,500"),
                ("Notional currency-Leg 1", "USD"),
                ("Total notional quantity-Leg 1", "100,000"),
                ("Quantity unit of measure-Leg 1", "MMBT"),
                ("Price", "3.215"),
                ("Price notation", "1"),
            ],
        ))
        // An equity swap with a share quantity: not a commodity, no
        // quantity.
        .row(&with(
            base_trade("5100000000000000002", "EQ", exec, exec),
            &[
                ("Total notional quantity-Leg 1", "2,000"),
                ("Quantity unit of measure-Leg 1", "SHAS"),
                ("Price", "1,234.5"),
                ("Price notation", "1"),
                ("Unique Product Identifier", "QZSYNEQS0201"),
            ],
        ));
    let (p, stats) = parse(&report, &ctx(Regime::CftcPart43, DtccAsset::Commodities));
    assert_eq!(stats.records, 2);
    check_all(&p);
    assert_eq!(p[0].notional, money("321500", "USD"));
    assert_eq!(p[0].quantity, None);
    assert_eq!(p[1].asset_class, AssetClass::Equity);
    assert_eq!(p[1].quantity, None);
}

#[test]
fn option_codes_strike_notations_and_bad_option_cells() {
    let exec = "2026-11-12T12:00:00Z";
    let upi = ("Unique Product Identifier", "QZSYNOPT0001");
    let report = Report::standard()
        // Cap with a basis-point strike and a Bermudan style; `PUT` as seen
        // in the ticker.
        .row(&with(
            base_trade("4000000000000000001", "IR", exec, exec),
            &[
                upi,
                ("Strike Price", "25"),
                ("Strike price notation", "4"),
                ("Option Type", "PUT"),
                ("Option Style", "BERM"),
            ],
        ))
        // Unmapped codes, a bad premium currency and a bad first exercise
        // date drop only those values.
        .row(&with(
            base_trade("4000000000000000002", "IR", exec, exec),
            &[
                upi,
                ("Fixed rate-Leg 1", "0.035"),
                ("Strike Price", "0.035"),
                ("Strike price notation", "2"),
                ("Option Type", "CHOO"),
                ("Option Style", "SPOT"),
                ("Option Premium Amount", "10,000"),
                ("Option Premium Currency", "US DOLLAR"),
                ("First exercise date", "soon"),
            ],
        ))
        // A strike with an unknown notation, and a negative premium.
        .row(&with(
            base_trade("4000000000000000003", "IR", exec, exec),
            &[
                upi,
                ("Strike Price", "1.5"),
                ("Strike price notation", "9"),
                ("Option Premium Amount", "-5"),
                ("Option Premium Currency", "USD"),
            ],
        ));
    let (p, stats) = parse(&report, &ctx(Regime::CftcPart43, DtccAsset::Rates));
    assert_eq!(stats.records, 3);
    check_all(&p);
    assert_eq!(p[0].strike, spread("25"));
    assert_eq!(p[0].price, None);
    assert_eq!(p[0].option_type, Some(OptionType::Put));
    assert_eq!(p[0].option_style, Some(OptionStyle::Bermudan));

    assert_eq!(p[1].price, rate("3.5"));
    assert_eq!(p[1].strike, rate("3.5"));
    assert_eq!(p[1].option_type, None);
    assert_eq!(p[1].option_style, None);
    assert_eq!(p[1].option_premium, None);
    assert_eq!(p[1].first_exercise_date, None);

    assert_eq!(p[2].strike, None);
    assert_eq!(p[2].option_premium, None);

    for (field, n) in [
        ("Option Type", 1),
        ("Option Style", 1),
        ("Option Premium Currency", 1),
        ("Option Premium Amount", 1),
        ("First exercise date", 1),
        ("Strike price notation", 1),
    ] {
        assert_eq!(stats.field_errors.get(field), Some(&n), "{field}");
    }
}

#[test]
fn cancels_corrections_and_lifecycle_actions() {
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
        // Modification of 100, with a new price.
        .row(&with(
            base_trade("104", "IR", exec, later),
            &[
                ("Action type", "MODI"),
                ("Original Dissemination Identifier", "100"),
                upi,
                ("Fixed rate-Leg 1", "0.0392"),
            ],
        ))
        // Early termination, priced, without an original ID (allowed for
        // `terminate`).
        .row(&with(
            base_trade("105", "IR", exec, later),
            &[
                ("Action type", "TERM"),
                ("Event type", "ETRM"),
                upi,
                ("Fixed rate-Leg 1", "0.0385"),
            ],
        ))
        // Revive without the original ID: rejected.
        .row(&with(
            base_trade("106", "IR", exec, later),
            &[("Action type", "REVI"), upi],
        ))
        // Revive of 100 (cancelled in error by 102).
        .row(&with(
            base_trade("107", "IR", exec, later),
            &[
                ("Action type", "REVI"),
                ("Event type", ""),
                ("Original Dissemination Identifier", "100"),
                upi,
                fixed,
            ],
        ))
        // A Part 45-only action type: skipped.
        .row(&with(
            base_trade("108", "IR", exec, later),
            &[("Action type", "PRTO"), upi],
        ))
        // Missing action type: rejected.
        .row(&with(
            base_trade("109", "IR", exec, later),
            &[("Action type", ""), upi],
        ))
        // A correction with an undocumented event type is kept, without
        // the event type.
        .row(&with(
            base_trade("110", "IR", exec, later),
            &[
                ("Action type", "CORR"),
                ("Event type", "ZZZZ"),
                ("Original Dissemination Identifier", "100"),
                upi,
                fixed,
            ],
        ));
    let (p, stats) = parse(&report, &ctx(Regime::CftcPart43, DtccAsset::Rates));
    check_all(&p);
    assert_eq!(stats.rows, 11);
    assert_eq!(stats.records, 7);
    assert_eq!(stats.row_errors, 3);
    assert_eq!(stats.skipped.get("unsupported_action"), Some(&1));
    assert_eq!(stats.skipped.len(), 1);
    assert_eq!(stats.field_errors.get("Event type"), Some(&1));

    assert_eq!(p[0].event_type, Some(SwapEventType::Trade));
    assert_eq!(p[1].action, SwapAction::Correct);
    assert_eq!(p[1].original_dissemination_id.as_deref(), Some("100"));
    assert_eq!(p[1].price, rate("3.91"));
    assert_eq!(p[1].event_type, None);
    assert_eq!(p[2].action, SwapAction::Cancel);
    assert_eq!(p[2].original_dissemination_id.as_deref(), Some("100"));
    assert_eq!(p[2].ts_dissemination_ms, ms(later));

    let modify = &p[3];
    assert_eq!(modify.dissemination_id, "104");
    assert_eq!(modify.action, SwapAction::Modify);
    assert_eq!(modify.original_dissemination_id.as_deref(), Some("100"));
    assert_eq!(modify.price, rate("3.92"));
    assert_eq!(modify.event_type, Some(SwapEventType::Trade));

    let terminate = &p[4];
    assert_eq!(terminate.dissemination_id, "105");
    assert_eq!(terminate.action, SwapAction::Terminate);
    assert_eq!(terminate.original_dissemination_id, None);
    assert_eq!(terminate.event_type, Some(SwapEventType::EarlyTermination));
    assert_eq!(terminate.price, rate("3.85"));

    let revive = &p[5];
    assert_eq!(revive.dissemination_id, "107");
    assert_eq!(revive.action, SwapAction::Revive);
    assert_eq!(revive.original_dissemination_id.as_deref(), Some("100"));
    assert_eq!(revive.event_type, None);

    assert_eq!(p[6].dissemination_id, "110");
    assert_eq!(p[6].action, SwapAction::Correct);
    assert_eq!(p[6].event_type, None);
}

#[test]
fn non_trade_events_are_emitted_with_their_event_type() {
    let exec = "2026-11-12T11:00:00Z";
    let upi = ("Unique Product Identifier", "QZSYN0000016");
    let fixed = ("Fixed rate-Leg 1", "0.039");
    let codes = [
        ("TRAD", Some(SwapEventType::Trade)),
        ("NOVA", Some(SwapEventType::Novation)),
        ("COMP", Some(SwapEventType::Compression)),
        ("ETRM", Some(SwapEventType::EarlyTermination)),
        ("CLRG", Some(SwapEventType::Clearing)),
        ("EXER", Some(SwapEventType::Exercise)),
        ("ALOC", Some(SwapEventType::Allocation)),
        ("CLAL", Some(SwapEventType::ClearingAllocation)),
        ("CREV", Some(SwapEventType::CreditEvent)),
        ("PTNG", Some(SwapEventType::Porting)),
        ("CORP", Some(SwapEventType::CorporateEvent)),
        ("updt", Some(SwapEventType::Update)),
        ("", None),
    ];
    const IDS: [&str; 13] = [
        "300", "301", "302", "303", "304", "305", "306", "307", "308", "309", "310", "311", "312",
    ];
    let mut report = Report::standard();
    for ((code, _), id) in codes.iter().zip(IDS) {
        report = report.row(&with(
            base_trade(id, "IR", exec, exec),
            &[("Event type", code), upi, fixed],
        ));
    }
    // A new message with an undocumented event type might not be a trade:
    // skipped and counted.
    report = report.row(&with(
        base_trade("399", "IR", exec, exec),
        &[("Event type", "ZZZZ"), upi, fixed],
    ));
    let (p, stats) = parse(&report, &ctx(Regime::CftcPart43, DtccAsset::Rates));
    check_all(&p);
    assert_eq!(stats.rows, 14);
    assert_eq!(stats.records, 13);
    assert_eq!(stats.skipped.get("unknown_event_type"), Some(&1));
    assert!(stats.field_errors.is_empty(), "{:?}", stats.field_errors);
    for (print, (code, want)) in p.iter().zip(codes) {
        assert_eq!(print.action, SwapAction::New, "{code}");
        assert_eq!(print.event_type, want, "{code}");
        assert_eq!(print.price, rate("3.9"), "{code}");
    }
    let text = String::from_utf8(
        mesh_types::hash::canonical_record(&Record::SwapPrint(p[3].clone())).unwrap(),
    )
    .unwrap();
    assert!(
        text.contains("\"event_type\":\"early_termination\""),
        "{text}"
    );
}

#[test]
fn sec_lifecycle_actions_and_event_types() {
    let exec = "2026-11-12T13:00:00Z";
    let upi = ("Unique Product Identifier", "QZSYNCDS0101");
    let spread_cells = [("Spread-Leg 1", "110"), ("Spread notation-Leg 1", "4")];
    let report = Report::standard()
        .row(&with(
            base_trade("200", "CR", exec, exec),
            &[upi, spread_cells[0], spread_cells[1]],
        ))
        .row(&with(
            base_trade("201", "CR", exec, exec),
            &[
                ("Action type", "TERM"),
                ("Event type", "CREV"),
                ("Original Dissemination Identifier", "200"),
                upi,
                spread_cells[0],
                spread_cells[1],
            ],
        ));
    let (p, stats) = parse(&report, &ctx(Regime::SecSbsr, DtccAsset::Credits));
    assert_eq!(stats.records, 2);
    check_all(&p);
    assert_eq!(p[1].action, SwapAction::Terminate);
    assert_eq!(p[1].event_type, Some(SwapEventType::CreditEvent));
    assert_eq!(p[1].price, spread("110"));
    assert_eq!(p[1].prov.admission.as_str(), "ADM-0004");
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
