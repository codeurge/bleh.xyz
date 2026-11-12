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
//! are the direct encoders of [`mesh_types::canonical`], which
//! [`mesh_types::hash::canonical_record`] also uses; they produce exactly
//! the canonical bytes without building a JSON value tree.

use mesh_types::{BookEvent, BookSnapshot, ContentHasher, DerivedHeader, MethodId, Trade};

use crate::error::InputError;

/// Canonical encoding of a trade.
///
/// # Errors
///
/// Never fails; the `Result` keeps the builder signatures uniform.
pub fn canonical_trade(t: &Trade) -> Result<Vec<u8>, InputError> {
    Ok(mesh_types::canonical::trade(t))
}

/// Canonical encoding of a book event.
///
/// # Errors
///
/// Never fails; the `Result` keeps the builder signatures uniform.
pub fn canonical_event(e: &BookEvent) -> Result<Vec<u8>, InputError> {
    Ok(mesh_types::canonical::book_event(e))
}

/// Canonical encoding of a book snapshot.
///
/// # Errors
///
/// Never fails; the `Result` keeps the builder signatures uniform.
pub fn canonical_snapshot(s: &BookSnapshot) -> Result<Vec<u8>, InputError> {
    Ok(mesh_types::canonical::book_snapshot(s))
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
    use mesh_types::{AggressorSide, BookSide, Flag, Record, Seq, UnixNanos};

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
