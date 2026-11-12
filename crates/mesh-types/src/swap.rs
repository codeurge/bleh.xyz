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
//! This kind exists so the schema is ready for ADM-0002. No reader emits it
//! until that admission is `Admitted`.

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
    /// Cancels an earlier print.
    Cancel,
    /// Corrects an earlier print.
    Correct,
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
    /// For cancels and corrections: the print being cancelled or corrected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_dissemination_id: Option<String>,
    /// New, cancel or correct.
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
            self.action == SwapAction::New || self.original_dissemination_id.is_some(),
            k,
            || "cancel and correct need original_dissemination_id".into(),
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
        if let Some(e) = &self.expiry {
            ensure(is_date(e), k, || "expiry must be YYYY-MM-DD".into())?;
        }
        if let Some(n) = &self.notional {
            ensure(
                n.currency.len() == 3 && n.currency.bytes().all(|c| c.is_ascii_uppercase()),
                k,
                || "currency must be an ISO 4217 code".into(),
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
            ts_execution_ms: UnixMillis(1_767_225_590_000),
        }
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
    }
}
