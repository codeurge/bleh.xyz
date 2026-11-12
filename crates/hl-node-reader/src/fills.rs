//! Pairing fills into trades (spec § 4.1), independent of the registry.
//!
//! Shared by the parser (which turns book-matched pairs into trades) and the
//! filter (which drops the pairs that are not book trades), so both apply
//! exactly the same rule.

use std::collections::HashMap;

use crate::assumptions::{
    LIQUIDATION_METHOD_BACKSTOP, LIQUIDATION_METHOD_MARKET, crossed_count_is_book_trade,
};
use crate::raw::Fill;

/// Why a fill group is not a trade and is dropped (not an error).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DropReason {
    /// A liquidation with `method = backstop`: a transfer at the mark price.
    BackstopLiquidation,
    /// A liquidation with a method the reader does not know. Dropped rather
    /// than guessed.
    UnknownLiquidationMethod,
    /// No `crossed = true` fill: backstop liquidations and halt settlements.
    NoTaker,
}

impl DropReason {
    /// Metric label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            DropReason::BackstopLiquidation => "backstop_liquidation",
            DropReason::UnknownLiquidationMethod => "unknown_liquidation_method",
            DropReason::NoTaker => "no_taker",
        }
    }
}

/// A fill group that is malformed (spec § 4.1 step 6): kept in filtered raw
/// so the anomaly is reproducible, never emitted as a trade, and the next
/// trade of the instrument carries `gap_before`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Anomaly {
    /// A single fill with no partner in the block.
    Unmatched,
    /// More than two fills share `(coin, tid)`.
    TooManyFills,
    /// Both fills have `crossed = true`.
    DoubleCrossed,
    /// Both fills are on the same side.
    SameSide,
}

impl Anomaly {
    /// Metric label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Anomaly::Unmatched => "unmatched",
            Anomaly::TooManyFills => "too_many_fills",
            Anomaly::DoubleCrossed => "double_crossed",
            Anomaly::SameSide => "same_side",
        }
    }
}

/// What a fill group is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupClass {
    /// An order-book trade: `taker` and `maker` index into the group.
    Trade {
        /// The `crossed = true` fill.
        taker: usize,
        /// The other fill.
        maker: usize,
    },
    /// Not a trade; dropped.
    Drop(DropReason),
    /// Malformed.
    Anomaly(Anomaly),
}

/// Fills sharing `(coin, tid)` in one block.
#[derive(Debug, Clone)]
pub struct FillGroup<'a> {
    /// Market.
    pub coin: &'a str,
    /// Trade ID.
    pub tid: u64,
    /// The fills, in file order.
    pub fills: Vec<&'a Fill>,
    /// The classification.
    pub class: GroupClass,
}

/// Groups a block's fills by `(coin, tid)`, in order of each group's first
/// fill, and classifies each group.
#[must_use]
pub fn group_fills<'a, I>(fills: I) -> Vec<FillGroup<'a>>
where
    I: IntoIterator<Item = &'a Fill>,
{
    let mut index: HashMap<(&'a str, u64), usize> = HashMap::new();
    let mut groups: Vec<FillGroup<'a>> = Vec::new();
    for f in fills {
        let key = (f.coin.as_str(), f.tid);
        match index.get(&key) {
            Some(&i) => groups[i].fills.push(f),
            None => {
                index.insert(key, groups.len());
                groups.push(FillGroup {
                    coin: f.coin.as_str(),
                    tid: f.tid,
                    fills: vec![f],
                    class: GroupClass::Drop(DropReason::NoTaker),
                });
            }
        }
    }
    for g in &mut groups {
        g.class = classify_group(&g.fills);
    }
    groups
}

/// Classifies one `(coin, tid)` group (spec § 4.1 steps 2, 4, 5, 6).
#[must_use]
pub fn classify_group(fills: &[&Fill]) -> GroupClass {
    for f in fills {
        if let Some(l) = &f.liquidation {
            if l.method == LIQUIDATION_METHOD_BACKSTOP {
                return GroupClass::Drop(DropReason::BackstopLiquidation);
            }
            if l.method != LIQUIDATION_METHOD_MARKET {
                return GroupClass::Drop(DropReason::UnknownLiquidationMethod);
            }
        }
    }
    match fills.len() {
        0 | 1 => return GroupClass::Anomaly(Anomaly::Unmatched),
        2 => {}
        _ => return GroupClass::Anomaly(Anomaly::TooManyFills),
    }
    let crossed = fills.iter().filter(|f| f.crossed).count();
    if crossed == 0 {
        return GroupClass::Drop(DropReason::NoTaker);
    }
    if !crossed_count_is_book_trade(crossed) {
        return GroupClass::Anomaly(Anomaly::DoubleCrossed);
    }
    if fills[0].side == fills[1].side {
        return GroupClass::Anomaly(Anomaly::SameSide);
    }
    let taker = usize::from(!fills[0].crossed);
    GroupClass::Trade {
        taker,
        maker: 1 - taker,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::Liquidation;

    fn fill(crossed: bool, side: &str, tid: u64) -> Fill {
        Fill::new("SYN", crossed, tid * 10, "1", side, "1", tid, 0)
    }

    #[test]
    fn classification() {
        let (a, b) = (fill(true, "B", 1), fill(false, "A", 1));
        assert_eq!(
            classify_group(&[&b, &a]),
            GroupClass::Trade { taker: 1, maker: 0 }
        );
        assert_eq!(
            classify_group(&[&a]),
            GroupClass::Anomaly(Anomaly::Unmatched)
        );
        assert_eq!(
            classify_group(&[&a, &b, &b]),
            GroupClass::Anomaly(Anomaly::TooManyFills)
        );
        let c = fill(true, "A", 1);
        assert_eq!(
            classify_group(&[&a, &c]),
            GroupClass::Anomaly(Anomaly::DoubleCrossed)
        );
        let d = fill(false, "B", 1);
        assert_eq!(
            classify_group(&[&a, &d]),
            GroupClass::Anomaly(Anomaly::SameSide)
        );
        let e = fill(false, "B", 1);
        assert_eq!(
            classify_group(&[&b, &e]),
            GroupClass::Drop(DropReason::NoTaker)
        );
        let mut l = fill(true, "B", 1);
        l.liquidation = Some(Liquidation::with_method("backstop"));
        assert_eq!(
            classify_group(&[&l, &b]),
            GroupClass::Drop(DropReason::BackstopLiquidation)
        );
        l.liquidation = Some(Liquidation::with_method("market"));
        assert!(matches!(
            classify_group(&[&l, &b]),
            GroupClass::Trade { .. }
        ));
        l.liquidation = Some(Liquidation::with_method("other"));
        assert_eq!(
            classify_group(&[&l, &b]),
            GroupClass::Drop(DropReason::UnknownLiquidationMethod)
        );
    }

    #[test]
    fn grouping_keeps_first_fill_order() {
        let fills = [
            fill(true, "B", 2),
            fill(true, "A", 1),
            fill(false, "A", 2),
            fill(false, "B", 1),
        ];
        let g = group_fills(&fills);
        assert_eq!(g.iter().map(|g| g.tid).collect::<Vec<_>>(), vec![2, 1]);
        assert!(
            g.iter()
                .all(|g| matches!(g.class, GroupClass::Trade { .. }))
        );
    }
}
