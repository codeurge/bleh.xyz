//! Identifier newtypes.
//!
//! String identifiers are validated on construction and on deserialization,
//! so a value of these types is always well-formed.
//!
//! # Instrument IDs
//!
//! An [`InstrumentId`] names one market on one venue:
//!
//! ```text
//! instrument = venue ":" class ":" symbol
//! venue      = [a-z0-9] [a-z0-9-]{0,31}              (a VenueId)
//! class      = [a-z0-9] [a-z0-9-]{0,15}              (perp, spot, outcome, swap, ...)
//! symbol     = segment *( ":" segment )
//! segment    = 1*( ALPHA / DIGIT / "_" / "." / "@" / "#" / "+" / "-" )
//! ```
//!
//! at most 128 bytes. The symbol is the venue's own market name, so any
//! market in the venue's metadata can be represented without a hard-coded
//! list: a core perpetual (`hyperliquid:perp:BTC`), a HIP-3 deployer market
//! with its namespace (`hyperliquid:perp:xyz:CL`), a spot pair
//! (`hyperliquid:spot:@107`) or an outcome market
//! (`hyperliquid:outcome:#9001`). `/`, `=`, `%`, `~`, whitespace and quotes
//! are excluded so IDs are safe in Hive-style partition paths, where `:`
//! is written as `~` (see [`InstrumentId::path_segment`]).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Error from validating an identifier.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {kind} {value:?}: {reason}")]
pub struct IdError {
    kind: &'static str,
    value: String,
    reason: &'static str,
}

impl IdError {
    fn new(kind: &'static str, value: &str, reason: &'static str) -> Self {
        Self {
            kind,
            value: value.chars().take(140).collect(),
            reason,
        }
    }
}

fn is_slug(s: &str, max: usize) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= max
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

fn is_symbol_segment(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'@' | b'#' | b'+' | b'-')
        })
}

