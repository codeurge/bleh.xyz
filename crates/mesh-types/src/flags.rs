//! Record flags.
//!
//! [`Flags`] is a `u32` bitset. Each bit is a [`Flag`]; unknown bits are
//! rejected, so adding a flag is a schema change (a new minor version of
//! the observation spec).
//!
//! JSON encodes flags as an array of names in bit order
//! (`["oracle_anchored_venue","backfill"]`); Arrow and binary encodings use
//! the integer.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One flag bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Flag {
    /// The venue anchors funding or price bands to an oracle. The record
    /// itself is a venue trade or order, but traders on the venue see
    /// oracle-derived incentives.
    OracleAnchoredVenue = 1 << 0,
    /// Imported from a historical archive rather than recorded live.
    Backfill = 1 << 1,
    /// The source skipped data immediately before this record (for
    /// example, a missing block). Set on the first record after the gap.
    GapBefore = 1 << 2,
    /// Lower resolution than the live stream (for example, hourly L2
    /// snapshots from a public archive).
    LowResolution = 1 << 3,
    /// The source capped the reported size or notional.
    SizeCapped = 1 << 4,
    /// The source delayed the record as a block trade.
    BlockTradeDelayed = 1 << 5,
    /// Reconstructed by replaying venue actions through an emulated
    /// matcher (for example order-level books rebuilt from a public action
    /// archive), not observed directly. Always set together with
    /// [`Flag::Backfill`].
    Replayed = 1 << 6,
    /// A public body's statistic whose underlying transactions may be
    /// privately priced by formula against an exchange price or an
    /// exchange-published index, or whose computation uses a factor an
    /// exchange supplies (ADR 0012, decision 6). The record is still the
    /// public body's statistic, never an exchange price.
    ExchangeReferenced = 1 << 7,
}

impl Flag {
    /// Every flag, in bit order.
    pub const ALL: [Flag; 8] = [
        Flag::OracleAnchoredVenue,
        Flag::Backfill,
        Flag::GapBefore,
        Flag::LowResolution,
        Flag::SizeCapped,
        Flag::BlockTradeDelayed,
        Flag::Replayed,
        Flag::ExchangeReferenced,
    ];

    /// The flag's bit.
    #[must_use]
    pub const fn bit(self) -> u32 {
        self as u32
    }

    /// The flag's JSON name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Flag::OracleAnchoredVenue => "oracle_anchored_venue",
            Flag::Backfill => "backfill",
            Flag::GapBefore => "gap_before",
            Flag::LowResolution => "low_resolution",
            Flag::SizeCapped => "size_capped",
            Flag::BlockTradeDelayed => "block_trade_delayed",
            Flag::Replayed => "replayed",
            Flag::ExchangeReferenced => "exchange_referenced",
        }
    }

    /// Looks up a flag by its JSON name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == name)
    }
}

/// Error for an integer with bits that are not defined flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("undefined flag bits {0:#x}")]
pub struct UnknownFlagBits(pub u32);

/// A set of [`Flag`]s.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Flags(u32);

impl Flags {
    /// No flags.
    pub const NONE: Flags = Flags(0);

    const KNOWN: u32 = (1 << Flag::ALL.len()) - 1;

    /// Builds a set from its integer form.
    ///
    /// # Errors
    ///
    /// Fails if any undefined bit is set.
    pub const fn from_bits(bits: u32) -> Result<Self, UnknownFlagBits> {
        if bits & !Self::KNOWN != 0 {
            Err(UnknownFlagBits(bits & !Self::KNOWN))
        } else {
            Ok(Self(bits))
        }
    }

    /// The integer form.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// True if `flag` is set.
    #[must_use]
    pub const fn contains(self, flag: Flag) -> bool {
        self.0 & flag.bit() != 0
    }

    /// Returns the set with `flag` added.
    #[must_use]
    pub const fn with(self, flag: Flag) -> Self {
        Self(self.0 | flag.bit())
    }

    /// Returns the set with `flag` removed.
    #[must_use]
    pub const fn without(self, flag: Flag) -> Self {
        Self(self.0 & !flag.bit())
    }

    /// Adds `flag` in place.
    pub fn insert(&mut self, flag: Flag) {
        self.0 |= flag.bit();
    }

    /// True if no flag is set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The set flags, in bit order.
    pub fn iter(self) -> impl Iterator<Item = Flag> {
        Flag::ALL.into_iter().filter(move |f| self.contains(*f))
    }
}

impl FromIterator<Flag> for Flags {
    fn from_iter<I: IntoIterator<Item = Flag>>(iter: I) -> Self {
        iter.into_iter().fold(Flags::NONE, Flags::with)
    }
}

impl fmt::Debug for Flags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter().map(Flag::name)).finish()
    }
}

impl Serialize for Flags {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter().map(Flag::name))
    }
}

impl<'de> Deserialize<'de> for Flags {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let names = Vec::<String>::deserialize(deserializer)?;
        let mut flags = Flags::NONE;
        let mut last_bit = 0u32;
        for name in &names {
            let flag = Flag::from_name(name)
                .ok_or_else(|| serde::de::Error::custom(format!("unknown flag {name:?}")))?;
            // Canonical form: bit order, no duplicates.
            if flag.bit() <= last_bit {
                return Err(serde::de::Error::custom(
                    "flags must be unique and in bit order",
                ));
            }
            last_bit = flag.bit();
            flags.insert(flag);
        }
        Ok(flags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_and_names() {
        let f = Flags::NONE
            .with(Flag::Backfill)
            .with(Flag::OracleAnchoredVenue);
        assert_eq!(f.bits(), 0b11);
        assert!(f.contains(Flag::Backfill));
        assert!(!f.contains(Flag::GapBefore));
        assert_eq!(f.without(Flag::Backfill).bits(), 1);
        assert_eq!(Flags::from_bits(0b11).unwrap(), f);
        assert_eq!(Flags::from_bits(1 << 31), Err(UnknownFlagBits(1 << 31)));
        let all: Flags = Flag::ALL.into_iter().collect();
        assert_eq!(all.bits(), 0b1111_1111);
        for flag in Flag::ALL {
            assert_eq!(Flag::from_name(flag.name()), Some(flag));
        }
    }

    #[test]
    fn json_is_ordered_names() {
        let f = Flags::NONE
            .with(Flag::GapBefore)
            .with(Flag::OracleAnchoredVenue);
        let json = serde_json::to_string(&f).unwrap();
        assert_eq!(json, r#"["oracle_anchored_venue","gap_before"]"#);
        assert_eq!(serde_json::from_str::<Flags>(&json).unwrap(), f);
        assert_eq!(serde_json::to_string(&Flags::NONE).unwrap(), "[]");
        assert!(
            serde_json::from_str::<Flags>(r#"["gap_before","oracle_anchored_venue"]"#).is_err()
        );
        assert!(serde_json::from_str::<Flags>(r#"["backfill","backfill"]"#).is_err());
        assert!(serde_json::from_str::<Flags>(r#"["mark_price"]"#).is_err());
        let x = Flags::NONE
            .with(Flag::ExchangeReferenced)
            .with(Flag::Backfill);
        let json = serde_json::to_string(&x).unwrap();
        assert_eq!(json, r#"["backfill","exchange_referenced"]"#);
        assert_eq!(x.bits(), 0b1000_0010);
        assert_eq!(Flags::from_bits(1 << 8), Err(UnknownFlagBits(1 << 8)));
    }
}
