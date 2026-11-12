//! Swap prints from public swap-data dissemination (CFTC Part 43 and SEC
//! Regulation SBSR).
//!
//! These sources are not chains, so [`Seq`](crate::Seq) is used as
//! `(batch, index)`: `block_number` is the source batch number (for
//! example, the sequence of the repository file or poll that delivered the
//! print) and `event_index` the position within it. `prov.venue` is the
//! public-data source (such as `cftc-part43`); `repository` is the swap
//! data repository that disseminated the print. `prov.ts_source_ms` is the
//! dissemination time.
//!
//! Capped notionals and delayed block trades are flagged with
//! [`Flag::SizeCapped`](crate::Flag::SizeCapped) and
//! [`Flag::BlockTradeDelayed`](crate::Flag::BlockTradeDelayed) in the
//! provenance flags.
//!
//! ADM-0002 and ADM-0004 are `Admitted`; `crates/swap-pollers` emits this
//! kind from the DTCC repository.

use serde::{Deserialize, Serialize};

use crate::decimal::{Price, Size};
use crate::ids::VenueId;
use crate::provenance::Provenance;
use crate::record::{RecordKind, ValidationError, ensure};
use crate::time::UnixMillis;

/// Swap asset class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetClass {
    /// Interest rates.
    InterestRate,
    /// Credit.
    Credit,
    /// Equity.
    Equity,
    /// Foreign exchange.
    ForeignExchange,
    /// Commodities.
    Commodity,
}

/// What a print does to the public record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwapAction {
    /// A new transaction.
    New,
    /// Cancels an earlier print (it was reported in error).
    Cancel,
    /// Corrects an earlier print.
    Correct,
    /// Amends the terms of an existing transaction (for example a price
    /// change); repository action `MODI`.
    Modify,
    /// Terminates an existing transaction, often with a price; repository
    /// action `TERM`.
    Terminate,
    /// Revives a print that was cancelled in error; repository action
    /// `REVI`.
    Revive,
}

impl SwapAction {
    /// True for actions that refer to an earlier print and therefore need
    /// `original_dissemination_id`.
    #[must_use]
    pub const fn needs_original(self) -> bool {
        matches!(self, Self::Cancel | Self::Correct | Self::Revive)
    }
}

/// The lifecycle event behind a print (CFTC technical specification event
/// types). Absent when the repository leaves it empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwapEventType {
    /// A trade (`TRAD`).
    Trade,
    /// A novation (`NOVA`).
    Novation,
    /// A compression (`COMP`).
    Compression,
    /// An early termination (`ETRM`).
    EarlyTermination,
    /// Clearing (`CLRG`).
    Clearing,
    /// An exercise (`EXER`).
    Exercise,
    /// An allocation (`ALOC`).
    Allocation,
    /// Clearing and allocation (`CLAL`).
    ClearingAllocation,
    /// A credit event (`CREV`).
    CreditEvent,
    /// Porting (`PTNG`).
    Porting,
    /// A corporate event (`CORP`).
    CorporateEvent,
    /// An update (`UPDT`).
    Update,
}

/// Option type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionType {
    /// A call (or payer swaption, cap).
    Call,
    /// A put (or receiver swaption, floor).
    Put,
}

/// Option exercise style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionStyle {
    /// Exercisable at any time (`AMER`).
    American,
    /// Exercisable at expiry only (`EURO`).
    European,
    /// Exercisable on set dates (`BERM`).
    Bermudan,
    /// Settled on an average (`ASIA`).
    Asian,
}

/// A quantity with a unit of measure, for notionals that are not amounts
/// of a currency (commodity swaps).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quantity {
    /// Amount as disseminated.
    pub amount: Size,
    /// Unit of measure as disseminated (for example `BBL`, `MMBTU`).
    pub unit: String,
}

/// The print's price, in the notation the repository used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SwapPrice {
    /// A price in currency units.
    Price {
        /// The value.
        value: Price,
    },
    /// A spread, in basis points.
    Spread {
        /// The value.
        value: Price,
    },
    /// A fixed rate, in percent.
    Rate {
        /// The value.
        value: Price,
    },
    /// A spread in currency units (repository notation 1, for example a
    /// commodity basis).
    SpreadAmount {
        /// The value.
        value: Price,
    },
}

