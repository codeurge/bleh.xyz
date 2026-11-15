//! Filtered raw lines to records.
//!
//! [`RecordParser`] is the seam. Three implementations:
//!
//! - [`NodeReaderParser`]: the node reader's live code path for by-block
//!   fills objects ([`is_by_block`]): each filtered line is parsed by
//!   [`hl_node_reader::raw::ParsedLine`], normalized by
//!   [`hl_node_reader::normalize()`] and sequenced by
//!   [`hl_node_reader::Sequencer`], then re-stamped as backfill.
//! - [`LocalParser`]: this crate's parser for the filtered shapes
//!   [`LocalFilter`](crate::filter::LocalFilter) writes: bare fills to
//!   `trade` records, `node_trades` lines to `trade` records and
//!   `market_data` lines to L2 `book_snapshot` records.
//! - [`DefaultParser`]: what the binary and [`crate::process`] use; the
//!   node reader's path for by-block objects, [`LocalParser`] otherwise.
//!
//! Every record gets admission `ADM-0003`, flag `backfill` (snapshots also
//! `low_resolution`) and the instrument's default flags from the registry.
//! Block numbers come from the line where present and from the
//! [`BlockClock`] otherwise; unresolved events are counted, not guessed.
//!
//! `node_trades` has no venue trade ID. Its records use a derived ID: the
//! first 50 bits (big-endian) of `SHA-256("hl-backfill/node_trades/v1" ||
//! u64_be(min oid) || u64_be(max oid) || u64_be(time_ms))`. The rule is
//! fixed so replays are reproducible; it is not the venue's `tid`.

use std::collections::BTreeMap;
use std::sync::Mutex;

use hl_node_reader::assumptions::venue_decimal;
use hl_node_reader::normalize::JoinedBlock;
use hl_node_reader::raw::{ParsedLine, Stream};
use hl_node_reader::sequence::{Sequencer, SequencerState};
use mesh_types::{
    AdmissionId, AggressorSide, Flag, Flags, InstrumentId, Level, Price, Provenance, Record,
    Registry, Seq, Size, SnapshotDepth, Trade, TradeId, UnixMillis, UnixNanos, VenueId,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::buckets::Dataset;
use crate::clock::BlockClock;
use crate::filter::is_by_block;

/// Errors from parsing. Any error rejects the object.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// A filtered line has an unexpected shape.
    #[error("line {line}: {reason}")]
    Shape {
        /// 1-based line number.
        line: usize,
        /// What is wrong.
        reason: String,
    },
    /// The dataset is not a record source.
    #[error("{0} does not produce records")]
    NotARecordSource(Dataset),
    /// The object is not in the node's `--batch-by-block` format, so the
    /// node reader's parser cannot read it.
    #[error("{0} object is not in the node's batch-by-block format")]
    NotByBlock(Dataset),
}

/// Counts from parsing one object.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParseStats {
    /// Events skipped because no block could be resolved.
    pub unresolved: u64,
    /// Events skipped because the coin is not in the registry.
    pub unknown_coin: u64,
    /// Snapshots skipped because they are crossed or mis-ordered.
    pub invalid: u64,
    /// Snapshots replaced by a later one that resolved to the same block.
    pub superseded: u64,
    /// Intervals (ms) between consecutive snapshots, to measure the
    /// snapshot frequency of `market_data` files.
    pub snapshot_intervals_ms: Vec<i64>,
}

/// Records from one object.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Parsed {
    /// Records, in source order.
    pub records: Vec<Record>,
    /// Counts.
    pub stats: ParseStats,
}

/// Parses the filtered lines of one object.
pub trait RecordParser: Send + Sync {
    /// Parses `lines` from an object of `dataset`.
    ///
    /// # Errors
    ///
    /// Fails if a line cannot be parsed.
    fn parse(
        &self,
        dataset: Dataset,
        lines: &[String],
        clock: &dyn BlockClock,
    ) -> Result<Parsed, ParseError>;
}

/// This crate's parser (see the module docs).
#[derive(Debug, Clone)]
pub struct LocalParser {
    registry: Registry,
    venue: VenueId,
    admission: AdmissionId,
}

fn shape(line: usize, reason: impl Into<String>) -> ParseError {
    ParseError::Shape {
        line,
        reason: reason.into(),
    }
}

fn str_field<'a>(n: usize, v: &'a Value, k: &str) -> Result<&'a str, ParseError> {
    v.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| shape(n, format!("missing string `{k}`")))
}

