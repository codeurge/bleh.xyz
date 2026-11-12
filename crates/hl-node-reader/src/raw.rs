//! Typed node-format lines (spec §§ 3-7), strict on input.
//!
//! Every struct lists every field the node is documented to write. Admitted
//! fields are kept; excluded and unused fields are accepted as
//! [`Excluded`] (parsed, never stored, never serialized). Anything else is
//! an unknown field and fails parsing: a node upgrade that changes a schema
//! stops that stream and raises an alert rather than leaking a new field.
//!
//! Serializing these types writes **filtered raw**: the same line format
//! with only the admitted fields. Fill events serialize as the bare fill
//! object (the `[user, fill]` pair loses its user), and the parser accepts
//! both forms, so filtered raw parses exactly like raw.

use std::fmt;

use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};

/// A field the node writes that the mesh drops: parsed (so the field is
/// known), never kept.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Excluded;

impl<'de> Deserialize<'de> for Excluded {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        IgnoredAny::deserialize(d).map(|_| Excluded)
    }
}

/// The three streams the mesh reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    /// `--write-fills`.
    Fills,
    /// `--write-order-statuses`.
    OrderStatuses,
    /// `--write-raw-book-diffs`.
    RawBookDiffs,
}

impl Stream {
    /// Every stream, in join order.
    pub const ALL: [Stream; 3] = [Stream::Fills, Stream::OrderStatuses, Stream::RawBookDiffs];

    /// Position in [`Stream::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Stream::Fills => 0,
            Stream::OrderStatuses => 1,
            Stream::RawBookDiffs => 2,
        }
    }

    /// Name used in metrics and checkpoints.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Stream::Fills => "fills",
            Stream::OrderStatuses => "order_statuses",
            Stream::RawBookDiffs => "raw_book_diffs",
        }
    }
}

impl fmt::Display for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One `--batch-by-block` line: one block's events of one stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockLine<E> {
    /// Block height.
    pub block_number: u64,
    /// Block timestamp, naive UTC text.
    pub block_time: String,
    /// The block's events for this stream, in processing order.
    pub events: Vec<E>,
    /// When the node processed the block, naive UTC text.
    pub local_time: String,
}

/// A fill (spec § 4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fill {
    /// Market.
    pub coin: String,
    /// True if this side crossed the spread (taker).
    pub crossed: bool,
    /// Order ID of this side.
    pub oid: u64,
    /// Price text.
    pub px: String,
    /// `B` or `A`.
    pub side: String,
    /// Size text.
    pub sz: String,
    /// Trade ID shared by both fills of a trade.
    pub tid: u64,
    /// Fill time, milliseconds.
    pub time: i64,
    /// Liquidation details. Read only to classify the pair; never written.
    #[serde(default, skip_serializing)]
    pub liquidation: Option<Liquidation>,
    #[serde(default, rename = "startPosition", skip_serializing)]
    start_position: Excluded,
    #[serde(default, skip_serializing)]
    dir: Excluded,
    #[serde(default, rename = "closedPnl", skip_serializing)]
    closed_pnl: Excluded,
    #[serde(default, skip_serializing)]
    hash: Excluded,
    #[serde(default, skip_serializing)]
    fee: Excluded,
    #[serde(default, rename = "feeToken", skip_serializing)]
    fee_token: Excluded,
    #[serde(default, rename = "builderFee", skip_serializing)]
    builder_fee: Excluded,
    #[serde(default, skip_serializing)]
    builder: Excluded,
    #[serde(default, rename = "subBuilders", skip_serializing)]
    sub_builders: Excluded,
    #[serde(default, rename = "deployerFee", skip_serializing)]
    deployer_fee: Excluded,
    #[serde(default, rename = "twapId", skip_serializing)]
    twap_id: Excluded,
}

impl Fill {
    /// A fill with only admitted fields (for tests and generators).
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        coin: &str,
        crossed: bool,
        oid: u64,
        px: &str,
        side: &str,
        sz: &str,
        tid: u64,
        time: i64,
    ) -> Self {
        Self {
            coin: coin.to_owned(),
            crossed,
            oid,
            px: px.to_owned(),
            side: side.to_owned(),
            sz: sz.to_owned(),
            tid,
            time,
            liquidation: None,
            start_position: Excluded,
            dir: Excluded,
            closed_pnl: Excluded,
            hash: Excluded,
            fee: Excluded,
            fee_token: Excluded,
            builder_fee: Excluded,
            builder: Excluded,
            sub_builders: Excluded,
            deployer_fee: Excluded,
            twap_id: Excluded,
        }
    }

    /// The same fill with the liquidation object (read side only) cleared,
    /// as written to filtered raw.
    #[must_use]
    pub fn admitted(&self) -> Self {
        let mut f = self.clone();
        f.liquidation = None;
        f
    }
}

