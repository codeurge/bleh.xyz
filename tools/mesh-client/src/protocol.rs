//! Gateway protocol v0 message types (`docs/protocol/gateway-v0.md`).
//!
//! One definition used by both the gateway and the reference client, so
//! the two cannot drift. Every message rejects unknown fields.
//!
//! Instrument patterns in channels: an exact instrument ID, `*` (every
//! instrument), or a prefix ending in `*` such as `hyperliquid:perp:*`.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use mesh_types::{
    BarInterval, BlockBatch, BlockCommitment, EthAddress, InstrumentId, MerkleProof, Record,
    RecordKind, Seq, StreamId, UnixMillis, UnixNanos,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Protocol name sent in `hello`.
pub const PROTOCOL: &str = "bleh-gateway-v0";
/// Websocket path.
pub const WS_PATH: &str = "/v0/ws";
/// JSON subprotocol.
pub const SUBPROTOCOL_JSON: &str = "bleh.v0.json";
/// CBOR subprotocol.
pub const SUBPROTOCOL_CBOR: &str = "bleh.v0.cbor";

/// Websocket close codes (gateway-v0 § 7).
pub mod close {
    /// Server going away (restart, deploy).
    pub const GOING_AWAY: u16 = 1001;
    /// A frame or message larger than the limit.
    pub const TOO_BIG: u16 = 1009;
    /// Protocol error.
    pub const PROTOCOL_ERROR: u16 = 4000;
    /// Unsupported protocol version or subprotocol.
    pub const UNSUPPORTED: u16 = 4001;
    /// Slow consumer.
    pub const SLOW_CONSUMER: u16 = 4008;
    /// Rate limited.
    pub const RATE_LIMITED: u16 = 4029;
}

/// Frame encoding, chosen by websocket subprotocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    /// UTF-8 JSON text frames (`bleh.v0.json`, the default).
    #[default]
    Json,
    /// CBOR binary frames (`bleh.v0.cbor`).
    Cbor,
}

impl Encoding {
    /// The subprotocol name.
    #[must_use]
    pub const fn subprotocol(self) -> &'static str {
        match self {
            Self::Json => SUBPROTOCOL_JSON,
            Self::Cbor => SUBPROTOCOL_CBOR,
        }
    }

    /// Parses a subprotocol name.
    #[must_use]
    pub fn from_subprotocol(s: &str) -> Option<Self> {
        match s.trim() {
            SUBPROTOCOL_JSON => Some(Self::Json),
            SUBPROTOCOL_CBOR => Some(Self::Cbor),
            _ => None,
        }
    }
}

impl FromStr for Encoding {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "json" => Ok(Self::Json),
            "cbor" => Ok(Self::Cbor),
            other => Err(format!("unknown encoding {other:?} (json or cbor)")),
        }
    }
}

/// Book depth for `book_event` channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Depth {
    /// Order-level snapshot (default).
    #[default]
    L4,
    /// Level snapshot.
    L2,
}

/// Instruments a channel covers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InstrumentPattern {
    /// Every instrument.
    All,
    /// Instruments whose ID starts with this prefix (written `prefix*`).
    Prefix(String),
    /// One instrument.
    Exact(InstrumentId),
}

impl InstrumentPattern {
    /// True if `id` is covered.
    #[must_use]
    pub fn matches(&self, id: &InstrumentId) -> bool {
        match self {
            Self::All => true,
            Self::Prefix(p) => id.as_str().starts_with(p.as_str()),
            Self::Exact(e) => e == id,
        }
    }

    /// The exact instrument, if this is not a wildcard.
    #[must_use]
    pub fn exact(&self) -> Option<&InstrumentId> {
        match self {
            Self::Exact(e) => Some(e),
            _ => None,
        }
    }
}

impl fmt::Display for InstrumentPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::All => f.write_str("*"),
            Self::Prefix(p) => write!(f, "{p}*"),
            Self::Exact(e) => f.write_str(e.as_str()),
        }
    }
}