fn u64_field(n: usize, v: &Value, k: &str) -> Result<u64, ParseError> {
    v.get(k)
        .and_then(Value::as_u64)
        .ok_or_else(|| shape(n, format!("missing integer `{k}`")))
}

fn naive_ns(n: usize, s: &str) -> Result<UnixNanos, ParseError> {
    UnixNanos::parse_rfc3339(s).map_err(|e| shape(n, e.to_string()))
}

fn ms(n: usize, v: u64) -> Result<UnixMillis, ParseError> {
    i64::try_from(v)
        .map(UnixMillis)
        .map_err(|_| shape(n, "time out of range"))
}

fn ms_to_ns(n: usize, t: UnixMillis) -> Result<UnixNanos, ParseError> {
    t.to_nanos().ok_or_else(|| shape(n, "time out of range"))
}

/// One fill, as filtered.
struct Fill {
    line: usize,
    coin: String,
    px: String,
    sz: String,
    side: String,
    crossed: bool,
    tid: u64,
    time: UnixMillis,
}

fn read_fill(n: usize, v: &Value) -> Result<Fill, ParseError> {
    Ok(Fill {
        line: n,
        coin: str_field(n, v, "coin")?.to_owned(),
        px: str_field(n, v, "px")?.to_owned(),
        sz: str_field(n, v, "sz")?.to_owned(),
        side: str_field(n, v, "side")?.to_owned(),
        crossed: v
            .get("crossed")
            .and_then(Value::as_bool)
            .ok_or_else(|| shape(n, "missing boolean `crossed`"))?,
        tid: u64_field(n, v, "tid")?,
        time: ms(n, u64_field(n, v, "time")?)?,
    })
}

fn aggressor(n: usize, side: &str) -> Result<AggressorSide, ParseError> {
    match side {
        "B" => Ok(AggressorSide::Buy),
        "A" => Ok(AggressorSide::Sell),
        other => Err(shape(n, format!("unknown side {other:?}"))),
    }
}

/// Assigns `event_index` per (instrument, block) in source order.
#[derive(Default)]
struct Indexer(BTreeMap<(InstrumentId, u64), u32>);

impl Indexer {
    fn next(&mut self, id: &InstrumentId, block: u64) -> u32 {
        let e = self.0.entry((id.clone(), block)).or_insert(0);
        let i = *e;
        *e += 1;
        i
    }
}

impl LocalParser {
    /// A parser that resolves coins through `registry`.
    ///
    /// # Panics
    ///
    /// Never: the venue and admission IDs are constants.
    #[must_use]
    pub fn new(registry: Registry) -> Self {
        Self {
            registry,
            venue: VenueId::new("hyperliquid").unwrap_or_else(|_| unreachable!()),
            admission: AdmissionId::new(crate::ADMISSION).unwrap_or_else(|_| unreachable!()),
        }
    }

    fn prov(
        &self,
        id: &InstrumentId,
        seq: Seq,
        ts: UnixMillis,
        ingest: UnixNanos,
        flags: Flags,
    ) -> Provenance {
        Provenance::new(
            id.clone(),
            self.venue.clone(),
            seq,
            ts,
            ingest,
            self.admission.clone(),
        )
        .with_flags(flags)
    }

    /// Pairs fills (already book-matched) into trades, in order of each
    /// pair's first fill. `block` gives each pair's block and ingest time.
    fn trades_from_fills(
        &self,
        fills: Vec<Fill>,
        mut block: impl FnMut(&Fill) -> Option<(u64, UnixNanos)>,
        idx: &mut Indexer,
        out: &mut Parsed,
    ) -> Result<(), ParseError> {
        let mut order: Vec<(String, u64)> = Vec::new();
        let mut groups: BTreeMap<(String, u64), Vec<Fill>> = BTreeMap::new();
        for f in fills {
            let k = (f.coin.clone(), f.tid);
            if !groups.contains_key(&k) {
                order.push(k.clone());
            }
            groups.entry(k).or_default().push(f);
        }
        for k in order {
            let pair = groups.remove(&k).unwrap_or_default();
            let n = pair.first().map_or(0, |f| f.line);
            let [a, b] = pair.as_slice() else {
                return Err(shape(
                    n,
                    format!("coin {} tid {} does not have exactly two fills", k.0, k.1),
                ));
            };
            if a.px != b.px || a.sz != b.sz || a.crossed == b.crossed {
                return Err(shape(
                    n,
                    format!("fills of tid {} disagree or have no single taker", k.1),
                ));
            }
            let taker = if a.crossed { a } else { b };
            let Some(spec) = self.registry.by_venue_symbol(&self.venue, &taker.coin) else {
                out.stats.unknown_coin += 1;
                continue;
            };
            let Some((block_number, ingest)) = block(taker) else {
                out.stats.unresolved += 1;
                continue;
            };
            let price = Price::parse_with_scale(&venue_decimal(&taker.px), spec.price_scale)
                .map_err(|e| shape(n, e.to_string()))?;
            let size = Size::parse_with_scale(&venue_decimal(&taker.sz), spec.size_scale)
                .map_err(|e| shape(n, e.to_string()))?;
            let seq = Seq::new(block_number, idx.next(&spec.id, block_number));
            let flags = spec.flags.with(Flag::Backfill);
            out.records.push(Record::from(Trade {
                prov: self.prov(&spec.id, seq, taker.time, ingest, flags),
                price,
                size,
                aggressor: aggressor(n, &taker.side)?,
                trade_id: TradeId(k.1),
            }));
        }
        Ok(())
    }

