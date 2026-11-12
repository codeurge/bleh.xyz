//! JSON and CBOR framing of protocol messages.
//!
//! Both encodings carry the same structures: decimals and `*_ns` stay
//! strings in CBOR too, so values are identical in both.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::protocol::Encoding;

/// Encoding or decoding failure.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// JSON error.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// CBOR encoding error.
    #[error("cbor: {0}")]
    CborEncode(String),
    /// CBOR decoding error.
    #[error("cbor: {0}")]
    CborDecode(String),
    /// A text frame in CBOR mode or a binary frame in JSON mode.
    #[error("frame type does not match the negotiated encoding")]
    WrongFrame,
}

/// Encodes a message.
///
/// # Errors
///
/// Serialization failure.
pub fn encode<T: Serialize>(msg: &T, encoding: Encoding) -> Result<Vec<u8>, CodecError> {
    match encoding {
        Encoding::Json => Ok(serde_json::to_vec(msg)?),
        Encoding::Cbor => {
            let mut out = Vec::with_capacity(256);
            ciborium::into_writer(msg, &mut out)
                .map_err(|e| CodecError::CborEncode(e.to_string()))?;
            Ok(out)
        }
    }
}

/// Decodes a message.
///
/// # Errors
///
/// Malformed input or unknown fields.
pub fn decode<T: DeserializeOwned>(bytes: &[u8], encoding: Encoding) -> Result<T, CodecError> {
    match encoding {
        Encoding::Json => Ok(serde_json::from_slice(bytes)?),
        Encoding::Cbor => {
            ciborium::from_reader(bytes).map_err(|e| CodecError::CborDecode(e.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ChannelSpec, ClientMessage, ServerMessage};
    use mesh_types::{
        AdmissionId, AggressorSide, BlockBatch, InstrumentId, Price, Provenance, Record,
        RecordKind, Seq, Size, Trade, TradeId, UnixMillis, UnixNanos, VenueId,
    };
    use std::sync::Arc;

    fn batch() -> BlockBatch {
        let mut prov = Provenance::new(
            InstrumentId::new("hyperliquid:perp:BTC").unwrap(),
            VenueId::new("hyperliquid").unwrap(),
            Seq::new(u64::MAX - 1, 0),
            UnixMillis(1_767_225_600_000),
            UnixNanos(1_767_225_600_004_000_001),
            AdmissionId::new("ADM-0001").unwrap(),
        );
        prov.stamp_publish(UnixNanos(1_767_225_600_005_000_000));
        BlockBatch {
            instrument: prov.instrument.clone(),
            venue: prov.venue.clone(),
            kind: RecordKind::Trade,
            block_number: u64::MAX - 1,
            block_time_ms: UnixMillis(1_767_225_600_000),
            prev_block_number: None,
            records: vec![Record::Trade(Trade {
                prov,
                price: Price::parse_exact("100.250").unwrap(),
                size: Size::parse_exact("0.001").unwrap(),
                aggressor: AggressorSide::Sell,
                trade_id: TradeId(u64::MAX),
            })],
        }
    }

    #[test]
    fn both_encodings_round_trip_identically() {
        let msg = ServerMessage::Batch {
            batch: Arc::new(batch()),
        };
        for enc in [Encoding::Json, Encoding::Cbor] {
            let bytes = encode(&msg, enc).unwrap();
            let back: ServerMessage = decode(&bytes, enc).unwrap();
            assert_eq!(back, msg, "{enc:?}");
        }
        // In CBOR, decimals and nanosecond times are text strings.
        let cbor = encode(&msg, Encoding::Cbor).unwrap();
        let v: ciborium::Value = ciborium::from_reader(cbor.as_slice()).unwrap();
        let text = format!("{v:?}");
        assert!(text.contains("Text(\"100.250\")"), "{text}");
        assert!(text.contains("Text(\"1767225600004000001\")"), "{text}");
    }

    #[test]
    fn unknown_fields_are_rejected_in_cbor_too() {
        let mut v = serde_json::to_value(ClientMessage::Subscribe {
            id: "1".into(),
            channels: vec![ChannelSpec::new(
                crate::protocol::InstrumentPattern::All,
                RecordKind::Trade,
            )],
            commitments: false,
        })
        .unwrap();
        v["surprise"] = serde_json::json!(true);
        let cbor = encode(&v, Encoding::Cbor).unwrap();
        assert!(decode::<ClientMessage>(&cbor, Encoding::Cbor).is_err());
        assert!(decode::<ClientMessage>(b"\xff\x00", Encoding::Cbor).is_err());
        assert!(decode::<ClientMessage>(b"{", Encoding::Json).is_err());
    }
}