impl FromStr for InstrumentPattern {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "*" {
            return Ok(Self::All);
        }
        if let Some(p) = s.strip_suffix('*') {
            let ok = !p.is_empty()
                && p.len() <= 128
                && p.bytes().all(|b| {
                    b.is_ascii_alphanumeric()
                        || matches!(b, b':' | b'_' | b'.' | b'@' | b'#' | b'+' | b'-')
                });
            return if ok {
                Ok(Self::Prefix(p.to_owned()))
            } else {
                Err(format!("bad instrument pattern {s:?}"))
            };
        }
        InstrumentId::new(s)
            .map(Self::Exact)
            .map_err(|e| e.to_string())
    }
}

impl Serialize for InstrumentPattern {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for InstrumentPattern {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// One channel: instruments and a kind.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelSpec {
    /// Instrument or pattern.
    pub instrument: InstrumentPattern,
    /// Record kind.
    pub kind: RecordKind,
    /// `book_event` only: snapshot depth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<Depth>,
    /// `bar` only: bar interval (all intervals if absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<BarInterval>,
}

impl ChannelSpec {
    /// A channel without depth or interval.
    #[must_use]
    pub fn new(instrument: InstrumentPattern, kind: RecordKind) -> Self {
        Self {
            instrument,
            kind,
            depth: None,
            interval: None,
        }
    }

    /// True if the channel covers `record`.
    #[must_use]
    pub fn matches_record(&self, record: &Record) -> bool {
        if record.kind() != self.kind || !self.instrument.matches(&record.prov().instrument) {
            return false;
        }
        match (record, self.interval) {
            (Record::Bar(b), Some(i)) => b.interval == i,
            _ => true,
        }
    }
}

impl FromStr for ChannelSpec {
    type Err = String;
    /// `INSTRUMENT|KIND[|l4|l2|INTERVAL]`, e.g. `*|trade`,
    /// `hyperliquid:perp:BTC|book_event|l4`, `hyperliquid:perp:*|bar|1m`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split('|');
        let instrument = parts.next().unwrap_or_default().parse()?;
        let kind_s = parts
            .next()
            .ok_or_else(|| format!("missing kind in {s:?}"))?;
        let kind: RecordKind = serde_json::from_value(serde_json::Value::String(kind_s.into()))
            .map_err(|_| format!("unknown kind {kind_s:?}"))?;
        let mut spec = Self::new(instrument, kind);
        if let Some(extra) = parts.next() {
            match extra {
                "l4" => spec.depth = Some(Depth::L4),
                "l2" => spec.depth = Some(Depth::L2),
                other => {
                    spec.interval = Some(
                        serde_json::from_value(serde_json::Value::String(other.into()))
                            .map_err(|_| format!("unknown depth or interval {other:?}"))?,
                    );
                }
            }
        }
        if parts.next().is_some() {
            return Err(format!("too many fields in {s:?}"));
        }
        Ok(spec)
    }
}

/// The `"<instrument>|<kind>"` key used in heartbeats.
#[must_use]
pub fn channel_key(instrument: &InstrumentId, kind: RecordKind) -> String {
    format!("{instrument}|{kind}")
}

/// Client to server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientMessage {
    /// Start channels.
    Subscribe {
        /// Echoed in the reply.
        id: String,
        /// Channels.
        channels: Vec<ChannelSpec>,
        /// Also send commitments for the channels' streams.
        #[serde(default)]
        commitments: bool,
    },
    /// Stop channels.
    Unsubscribe {
        /// Echoed in the reply.
        id: String,
        /// Channels, exactly as subscribed.
        channels: Vec<ChannelSpec>,
    },
    /// Ask for a fresh book snapshot.
    Resnapshot {
        /// Echoed in errors.
        id: String,
        /// Instrument.
        instrument: InstrumentId,
        /// Free text, for logs.
        reason: String,
    },
    /// Ask for an inclusion proof.
    Proof {
        /// Echoed in the reply.
        id: String,
        /// Stream.
        stream: StreamId,
        /// Block.
        block_number: u64,
        /// Instrument of the record.
        instrument: InstrumentId,
        /// Index of the record.
        event_index: u32,
    },
    /// Liveness check.
    Ping {
        /// Echoed in the reply.
        id: String,
    },
}