/// Notional amount.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notional {
    /// Amount as disseminated (the cap value when capped).
    pub amount: Size,
    /// ISO 4217 currency code.
    pub currency: String,
}

/// One publicly disseminated swap transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwapPrint {
    /// Provenance. `ts_source_ms` is the dissemination time.
    pub prov: Provenance,
    /// Swap data repository that disseminated the print.
    pub repository: VenueId,
    /// The repository's identifier for this print.
    pub dissemination_id: String,
    /// The print a cancel, correction or revival refers to (and, when the
    /// repository gives it, a modification or termination).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_dissemination_id: Option<String>,
    /// Effect on the public record.
    pub action: SwapAction,
    /// Asset class.
    pub asset_class: AssetClass,
    /// Unique Product Identifier, if disseminated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upi: Option<String>,
    /// Product description or taxonomy, if disseminated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    /// Underlier (rate index, reference entity, asset), if disseminated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underlier: Option<String>,
    /// Tenor, such as `5Y`, if disseminated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenor: Option<String>,
    /// Expiration or end date, `YYYY-MM-DD`, if disseminated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiry: Option<String>,
    /// Price, spread or rate, if disseminated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<SwapPrice>,
    /// Notional, if disseminated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notional: Option<Notional>,
    /// Notional quantity with a unit (commodities), if disseminated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity: Option<Quantity>,
    /// Lifecycle event type, if disseminated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_type: Option<SwapEventType>,
    /// Option strike, if disseminated (options only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strike: Option<SwapPrice>,
    /// Option premium amount and currency, if disseminated. (`premium` is
    /// an excluded venue field name, so the field is `option_premium`.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_premium: Option<Notional>,
    /// Option type, if disseminated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_type: Option<OptionType>,
    /// Option exercise style, if disseminated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_style: Option<OptionStyle>,
    /// First exercise date, `YYYY-MM-DD`, if disseminated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_exercise_date: Option<String>,
    /// Execution time.
    pub ts_execution_ms: UnixMillis,
    /// Dissemination time (equals `prov.ts_source_ms`).
    pub ts_dissemination_ms: UnixMillis,
}