/// The fill `liquidation` object. Only `method` is read.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Liquidation {
    /// `market` or `backstop`.
    pub method: String,
    #[serde(default, rename = "liquidatedUser")]
    liquidated_user: Excluded,
    #[serde(default, rename = "markPx")]
    mark_px: Excluded,
}

impl Liquidation {
    /// A liquidation object with only a method (for tests).
    #[must_use]
    pub fn with_method(method: &str) -> Self {
        Self {
            method: method.to_owned(),
            liquidated_user: Excluded,
            mark_px: Excluded,
        }
    }
}

/// A fills-stream event: `[user, fill]` in raw, the bare fill in filtered
/// raw. Serializes as the bare fill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct FillEvent(pub Fill);

impl<'de> Deserialize<'de> for FillEvent {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = FillEvent;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a [user, fill] pair or a fill object")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<FillEvent, A::Error> {
                let _user: Excluded = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let fill: Fill = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                if seq.next_element::<IgnoredAny>()?.is_some() {
                    return Err(de::Error::invalid_length(3, &self));
                }
                Ok(FillEvent(fill))
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<FillEvent, A::Error> {
                Fill::deserialize(de::value::MapAccessDeserializer::new(map)).map(FillEvent)
            }
        }
        d.deserialize_any(V)
    }
}

/// An order as it appears in order statuses and L4 snapshots (spec § 5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Order {
    /// Market.
    pub coin: String,
    /// Limit price text.
    #[serde(rename = "limitPx")]
    pub limit_px: String,
    /// Order ID.
    pub oid: u64,
    /// Original size text.
    #[serde(default, rename = "origSz", skip_serializing_if = "Option::is_none")]
    pub orig_sz: Option<String>,
    /// `B` or `A`.
    pub side: String,
    /// Size text (see assumption 5).
    pub sz: String,
    /// Placement time, milliseconds.
    pub timestamp: i64,
    /// True for trigger orders. Read only to drop them from snapshots.
    #[serde(default, rename = "isTrigger", skip_serializing)]
    pub is_trigger: Option<bool>,
    #[serde(default, rename = "orderType", skip_serializing)]
    order_type: Excluded,
    #[serde(default, skip_serializing)]
    tif: Excluded,
    #[serde(default, rename = "reduceOnly", skip_serializing)]
    reduce_only: Excluded,
    #[serde(default, rename = "triggerPx", skip_serializing)]
    trigger_px: Excluded,
    #[serde(default, rename = "triggerCondition", skip_serializing)]
    trigger_condition: Excluded,
    #[serde(default, rename = "isPositionTpsl", skip_serializing)]
    is_position_tpsl: Excluded,
    #[serde(default, skip_serializing)]
    children: Excluded,
    #[serde(default, skip_serializing)]
    cloid: Excluded,
    #[serde(default, skip_serializing)]
    user: Excluded,
}

impl Order {
    /// An order with only admitted fields (for tests).
    #[must_use]
    pub fn new(coin: &str, side: &str, limit_px: &str, sz: &str, oid: u64, timestamp: i64) -> Self {
        Self {
            coin: coin.to_owned(),
            limit_px: limit_px.to_owned(),
            oid,
            orig_sz: None,
            side: side.to_owned(),
            sz: sz.to_owned(),
            timestamp,
            is_trigger: None,
            order_type: Excluded,
            tif: Excluded,
            reduce_only: Excluded,
            trigger_px: Excluded,
            trigger_condition: Excluded,
            is_position_tpsl: Excluded,
            children: Excluded,
            cloid: Excluded,
            user: Excluded,
        }
    }

    /// True if this is a trigger order (not resting).
    #[must_use]
    pub fn is_trigger(&self) -> bool {
        self.is_trigger == Some(true)
    }
}