    fn fills(&self, lines: &[String], clock: &dyn BlockClock) -> Result<Parsed, ParseError> {
        let mut out = Parsed::default();
        let mut idx = Indexer::default();
        let mut bare = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let n = i + 1;
            let v: Value = serde_json::from_str(line).map_err(|e| shape(n, e.to_string()))?;
            if let Some(events) = v.get("events").and_then(Value::as_array) {
                let block = u64_field(n, &v, "block_number")?;
                let ingest = naive_ns(n, str_field(n, &v, "local_time")?)?;
                let fills = events
                    .iter()
                    .map(|e| read_fill(n, e))
                    .collect::<Result<Vec<_>, _>>()?;
                self.trades_from_fills(fills, |_| Some((block, ingest)), &mut idx, &mut out)?;
            } else {
                bare.push(read_fill(n, &v)?);
            }
        }
        if !bare.is_empty() {
            // Bare fills carry no block: resolve it from the clock. Ingest
            // time is the fill time (no receive time in the file).
            self.trades_from_fills(
                bare,
                |f| Some((clock.block_at(f.time)?, f.time.to_nanos()?)),
                &mut idx,
                &mut out,
            )?;
        }
        Ok(out)
    }

    fn node_trades(&self, lines: &[String], clock: &dyn BlockClock) -> Result<Parsed, ParseError> {
        let mut out = Parsed::default();
        let mut idx = Indexer::default();
        for (i, line) in lines.iter().enumerate() {
            let n = i + 1;
            let v: Value = serde_json::from_str(line).map_err(|e| shape(n, e.to_string()))?;
            let coin = str_field(n, &v, "coin")?;
            let ingest = naive_ns(n, str_field(n, &v, "time")?)?;
            let time = ingest.to_millis();
            let oids: Vec<u64> = v
                .get("oids")
                .and_then(Value::as_array)
                .map(|s| s.iter().filter_map(Value::as_u64).collect())
                .unwrap_or_default();
            let [o1, o2] = oids.as_slice() else {
                return Err(shape(n, "oids must hold two order IDs"));
            };
            let Some(spec) = self.registry.by_venue_symbol(&self.venue, coin) else {
                out.stats.unknown_coin += 1;
                continue;
            };
            let Some(block) = clock.block_at(time) else {
                out.stats.unresolved += 1;
                continue;
            };
            let price =
                Price::parse_with_scale(&venue_decimal(str_field(n, &v, "px")?), spec.price_scale)
                    .map_err(|e| shape(n, e.to_string()))?;
            let size =
                Size::parse_with_scale(&venue_decimal(str_field(n, &v, "sz")?), spec.size_scale)
                    .map_err(|e| shape(n, e.to_string()))?;
            let seq = Seq::new(block, idx.next(&spec.id, block));
            out.records.push(Record::from(Trade {
                prov: self.prov(&spec.id, seq, time, ingest, spec.flags.with(Flag::Backfill)),
                price,
                size,
                aggressor: aggressor(n, str_field(n, &v, "side")?)?,
                trade_id: TradeId(derived_trade_id(*o1, *o2, time)),
            }));
        }
        Ok(out)
    }

    fn levels(&self, n: usize, side: &Value, ps: u8, ss: u8) -> Result<Vec<Level>, ParseError> {
        side.as_array()
            .ok_or_else(|| shape(n, "side is not an array"))?
            .iter()
            .map(|l| {
                Ok(Level {
                    price: Price::parse_with_scale(&venue_decimal(str_field(n, l, "px")?), ps)
                        .map_err(|e| shape(n, e.to_string()))?,
                    size: Size::parse_with_scale(&venue_decimal(str_field(n, l, "sz")?), ss)
                        .map_err(|e| shape(n, e.to_string()))?,
                    order_count: u32::try_from(u64_field(n, l, "n")?)
                        .map_err(|_| shape(n, "n out of range"))?,
                })
            })
            .collect()
    }

    fn snapshots(&self, lines: &[String], clock: &dyn BlockClock) -> Result<Parsed, ParseError> {
        let mut out = Parsed::default();
        // Last snapshot per (instrument, block): several snapshots can fall
        // in one block; the record is the book after the block.
        let mut by_block: BTreeMap<(InstrumentId, u64), usize> = BTreeMap::new();
        let mut prev_time: Option<i64> = None;
        for (i, line) in lines.iter().enumerate() {
            let n = i + 1;
            let v: Value = serde_json::from_str(line).map_err(|e| shape(n, e.to_string()))?;
            let data = v
                .get("raw")
                .and_then(|r| r.get("data"))
                .ok_or_else(|| shape(n, "missing raw.data"))?;
            let time = ms(n, u64_field(n, data, "time")?)?;
            if let Some(p) = prev_time {
                out.stats.snapshot_intervals_ms.push(time.0 - p);
            }
            prev_time = Some(time.0);
            let ingest = match v.get("time").and_then(Value::as_str) {
                Some(s) => naive_ns(n, s)?,
                None => ms_to_ns(n, time)?,
            };
            let Some(spec) = self
                .registry
                .by_venue_symbol(&self.venue, str_field(n, data, "coin")?)
            else {
                out.stats.unknown_coin += 1;
                continue;
            };
            let levels = data
                .get("levels")
                .and_then(Value::as_array)
                .filter(|l| l.len() == 2)
                .ok_or_else(|| shape(n, "levels is not [bids, asks]"))?;
            let bids = self.levels(n, &levels[0], spec.price_scale, spec.size_scale)?;
            let asks = self.levels(n, &levels[1], spec.price_scale, spec.size_scale)?;
            let Some(block) = clock.block_at(time) else {
                out.stats.unresolved += 1;
                continue;
            };
            let flags = spec.flags.with(Flag::Backfill).with(Flag::LowResolution);
            let snap = mesh_types::BookSnapshot {
                prov: self.prov(&spec.id, Seq::end_of_block(block), time, ingest, flags),
                depth: SnapshotDepth::L2 { bids, asks },
            };
            if snap.validate().is_err() {
                out.stats.invalid += 1;
                continue;
            }
            let rec = Record::from(snap);
            match by_block.get(&(spec.id.clone(), block)) {
                Some(&at) => {
                    out.records[at] = rec;
                    out.stats.superseded += 1;
                }
                None => {
                    by_block.insert((spec.id.clone(), block), out.records.len());
                    out.records.push(rec);
                }
            }
        }
        Ok(out)
    }
}

