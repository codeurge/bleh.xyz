//! Sequence numbers.
//!
//! A [`Seq`] is `(block_number, event_index)` and orders records totally:
//! first by block, then by position within the block.
//!
//! - For chain sources (Hyperliquid), `block_number` is the L1 block height.
//!   For other sources it is the source's batch number (for example, the
//!   index of a repository dissemination file); the spec for each source
//!   says which.
//! - `event_index` is the record's zero-based position within its block
//!   **for one instrument and one record kind**, in source order. A
//!   subscriber to one instrument and kind therefore sees indices
//!   `0, 1, 2, ...` in every block and can detect a missing record.
//! - [`Seq::end_of_block`] (`event_index = u32::MAX`) tags state "after all
//!   events of this block", which is what book snapshots carry.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Position of a record in its source: block, then index within the block.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(deny_unknown_fields)]
pub struct Seq {
    /// L1 block height (or source batch number).
    pub block_number: u64,
    /// Zero-based index within the block for one instrument and kind.
    pub event_index: u32,
}

impl Seq {
    /// Builds a sequence number.
    #[must_use]
    pub const fn new(block_number: u64, event_index: u32) -> Self {
        Self {
            block_number,
            event_index,
        }
    }

    /// The position after every event of `block_number`. Used to tag
    /// snapshots: deltas with a greater `Seq` follow the snapshot.
    #[must_use]
    pub const fn end_of_block(block_number: u64) -> Self {
        Self::new(block_number, u32::MAX)
    }

    /// True if this is an [`end_of_block`](Self::end_of_block) marker.
    #[must_use]
    pub const fn is_end_of_block(self) -> bool {
        self.event_index == u32::MAX
    }

    /// The next index in the same block, or `None` at the end of the index
    /// space.
    #[must_use]
    pub fn next_in_block(self) -> Option<Self> {
        self.event_index
            .checked_add(1)
            .filter(|&i| i != u32::MAX)
            .map(|i| Self::new(self.block_number, i))
    }
}

impl fmt::Display for Seq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_end_of_block() {
            write!(f, "{}:end", self.block_number)
        } else {
            write!(f, "{}:{}", self.block_number, self.event_index)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_order_is_block_then_index() {
        let mut v = vec![
            Seq::new(2, 0),
            Seq::end_of_block(1),
            Seq::new(1, 5),
            Seq::new(1, 0),
            Seq::new(0, 9),
        ];
        v.sort();
        assert_eq!(
            v,
            vec![
                Seq::new(0, 9),
                Seq::new(1, 0),
                Seq::new(1, 5),
                Seq::end_of_block(1),
                Seq::new(2, 0)
            ]
        );
        assert!(Seq::end_of_block(1) < Seq::new(2, 0));
        assert!(Seq::new(1, u32::MAX - 1) < Seq::end_of_block(1));
    }

    #[test]
    fn next_and_display() {
        assert_eq!(Seq::new(3, 0).next_in_block(), Some(Seq::new(3, 1)));
        assert_eq!(Seq::new(3, u32::MAX - 1).next_in_block(), None);
        assert_eq!(Seq::new(3, 4).to_string(), "3:4");
        assert_eq!(Seq::end_of_block(3).to_string(), "3:end");
    }

    #[test]
    fn json_shape() {
        let s = serde_json::to_string(&Seq::new(7, 2)).unwrap();
        assert_eq!(s, r#"{"block_number":7,"event_index":2}"#);
        assert!(
            serde_json::from_str::<Seq>(r#"{"block_number":7,"event_index":2,"x":1}"#).is_err()
        );
    }
}