/// An order-status event (spec § 5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderStatusEvent {
    /// The order.
    pub order: Order,
    /// Status value, such as `open`, `filled`, `canceled`.
    pub status: String,
    /// Status time, naive UTC text.
    pub time: String,
    #[serde(default, skip_serializing)]
    user: Excluded,
}

impl OrderStatusEvent {
    /// A status event (for tests).
    #[must_use]
    pub fn new(order: Order, status: &str, time: &str) -> Self {
        Self {
            order,
            status: status.to_owned(),
            time: time.to_owned(),
            user: Excluded,
        }
    }
}

/// The change a raw book diff describes (spec § 6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub enum RawBookDiff {
    /// New resting order.
    New {
        /// Size text.
        sz: String,
    },
    /// Resting size changes.
    Update {
        /// Size before, text.
        #[serde(rename = "origSz")]
        orig_sz: String,
        /// Size after, text.
        #[serde(rename = "newSz")]
        new_sz: String,
    },
    /// Resting order removed.
    Remove,
}

/// A raw book diff event (spec § 6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BookDiffEvent {
    /// Market.
    pub coin: String,
    /// Order ID.
    pub oid: u64,
    /// Limit price text.
    pub px: String,
    /// The change.
    pub raw_book_diff: RawBookDiff,
    /// `Bid` or `Ask`.
    pub side: String,
    #[serde(default, skip_serializing)]
    user: Excluded,
}

impl BookDiffEvent {
    /// A diff event (for tests).
    #[must_use]
    pub fn new(coin: &str, oid: u64, px: &str, side: &str, diff: RawBookDiff) -> Self {
        Self {
            coin: coin.to_owned(),
            oid,
            px: px.to_owned(),
            raw_book_diff: diff,
            side: side.to_owned(),
            user: Excluded,
        }
    }
}

/// A fills line.
pub type FillsLine = BlockLine<FillEvent>;
/// An order-status line.
pub type OrderStatusesLine = BlockLine<OrderStatusEvent>;
/// A raw-book-diff line.
pub type RawBookDiffsLine = BlockLine<BookDiffEvent>;

/// A parsed line of any stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedLine {
    /// Fills.
    Fills(FillsLine),
    /// Order statuses.
    OrderStatuses(OrderStatusesLine),
    /// Raw book diffs.
    RawBookDiffs(RawBookDiffsLine),
}

impl ParsedLine {
    /// Parses one line of `stream`.
    ///
    /// # Errors
    ///
    /// Fails on malformed JSON, a missing field or an unknown field.
    pub fn parse(stream: Stream, line: &str) -> Result<Self, serde_json::Error> {
        Ok(match stream {
            Stream::Fills => ParsedLine::Fills(serde_json::from_str(line)?),
            Stream::OrderStatuses => ParsedLine::OrderStatuses(serde_json::from_str(line)?),
            Stream::RawBookDiffs => ParsedLine::RawBookDiffs(serde_json::from_str(line)?),
        })
    }

    /// The line's stream.
    #[must_use]
    pub const fn stream(&self) -> Stream {
        match self {
            ParsedLine::Fills(_) => Stream::Fills,
            ParsedLine::OrderStatuses(_) => Stream::OrderStatuses,
            ParsedLine::RawBookDiffs(_) => Stream::RawBookDiffs,
        }
    }

    /// The line's block number.
    #[must_use]
    pub const fn block_number(&self) -> u64 {
        match self {
            ParsedLine::Fills(l) => l.block_number,
            ParsedLine::OrderStatuses(l) => l.block_number,
            ParsedLine::RawBookDiffs(l) => l.block_number,
        }
    }

