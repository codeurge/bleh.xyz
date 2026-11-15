//! The admitted sources: identity, admission, notices, history start and
//! poll schedule.
//!
//! Only sources whose admission record is `Admitted` appear here, and each
//! source module reads only that record's admitted channels and fields.
//! Rejected or pending sources (CAISO, PJM, MISO, ISO New England, SPP,
//! NYISO; USDA grain prices; the MyMarketNews API; the ERCOT API;
//! treasurydirect.gov; the EIA and BLS APIs, which are for checks only;
//! BLS producer and import/export price indexes) have no reader.

use std::fmt;
use std::str::FromStr;

use mesh_types::{AdmissionId, SourceNotice, VenueId};

use crate::time::Date;

/// An admitted source the pollers read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceId {
    /// CFTC Commitments of Traders (ADM-0005).
    Cot,
    /// Treasury daily interest rate statistics (ADM-0006).
    TreasuryRates,
    /// Treasury auction results from Fiscal Data (ADM-0006).
    TreasuryAuctions,
    /// New York Fed reference rates (ADM-0007).
    NyFed,
    /// Federal Reserve Board H.15 (ADM-0008).
    FrbH15,
    /// Federal Reserve Board H.10 (ADM-0009).
    FrbH10,
    /// USDA Livestock Mandatory Reporting (ADM-0010).
    UsdaLmr,
    /// USDA dairy NDPSR and announced class prices (ADM-0011).
    UsdaDairy,
    /// USDA Market News feeder cattle report files (ADM-0012).
    UsdaFeeder,
    /// ERCOT public website downloads (ADM-0013).
    Ercot,
    /// EIA energy statistics from EIA's own surveys, bulk files (ADM-0021).
    Eia,
    /// BLS consumer price indexes and average prices, bulk files
    /// (ADM-0022).
    Bls,
}

impl SourceId {
    /// Every source, in a fixed order.
    pub const ALL: [SourceId; 12] = [
        SourceId::Cot,
        SourceId::TreasuryRates,
        SourceId::TreasuryAuctions,
        SourceId::NyFed,
        SourceId::FrbH15,
        SourceId::FrbH10,
        SourceId::UsdaLmr,
        SourceId::UsdaDairy,
        SourceId::UsdaFeeder,
        SourceId::Ercot,
        SourceId::Eia,
        SourceId::Bls,
    ];

    /// Command-line and state name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Cot => "cot",
            Self::TreasuryRates => "treasury-rates",
            Self::TreasuryAuctions => "treasury-auctions",
            Self::NyFed => "nyfed",
            Self::FrbH15 => "frb-h15",
            Self::FrbH10 => "frb-h10",
            Self::UsdaLmr => "usda-lmr",
            Self::UsdaDairy => "usda-dairy",
            Self::UsdaFeeder => "usda-feeder",
            Self::Ercot => "ercot",
            Self::Eia => "eia",
            Self::Bls => "bls",
        }
    }

    /// The records' `prov.venue`.
    #[must_use]
    pub const fn venue_str(self) -> &'static str {
        match self {
            Self::Cot => "cftc-cot",
            Self::TreasuryRates | Self::TreasuryAuctions => "us-treasury",
            Self::NyFed => "nyfed",
            Self::FrbH15 => "frb-h15",
            Self::FrbH10 => "frb-h10",
            Self::UsdaLmr => "usda-lmr",
            Self::UsdaDairy => "usda-dairy",
            Self::UsdaFeeder => "usda-mn",
            Self::Ercot => "ercot",
            Self::Eia => "eia",
            Self::Bls => "bls",
        }
    }

    /// The records' `prov.venue` as an ID.
    #[must_use]
    pub fn venue(self) -> VenueId {
        VenueId::new(self.venue_str()).unwrap_or_else(|_| unreachable_venue())
    }

    /// The admission covering the source's records.
    #[must_use]
    pub const fn admission_str(self) -> &'static str {
        match self {
            Self::Cot => "ADM-0005",
            Self::TreasuryRates | Self::TreasuryAuctions => "ADM-0006",
            Self::NyFed => "ADM-0007",
            Self::FrbH15 => "ADM-0008",
            Self::FrbH10 => "ADM-0009",
            Self::UsdaLmr => "ADM-0010",
            Self::UsdaDairy => "ADM-0011",
            Self::UsdaFeeder => "ADM-0012",
            Self::Ercot => "ADM-0013",
            Self::Eia => "ADM-0021",
            Self::Bls => "ADM-0022",
        }
    }

    /// The admission as an ID.
    #[must_use]
    pub fn admission(self) -> AdmissionId {
        AdmissionId::new(self.admission_str()).unwrap_or_else(|_| unreachable_admission())
    }

    /// The earliest date the admitted channels hold, where backfill starts.
    /// `None` when the admitted channel has no history (feeder cattle: the
    /// report files hold only the latest issue, and the archive channel is
    /// not admitted).
    #[must_use]
    pub fn earliest(self) -> Option<Date> {
        let (y, m, d) = match self {
            Self::Cot => (1986, 1, 15),
            Self::TreasuryRates => (1990, 1, 2),
            Self::TreasuryAuctions => (1979, 11, 15),
            Self::NyFed => (2000, 7, 3),
            Self::FrbH15 => (1954, 1, 4),
            Self::FrbH10 => (1971, 1, 4),
            Self::UsdaLmr => (2001, 4, 2),
            Self::UsdaDairy => (2000, 1, 1),
            Self::UsdaFeeder => return None,
            Self::Ercot => (2010, 1, 1),
            // The earliest admitted price series (crude first purchase
            // prices, monthly); volumes start earlier but are not emitted.
            Self::Eia => (1974, 1, 1),
            Self::Bls => (1913, 1, 1),
        };
        Date::new(y, m, d)
    }

    /// Days a publication can precede the date its values are for (the
    /// grid operator posts day-ahead prices the day before delivery). The
    /// backfill keeps partitions open this long before a unit's start.
    #[must_use]
    pub const fn max_lead_days(self) -> i64 {
        match self {
            Self::Ercot => 1,
            _ => 0,
        }
    }

    /// Notices that travel with the source's records (registry entry and
    /// manifest). Required notices come from the admission record; the
    /// rest are the source credits the publishers' policies request.
    #[must_use]
    pub fn notices(self) -> Vec<SourceNotice> {
        let text = match self {
            Self::Cot => CFTC_CREDIT.to_owned(),
            Self::TreasuryRates | Self::TreasuryAuctions => TREASURY_CREDIT.to_owned(),
            Self::NyFed => nyfed_notice(NYFED_RATES_PHRASE),
            Self::FrbH15 => format!(
                "{BOARD_CREDIT} {}",
                nyfed_notice("The Effective Federal Funds Rate (EFFR)")
            ),
            Self::FrbH10 => BOARD_CREDIT.to_owned(),
            Self::UsdaLmr | Self::UsdaDairy | Self::UsdaFeeder => USDA_CREDIT.to_owned(),
            Self::Ercot => ERCOT_CREDIT.to_owned(),
            Self::Eia => EIA_NOTICE.to_owned(),
            Self::Bls => BLS_NOTICE.to_owned(),
        };
        vec![SourceNotice {
            admission: self.admission(),
            source: self.venue(),
            notice: text,
        }]
    }
}

