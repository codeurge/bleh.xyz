//! Input hashing for derived records.
//!
//! Every derived record carries a [`DerivedHeader`]: the method
//! (`name@version`), the [`ContentHash`](mesh_types::ContentHash) of its
//! input records and their number. Inputs are hashed from their canonical
//! encodings ([`mesh_types::hash::canonical_record`]); the processor encodes
//! each input record once and feeds the bytes to every builder that uses
//! it.
//!
//! # Input order
//!
//! Inputs of one kind are hashed in `seq` order, which is the archive's
//! canonical order for one instrument and kind. Analytics with inputs of
//! more than one kind (trades and book events) hash them in
//! **block-major order**: by block, then kind (`trade`, `book_event`,
//! `book_snapshot`, the order of [`RecordKind`](mesh_types::RecordKind)),
//! then `event_index`. That order can be produced while streaming, block
//! by block; see `docs/operations/book.md`.

//!
//! # Fast encoding
//!
//! [`canonical_trade`], [`canonical_event`] and [`canonical_snapshot`]
//! write the canonical encoding directly, without building a JSON value
//! tree, and produce exactly the bytes of
//! [`mesh_types::hash::canonical_record`] (tests compare them on random
//! and fixture records). Keys are written in byte order; strings are
//! escaped like `mesh_types::hash`.

use std::io::Write as _;

use mesh_types::{
    BookAction, BookEvent, BookSnapshot, ContentHasher, DerivedHeader, Flag, Level, MethodId,
    Provenance, RestingOrder, SnapshotDepth, Trade,
};

use crate::error::InputError;

fn io(e: std::io::Error) -> InputError {
    InputError::Canonical(e.to_string())
}