/// The derived trade ID for `node_trades` records (see the module docs).
#[must_use]
pub fn derived_trade_id(oid_a: u64, oid_b: u64, time: UnixMillis) -> u64 {
    let (lo, hi) = (oid_a.min(oid_b), oid_a.max(oid_b));
    let mut h = Sha256::new();
    h.update(b"hl-backfill/node_trades/v1");
    h.update(lo.to_be_bytes());
    h.update(hi.to_be_bytes());
    h.update(time.0.to_be_bytes());
    let d = h.finalize();
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[..8]);
    u64::from_be_bytes(b) >> 14
}

impl RecordParser for LocalParser {
    fn parse(
        &self,
        dataset: Dataset,
        lines: &[String],
        clock: &dyn BlockClock,
    ) -> Result<Parsed, ParseError> {
        match dataset {
            Dataset::NodeFillsByBlock | Dataset::NodeFills => self.fills(lines, clock),
            Dataset::NodeTrades => self.node_trades(lines, clock),
            Dataset::MarketData => self.snapshots(lines, clock),
            Dataset::ReplicaCmds | Dataset::ExplorerBlocks => {
                Err(ParseError::NotARecordSource(dataset))
            }
        }
    }
}

/// The node reader's parser, for by-block fills objects
/// ([`is_by_block`]), so backfilled trades come from exactly the live code
/// path.
///
/// Each filtered line is parsed as a fills line
/// ([`ParsedLine::parse`]), joined into a block with no order-status or
/// book-diff line, normalized ([`hl_node_reader::normalize()`]) and
/// sequenced ([`Sequencer`]); the records are then re-stamped with
/// admission `ADM-0003` and flag `backfill`. Everything else, including
/// `seq`, times, trade IDs, instrument flags and `gap_before`, is the node
/// reader's.
///
/// Two adaptations to the buckets, which carry only the fills stream:
///
/// - Normalization marks a block without order-status and book-diff lines
///   incomplete, which would flag a gap on every block. Trades need only
///   the fills line, so the flag is cleared; `gap_before` then comes, as
///   in live reading, from block-number jumps (inside an object and
///   between consecutive objects of a dataset) and from malformed fill
///   groups.
/// - A rejected block (bad time, a pair whose fills disagree, ...) fails
///   the object, as any parse error does in this pipeline, instead of
///   being skipped with a gap.
///
/// Sequencer state is kept per dataset across objects (the pipeline
/// processes objects in hour order) and committed only when an object
/// parses.
#[derive(Debug)]
pub struct NodeReaderParser {
    registry: Registry,
    admission: AdmissionId,
    sequencers: Mutex<BTreeMap<Dataset, SequencerState>>,
}

