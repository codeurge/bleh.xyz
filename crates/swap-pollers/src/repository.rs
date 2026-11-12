//! Repositories, regimes and the report buckets each one publishes.
//!
//! Only repositories admitted under ADM-0002 (CFTC Part 43) and ADM-0004
//! (SEC Regulation SBSR) are defined here. Both admissions exclude
//! repositories operated by an exchange group, so ICE Trade Vault is not
//! implemented (see `docs/specs/swap-repositories.md`). Canadian buckets on
//! the same DTCC site are not admitted and cannot be configured.

use std::fmt;
use std::str::FromStr;

use mesh_types::{AdmissionId, AssetClass, VenueId};

/// Regulatory regime of a public dissemination stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Regime {
    /// CFTC Part 43 real-time public reporting.
    CftcPart43,
    /// SEC Regulation SBSR public dissemination.
    SecSbsr,
}

impl Regime {
    /// The mesh venue (public-data source) for records of this regime.
    #[must_use]
    pub const fn venue_str(self) -> &'static str {
        match self {
            Regime::CftcPart43 => "cftc-part43",
            Regime::SecSbsr => "sec-sbsr",
        }
    }

    /// The mesh venue ID.
    ///
    /// # Panics
    ///
    /// Never: the literals are valid venue IDs (checked by a test).
    #[must_use]
    pub fn venue(self) -> VenueId {
        VenueId::new(self.venue_str()).unwrap_or_else(|_| unreachable!("valid venue literal"))
    }

    /// The admission record covering this regime's fields.
    #[must_use]
    pub fn admission(self) -> AdmissionId {
        let id = match self {
            Regime::CftcPart43 => "ADM-0002",
            Regime::SecSbsr => "ADM-0004",
        };
        AdmissionId::new(id).unwrap_or_else(|_| unreachable!("valid admission literal"))
    }

    /// The prefix DTCC uses in bucket keys and file names (`CFTC`, `SEC`).
    #[must_use]
    pub const fn dtcc_prefix(self) -> &'static str {
        match self {
            Regime::CftcPart43 => "CFTC",
            Regime::SecSbsr => "SEC",
        }
    }

    /// The lower-case path segment DTCC uses in object keys.
    #[must_use]
    pub const fn dtcc_path(self) -> &'static str {
        match self {
            Regime::CftcPart43 => "cftc",
            Regime::SecSbsr => "sec",
        }
    }
}

impl fmt::Display for Regime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.venue_str())
    }
}

/// A swap data repository implemented by this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Repository {
    /// DTCC Data Repository (U.S.) LLC: CFTC SDR and SEC SBSDR, one
    /// public price dissemination site for both.
    DtccDdr,
}

impl Repository {
    /// Repository ID carried in `SwapPrint.repository`.
    #[must_use]
    pub const fn id_str(self) -> &'static str {
        match self {
            Repository::DtccDdr => "dtcc-ddr",
        }
    }

    /// Repository ID as a [`VenueId`].
    #[must_use]
    pub fn id(self) -> VenueId {
        VenueId::new(self.id_str()).unwrap_or_else(|_| unreachable!("valid repository literal"))
    }
}

/// One DTCC report bucket: a regime and an asset class, such as
/// `CFTC_IR`. DTCC keys its listings by these names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DtccBucket {
    /// Regime.
    pub regime: Regime,
    /// Asset class.
    pub asset: DtccAsset,
}

/// DTCC asset-class codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DtccAsset {
    /// `IR`, file word `RATES`.
    Rates,
    /// `CR`, file word `CREDITS`.
    Credits,
    /// `EQ`, file word `EQUITIES`.
    Equities,
    /// `FX`, file word `FOREX`.
    Forex,
    /// `CO`, file word `COMMODITIES`.
    Commodities,
}

impl DtccAsset {
    /// Every asset code.
    pub const ALL: [DtccAsset; 5] = [
        DtccAsset::Rates,
        DtccAsset::Credits,
        DtccAsset::Equities,
        DtccAsset::Forex,
        DtccAsset::Commodities,
    ];

    /// Two-letter code used in listing keys and the `Asset Class` column.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            DtccAsset::Rates => "IR",
            DtccAsset::Credits => "CR",
            DtccAsset::Equities => "EQ",
            DtccAsset::Forex => "FX",
            DtccAsset::Commodities => "CO",
        }
    }

    /// Word used in file names (`CFTC_SLICE_RATES_...`).
    #[must_use]
    pub const fn file_word(self) -> &'static str {
        match self {
            DtccAsset::Rates => "RATES",
            DtccAsset::Credits => "CREDITS",
            DtccAsset::Equities => "EQUITIES",
            DtccAsset::Forex => "FOREX",
            DtccAsset::Commodities => "COMMODITIES",
        }
    }

    /// The mesh asset class.
    #[must_use]
    pub const fn asset_class(self) -> AssetClass {
        match self {
            DtccAsset::Rates => AssetClass::InterestRate,
            DtccAsset::Credits => AssetClass::Credit,
            DtccAsset::Equities => AssetClass::Equity,
            DtccAsset::Forex => AssetClass::ForeignExchange,
            DtccAsset::Commodities => AssetClass::Commodity,
        }
    }

    /// Parses a two-letter code or a file word, ignoring case.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|a| a.code().eq_ignore_ascii_case(s) || a.file_word().eq_ignore_ascii_case(s))
    }
}

