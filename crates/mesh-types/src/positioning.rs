//! Futures positioning statistics published by a regulator
//! (observation-v0 § 2.14).
//!
//! A [`Positioning`] record is one trader category's open positions in one
//! market on one report date, as a regulator publishes them (the CFTC's
//! Commitments of Traders, ADM-0005). It is not derived and holds no
//! price: positions and open interest are the regulator's own aggregates
//! of mandatory position reports.
//!
//! - `prov.venue` is the publishing source and `prov.instrument` is
//!   `<source>:positioning:<contract market code>`; the symbol is the
//!   regulator's code for the market, so outputs name no exchange product.
//! - One record per report family, form (futures only or futures and
//!   options combined), category and report date. `prov.seq` is
//!   `(batch, index)` as for reference values.
//! - Combined (futures-and-options) figures convert option positions with
//!   exchange-supplied deltas, so they always carry the
//!   `exchange_referenced` flag (ADR 0012, decision 6); futures-only
//!   figures never do.
//! - A later publication for the same market, family, form, category and
//!   `as_of_date` with a higher `revision` supersedes an earlier one.

use serde::{Deserialize, Serialize};

use crate::flags::Flag;
use crate::provenance::Provenance;
use crate::record::{RecordKind, ValidationError, ensure};
use crate::reference::is_calendar_date;

/// Market class of positioning instruments
/// (`<source>:positioning:<code>`).
pub const POSITIONING_CLASS: &str = "positioning";

/// Report family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositioningReport {
    /// Legacy: commercial, non-commercial, nonreportable.
    Legacy,
    /// Disaggregated: producer/merchant, swap dealers, managed money,
    /// other reportables.
    Disaggregated,
    /// Traders in Financial Futures: dealers, asset managers, leveraged
    /// funds, other reportables.
    Tff,
    /// Supplemental: adds index traders (combined only).
    Supplemental,
}

/// Trader category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositioningCategory {
    /// Every reportable trader (all families).
    TotalReportable,
    /// Positions below the reporting levels (all families).
    Nonreportable,
    /// Legacy and supplemental non-commercial traders.
    Noncommercial,
    /// Legacy and supplemental commercial traders.
    Commercial,
    /// Supplemental index traders.
    IndexTraders,
    /// Disaggregated producer, merchant, processor or user.
    ProducerMerchant,
    /// Disaggregated swap dealers.
    SwapDealer,
    /// Disaggregated managed money.
    ManagedMoney,
    /// Disaggregated and TFF other reportables.
    OtherReportable,
    /// TFF dealers and intermediaries.
    Dealer,
    /// TFF asset managers and institutional investors.
    AssetManager,
    /// TFF leveraged funds.
    LeveragedFunds,
}

impl PositioningCategory {
    /// True if the category exists in `report`.
    #[must_use]
    pub const fn in_report(self, report: PositioningReport) -> bool {
        use PositioningCategory as C;
        use PositioningReport as R;
        match self {
            C::TotalReportable | C::Nonreportable => true,
            C::Noncommercial | C::Commercial => {
                matches!(report, R::Legacy | R::Supplemental)
            }
            C::IndexTraders => matches!(report, R::Supplemental),
            C::ProducerMerchant | C::SwapDealer | C::ManagedMoney => {
                matches!(report, R::Disaggregated)
            }
            C::OtherReportable => matches!(report, R::Disaggregated | R::Tff),
            C::Dealer | C::AssetManager | C::LeveragedFunds => matches!(report, R::Tff),
        }
    }
}

/// One category's positions in one market on one report date.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Positioning {
    /// Provenance (see the module docs).
    pub prov: Provenance,
    /// Report family.
    pub report: PositioningReport,
    /// True for futures-and-options-combined figures.
    pub combined: bool,
    /// The regulator's code for the market (the instrument symbol).
    pub contract_market_code: String,
    /// Report date (positions as of that day's close), `YYYY-MM-DD`.
    pub as_of_date: String,
    /// Trader category.
    pub category: PositioningCategory,
    /// Long positions, contracts (or futures equivalents when combined).
    pub long: u64,
    /// Short positions.
    pub short: u64,
    /// Spreading positions, for categories that report them.
    pub spreading: Option<u64>,
    /// The market's total open interest in the same form.
    pub open_interest: u64,
    /// Revision number: 0 for the first publication.
    pub revision: u32,
}