/// Limits in force, announced in `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Connections per client IP.
    pub connections_per_ip: u32,
    /// Channels per connection.
    pub channels_per_connection: u32,
    /// Sustained client messages per second.
    pub messages_per_second: u32,
    /// Client message burst.
    pub message_burst: u32,
    /// Minimum interval between `resnapshot`s for one instrument.
    pub resnapshot_interval_ms: u64,
    /// Proof requests per second.
    pub proofs_per_second: u32,
    /// Send buffer limit, bytes.
    pub send_buffer_bytes: u64,
    /// Send buffer limit: how far behind the newest block, milliseconds.
    pub send_buffer_ms: u64,
    /// Largest accepted client message, bytes.
    pub max_message_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            connections_per_ip: 10,
            channels_per_connection: 200,
            messages_per_second: 20,
            message_burst: 50,
            resnapshot_interval_ms: 5_000,
            proofs_per_second: 50,
            send_buffer_bytes: 8 * 1024 * 1024,
            send_buffer_ms: 2_000,
            max_message_bytes: 64 * 1024,
        }
    }
}

/// Why a snapshot was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotReason {
    /// First snapshot after `subscribe`.
    Subscribe,
    /// Answer to a client `resnapshot`.
    Resnapshot,
    /// The gateway's upstream skipped data.
    UpstreamGap,
    /// The gateway's book failed reconciliation.
    Reconcile,
    /// The gateway restarted from its replay window.
    Restart,
    /// The connection fell behind; queued data was dropped.
    SlowConsumer,
}

impl SnapshotReason {
    /// Text form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Subscribe => "subscribe",
            Self::Resnapshot => "resnapshot",
            Self::UpstreamGap => "upstream_gap",
            Self::Reconcile => "reconcile",
            Self::Restart => "restart",
            Self::SlowConsumer => "slow_consumer",
        }
    }
}

/// Error codes (gateway-v0 § 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Malformed message, unknown field, or a limit exceeded.
    BadRequest,
    /// Not in the registry.
    UnknownInstrument,
    /// Kind or depth not offered.
    UnsupportedKind,
    /// Rate limit exceeded.
    RateLimited,
    /// Proof not available.
    ProofUnavailable,
}

/// The `hello` message body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    /// [`PROTOCOL`].
    pub protocol: String,
    /// Server clock.
    pub server_time_ns: UnixNanos,
    /// Commitment signer addresses.
    pub signers: Vec<EthAddress>,
    /// Limits in force.
    pub limits: Limits,
}