    /// The line's `block_time` text.
    #[must_use]
    pub fn block_time(&self) -> &str {
        match self {
            ParsedLine::Fills(l) => &l.block_time,
            ParsedLine::OrderStatuses(l) => &l.block_time,
            ParsedLine::RawBookDiffs(l) => &l.block_time,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const RAW_FILL: &str = r#"{"block_number":7,"block_time":"2026-01-01T00:00:00.100","events":[["0x00000000000000000000000000000000000000aa",{"closedPnl":"0.0","coin":"SYN","crossed":true,"dir":"Open Long","fee":"0.1","feeToken":"USDC","hash":"0x01","oid":5,"px":"10.5","side":"B","startPosition":"0.0","sz":"2","tid":9,"time":1767225600100,"twapId":null,"liquidation":{"liquidatedUser":"0x00","markPx":"10.4","method":"market"}}]],"local_time":"2026-01-01T00:00:00.120000000"}"#;

    #[test]
    fn fill_line_strips_on_serialize_and_round_trips() {
        let line: FillsLine = serde_json::from_str(RAW_FILL).unwrap();
        let fill = &line.events[0].0;
        assert_eq!(fill.liquidation.as_ref().unwrap().method, "market");
        let out = serde_json::to_string(&line).unwrap();
        for banned in [
            "user",
            "0xaa",
            "hash",
            "fee",
            "markPx",
            "liquidation",
            "dir",
        ] {
            assert!(!out.contains(banned), "{banned} in {out}");
        }
        let back: FillsLine = serde_json::from_str(&out).unwrap();
        assert_eq!(back.events[0].0, fill.admitted());
        assert_eq!(serde_json::to_string(&back).unwrap(), out);
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let bad = RAW_FILL.replace("\"twapId\"", "\"newField\":1,\"twapId\"");
        assert!(serde_json::from_str::<FillsLine>(&bad).is_err());
        let bad = RAW_FILL.replace("\"method\"", "\"oraclePx\":\"1\",\"method\"");
        assert!(serde_json::from_str::<FillsLine>(&bad).is_err());
        let bad = RAW_FILL.replace("\"local_time\"", "\"extra\":0,\"local_time\"");
        assert!(serde_json::from_str::<FillsLine>(&bad).is_err());
        // A fill pair with a third element is malformed.
        let bad = RAW_FILL.replace("}]],\"local_time\"", "},1]],\"local_time\"");
        assert!(serde_json::from_str::<FillsLine>(&bad).is_err());
    }

    #[test]
    fn diffs_and_statuses() {
        let diffs = r#"{"block_number":1,"block_time":"2026-01-01T00:00:00.000","events":[{"coin":"SYN","oid":1,"px":"1","raw_book_diff":{"new":{"sz":"2"}},"side":"Bid","user":"0x01"},{"coin":"SYN","oid":1,"px":"1","raw_book_diff":{"update":{"origSz":"2","newSz":"1"}},"side":"Bid","user":"0x01"},{"coin":"SYN","oid":1,"px":"1","raw_book_diff":"remove","side":"Bid","user":"0x01"}],"local_time":"2026-01-01T00:00:00.000000001"}"#;
        let p = ParsedLine::parse(Stream::RawBookDiffs, diffs).unwrap();
        let ParsedLine::RawBookDiffs(l) = &p else {
            panic!()
        };
        assert_eq!(l.events[2].raw_book_diff, RawBookDiff::Remove);
        let out = serde_json::to_string(l).unwrap();
        assert!(!out.contains("user"));
        assert!(out.contains(r#""raw_book_diff":"remove""#));
        assert_eq!(
            ParsedLine::parse(Stream::RawBookDiffs, &out)
                .unwrap()
                .block_number(),
            1
        );
        let bad = diffs.replace(r#""remove""#, r#""delete""#);
        assert!(ParsedLine::parse(Stream::RawBookDiffs, &bad).is_err());

        let statuses = r#"{"block_number":1,"block_time":"2026-01-01T00:00:00.000","events":[{"order":{"children":[],"cloid":"0xab","coin":"SYN","isPositionTpsl":false,"isTrigger":false,"limitPx":"1","oid":1,"orderType":"Limit","origSz":"2","reduceOnly":false,"side":"B","sz":"2","tif":"Gtc","timestamp":1,"triggerCondition":"N/A","triggerPx":"0.0"},"status":"open","time":"2026-01-01T00:00:00.000","user":"0x01"}],"local_time":"2026-01-01T00:00:00.000000001"}"#;
        let p = ParsedLine::parse(Stream::OrderStatuses, statuses).unwrap();
        let ParsedLine::OrderStatuses(l) = &p else {
            panic!()
        };
        let out = serde_json::to_string(l).unwrap();
        for banned in ["user", "cloid", "0xab", "tif", "children"] {
            assert!(!out.contains(banned), "{banned} in {out}");
        }
        assert!(ParsedLine::parse(Stream::OrderStatuses, &out).is_ok());
    }
}