macro_rules! string_id {
    ($(#[$doc:meta])* $name:ident, $kind:literal, $validate:expr) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// Validates and wraps an identifier.
            ///
            /// # Errors
            ///
            /// Fails if the string does not match the identifier's grammar.
            pub fn new(value: impl Into<String>) -> Result<Self, IdError> {
                let value = value.into();
                let check: fn(&str) -> Result<(), &'static str> = $validate;
                check(&value).map_err(|reason| IdError::new($kind, &value, reason))?;
                Ok(Self(value))
            }

            /// The identifier as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = IdError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::new(s)
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdError;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

string_id!(
    /// A venue or public-data source, such as `hyperliquid`.
    ///
    /// Grammar: `[a-z0-9][a-z0-9-]{0,31}`.
    VenueId,
    "venue id",
    |s| if is_slug(s, 32) { Ok(()) } else { Err("expected [a-z0-9][a-z0-9-]{0,31}") }
);

string_id!(
    /// A mesh instrument: one market on one venue. See the module docs for
    /// the grammar.
    InstrumentId,
    "instrument id",
    validate_instrument
);

string_id!(
    /// An admission decision, such as `ADM-0001`. Grammar: `ADM-[0-9]{4,}`.
    AdmissionId,
    "admission id",
    |s| match s.strip_prefix("ADM-") {
        Some(n) if n.len() >= 4 && n.len() <= 9 && n.bytes().all(|c| c.is_ascii_digit()) => Ok(()),
        _ => Err("expected ADM-NNNN"),
    }
);

string_id!(
    /// A versioned method, such as `bars@1`. Grammar:
    /// `[a-z0-9][a-z0-9-]{0,47}@[1-9][0-9]{0,5}`.
    MethodId,
    "method id",
    |s| match s.split_once('@') {
        Some((name, ver))
            if is_slug(name, 48)
                && !ver.is_empty()
                && ver.len() <= 6
                && !ver.starts_with('0')
                && ver.bytes().all(|c| c.is_ascii_digit()) =>
        {
            Ok(())
        }
        _ => Err("expected name@version"),
    }
);

fn validate_instrument(s: &str) -> Result<(), &'static str> {
    if s.len() > 128 {
        return Err("longer than 128 bytes");
    }
    let mut parts = s.splitn(3, ':');
    let venue = parts.next().unwrap_or("");
    let class = parts.next().ok_or("expected venue:class:symbol")?;
    let symbol = parts.next().ok_or("expected venue:class:symbol")?;
    if !is_slug(venue, 32) {
        return Err("venue part must be [a-z0-9][a-z0-9-]{0,31}");
    }
    if !is_slug(class, 16) {
        return Err("class part must be [a-z0-9][a-z0-9-]{0,15}");
    }
    if !symbol.split(':').all(is_symbol_segment) {
        return Err("symbol segments must be non-empty [A-Za-z0-9_.@#+-]");
    }
    Ok(())
}

impl InstrumentId {
    /// Builds an ID from its parts.
    ///
    /// # Errors
    ///
    /// Fails if a part does not match the grammar.
    pub fn from_parts(venue: &VenueId, class: &str, symbol: &str) -> Result<Self, IdError> {
        Self::new(format!("{venue}:{class}:{symbol}"))
    }

    fn parts(&self) -> (&str, &str, &str) {
        let mut it = self.0.splitn(3, ':');
        let venue = it.next().unwrap_or_default();
        let class = it.next().unwrap_or_default();
        let symbol = it.next().unwrap_or_default();
        (venue, class, symbol)
    }

    /// The venue part.
    #[must_use]
    pub fn venue_part(&self) -> &str {
        self.parts().0
    }

    /// The market class part (`perp`, `spot`, `outcome`, `swap`, ...).
    #[must_use]
    pub fn class_part(&self) -> &str {
        self.parts().1
    }

    /// The venue's market symbol, which may contain `:` (for example a
    /// HIP-3 deployer namespace: `xyz:CL`).
    #[must_use]
    pub fn symbol(&self) -> &str {
        self.parts().2
    }

    /// Path-safe form for Hive-style partition directories: `:` becomes
    /// `~`. Reversible with [`InstrumentId::from_path_segment`].
    #[must_use]
    pub fn path_segment(&self) -> String {
        self.0.replace(':', "~")
    }

    /// Parses the output of [`InstrumentId::path_segment`].
    ///
    /// # Errors
    ///
    /// Fails if the result is not a valid instrument ID.
    pub fn from_path_segment(segment: &str) -> Result<Self, IdError> {
        Self::new(segment.replace('~', ":"))
    }
}

/// A venue order ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OrderId(pub u64);

/// A venue trade ID. For Hyperliquid this is the fill `tid`; the globally
/// unique trade key is `(ts_source_ms, instrument, tid)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TradeId(pub u64);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instrument_ids_cover_all_market_shapes() {
        for s in [
            "hyperliquid:perp:BTC",
            "hyperliquid:perp:xyz:CL",
            "hyperliquid:spot:@107",
            "hyperliquid:spot:PURR-USDC",
            "hyperliquid:outcome:#9001",
            "cftc-part43:swap:USD-SOFR-OIS.5Y",
            "synthetic:perp:kSYN",
        ] {
            let id = InstrumentId::new(s).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(id.as_str(), s);
            assert_eq!(
                InstrumentId::from_path_segment(&id.path_segment()).unwrap(),
                id
            );
            assert!(!id.path_segment().contains(':'));
        }
        let id = InstrumentId::new("hyperliquid:perp:xyz:CL").unwrap();
        assert_eq!(id.venue_part(), "hyperliquid");
        assert_eq!(id.class_part(), "perp");
        assert_eq!(id.symbol(), "xyz:CL");
        let v = VenueId::new("hyperliquid").unwrap();
        assert_eq!(InstrumentId::from_parts(&v, "perp", "xyz:CL").unwrap(), id);
    }

    #[test]
    fn instrument_ids_reject_unsafe_text() {
        for s in [
            "",
            "BTC",
            "hyperliquid:perp",
            "hyperliquid:perp:",
            "Hyperliquid:perp:BTC",
            "hyperliquid:perp:BTC/USD",
            "hyperliquid:perp:a=b",
            "hyperliquid:perp:a b",
            "hyperliquid:perp:a::b",
            "hyperliquid:perp:a~b",
            "hyperliquid:perp:50%",
            "hyperliquid:PERP:BTC",
        ] {
            assert!(InstrumentId::new(s).is_err(), "accepted {s:?}");
        }
        assert!(InstrumentId::new(format!("v:c:{}", "A".repeat(125))).is_err());
    }

    #[test]
    fn other_ids() {
        assert!(VenueId::new("hyperliquid").is_ok());
        assert!(VenueId::new("-x").is_err());
        assert!(AdmissionId::new("ADM-0001").is_ok());
        assert!(AdmissionId::new("ADM-1").is_err());
        assert!(AdmissionId::new("adm-0001").is_err());
        assert!(MethodId::new("bars@1").is_ok());
        assert!(MethodId::new("bars@0").is_err());
        assert!(MethodId::new("bars").is_err());
        assert!(MethodId::new("Bars@1").is_err());
    }

    #[test]
    fn serde_validates() {
        let id: InstrumentId = serde_json::from_str("\"hyperliquid:perp:BTC\"").unwrap();
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            "\"hyperliquid:perp:BTC\""
        );
        assert!(serde_json::from_str::<InstrumentId>("\"bad id\"").is_err());
        assert!(serde_json::from_str::<AdmissionId>("\"ADM-12\"").is_err());
        assert_eq!(serde_json::to_string(&OrderId(5)).unwrap(), "5");
    }
}