/// Writes a JSON string with the escapes of `mesh_types::hash`.
fn write_str(out: &mut Vec<u8>, s: &str) {
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
fn write_display(out: &mut Vec<u8>, v: &dyn std::fmt::Display) -> Result<(), InputError> {
    out.push(b'"');
    write!(out, "{v}").map_err(io)?;
    out.push(b'"');
    Ok(())
}

fn write_prov(out: &mut Vec<u8>, p: &Provenance) -> Result<(), InputError> {
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
    write!(
        out,
        r#","seq":{{"block_number":{},"event_index":{}}},"ts_ingest_ns":"{}","ts_source_ms":{},"venue":"#,
        p.seq.block_number, p.seq.event_index, p.ts_ingest_ns.0, p.ts_source_ms.0
    )
    .map_err(io)?;
    write_str(out, p.venue.as_str());
    out.push(b'}');
    Ok(())
}

/// Canonical encoding of a trade.
///
/// # Errors
///
/// Fails only if writing fails.
pub fn canonical_trade(t: &Trade) -> Result<Vec<u8>, InputError> {
    let mut out = Vec::with_capacity(320);
    out.extend_from_slice(br#"{"aggressor":"#);
    write_str(&mut out, t.aggressor.as_str());
    out.extend_from_slice(br#","kind":"trade","price":"#);
    write_display(&mut out, &t.price)?;
    out.extend_from_slice(br#","prov":"#);
    write_prov(&mut out, &t.prov)?;
    out.extend_from_slice(br#","size":"#);
    write_display(&mut out, &t.size)?;
    write!(out, r#","trade_id":{}}}"#, t.trade_id.0).map_err(io)?;
    Ok(out)
}

/// Canonical encoding of a book event.
///
/// # Errors
///
/// Fails only if writing fails.
pub fn canonical_event(e: &BookEvent) -> Result<Vec<u8>, InputError> {
    let mut out = Vec::with_capacity(320);
    out.extend_from_slice(br#"{"action":{"#);
    match e.action {
        BookAction::Add { size } => {
            out.extend_from_slice(br#""size":"#);
            write_display(&mut out, &size)?;
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
            write_display(&mut out, &new_size)?;
            if let Some(o) = orig_size {
                out.extend_from_slice(br#","orig_size":"#);
                write_display(&mut out, &o)?;
            }
            out.extend_from_slice(br#","type":"#);
            write_str(&mut out, e.action.as_str());
        }
        BookAction::Cancel {} => out.extend_from_slice(br#""type":"cancel""#),
    }
    write!(
        out,
        r#"}},"kind":"book_event","order_id":{},"price":"#,
        e.order_id.0
    )
    .map_err(io)?;
    write_display(&mut out, &e.price)?;
    out.extend_from_slice(br#","prov":"#);
    write_prov(&mut out, &e.prov)?;
    out.extend_from_slice(br#","side":"#);
    write_str(&mut out, e.side.as_str());
    out.push(b'}');
    Ok(out)
}

fn write_orders(out: &mut Vec<u8>, orders: &[RestingOrder]) -> Result<(), InputError> {
    out.push(b'[');
    for (i, o) in orders.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        write!(out, r#"{{"order_id":{},"price":"#, o.order_id.0).map_err(io)?;
        write_display(out, &o.price)?;
        out.extend_from_slice(br#","size":"#);
        write_display(out, &o.size)?;
        write!(out, r#","ts_order_ms":{}}}"#, o.ts_order_ms.0).map_err(io)?;
    }
    out.push(b']');
    Ok(())
}

fn write_levels(out: &mut Vec<u8>, levels: &[Level]) -> Result<(), InputError> {
    out.push(b'[');
    for (i, l) in levels.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        write!(out, r#"{{"order_count":{},"price":"#, l.order_count).map_err(io)?;
        write_display(out, &l.price)?;
        out.extend_from_slice(br#","size":"#);
        write_display(out, &l.size)?;
        out.push(b'}');
    }
    out.push(b']');
    Ok(())
}

/// Canonical encoding of a book snapshot.
///
/// # Errors
///
/// Fails only if writing fails.
pub fn canonical_snapshot(s: &BookSnapshot) -> Result<Vec<u8>, InputError> {
    let mut out = Vec::with_capacity(512);
    out.extend_from_slice(br#"{"depth":{"asks":"#);
    let kind = match &s.depth {
        SnapshotDepth::L4 { bids, asks } => {
            write_orders(&mut out, asks)?;
            out.extend_from_slice(br#","bids":"#);
            write_orders(&mut out, bids)?;
            "l4"
        }
        SnapshotDepth::L2 { bids, asks } => {
            write_levels(&mut out, asks)?;
            out.extend_from_slice(br#","bids":"#);
            write_levels(&mut out, bids)?;
            "l2"
        }
    };
    out.extend_from_slice(br#","type":"#);
    write_str(&mut out, kind);
    out.extend_from_slice(br#"},"kind":"book_snapshot","prov":"#);
    write_prov(&mut out, &s.prov)?;
    out.push(b'}');
    Ok(out)
}

/// A trade together with its canonical encoding.
#[derive(Debug, Clone, Copy)]
pub struct TradeIn<'a> {
    /// The trade.
    pub trade: &'a Trade,
    /// Its canonical encoding ([`canonical_trade`]).
    pub canonical: &'a [u8],
}

/// A book event together with its canonical encoding.
#[derive(Debug, Clone, Copy)]
pub struct EventIn<'a> {
    /// The event.
    pub event: &'a BookEvent,
    /// Its canonical encoding ([`canonical_event`]).
    pub canonical: &'a [u8],
}

/// Encodes every trade once.
///
/// # Errors
///
/// Fails if a record cannot be encoded.
pub fn encode_trades(trades: &[Trade]) -> Result<Vec<Vec<u8>>, InputError> {
    trades.iter().map(canonical_trade).collect()
}

/// Encodes every event once.
///
/// # Errors
///
/// Fails if a record cannot be encoded.
pub fn encode_events(events: &[BookEvent]) -> Result<Vec<Vec<u8>>, InputError> {
    events.iter().map(canonical_event).collect()
}

/// Checks that records arrive for one instrument, in strictly increasing
/// `seq` and non-decreasing source time.
#[derive(Debug, Clone, Default)]
pub(crate) struct Cursor {
    instrument: Option<mesh_types::InstrumentId>,
    last_seq: Option<mesh_types::Seq>,
    watermark: Option<mesh_types::UnixMillis>,
}

impl Cursor {
    /// Checks and records the next record's provenance.
    pub(crate) fn check(&mut self, prov: &mesh_types::Provenance) -> Result<(), InputError> {
        match &self.instrument {
            Some(i) if *i != prov.instrument => {
                return Err(InputError::InstrumentMismatch {
                    expected: i.clone(),
                    found: prov.instrument.clone(),
                });
            }
            Some(_) => {}
            None => self.instrument = Some(prov.instrument.clone()),
        }
        if let Some(last) = self.last_seq
            && prov.seq <= last
        {
            return Err(InputError::OutOfOrder {
                last,
                got: prov.seq,
            });
        }
        if let Some(w) = self.watermark
            && prov.ts_source_ms < w
        {
            return Err(InputError::TimeWentBackwards {
                last: w,
                got: prov.ts_source_ms,
            });
        }
        self.last_seq = Some(prov.seq);
        self.watermark = Some(prov.ts_source_ms);
        Ok(())
    }

    /// Records that time has reached `ts`: later records must not be
    /// earlier.
    pub(crate) fn advance(&mut self, ts: mesh_types::UnixMillis) {
        if self.watermark.is_none_or(|w| ts > w) {
            self.watermark = Some(ts);
        }
    }
}

/// Builds the header for `method` from a finished hasher.
pub(crate) fn header(method: &str, hasher: ContentHasher) -> Result<DerivedHeader, InputError> {
    let method = MethodId::new(method).map_err(|e| InputError::InvalidConfig(e.to_string()))?;
    let input_count = hasher.count();
    Ok(DerivedHeader {
        method,
        inputs_hash: hasher.finalize(),
        input_count,
    })
}

/// Builds a header from a hasher that keeps running (cumulative inputs).
pub(crate) fn header_of_clone(
    method: &str,
    hasher: &ContentHasher,
) -> Result<DerivedHeader, InputError> {
    header(method, hasher.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use mesh_types::hash::canonical_record;
    use mesh_types::{AggressorSide, BookSide, Record, Seq, UnixNanos};

    #[test]
    fn fast_encoding_equals_mesh_types() {
        let mut t = trade(5, 1, -3, "-12.50", "0.001", AggressorSide::Unknown);
        t.prov.flags = t
            .prov
            .flags
            .with(Flag::Backfill)
            .with(Flag::OracleAnchoredVenue)
            .with(Flag::BlockTradeDelayed);
        t.prov.ts_ingest_ns = UnixNanos(-7);
        t.prov.stamp_publish(UnixNanos(99));
        t.prov.instrument = mesh_types::InstrumentId::new("hyperliquid:outcome:#9001").unwrap();
        assert_eq!(
            canonical_trade(&t).unwrap(),
            canonical_record(&Record::Trade(t.clone())).unwrap()
        );
        let events = [
            add(1, 0, 1, BookSide::Bid, "100.0", "1.00"),
            modify(1, 1, 1, BookSide::Bid, "100.0", Some("1.00"), "0.50"),
            modify(1, 2, 1, BookSide::Bid, "100.0", None, "0.40"),
            fill(1, 3, 1, BookSide::Ask, "100.0", Some("0.40"), "0.10"),
            fill(1, 4, 1, BookSide::Ask, "100.0", None, "0.00"),
            cancel(1, 5, 1, BookSide::Ask, "0"),
        ];
        for e in &events {
            assert_eq!(
                canonical_event(e).unwrap(),
                canonical_record(&Record::BookEvent(e.clone())).unwrap()
            );
        }
        let mut book = crate::OrderBook::new(inst());
        book.apply_block(
            1,
            mesh_types::UnixMillis(block_ms(1)),
            &[
                add(1, 0, 1, BookSide::Bid, "100.0", "1.00"),
                add(1, 1, 2, BookSide::Ask, "101.0", "2.00"),
                add(1, 2, 3, BookSide::Ask, "101.0", "3.00"),
            ],
        )
        .unwrap();
        let mut p = prov_eob(1, 1_000);
        p.seq = Seq::end_of_block(1);
        for s in [
            book.to_snapshot(p.clone()).unwrap(),
            book.to_l2_snapshot(p.clone(), None).unwrap(),
            crate::OrderBook::new(inst())
                .to_snapshot(p.clone())
                .unwrap(),
        ] {
            assert_eq!(
                canonical_snapshot(&s).unwrap(),
                canonical_record(&Record::BookSnapshot(s.clone())).unwrap()
            );
        }
    }
}