fn unreachable_venue() -> VenueId {
    // Every venue string above is a valid ID; keep a total fallback.
    VenueId::new("unknown").unwrap_or_else(|_| unreachable_venue())
}

fn unreachable_admission() -> AdmissionId {
    AdmissionId::new("ADM-0000").unwrap_or_else(|_| unreachable_admission())
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for SourceId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|x| x.name() == s.trim())
            .ok_or_else(|| {
                let names: Vec<&str> = Self::ALL.iter().map(|x| x.name()).collect();
                format!("unknown source {s:?}; one of {}", names.join(", "))
            })
    }
}

/// The rates named in the New York Fed notice for this archive.
pub const NYFED_RATES_PHRASE: &str = "The Secured Overnight Financing Rate (SOFR), the Broad General Collateral Rate (BGCR), the Tri-Party General Collateral Rate (TGCR), the SOFR Averages and SOFR Index, the Effective Federal Funds Rate (EFFR) and the Overnight Bank Funding Rate (OBFR)";

/// The SOFR and BGCR disclaimer, verbatim from the New York Fed's Terms of
/// Use (newyorkfed.org/privacy/termsofuse, reference rates section),
/// required with SOFR and BGCR data and anything computed from SOFR
/// (ADM-0007).
pub const NYFED_DTCC_DISCLAIMER: &str = "The Secured Overnight Financing Rate (SOFR) Data and Broad General Collateral Rate (BGCR) Data are calculated using data provided under a license granted to the New York Fed by DTCC Solutions LLC (\u{201c}Solutions\u{201d}), an affiliate of The Depository Trust & Clearing Corporation. Solutions, its affiliates, and third parties from which they obtained data have no liability for the content of this material.";

/// The New York Fed reference-rates notice (ADM-0007, from the Terms of
/// Use, completed with the rate names and the publisher), for `rates` (a
/// phrase naming one or more rates, used as the grammatical subject).
/// Rates that are or derive from SOFR or BGCR also carry
/// [`NYFED_DTCC_DISCLAIMER`].
#[must_use]
pub fn nyfed_notice(rates: &str) -> String {
    let plural = rates.contains(" and ");
    let (verb, them) = if plural {
        ("are", "these rates")
    } else {
        ("is", "this rate")
    };
    let dtcc = if rates.contains("SOFR") || rates.contains("BGCR") {
        format!(" {NYFED_DTCC_DISCLAIMER}")
    } else {
        String::new()
    };
    format!(
        "{rates} {verb} subject to the Terms of Use posted at newyorkfed.org. The New York Fed is not responsible for publication of {them} by bleh.xyz, does not sanction or endorse any particular republication, and has no liability for your use.{dtcc} bleh.xyz is not affiliated with the New York Fed, and the New York Fed does not sanction, endorse, or recommend any products or services offered by bleh.xyz."
    )
}