impl NodeReaderParser {
    /// A parser that resolves coins through `registry`.
    ///
    /// # Panics
    ///
    /// Never: the admission ID is a constant.
    #[must_use]
    pub fn new(registry: Registry) -> Self {
        Self {
            registry,
            admission: AdmissionId::new(crate::ADMISSION).unwrap_or_else(|_| unreachable!()),
            sequencers: Mutex::default(),
        }
    }
}

impl RecordParser for NodeReaderParser {
    fn parse(
        &self,
        dataset: Dataset,
        lines: &[String],
        _clock: &dyn BlockClock,
    ) -> Result<Parsed, ParseError> {
        if !is_by_block(dataset, lines) {
            return Err(ParseError::NotByBlock(dataset));
        }
        let poisoned = |_| shape(0, "sequencer state lock poisoned");
        let state = self
            .sequencers
            .lock()
            .map_err(poisoned)?
            .get(&dataset)
            .cloned()
            .unwrap_or_default();
        let mut sequencer = Sequencer::new(state);
        let mut out = Parsed::default();
        for (i, line) in lines.iter().enumerate() {
            let n = i + 1;
            if line.trim().is_empty() {
                continue;
            }
            let ParsedLine::Fills(fills) =
                ParsedLine::parse(Stream::Fills, line).map_err(|e| shape(n, e.to_string()))?
            else {
                return Err(shape(n, "not a fills line"));
            };
            let joined = JoinedBlock {
                block_number: fills.block_number,
                fills: Some(fills),
                statuses: None,
                diffs: None,
            };
            let mut block = hl_node_reader::normalize(&joined, &self.registry);
            if let Some(why) = &block.rejected {
                return Err(shape(
                    n,
                    format!("block {} rejected: {why}", block.block_number),
                ));
            }
            block.incomplete = false;
            out.stats.unknown_coin += block.stats.unknown_coins.len() as u64;
            let number = block.block_number;
            let Some(output) = sequencer.sequence(block) else {
                return Err(shape(
                    n,
                    format!("block {number} is not after the last block sequenced"),
                ));
            };
            for batch in output.batches {
                for mut r in batch.records {
                    let p = r.prov_mut();
                    p.admission = self.admission.clone();
                    p.flags.insert(Flag::Backfill);
                    out.records.push(r);
                }
            }
        }
        self.sequencers
            .lock()
            .map_err(poisoned)?
            .insert(dataset, sequencer.state().clone());
        Ok(out)
    }
}

/// The parser the binary and the pipeline use: [`NodeReaderParser`] for
/// by-block objects ([`is_by_block`]), [`LocalParser`] for every other
/// bucket format.
#[derive(Debug)]
pub struct DefaultParser {
    /// For by-block objects.
    pub node: NodeReaderParser,
    /// For the other bucket formats.
    pub local: LocalParser,
}

impl DefaultParser {
    /// A parser that resolves coins through `registry`.
    #[must_use]
    pub fn new(registry: Registry) -> Self {
        Self {
            node: NodeReaderParser::new(registry.clone()),
            local: LocalParser::new(registry),
        }
    }
}