/// Error for an unknown or unadmitted bucket name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "unknown or unadmitted DTCC bucket {0:?} (expected CFTC_IR, CFTC_CR, CFTC_EQ, CFTC_FX, CFTC_CO, SEC_IR, SEC_CR or SEC_EQ)"
)]
pub struct BucketError(pub String);

impl DtccBucket {
    /// Every admitted bucket: five CFTC asset classes and the three SEC
    /// asset classes DTCC publishes.
    pub const ADMITTED: [DtccBucket; 8] = [
        DtccBucket::new(Regime::CftcPart43, DtccAsset::Rates),
        DtccBucket::new(Regime::CftcPart43, DtccAsset::Credits),
        DtccBucket::new(Regime::CftcPart43, DtccAsset::Equities),
        DtccBucket::new(Regime::CftcPart43, DtccAsset::Forex),
        DtccBucket::new(Regime::CftcPart43, DtccAsset::Commodities),
        DtccBucket::new(Regime::SecSbsr, DtccAsset::Rates),
        DtccBucket::new(Regime::SecSbsr, DtccAsset::Credits),
        DtccBucket::new(Regime::SecSbsr, DtccAsset::Equities),
    ];

    /// Builds a bucket.
    #[must_use]
    pub const fn new(regime: Regime, asset: DtccAsset) -> Self {
        Self { regime, asset }
    }

    /// Listing key, such as `CFTC_IR`.
    #[must_use]
    pub fn key(self) -> String {
        format!("{}_{}", self.regime.dtcc_prefix(), self.asset.code())
    }

    /// Infers the bucket from a DTCC file name such as
    /// `SEC_SLICE_CREDITS_2026_11_12_2.zip` or
    /// `CFTC_CUMULATIVE_RATES_2026_11_11.zip`.
    #[must_use]
    pub fn from_file_name(name: &str) -> Option<Self> {
        let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
        let mut parts = base.split('_');
        let regime = match parts.next()? {
            "CFTC" => Regime::CftcPart43,
            "SEC" => Regime::SecSbsr,
            _ => return None,
        };
        let _kind = parts.next()?;
        let asset = DtccAsset::parse(parts.next()?)?;
        let bucket = Self::new(regime, asset);
        Self::ADMITTED.contains(&bucket).then_some(bucket)
    }
}

impl FromStr for DtccBucket {
    type Err = BucketError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || BucketError(s.to_string());
        let (prefix, code) = s.split_once('_').ok_or_else(err)?;
        let regime = match prefix.to_ascii_uppercase().as_str() {
            "CFTC" => Regime::CftcPart43,
            "SEC" => Regime::SecSbsr,
            _ => return Err(err()),
        };
        let asset = DtccAsset::parse(code).ok_or_else(err)?;
        let bucket = Self::new(regime, asset);
        if Self::ADMITTED.contains(&bucket) {
            Ok(bucket)
        } else {
            Err(err())
        }
    }
}

impl fmt::Display for DtccBucket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_are_valid() {
        assert_eq!(Regime::CftcPart43.venue().as_str(), "cftc-part43");
        assert_eq!(Regime::SecSbsr.venue().as_str(), "sec-sbsr");
        assert_eq!(Regime::CftcPart43.admission().as_str(), "ADM-0002");
        assert_eq!(Regime::SecSbsr.admission().as_str(), "ADM-0004");
        assert_eq!(Repository::DtccDdr.id().as_str(), "dtcc-ddr");
    }

    #[test]
    fn bucket_names() {
        for b in DtccBucket::ADMITTED {
            assert_eq!(b.key().parse::<DtccBucket>().unwrap(), b);
        }
        assert!("CA_IR".parse::<DtccBucket>().is_err());
        assert!("SEC_FX".parse::<DtccBucket>().is_err());
        assert!("SEC_CO".parse::<DtccBucket>().is_err());
        assert_eq!(
            DtccBucket::from_file_name("SEC_SLICE_CREDITS_2026_11_12_2.zip"),
            Some(DtccBucket::new(Regime::SecSbsr, DtccAsset::Credits))
        );
        assert_eq!(
            DtccBucket::from_file_name("/x/CFTC_CUMULATIVE_FOREX_2026_11_11.zip"),
            Some(DtccBucket::new(Regime::CftcPart43, DtccAsset::Forex))
        );
        assert_eq!(
            DtccBucket::from_file_name("CA_SLICE_RATES_2026_11_12_1.zip"),
            None
        );
    }
}