fn is_clean_text(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

impl SwapPrint {
    /// Checks internal consistency.
    ///
    /// # Errors
    ///
    /// Fails if a cancel or correct lacks the original ID, text fields are
    /// empty, too long or contain control characters, the currency is not
    /// three uppercase letters, the expiry is not `YYYY-MM-DD`, or the
    /// dissemination times disagree.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let k = RecordKind::SwapPrint;
        ensure(is_clean_text(&self.dissemination_id), k, || {
            "bad dissemination_id".into()
        })?;
        ensure(
            !self.action.needs_original() || self.original_dissemination_id.is_some(),
            k,
            || "cancel, correct and revive need original_dissemination_id".into(),
        )?;
        for (name, v) in [
            ("original_dissemination_id", &self.original_dissemination_id),
            ("upi", &self.upi),
            ("product", &self.product),
            ("underlier", &self.underlier),
            ("tenor", &self.tenor),
        ] {
            if let Some(v) = v {
                ensure(is_clean_text(v), k, || format!("bad {name}"))?;
            }
        }
        for (name, d) in [
            ("expiry", &self.expiry),
            ("first_exercise_date", &self.first_exercise_date),
        ] {
            if let Some(d) = d {
                ensure(is_date(d), k, || format!("{name} must be YYYY-MM-DD"))?;
            }
        }
        for n in [&self.notional, &self.option_premium].into_iter().flatten() {
            ensure(
                n.currency.len() == 3 && n.currency.bytes().all(|c| c.is_ascii_uppercase()),
                k,
                || "currency must be an ISO 4217 code".into(),
            )?;
        }
        if let Some(q) = &self.quantity {
            ensure(
                !q.unit.is_empty()
                    && q.unit.len() <= 16
                    && q.unit.bytes().all(|c| c.is_ascii_alphanumeric()),
                k,
                || "quantity unit must be 1-16 ASCII letters or digits".into(),
            )?;
        }
        ensure(
            self.ts_dissemination_ms == self.prov.ts_source_ms,
            k,
            || "ts_dissemination_ms must equal prov.ts_source_ms".into(),
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::provenance::tests::sample;
    use crate::record::tests::{px, sz};
    use crate::{Flag, Flags, Record};

    pub(crate) fn print() -> SwapPrint {
        let mut prov = sample("cftc-part43:swap:USD-SOFR-OIS.5Y", 3, 0);
        prov.venue = VenueId::new("cftc-part43").unwrap();
        prov.admission = crate::AdmissionId::new("ADM-0002").unwrap();
        prov.flags = Flags::NONE.with(Flag::SizeCapped);
        SwapPrint {
            ts_dissemination_ms: prov.ts_source_ms,
            prov,
            repository: VenueId::new("sdr-a").unwrap(),
            dissemination_id: "SYN-1".into(),
            original_dissemination_id: None,
            action: SwapAction::New,
            asset_class: AssetClass::InterestRate,
            upi: None,
            product: Some("InterestRate:IRSwap:OIS".into()),
            underlier: Some("USD-SOFR".into()),
            tenor: Some("5Y".into()),
            expiry: Some("2031-01-02".into()),
            price: Some(SwapPrice::Rate {
                value: px("3.8125"),
            }),
            notional: Some(Notional {
                amount: sz("250000000"),
                currency: "USD".into(),
            }),
            quantity: None,
            event_type: Some(SwapEventType::Trade),
            strike: None,
            option_premium: None,
            option_type: None,
            option_style: None,
            first_exercise_date: None,
            ts_execution_ms: UnixMillis(1_767_225_590_000),
        }
    }

    /// A commodity option print using every optional field added for
    /// lifecycle actions, options and quantities.
    pub(crate) fn option_print() -> SwapPrint {
        let mut p = print();
        p.asset_class = AssetClass::Commodity;
        p.action = SwapAction::Modify;
        p.original_dissemination_id = Some("SYN-0".into());
        p.event_type = Some(SwapEventType::Exercise);
        p.price = Some(SwapPrice::SpreadAmount { value: px("0.25") });
        p.notional = None;
        p.quantity = Some(Quantity {
            amount: sz("10000"),
            unit: "MMBTU".into(),
        });
        p.strike = Some(SwapPrice::Price { value: px("3.50") });
        p.option_premium = Some(Notional {
            amount: sz("1250.00"),
            currency: "USD".into(),
        });
        p.option_type = Some(OptionType::Call);
        p.option_style = Some(OptionStyle::European);
        p.first_exercise_date = Some("2027-01-15".into());
        p
    }

    #[test]
    fn round_trip_and_validate() {
        let r = Record::from(print());
        r.validate().unwrap();
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Record>(&json).unwrap(), r);

        let mut p = print();
        p.action = SwapAction::Cancel;
        assert!(p.validate().is_err());
        p.original_dissemination_id = Some("SYN-0".into());
        assert!(p.validate().is_ok());
        p.expiry = Some("2031-1-2".into());
        assert!(p.validate().is_err());
        let mut p = print();
        p.notional = Some(Notional {
            amount: sz("1"),
            currency: "usd".into(),
        });
        assert!(p.validate().is_err());

        let o = option_print();
        o.validate().unwrap();
        let r = Record::from(o.clone());
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Record>(&json).unwrap(), r);
        assert!(json.contains(r#""option_premium""#));
        let mut revive = print();
        revive.action = SwapAction::Revive;
        assert!(revive.validate().is_err());
        let mut terminate = print();
        terminate.action = SwapAction::Terminate;
        assert!(terminate.validate().is_ok());
        let mut bad = option_print();
        bad.quantity = Some(Quantity {
            amount: sz("1"),
            unit: "m 3".into(),
        });
        assert!(bad.validate().is_err());
    }
}
