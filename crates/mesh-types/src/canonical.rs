//! Fast canonical encoders for the high-volume kinds.
//!
//! [`trade`], [`book_event`] and [`book_snapshot`] write the canonical
//! encoding (observation-v0 § 6.1) directly, without building a JSON value
//! tree. They produce exactly the bytes of the generic path (serialize,
//! drop `prov.ts_publish_ns`, write with sorted keys); tests compare both
//! on fixed and random records. [`crate::hash::canonical_record`] uses them
//! for these kinds, so the reader, the block signer, the book builder and
//! the archive all share one implementation.
//!
//! Keys are written in byte order; strings are escaped like
//! [`crate::hash::write_canonical`]. Writing into a `Vec<u8>` cannot fail,
//! so the encoders are infallible.

use std::io::Write as _;

use crate::record::{
    BookAction, BookEvent, BookSnapshot, Level, RestingOrder, SnapshotDepth, Trade,
};
use crate::{Flag, Provenance};

/// Writes a JSON string with the canonical escapes.
pub(crate) fn write_str(out: &mut Vec<u8>, s: &str) {
    out.push(b'"');
    for c in s.chars() {
        match c {
            '"' => out.extend_from_slice(br#"\""#),
            '\\' => out.extend_from_slice(br"\\"),
            '\u{08}' => out.extend_from_slice(br"\b"),
            '\u{0c}' => out.extend_from_slice(br"\f"),
            '\n' => out.extend_from_slice(br"\n"),
            '\r' => out.extend_from_slice(br"\r"),
            '\t' => out.extend_from_slice(br"\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out.push(b'"');
}

/// Writes a value's `Display` text as a JSON string. Used for decimals,
/// whose text has only digits, `-` and `.`.
fn write_display(out: &mut Vec<u8>, v: &dyn std::fmt::Display) {
    out.push(b'"');
    let _ = write!(out, "{v}");
    out.push(b'"');
}

fn write_prov(out: &mut Vec<u8>, p: &Provenance) {
    out.extend_from_slice(br#"{"admission":"#);
    write_str(out, p.admission.as_str());
    out.extend_from_slice(br#","flags":["#);
    let mut first = true;
    for f in Flag::ALL {
        if p.flags.contains(f) {
            if !first {
                out.push(b',');
            }
            first = false;
            write_str(out, f.name());
        }
    }
    out.extend_from_slice(br#"],"instrument":"#);
    write_str(out, p.instrument.as_str());
    let _ = write!(
        out,
        r#","seq":{{"block_number":{},"event_index":{}}},"ts_ingest_ns":"{}","ts_source_ms":{},"venue":"#,
        p.seq.block_number, p.seq.event_index, p.ts_ingest_ns.0, p.ts_source_ms.0
    );
    write_str(out, p.venue.as_str());
    out.push(b'}');
}

/// Canonical encoding of a trade.
#[must_use]
pub fn trade(t: &Trade) -> Vec<u8> {
    let mut out = Vec::with_capacity(320);
    out.extend_from_slice(br#"{"aggressor":"#);
    write_str(&mut out, t.aggressor.as_str());
    out.extend_from_slice(br#","kind":"trade","price":"#);
    write_display(&mut out, &t.price);
    out.extend_from_slice(br#","prov":"#);
    write_prov(&mut out, &t.prov);
    out.extend_from_slice(br#","size":"#);
    write_display(&mut out, &t.size);
    let _ = write!(out, r#","trade_id":{}}}"#, t.trade_id.0);
    out
}

/// Canonical encoding of a book event.
#[must_use]
pub fn book_event(e: &BookEvent) -> Vec<u8> {
    let mut out = Vec::with_capacity(320);
    out.extend_from_slice(br#"{"action":{"#);
    match e.action {
        BookAction::Add { size } => {
            out.extend_from_slice(br#""size":"#);
            write_display(&mut out, &size);
            out.extend_from_slice(br#","type":"add""#);
        }
        BookAction::Modify {
            orig_size,
            new_size,
        }
        | BookAction::Fill {
            orig_size,
            new_size,
        } => {
            out.extend_from_slice(br#""new_size":"#);
            write_display(&mut out, &new_size);
            if let Some(o) = orig_size {
                out.extend_from_slice(br#","orig_size":"#);
                write_display(&mut out, &o);
            }
            out.extend_from_slice(br#","type":"#);
            write_str(&mut out, e.action.as_str());
        }
        BookAction::Cancel {} => out.extend_from_slice(br#""type":"cancel""#),
    }
    let _ = write!(
        out,
        r#"}},"kind":"book_event","order_id":{},"price":"#,
        e.order_id.0
    );
    write_display(&mut out, &e.price);
    out.extend_from_slice(br#","prov":"#);
    write_prov(&mut out, &e.prov);
    out.extend_from_slice(br#","side":"#);
    write_str(&mut out, e.side.as_str());
    out.push(b'}');
    out
}

fn write_orders(out: &mut Vec<u8>, orders: &[RestingOrder]) {
    out.push(b'[');
    for (i, o) in orders.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        let _ = write!(out, r#"{{"order_id":{},"price":"#, o.order_id.0);
        write_display(out, &o.price);
        out.extend_from_slice(br#","size":"#);
        write_display(out, &o.size);
        let _ = write!(out, r#","ts_order_ms":{}}}"#, o.ts_order_ms.0);
    }
    out.push(b']');
}

fn write_levels(out: &mut Vec<u8>, levels: &[Level]) {
    out.push(b'[');
    for (i, l) in levels.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        let _ = write!(out, r#"{{"order_count":{},"price":"#, l.order_count);
        write_display(out, &l.price);
        out.extend_from_slice(br#","size":"#);
        write_display(out, &l.size);
        out.push(b'}');
    }
    out.push(b']');
}

/// Canonical encoding of a book snapshot.
#[must_use]
pub fn book_snapshot(s: &BookSnapshot) -> Vec<u8> {
    let mut out = Vec::with_capacity(512);
    out.extend_from_slice(br#"{"depth":{"asks":"#);
    let kind = match &s.depth {
        SnapshotDepth::L4 { bids, asks } => {
            write_orders(&mut out, asks);
            out.extend_from_slice(br#","bids":"#);
            write_orders(&mut out, bids);
            "l4"
        }
        SnapshotDepth::L2 { bids, asks } => {
            write_levels(&mut out, asks);
            out.extend_from_slice(br#","bids":"#);
            write_levels(&mut out, bids);
            "l2"
        }
    };
    out.extend_from_slice(br#","type":"#);
    write_str(&mut out, kind);
    out.extend_from_slice(br#"},"kind":"book_snapshot","prov":"#);
    write_prov(&mut out, &s.prov);
    out.push(b'}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::canonical_record_generic;
    use crate::record::tests::{book_event as event, px, sz, trade as sample_trade};
    use crate::{
        AggressorSide, BookSide, Flags, InstrumentId, OrderId, Record, Seq, UnixMillis, UnixNanos,
    };

    fn same(r: &Record, fast: &[u8]) {
        assert_eq!(
            String::from_utf8(fast.to_vec()).unwrap(),
            String::from_utf8(canonical_record_generic(r).unwrap()).unwrap()
        );
    }

    #[test]
    fn fast_trade_and_event_equal_generic() {
        let mut t = sample_trade(5, 1);
        t.aggressor = AggressorSide::Unknown;
        t.price = px("-12.50");
        t.prov.flags = Flags::NONE
            .with(Flag::Backfill)
            .with(Flag::OracleAnchoredVenue)
            .with(Flag::Replayed);
        t.prov.ts_ingest_ns = UnixNanos(-7);
        t.prov.stamp_publish(UnixNanos(99));
        t.prov.instrument = InstrumentId::new("hyperliquid:outcome:#9001").unwrap();
        same(&Record::Trade(t.clone()), &trade(&t));
        for action in [
            BookAction::Add { size: sz("1.00") },
            BookAction::Modify {
                orig_size: Some(sz("1.00")),
                new_size: sz("0.50"),
            },
            BookAction::Modify {
                orig_size: None,
                new_size: sz("0.40"),
            },
            BookAction::Fill {
                orig_size: Some(sz("0.40")),
                new_size: sz("0.10"),
            },
            BookAction::Fill {
                orig_size: None,
                new_size: sz("0.00"),
            },
            BookAction::Cancel {},
        ] {
            let e = event(3, 2, action);
            same(&Record::BookEvent(e.clone()), &book_event(&e));
        }
    }

    #[test]
    fn fast_snapshot_equals_generic() {
        let mut prov = sample_trade(9, 0).prov;
        prov.seq = Seq::end_of_block(9);
        let order = |id: u64, p: &str, s: &str| RestingOrder {
            order_id: OrderId(id),
            price: px(p),
            size: sz(s),
            ts_order_ms: UnixMillis(1_000 + i64::try_from(id).unwrap()),
        };
        let l4 = BookSnapshot {
            prov: prov.clone(),
            depth: SnapshotDepth::L4 {
                bids: vec![order(1, "100.0", "1.00"), order(2, "99.5", "2.00")],
                asks: vec![order(3, "101.0", "3.00")],
            },
        };
        same(&Record::BookSnapshot(l4.clone()), &book_snapshot(&l4));
        let l2 = BookSnapshot {
            prov: prov.clone(),
            depth: l4.depth.to_l2().unwrap(),
        };
        same(&Record::BookSnapshot(l2.clone()), &book_snapshot(&l2));
        let empty = BookSnapshot {
            prov,
            depth: SnapshotDepth::L4 {
                bids: vec![],
                asks: vec![],
            },
        };
        same(&Record::BookSnapshot(empty.clone()), &book_snapshot(&empty));
        let _ = BookSide::Bid;
    }

    #[test]
    fn escapes_match_generic() {
        let mut out = Vec::new();
        write_str(&mut out, "x\"\\\n\u{1}\u{8}\u{c}\r\té");
        let mut generic = Vec::new();
        crate::hash::write_canonical(
            &serde_json::Value::String("x\"\\\n\u{1}\u{8}\u{c}\r\té".into()),
            &mut generic,
        )
        .unwrap();
        assert_eq!(out, generic);
    }
}