/// Credit requested by the CFTC web policy.
pub const CFTC_CREDIT: &str =
    "Source: U.S. Commodity Futures Trading Commission, Commitments of Traders reports.";
/// Credit for Treasury data (no notice is required).
pub const TREASURY_CREDIT: &str =
    "Source: U.S. Department of the Treasury (home.treasury.gov and Fiscal Data).";
/// Credit requested by the Board's website policies.
pub const BOARD_CREDIT: &str = "Source: Board of Governors of the Federal Reserve System.";
/// Credit for USDA data, with the DataMart's no-alteration condition.
pub const USDA_CREDIT: &str = "Source: USDA Agricultural Marketing Service. Values presented as USDA data are carried unaltered; values derived by bleh.xyz are labelled as such.";
/// Credit for ERCOT data (raw data may be redistributed without notices).
pub const ERCOT_CREDIT: &str = "Source: ERCOT public market information (www.ercot.com).";

/// The EIA acknowledgement (ADM-0021): EIA's reuse page asks for an
/// acknowledgement with the publication date, and the API terms ask to
/// name EIA, not to imply endorsement and not to present modified content
/// as EIA's.
pub const EIA_NOTICE: &str = "Source: U.S. Energy Information Administration (EIA), from EIA's own surveys, read from the EIA bulk download files (www.eia.gov/opendata). The publication date of each value is its record's ts_source_ms (estimated from EIA's release schedule on records flagged backfill). Values are carried as EIA published them; values derived by bleh.xyz are labelled as such. bleh.xyz is not affiliated with EIA, and EIA does not endorse or approve any product or service of bleh.xyz.";
/// The BLS citation, retrieval date and disclaimer (ADM-0022): BLS asks to
/// be cited, and the BLS.gov terms ask users to cite the retrieval date and
/// to state the disclaimer verbatim.
pub const BLS_NOTICE: &str = "Source: U.S. Bureau of Labor Statistics, Consumer Price Index and Average Price data (download.bls.gov/pub/time.series). Retrieved from BLS.gov: live records were retrieved at their ts_ingest_ns; records flagged backfill were retrieved no later than the archive manifest that first lists them. BLS.gov cannot vouch for the data or analyses derived from these data after the data have been retrieved from BLS.gov.";

/// Every source's notices, sorted and unique, for an archive's
/// `_notices.json`.
#[must_use]
pub fn all_notices(sources: &[SourceId]) -> Vec<SourceNotice> {
    let mut v: Vec<SourceNotice> = sources.iter().flat_map(|s| s.notices()).collect();
    v.sort();
    v.dedup_by(|a, b| a.admission == b.admission && a.source == b.source);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_are_valid() {
        for s in SourceId::ALL {
            assert_eq!(s.name().parse::<SourceId>(), Ok(s));
            let _ = s.venue();
            let _ = s.admission();
            for n in s.notices() {
                assert!(!n.notice.is_empty() && n.notice.len() <= 4096);
                assert!(!n.notice.chars().any(char::is_control));
            }
        }
        assert!("pjm".parse::<SourceId>().is_err());
        assert!(SourceId::UsdaFeeder.earliest().is_none());
        let n = all_notices(&SourceId::ALL);
        // Treasury rates and auctions share one admission and venue.
        assert_eq!(n.len(), SourceId::ALL.len() - 1);
        assert!(
            n.windows(2)
                .all(|w| (&w[0].admission, &w[0].source) < (&w[1].admission, &w[1].source))
        );
        let ny = &SourceId::NyFed.notices()[0].notice;
        assert!(ny.contains("subject to the Terms of Use posted at newyorkfed.org"));
        assert!(ny.contains("by bleh.xyz"));
        assert!(SourceId::FrbH15.notices()[0].notice.contains("EFFR"));
        // The DTCC disclaimer goes with SOFR and BGCR (and SOFR-derived
        // rates), not with EFFR on its own.
        assert!(ny.contains(NYFED_DTCC_DISCLAIMER));
        assert!(nyfed_notice("The SOFR Averages and SOFR Index").contains(NYFED_DTCC_DISCLAIMER));
        assert!(!nyfed_notice("The Effective Federal Funds Rate (EFFR)").contains("DTCC"));
        // ADM-0022: citation, retrieval date and the verbatim disclaimer.
        let bls = &SourceId::Bls.notices()[0].notice;
        assert!(bls.contains("Bureau of Labor Statistics") && bls.contains("Retrieved"));
        assert!(bls.contains("BLS.gov cannot vouch for the data or analyses derived from these data after the data have been retrieved from BLS.gov."));
        // ADM-0021: EIA named, publication date, no endorsement implied.
        let eia = &SourceId::Eia.notices()[0].notice;
        assert!(
            eia.contains("Energy Information Administration") && eia.contains("publication date")
        );
        assert!(eia.contains("does not endorse"));
    }
}