impl RecordParser for DefaultParser {
    fn parse(
        &self,
        dataset: Dataset,
        lines: &[String],
        clock: &dyn BlockClock,
    ) -> Result<Parsed, ParseError> {
        if is_by_block(dataset, lines) {
            self.node.parse(dataset, lines, clock)
        } else {
            self.local.parse(dataset, lines, clock)
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::clock::TableClock;
    use crate::filter::tests::{block_line, fill};
    use crate::filter::{LocalFilter, RawLineFilter};
    use mesh_types::{InstrumentSpec, MarketClass};

    pub(crate) fn registry() -> Registry {
        let adm = AdmissionId::new("ADM-0003").unwrap();
        let mut r = Registry::new();
        r.insert(
            InstrumentSpec::hyperliquid(MarketClass::Perp, "BTC", 3, None, adm.clone()).unwrap(),
        )
        .unwrap();
        r.insert(
            InstrumentSpec::hyperliquid(MarketClass::Perp, "xyz:CL", 2, None, adm.clone())
                .unwrap()
                .with_flags(Flags::NONE.with(Flag::OracleAnchoredVenue)),
        )
        .unwrap();
        r.insert(InstrumentSpec::hyperliquid(MarketClass::Perp, "SOL", 2, None, adm).unwrap())
            .unwrap();
        r
    }

    fn filtered(d: Dataset, lines: &[String]) -> Vec<String> {
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        LocalFilter.filter_lines(d, &refs).unwrap().lines
    }

    #[test]
    fn batched_fills_become_trades() {
        let lines = vec![
            block_line(
                100,
                "2025-07-27T00:00:01.000",
                vec![
                    fill(
                        "BTC",
                        "100000.5",
                        "0.010",
                        "A",
                        false,
                        9,
                        1,
                        1_753_574_401_000,
                    ),
                    fill("xyz:CL", "70.25", "3", "B", true, 4, 3, 1_753_574_401_000),
                    fill(
                        "BTC",
                        "100000.5",
                        "0.010",
                        "B",
                        true,
                        9,
                        2,
                        1_753_574_401_000,
                    ),
                    fill("xyz:CL", "70.25", "3", "A", false, 4, 5, 1_753_574_401_000),
                    fill("DOGE", "0.1", "3", "A", false, 6, 7, 1_753_574_401_000),
                    fill("DOGE", "0.1", "3", "B", true, 6, 8, 1_753_574_401_000),
                ],
            ),
            block_line(
                101,
                "2025-07-27T00:00:01.500",
                vec![
                    fill("BTC", "100001", "1", "A", true, 10, 1, 1_753_574_401_500),
                    fill("BTC", "100001", "1", "B", false, 10, 2, 1_753_574_401_500),
                ],
            ),
        ];
        let p = LocalParser::new(registry())
            .parse(
                Dataset::NodeFillsByBlock,
                &filtered(Dataset::NodeFillsByBlock, &lines),
                &TableClock::default(),
            )
            .unwrap();
        assert_eq!(p.records.len(), 3);
        assert_eq!(p.stats.unknown_coin, 1);
        let Record::Trade(t) = &p.records[0] else {
            panic!()
        };
        assert_eq!(t.prov.instrument.as_str(), "hyperliquid:perp:BTC");
        assert_eq!(t.price.to_string(), "100000.500");
        assert_eq!(t.size.to_string(), "0.010");
        assert_eq!(t.aggressor, AggressorSide::Buy);
        assert_eq!(t.trade_id, TradeId(9));
        assert_eq!(t.prov.seq, Seq::new(100, 0));
        assert_eq!(t.prov.admission.as_str(), "ADM-0003");
        assert!(t.prov.flags.contains(Flag::Backfill));
        assert_eq!(
            t.prov.ts_ingest_ns,
            UnixNanos::parse_rfc3339("2025-07-27T00:00:01.000000").unwrap()
        );
        let Record::Trade(cl) = &p.records[1] else {
            panic!()
        };
        assert!(cl.prov.flags.contains(Flag::OracleAnchoredVenue));
        assert_eq!(cl.prov.seq, Seq::new(100, 0));
        let Record::Trade(t2) = &p.records[2] else {
            panic!()
        };
        assert_eq!(
            (t2.prov.seq, t2.aggressor),
            (Seq::new(101, 0), AggressorSide::Sell)
        );
        for r in &p.records {
            mesh_types::allowlist::check_record(r).unwrap();
            r.validate().unwrap();
        }
    }

    #[test]
    fn bare_fills_and_trades_use_the_clock() {
        let clock = TableClock::new(
            [(UnixMillis(1_000_000), 50), (UnixMillis(1_000_400), 51)],
            1000,
        );
        let bare: Vec<String> = [
            fill("SOL", "20.5", "1", "B", true, 1, 1, 1_000_100),
            fill("SOL", "20.5", "1", "A", false, 1, 2, 1_000_100),
            fill("SOL", "20.6", "1", "B", true, 2, 3, 9_000_000),
            fill("SOL", "20.6", "1", "A", false, 2, 4, 9_000_000),
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        let p = LocalParser::new(registry())
            .parse(
                Dataset::NodeFills,
                &filtered(Dataset::NodeFills, &bare),
                &clock,
            )
            .unwrap();
        assert_eq!((p.records.len(), p.stats.unresolved), (1, 1));
        assert_eq!(p.records[0].prov().seq, Seq::new(50, 0));

        let t =
            serde_json::json!({"coin": "SOL", "side": "A", "time": "1970-01-01T00:16:40.500123",
            "px": "20.7", "sz": "2", "trade_dir_override": "Na",
            "side_info": [{"user": "0x1", "oid": 9}, {"user": "0x2", "oid": 8}]})
            .to_string();
        let p = LocalParser::new(registry())
            .parse(
                Dataset::NodeTrades,
                &filtered(Dataset::NodeTrades, &[t]),
                &clock,
            )
            .unwrap();
        let Record::Trade(tr) = &p.records[0] else {
            panic!()
        };
        assert_eq!(tr.prov.seq, Seq::new(51, 0));
        assert_eq!(
            tr.trade_id,
            TradeId(derived_trade_id(8, 9, UnixMillis(1_000_500)))
        );
        assert!(tr.trade_id.0 < 1 << 50);
    }

    pub(crate) fn snapshot_line(coin: &str, time_ms: u64, bid: &str, ask: &str) -> String {
        serde_json::json!({"time": "2023-09-16T09:00:03.296545473", "ver_num": 1,
            "raw": {"channel": "l2Book", "data": {"coin": coin, "time": time_ms,
            "levels": [[{"px": bid, "sz": "5", "n": 2}, {"px": "19.00", "sz": "1", "n": 1}],
                       [{"px": ask, "sz": "3", "n": 1}]]}}})
        .to_string()
    }

    #[test]
    fn snapshots_become_l2_records() {
        let clock = TableClock::new([(UnixMillis(1000), 7), (UnixMillis(2000), 8)], 5000);
        let lines = vec![
            snapshot_line("SOL", 1000, "19.90", "20.10"),
            snapshot_line("SOL", 1500, "19.95", "20.10"),
            snapshot_line("SOL", 2000, "19.95", "20.05"),
            snapshot_line("SOL", 2500, "20.20", "20.05"),
        ];
        let p = LocalParser::new(registry())
            .parse(
                Dataset::MarketData,
                &filtered(Dataset::MarketData, &lines),
                &clock,
            )
            .unwrap();
        assert_eq!(p.records.len(), 2);
        assert_eq!(p.stats.superseded, 1);
        assert_eq!(p.stats.invalid, 1);
        assert_eq!(p.stats.snapshot_intervals_ms, [500, 500, 500]);
        let Record::BookSnapshot(s) = &p.records[0] else {
            panic!()
        };
        assert_eq!(s.prov.seq, Seq::end_of_block(7));
        assert!(
            s.prov.flags.contains(Flag::LowResolution) && s.prov.flags.contains(Flag::Backfill)
        );
        let SnapshotDepth::L2 { bids, .. } = &s.depth else {
            panic!()
        };
        assert_eq!(bids[0].price.to_string(), "19.9500");
        for r in &p.records {
            mesh_types::allowlist::check_record(r).unwrap();
        }
    }

    /// A trade in hl-synth's `expected.json` shape.
    pub(crate) fn trade_shape(r: &Record) -> Value {
        let Record::Trade(t) = r else {
            panic!("not a trade: {r:?}")
        };
        serde_json::json!({
            "seq": [t.prov.seq.block_number, t.prov.seq.event_index],
            "ts_source_ms": t.prov.ts_source_ms.0,
            "trade_id": t.trade_id.0,
            "price": t.price.to_string(),
            "size": t.size.to_string(),
            "aggressor": t.aggressor.as_str(),
        })
    }

    /// Filters and parses hl-synth's two hourly fills files of `scenario`
    /// as two `node_fills_by_block` objects, with the default stages.
    fn synth_records(
        files: &hl_synth::FileSet,
        scenario: &str,
        registry: &Registry,
    ) -> Vec<Record> {
        let parser = DefaultParser::new(registry.clone());
        let mut out = Vec::new();
        for hour in ["0", "1"] {
            let key = format!("{scenario}/node_fills_by_block/hourly/20260101/{hour}");
            let text = String::from_utf8(files[&key].clone()).unwrap();
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            let f = crate::filter::DefaultFilter::default()
                .filter_lines(Dataset::NodeFillsByBlock, &lines)
                .unwrap();
            let p = parser
                .parse(Dataset::NodeFillsByBlock, &f.lines, &TableClock::default())
                .unwrap();
            out.extend(p.records);
        }
        out
    }

    #[test]
    fn node_reader_path_reproduces_the_synthetic_ground_truth() {
        let files = hl_synth::generate(hl_synth::Config::default()).unwrap();
        let registry: Registry = serde_json::from_slice(&files["markets.json"]).unwrap();
        for scenario in ["base", "gap"] {
            let expected: Value =
                serde_json::from_slice(&files[&format!("{scenario}/expected.json")]).unwrap();
            let records = synth_records(&files, scenario, &registry);
            let mut by_coin: BTreeMap<String, Vec<Value>> = BTreeMap::new();
            let mut flagged: Vec<(InstrumentId, Seq)> = Vec::new();
            for r in &records {
                let p = r.prov();
                assert_eq!(p.admission.as_str(), "ADM-0003");
                assert!(p.flags.contains(Flag::Backfill));
                r.validate().unwrap();
                mesh_types::allowlist::check_record(r).unwrap();
                let spec = registry.get(&p.instrument).unwrap();
                assert_eq!(
                    p.flags.contains(Flag::OracleAnchoredVenue),
                    spec.flags.contains(Flag::OracleAnchoredVenue)
                );
                if p.flags.contains(Flag::GapBefore) {
                    flagged.push((p.instrument.clone(), p.seq));
                }
                by_coin
                    .entry(spec.venue_symbol.clone())
                    .or_default()
                    .push(trade_shape(r));
            }
            let coins = expected["coins"].as_object().unwrap();
            assert_eq!(coins.len(), by_coin.len(), "{scenario}");
            for (coin, c) in coins {
                assert_eq!(
                    &by_coin[coin],
                    c["trades"].as_array().unwrap(),
                    "{scenario} {coin}"
                );
            }
            match expected["missing_blocks"][0].as_u64() {
                None => assert!(flagged.is_empty(), "{flagged:?}"),
                Some(missing) => {
                    // The first trade of each market after the missing
                    // block carries gap_before, and no other.
                    assert!(!flagged.is_empty());
                    for (id, seq) in &flagged {
                        let first_after = records
                            .iter()
                            .filter(|r| &r.prov().instrument == id)
                            .map(|r| r.prov().seq)
                            .find(|s| s.block_number > missing)
                            .unwrap();
                        assert_eq!(*seq, first_after);
                    }
                    let ids: std::collections::BTreeSet<_> =
                        flagged.iter().map(|(id, _)| id).collect();
                    assert_eq!(ids.len(), flagged.len());
                }
            }
        }
    }

    #[test]
    fn node_reader_parser_keeps_sequencer_state_per_dataset() {
        let reg = registry();
        let p = NodeReaderParser::new(reg.clone());
        let bare = vec![fill("SOL", "20.5", "1", "B", true, 1, 1, 1_000_100).to_string()];
        assert_eq!(
            p.parse(Dataset::NodeFills, &bare, &TableClock::default()),
            Err(ParseError::NotByBlock(Dataset::NodeFills))
        );
        let line = |b: u64| {
            serde_json::json!({"block_number": b, "block_time": "2025-07-27T00:00:01.000",
                "events": [], "local_time": "2025-07-27T00:00:01.000000001"})
            .to_string()
        };
        let clock = TableClock::default();
        p.parse(Dataset::NodeFillsByBlock, &[line(5)], &clock)
            .unwrap();
        // A block at or below the last one sequenced fails the object and
        // leaves the state unchanged.
        assert!(
            p.parse(Dataset::NodeFillsByBlock, &[line(5)], &clock)
                .is_err()
        );
        // State is per dataset.
        p.parse(Dataset::NodeFills, &[line(5)], &clock).unwrap();
        p.parse(Dataset::NodeFillsByBlock, &[line(6)], &clock)
            .unwrap();
    }
}