/// True for a contract market code: 1 to 12 ASCII uppercase letters or
/// digits.
#[must_use]
pub fn is_contract_market_code(s: &str) -> bool {
    (1..=12).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

impl Positioning {
    /// Checks internal consistency.
    ///
    /// # Errors
    ///
    /// Fails if the instrument class is not `positioning` or its symbol is
    /// not the contract market code, the code or date is malformed, the
    /// category does not exist in the report family, a supplemental record
    /// is futures-only, or `exchange_referenced` is not set exactly on
    /// combined records.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let k = RecordKind::Positioning;
        ensure(
            self.prov.instrument.class_part() == POSITIONING_CLASS,
            k,
            || "instrument class must be positioning".into(),
        )?;
        ensure(
            is_contract_market_code(&self.contract_market_code),
            k,
            || format!("bad contract market code {:?}", self.contract_market_code),
        )?;
        ensure(
            self.prov.instrument.symbol() == self.contract_market_code,
            k,
            || "instrument symbol must be the contract market code".into(),
        )?;
        ensure(is_calendar_date(&self.as_of_date), k, || {
            "as_of_date must be a YYYY-MM-DD date".into()
        })?;
        ensure(self.category.in_report(self.report), k, || {
            "category does not exist in this report family".into()
        })?;
        ensure(
            self.report != PositioningReport::Supplemental || self.combined,
            k,
            || "supplemental reports are combined only".into(),
        )?;
        ensure(
            self.prov.flags.contains(Flag::ExchangeReferenced) == self.combined,
            k,
            || "exchange_referenced goes on combined records exactly".into(),
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::provenance::tests::sample;
    use crate::{AdmissionId, Flags, InstrumentId, Record, VenueId};

    /// A synthetic futures-only record (invented source, code and counts).
    pub(crate) fn futures_only() -> Positioning {
        let mut prov = sample("example-cot:positioning:000001", 20_260_106, 0);
        prov.venue = VenueId::new("example-cot").unwrap();
        prov.admission = AdmissionId::new("ADM-9001").unwrap();
        Positioning {
            prov,
            report: PositioningReport::Disaggregated,
            combined: false,
            contract_market_code: "000001".into(),
            as_of_date: "2026-01-06".into(),
            category: PositioningCategory::ManagedMoney,
            long: 1200,
            short: 800,
            spreading: Some(150),
            open_interest: 10_000,
            revision: 0,
        }
    }

    /// A synthetic combined record.
    pub(crate) fn combined() -> Positioning {
        let mut r = futures_only();
        r.prov.seq.event_index = 1;
        r.prov.flags = Flags::NONE.with(Flag::ExchangeReferenced);
        r.report = PositioningReport::Legacy;
        r.combined = true;
        r.category = PositioningCategory::Nonreportable;
        r.spreading = None;
        r.open_interest = 12_500;
        r.revision = 1;
        r
    }

    pub(crate) fn samples() -> Vec<Record> {
        vec![futures_only().into(), combined().into()]
    }

    #[test]
    fn samples_validate_and_round_trip() {
        for r in samples() {
            r.validate().unwrap_or_else(|e| panic!("{e}"));
            let json = serde_json::to_string(&r).unwrap();
            assert_eq!(serde_json::from_str::<Record>(&json).unwrap(), r, "{json}");
            assert!(r.derived().is_none());
            assert!(!r.kind().is_derived());
            assert!(r.kind().is_analytics());
        }
        let json = serde_json::to_value(Record::from(combined())).unwrap();
        assert_eq!(json["kind"], "positioning");
        assert_eq!(json["spreading"], serde_json::Value::Null);
        assert_eq!(json["prov"]["flags"][0], "exchange_referenced");
    }

    #[test]
    fn invalid_records_fail() {
        let mut r = combined();
        r.prov.flags = Flags::NONE;
        assert!(r.validate().is_err(), "combined without the flag");
        let mut r = futures_only();
        r.prov.flags = Flags::NONE.with(Flag::ExchangeReferenced);
        assert!(r.validate().is_err(), "futures-only with the flag");
        let mut r = futures_only();
        r.category = PositioningCategory::Dealer;
        assert!(r.validate().is_err(), "TFF category in disaggregated");
        let mut r = futures_only();
        r.report = PositioningReport::Supplemental;
        r.category = PositioningCategory::IndexTraders;
        assert!(r.validate().is_err(), "futures-only supplemental");
        let mut r = futures_only();
        r.contract_market_code = "00000A".into();
        assert!(r.validate().is_err(), "symbol mismatch");
        let mut r = futures_only();
        r.prov.instrument = InstrumentId::new("example-cot:ref:000001").unwrap();
        assert!(r.validate().is_err(), "class");
        let mut r = futures_only();
        r.as_of_date = "2026-02-30".into();
        assert!(r.validate().is_err(), "date");
        assert!(!is_contract_market_code("abc"));
        assert!(!is_contract_market_code(""));
        assert!(is_contract_market_code("13874A"));
    }
}