/// Server to client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServerMessage {
    /// First message.
    Hello(Hello),
    /// Subscription acknowledged.
    Subscribed {
        /// Request ID.
        id: String,
        /// Channels.
        channels: Vec<ChannelSpec>,
    },
    /// Unsubscription acknowledged.
    Unsubscribed {
        /// Request ID.
        id: String,
        /// Channels.
        channels: Vec<ChannelSpec>,
    },
    /// Book state after block `as_of.block_number`.
    Snapshot {
        /// Instrument.
        instrument: InstrumentId,
        /// End-of-block marker.
        as_of: Seq,
        /// Why.
        reason: SnapshotReason,
        /// A `book_snapshot` record.
        record: Arc<Record>,
    },
    /// All records of one channel in one block.
    Batch {
        /// The batch.
        batch: Arc<BlockBatch>,
    },
    /// A single record.
    Record {
        /// The record.
        record: Arc<Record>,
    },
    /// A signed Merkle root.
    Commitment {
        /// The commitment.
        commitment: Arc<BlockCommitment>,
    },
    /// Per-block heartbeat.
    Block {
        /// Block number.
        block_number: u64,
        /// Block time.
        block_time_ms: UnixMillis,
        /// When the upstream closed the block.
        ts_publish_ns: UnixNanos,
        /// Last block with a batch, per subscribed channel key.
        last: BTreeMap<String, u64>,
    },
    /// Inclusion proof.
    Proof {
        /// Request ID.
        id: String,
        /// Commitment covering the record.
        commitment: Arc<BlockCommitment>,
        /// Audit path.
        proof: MerkleProof,
        /// The record.
        record: Arc<Record>,
    },
    /// Reply to `ping`.
    Pong {
        /// Request ID.
        id: String,
        /// Server clock.
        server_time_ns: UnixNanos,
    },
    /// Error.
    Error {
        /// Request ID, when the error answers a request.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Code.
        code: ErrorCode,
        /// Human-readable detail.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        let btc = InstrumentId::new("hyperliquid:perp:BTC").unwrap();
        let p: InstrumentPattern = "hyperliquid:perp:*".parse().unwrap();
        assert!(p.matches(&btc));
        assert!(
            !"hyperliquid:spot:*"
                .parse::<InstrumentPattern>()
                .unwrap()
                .matches(&btc)
        );
        assert!(InstrumentPattern::All.matches(&btc));
        assert_eq!(
            "hyperliquid:perp:BTC".parse::<InstrumentPattern>().unwrap(),
            InstrumentPattern::Exact(btc.clone())
        );
        assert!("bad pattern*".parse::<InstrumentPattern>().is_err());
        assert!("**".parse::<InstrumentPattern>().is_err());
        assert!("nocolon".parse::<InstrumentPattern>().is_err());
        assert_eq!(p.to_string(), "hyperliquid:perp:*");
    }

    #[test]
    fn channel_specs_parse_from_cli_form() {
        let c: ChannelSpec = "hyperliquid:perp:BTC|book_event|l2".parse().unwrap();
        assert_eq!(c.kind, RecordKind::BookEvent);
        assert_eq!(c.depth, Some(Depth::L2));
        let c: ChannelSpec = "*|bar|1m".parse().unwrap();
        assert_eq!(c.interval, Some(BarInterval::M1));
        assert!("*|nope".parse::<ChannelSpec>().is_err());
        assert!("*|trade|l4|x".parse::<ChannelSpec>().is_err());
        assert!("*".parse::<ChannelSpec>().is_err());
    }

    #[test]
    fn client_messages_match_the_protocol_doc() {
        let m: ClientMessage = serde_json::from_str(
            r#"{"op":"subscribe","id":"1","channels":[{"instrument":"*","kind":"trade"}],"commitments":true}"#,
        )
        .unwrap();
        assert!(matches!(
            m,
            ClientMessage::Subscribe {
                commitments: true,
                ..
            }
        ));
        let m: ClientMessage = serde_json::from_str(
            r#"{"op":"subscribe","id":"1","channels":[{"instrument":"hyperliquid:perp:BTC","kind":"book_event","depth":"l4"}]}"#,
        )
        .unwrap();
        assert!(matches!(
            m,
            ClientMessage::Subscribe {
                commitments: false,
                ..
            }
        ));
        let m: ClientMessage = serde_json::from_str(
            r#"{"op":"proof","id":"p","stream":{"venue":"hyperliquid","kind":"trade"},"block_number":5,"instrument":"hyperliquid:perp:BTC","event_index":0}"#,
        )
        .unwrap();
        assert!(matches!(
            m,
            ClientMessage::Proof {
                block_number: 5,
                ..
            }
        ));
        assert!(
            serde_json::from_str::<ClientMessage>(r#"{"op":"ping","id":"x","extra":1}"#).is_err()
        );
        assert!(serde_json::from_str::<ClientMessage>(r#"{"op":"nope","id":"x"}"#).is_err());
    }

    #[test]
    fn server_messages_shape() {
        let hello = ServerMessage::Hello(Hello {
            protocol: PROTOCOL.into(),
            server_time_ns: UnixNanos(5),
            signers: vec![],
            limits: Limits::default(),
        });
        let v = serde_json::to_value(&hello).unwrap();
        assert_eq!(v["type"], "hello");
        assert_eq!(v["server_time_ns"], "5");
        assert_eq!(v["limits"]["connections_per_ip"], 10);
        let back: ServerMessage = serde_json::from_value(v).unwrap();
        assert_eq!(back, hello);
        let err = ServerMessage::Error {
            id: None,
            code: ErrorCode::RateLimited,
            message: "slow down".into(),
        };
        assert_eq!(
            serde_json::to_string(&err).unwrap(),
            r#"{"type":"error","code":"rate_limited","message":"slow down"}"#
        );
        assert_eq!(
            Encoding::from_subprotocol(" bleh.v0.cbor"),
            Some(Encoding::Cbor)
        );
        assert_eq!(SnapshotReason::SlowConsumer.as_str(), "slow_consumer");
    }
}
