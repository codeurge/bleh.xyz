//! The field filter: raw archive lines in, filtered raw lines out.
//!
//! [`RawLineFilter`] is the seam. Three implementations:
//!
//! - [`NodeReaderFilter`]: the node reader's ADM-0001 filter
//!   ([`hl_node_reader::filter_line`]), for objects in the node's
//!   `--batch-by-block` fills format (see [`is_by_block`]). Its output is
//!   the node reader's filtered raw: the same block lines with only
//!   admitted fields, bare fill objects, no spot events and no fill groups
//!   that are not book trades; byte for byte what live capture writes for
//!   the same lines (sorted keys, no whitespace).
//! - [`LocalFilter`]: this crate's own filter, written from
//!   `docs/specs/hyperliquid-archive-buckets.md` § 4, for the bucket
//!   formats the node reader cannot read (`node_trades`, `market_data` L2
//!   snapshots, bare `node_fills` lines).
//! - [`DefaultFilter`]: what the binary and [`crate::process`] use; it
//!   sends each object to [`NodeReaderFilter`] when [`is_by_block`] holds
//!   and to [`LocalFilter`] otherwise.
//!
//! Every filter output line is canonical JSON (sorted keys, no
//! whitespace) and holds only fields on an explicit keep-list, so unknown
//! or excluded fields can never pass. [`LocalFilter`] shapes:
//!
//! | Dataset | Raw line | Filtered line |
//! |---|---|---|
//! | batched fills (tests only; the default sends these to the node reader) | `{local_time, block_time, block_number, events: [[user, fill]...]}` | same envelope, `events: [fill...]` with book-matched fills only |
//! | bare `node_fills` | `[user, fill]` | `fill`, book-matched (paired within the file) |
//! | `node_trades` | `{coin, side, time, px, sz, hash, trade_dir_override, side_info}` | `{coin, side, time, px, sz, oids: [oid, oid]}` (`side_info` holds owner addresses and is a denylisted name, so only its two `oid`s are kept, under `oids`) |
//! | `market_data` | `{time, ver_num, raw: {channel, data: {coin, time, levels}}}` | same, with only those fields and level fields `px`, `sz`, `n` |
//!
//! A fill is kept as `{coin, px, sz, side, time, crossed, tid, oid}`.

use std::collections::{BTreeMap, BTreeSet};

use hl_node_reader::raw::Stream;
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::{Map, Value};

use crate::buckets::Dataset;

/// Fill fields admitted by ADM-0001.
pub const FILL_KEEP: [&str; 8] = ["coin", "px", "sz", "side", "time", "crossed", "tid", "oid"];

/// Errors from filtering. Any error rejects the whole object: the archive
/// never carries a partially understood file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FilterError {
    /// A line is not valid JSON.
    #[error("line {line}: invalid JSON: {reason}")]
    Json {
        /// 1-based line number.
        line: usize,
        /// Parser message.
        reason: String,
    },
    /// A line does not have the expected shape.
    #[error("line {line}: {reason}")]
    Shape {
        /// 1-based line number.
        line: usize,
        /// What is wrong.
        reason: String,
    },
    /// The dataset is replay input only; its raw lines are never published.
    #[error("{0} is replay input only and has no filtered raw form")]
    NotPublished(Dataset),
    /// The object is not in the node's `--batch-by-block` format, so the
    /// node reader's filter cannot read it.
    #[error("{0} object is not in the node's batch-by-block format")]
    NotByBlock(Dataset),
}

/// True if an object of `dataset` with these lines is in the node's
/// `--batch-by-block` fills format, which the node reader reads:
/// `node_fills_by_block` always (`hyperliquid-archive-buckets.md` § 4.1),
/// and `node_fills` when its lines are block objects rather than bare
/// `[user, fill]` pairs (§ 4.2: batching is unverified, so it is decided
/// per object from the first non-blank line). Every other dataset has its
/// own format.
#[must_use]
pub fn is_by_block<S: AsRef<str>>(dataset: Dataset, lines: &[S]) -> bool {
    match dataset {
        Dataset::NodeFillsByBlock => true,
        // A block object has an `events` array. Checking for it (not just
        // a leading `{`) also classifies filtered lines correctly: the
        // local filter writes bare fills as `{"coin": ...}` objects.
        Dataset::NodeFills => lines
            .iter()
            .map(|l| l.as_ref().trim_start())
            .find(|l| !l.is_empty())
            .is_some_and(|l| {
                l.starts_with('{')
                    && serde_json::from_str::<Value>(l)
                        .ok()
                        .and_then(|v| v.get("events").map(Value::is_array))
                        .unwrap_or(false)
            }),
        _ => false,
    }
}

/// Counts from filtering one object.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterStats {
    /// Lines read.
    pub lines_in: u64,
    /// Lines written.
    pub lines_out: u64,
    /// Events (fills, trades, snapshots) kept.
    pub events_kept: u64,
    /// Events dropped (not book-matched, liquidation backstop, unknown
    /// `trade_dir_override`, ...).
    pub events_dropped: u64,
}

/// Filtered lines of one object.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filtered {
    /// Canonical JSON lines, without trailing newlines.
    pub lines: Vec<String>,
    /// Counts.
    pub stats: FilterStats,
}

/// Filters the lines of one decompressed source object.
///
/// Implementations must keep only admitted fields (ADM-0001 field rules,
/// as applied by ADM-0003) and only book-matched fills, and must preserve
/// line order and block envelopes.
pub trait RawLineFilter: Send + Sync {
    /// Filters `lines` (one object's lines, in order).
    ///
    /// # Errors
    ///
    /// Fails if any line cannot be understood.
    fn filter_lines(&self, dataset: Dataset, lines: &[&str]) -> Result<Filtered, FilterError>;
}

/// The node reader's filter, for objects where [`is_by_block`] holds.
///
/// Each line goes through [`hl_node_reader::filter_line`] for the fills
/// stream, unchanged: unknown fields fail (schema drift), spot events and
/// fill groups that are not book trades are removed, and malformed groups
/// (an unmatched fill) keep their admitted fields so the node reader's
/// parser flags them, exactly as in live capture.
#[derive(Debug, Clone, Copy, Default)]
pub struct NodeReaderFilter;

/// Only the event count of a block line (other fields are skipped).
#[derive(Deserialize)]
struct EventCount {
    events: Vec<IgnoredAny>,
}

fn event_count(n: usize, line: &str) -> Result<u64, FilterError> {
    serde_json::from_str::<EventCount>(line)
        .map(|c| c.events.len() as u64)
        .map_err(|e| FilterError::Json {
            line: n,
            reason: e.to_string(),
        })
}

impl RawLineFilter for NodeReaderFilter {
    fn filter_lines(&self, dataset: Dataset, lines: &[&str]) -> Result<Filtered, FilterError> {
        if !is_by_block(dataset, lines) {
            return Err(FilterError::NotByBlock(dataset));
        }
        let mut out = Filtered::default();
        for (i, line) in lines.iter().enumerate() {
            let n = i + 1;
            out.stats.lines_in += 1;
            if line.trim().is_empty() {
                continue;
            }
            let filtered = hl_node_reader::filter_line(Stream::Fills, line)
                .map_err(|e| shape(n, e.to_string()))?;
            let (before, after) = (event_count(n, line)?, event_count(n, &filtered)?);
            out.stats.events_kept += after;
            out.stats.events_dropped += before.saturating_sub(after);
            out.lines.push(filtered);
        }
        out.stats.lines_out = out.lines.len() as u64;
        Ok(out)
    }
}

/// The filter the binary and the pipeline use: [`NodeReaderFilter`] for
/// by-block objects ([`is_by_block`]), [`LocalFilter`] for every other
/// bucket format.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultFilter {
    /// For by-block objects.
    pub node: NodeReaderFilter,
    /// For the other bucket formats.
    pub local: LocalFilter,
}

impl RawLineFilter for DefaultFilter {
    fn filter_lines(&self, dataset: Dataset, lines: &[&str]) -> Result<Filtered, FilterError> {
        if is_by_block(dataset, lines) {
            self.node.filter_lines(dataset, lines)
        } else {
            self.local.filter_lines(dataset, lines)
        }
    }
}

/// This crate's own filter (see the module docs).
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalFilter;

fn shape(line: usize, reason: impl Into<String>) -> FilterError {
    FilterError::Shape {
        line,
        reason: reason.into(),
    }
}

fn parse_line(n: usize, line: &str) -> Result<Value, FilterError> {
    serde_json::from_str(line).map_err(|e| FilterError::Json {
        line: n,
        reason: e.to_string(),
    })
}

fn canonical(n: usize, v: &Value) -> Result<String, FilterError> {
    let bytes = mesh_types::canonical_json(v).map_err(|e| shape(n, e.to_string()))?;
    String::from_utf8(bytes).map_err(|e| shape(n, e.to_string()))
}

/// Copies `keep` fields of `obj`; every one must be present.
fn pick(
    n: usize,
    obj: &Map<String, Value>,
    keep: &[&str],
    what: &str,
) -> Result<Map<String, Value>, FilterError> {
    let mut out = Map::new();
    for k in keep {
        let v = obj
            .get(*k)
            .ok_or_else(|| shape(n, format!("{what} without `{k}`")))?;
        out.insert((*k).to_owned(), v.clone());
    }
    Ok(out)
}

/// One fill with the facts needed to pair it.
struct FillIn {
    kept: Map<String, Value>,
    coin: String,
    tid: u64,
    crossed: bool,
    side: String,
    backstop: bool,
}

fn read_fill(n: usize, event: &Value) -> Result<FillIn, FilterError> {
    // An event is `[user, fill]`; the user is never read.
    let fill = match event {
        Value::Array(pair) if pair.len() == 2 => &pair[1],
        _ => return Err(shape(n, "fill event is not a [user, fill] pair")),
    };
    let obj = fill
        .as_object()
        .ok_or_else(|| shape(n, "fill is not an object"))?;
    let kept = pick(n, obj, &FILL_KEEP, "fill")?;
    let coin = kept["coin"]
        .as_str()
        .ok_or_else(|| shape(n, "coin is not a string"))?
        .to_owned();
    let tid = kept["tid"]
        .as_u64()
        .ok_or_else(|| shape(n, "tid is not an integer"))?;
    let crossed = kept["crossed"]
        .as_bool()
        .ok_or_else(|| shape(n, "crossed is not a boolean"))?;
    let side = kept["side"]
        .as_str()
        .ok_or_else(|| shape(n, "side is not a string"))?
        .to_owned();
    if side != "B" && side != "A" {
        return Err(shape(n, format!("unknown fill side {side:?}")));
    }
    for k in ["px", "sz"] {
        if !kept[k].is_string() {
            return Err(shape(n, format!("{k} is not a string")));
        }
    }
    if !kept["time"].is_u64() || !kept["oid"].is_u64() {
        return Err(shape(n, "time and oid must be integers"));
    }
    let backstop = obj
        .get("liquidation")
        .and_then(|l| l.get("method"))
        .and_then(Value::as_str)
        == Some("backstop");
    Ok(FillIn {
        kept,
        coin,
        tid,
        crossed,
        side,
        backstop,
    })
}

/// Indices of fills that form book-matched pairs: exactly two fills per
/// `(coin, tid)`, opposite sides, exactly one `crossed`, neither a
/// backstop liquidation.
fn book_matched(fills: &[FillIn]) -> BTreeSet<usize> {
    let mut groups: BTreeMap<(&str, u64), Vec<usize>> = BTreeMap::new();
    for (i, f) in fills.iter().enumerate() {
        groups.entry((f.coin.as_str(), f.tid)).or_default().push(i);
    }
    let mut keep = BTreeSet::new();
    for idx in groups.values() {
        if let [a, b] = idx.as_slice() {
            let (fa, fb) = (&fills[*a], &fills[*b]);
            if fa.crossed != fb.crossed && fa.side != fb.side && !fa.backstop && !fb.backstop {
                keep.insert(*a);
                keep.insert(*b);
            }
        }
    }
    keep
}

impl LocalFilter {
    fn fills_batched(
        n: usize,
        obj: &Map<String, Value>,
        out: &mut Filtered,
    ) -> Result<String, FilterError> {
        let mut env = pick(
            n,
            obj,
            &["local_time", "block_time", "block_number"],
            "block line",
        )?;
        if !env["block_number"].is_u64() {
            return Err(shape(n, "block_number is not an integer"));
        }
        let events = obj
            .get("events")
            .and_then(Value::as_array)
            .ok_or_else(|| shape(n, "block line without an events array"))?;
        let fills = events
            .iter()
            .map(|e| read_fill(n, e))
            .collect::<Result<Vec<_>, _>>()?;
        let keep = book_matched(&fills);
        out.stats.events_kept += keep.len() as u64;
        out.stats.events_dropped += (fills.len() - keep.len()) as u64;
        let kept: Vec<Value> = fills
            .into_iter()
            .enumerate()
            .filter(|(i, _)| keep.contains(i))
            .map(|(_, f)| Value::Object(f.kept))
            .collect();
        env.insert("events".into(), Value::Array(kept));
        canonical(n, &Value::Object(env))
    }

    fn fills(lines: &[&str]) -> Result<Filtered, FilterError> {
        let mut out = Filtered::default();
        let mut bare: Vec<(usize, FillIn)> = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let n = i + 1;
            out.stats.lines_in += 1;
            match parse_line(n, line)? {
                Value::Object(obj) => {
                    if !bare.is_empty() {
                        return Err(shape(n, "batched and bare fill lines mixed in one file"));
                    }
                    let l = Self::fills_batched(n, &obj, &mut out)?;
                    out.lines.push(l);
                }
                v @ Value::Array(_) => {
                    if !out.lines.is_empty() {
                        return Err(shape(n, "batched and bare fill lines mixed in one file"));
                    }
                    bare.push((n, read_fill(n, &v)?));
                }
                _ => return Err(shape(n, "line is neither a block object nor a fill pair")),
            }
        }
        if !bare.is_empty() {
            let (ns, fills): (Vec<usize>, Vec<FillIn>) = bare.into_iter().unzip();
            let keep = book_matched(&fills);
            out.stats.events_kept += keep.len() as u64;
            out.stats.events_dropped += (fills.len() - keep.len()) as u64;
            for (i, f) in fills.into_iter().enumerate() {
                if keep.contains(&i) {
                    out.lines.push(canonical(ns[i], &Value::Object(f.kept))?);
                }
            }
        }
        out.stats.lines_out = out.lines.len() as u64;
        Ok(out)
    }

    fn trades(lines: &[&str]) -> Result<Filtered, FilterError> {
        let mut out = Filtered::default();
        for (i, line) in lines.iter().enumerate() {
            let n = i + 1;
            out.stats.lines_in += 1;
            let v = parse_line(n, line)?;
            let obj = v
                .as_object()
                .ok_or_else(|| shape(n, "trade line is not an object"))?;
            // Undocumented override values may mark liquidation or
            // settlement trades: keep only the documented neutral value.
            let override_ok = obj
                .get("trade_dir_override")
                .is_none_or(|o| o.as_str() == Some("Na"));
            let mut kept = pick(n, obj, &["coin", "side", "time", "px", "sz"], "trade")?;
            let side_info = obj
                .get("side_info")
                .and_then(Value::as_array)
                .filter(|s| s.len() == 2)
                .ok_or_else(|| shape(n, "side_info is not a two-element array"))?;
            let oids = side_info
                .iter()
                .map(|s| {
                    s.get("oid")
                        .filter(|o| o.is_u64())
                        .cloned()
                        .ok_or_else(|| shape(n, "side_info entry without an integer oid"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            if !override_ok {
                out.stats.events_dropped += 1;
                continue;
            }
            kept.insert("oids".into(), Value::Array(oids));
            out.lines.push(canonical(n, &Value::Object(kept))?);
            out.stats.events_kept += 1;
        }
        out.stats.lines_out = out.lines.len() as u64;
        Ok(out)
    }

    fn l2(lines: &[&str]) -> Result<Filtered, FilterError> {
        let mut out = Filtered::default();
        for (i, line) in lines.iter().enumerate() {
            let n = i + 1;
            out.stats.lines_in += 1;
            let v = parse_line(n, line)?;
            let obj = v
                .as_object()
                .ok_or_else(|| shape(n, "snapshot line is not an object"))?;
            let mut wrapper = Map::new();
            for k in ["time", "ver_num"] {
                if let Some(x) = obj.get(k) {
                    wrapper.insert(k.into(), x.clone());
                }
            }
            let raw = obj.get("raw").unwrap_or(&v);
            let raw = raw
                .as_object()
                .ok_or_else(|| shape(n, "raw is not an object"))?;
            let channel = raw.get("channel").and_then(Value::as_str);
            if channel != Some("l2Book") {
                return Err(shape(n, format!("unexpected channel {channel:?}")));
            }
            let data = raw
                .get("data")
                .and_then(Value::as_object)
                .ok_or_else(|| shape(n, "l2Book message without data"))?;
            let mut d = pick(n, data, &["coin", "time"], "l2Book data")?;
            let levels = data
                .get("levels")
                .and_then(Value::as_array)
                .filter(|l| l.len() == 2)
                .ok_or_else(|| shape(n, "levels is not [bids, asks]"))?;
            let mut sides = Vec::with_capacity(2);
            for side in levels {
                let side = side
                    .as_array()
                    .ok_or_else(|| shape(n, "a side is not an array"))?;
                let lv = side
                    .iter()
                    .map(|l| {
                        l.as_object()
                            .ok_or_else(|| shape(n, "level is not an object"))
                            .and_then(|o| pick(n, o, &["px", "sz", "n"], "level"))
                            .map(Value::Object)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                sides.push(Value::Array(lv));
            }
            d.insert("levels".into(), Value::Array(sides));
            let mut r = Map::new();
            r.insert("channel".into(), Value::String("l2Book".into()));
            r.insert("data".into(), Value::Object(d));
            wrapper.insert("raw".into(), Value::Object(r));
            out.lines.push(canonical(n, &Value::Object(wrapper))?);
            out.stats.events_kept += 1;
        }
        out.stats.lines_out = out.lines.len() as u64;
        Ok(out)
    }
}

impl RawLineFilter for LocalFilter {
    fn filter_lines(&self, dataset: Dataset, lines: &[&str]) -> Result<Filtered, FilterError> {
        match dataset {
            Dataset::NodeFillsByBlock | Dataset::NodeFills => Self::fills(lines),
            Dataset::NodeTrades => Self::trades(lines),
            Dataset::MarketData => Self::l2(lines),
            Dataset::ReplicaCmds | Dataset::ExplorerBlocks => {
                Err(FilterError::NotPublished(dataset))
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use mesh_types::allowlist::EXCLUDED_SOURCE_FIELDS;

    /// A synthetic fill with every field the node writes, including the
    /// excluded ones. Values are invented.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn fill(
        coin: &str,
        px: &str,
        sz: &str,
        side: &str,
        crossed: bool,
        tid: u64,
        oid: u64,
        time: u64,
    ) -> Value {
        serde_json::json!([
            "0x0000000000000000000000000000000000000001",
            {
                "coin": coin, "px": px, "sz": sz, "side": side, "time": time,
                "startPosition": "0.0", "dir": "Open Long", "closedPnl": "0.0",
                "hash": "0x00", "oid": oid, "crossed": crossed, "fee": "0.01",
                "tid": tid, "feeToken": "USDC", "builderFee": "0.0", "twapId": null,
                "cloid": "0x01"
            }
        ])
    }

    pub(crate) fn block_line(block: u64, block_time: &str, events: Vec<Value>) -> String {
        serde_json::json!({
            "local_time": format!("{block_time}000000"),
            "block_time": block_time,
            "block_number": block,
            "events": events,
        })
        .to_string()
    }

    fn assert_clean(lines: &[String]) {
        for l in lines {
            for f in EXCLUDED_SOURCE_FIELDS {
                assert!(!l.contains(&format!("\"{f}\"")), "{f} leaked in {l}");
            }
        }
    }

    #[test]
    fn keeps_only_book_matched_fills_and_admitted_fields() {
        let mut liq = fill("BTC", "100.0", "1", "A", false, 7, 70, 1000);
        liq[1]["liquidation"] =
            serde_json::json!({"liquidatedUser": "0x02", "markPx": "99.0", "method": "backstop"});
        let mut market_liq = fill("BTC", "101.0", "1", "A", true, 8, 80, 1000);
        market_liq[1]["liquidation"] = serde_json::json!({"markPx": "99.0", "method": "market"});
        let line = block_line(
            10,
            "2025-07-27T00:00:01.000",
            vec![
                fill("BTC", "100.5", "0.1", "B", true, 1, 11, 1000),
                fill("BTC", "100.5", "0.1", "A", false, 1, 12, 1000),
                // No taker (settlement-like): dropped.
                fill("BTC", "100.0", "1", "B", false, 2, 21, 1000),
                fill("BTC", "100.0", "1", "A", false, 2, 22, 1000),
                // Backstop liquidation: dropped.
                fill("BTC", "100.0", "1", "B", true, 7, 71, 1000),
                liq,
                // Market liquidation that matched the book: kept, object dropped.
                fill("BTC", "101.0", "1", "B", false, 8, 81, 1000),
                market_liq,
                // Unpaired single fill: dropped.
                fill("ETH", "10", "1", "B", true, 3, 31, 1000),
            ],
        );
        let out = LocalFilter
            .filter_lines(Dataset::NodeFillsByBlock, &[line.as_str()])
            .unwrap();
        assert_eq!(out.stats.events_kept, 4);
        assert_eq!(out.stats.events_dropped, 5);
        assert_clean(&out.lines);
        let v: Value = serde_json::from_str(&out.lines[0]).unwrap();
        assert_eq!(v["block_number"], 10);
        assert_eq!(v["events"].as_array().unwrap().len(), 4);
        assert!(
            out.lines[0].find("\"block_number\"").unwrap()
                < out.lines[0].find("\"events\"").unwrap()
        );
        assert!(!out.lines[0].contains("markPx"));
    }

    #[test]
    fn pairs_bare_fills_across_the_file() {
        let a = fill("SOL", "20", "2", "B", true, 5, 1, 1000).to_string();
        let b = fill("SOL", "20", "2", "A", false, 5, 2, 1000).to_string();
        let c = fill("SOL", "21", "2", "A", false, 6, 3, 1000).to_string();
        let out = LocalFilter
            .filter_lines(Dataset::NodeFills, &[a.as_str(), c.as_str(), b.as_str()])
            .unwrap();
        assert_eq!(out.lines.len(), 2);
        assert_eq!(out.stats.events_dropped, 1);
        assert_clean(&out.lines);
    }

    #[test]
    fn rejects_unknown_shapes() {
        for (d, line) in [
            (Dataset::NodeFillsByBlock, "not json"),
            (
                Dataset::NodeFillsByBlock,
                r#"{"block_number":1,"block_time":"x","local_time":"y"}"#,
            ),
            (
                Dataset::NodeFillsByBlock,
                r#"{"block_number":1,"block_time":"x","local_time":"y","events":[["u",{"coin":"BTC"}]]}"#,
            ),
            (
                Dataset::MarketData,
                r#"{"raw":{"channel":"trades","data":{}}}"#,
            ),
            (
                Dataset::NodeTrades,
                r#"{"coin":"BTC","side":"B","time":"t","px":"1","sz":"1","side_info":[]}"#,
            ),
        ] {
            assert!(LocalFilter.filter_lines(d, &[line]).is_err(), "{line}");
        }
        assert_eq!(
            LocalFilter.filter_lines(Dataset::ReplicaCmds, &[]),
            Err(FilterError::NotPublished(Dataset::ReplicaCmds))
        );
    }

    #[test]
    fn filters_trades_and_snapshots() {
        let t = serde_json::json!({
            "coin": "SOL", "side": "A", "time": "2025-04-11T02:59:59.991721263", "px": "100.0", "sz": "1.0",
            "hash": "0x00", "trade_dir_override": "Na",
            "side_info": [
                {"user": "0x01", "start_pos": "0", "oid": 1, "twap_id": null, "cloid": null},
                {"user": "0x02", "start_pos": "0", "oid": 2, "twap_id": null, "cloid": "0x03"}
            ]
        });
        let mut liq = t.clone();
        liq["trade_dir_override"] = serde_json::json!("LiquidatedCross");
        let (a, b) = (t.to_string(), liq.to_string());
        let out = LocalFilter
            .filter_lines(Dataset::NodeTrades, &[a.as_str(), b.as_str()])
            .unwrap();
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.stats.events_dropped, 1);
        assert_clean(&out.lines);

        let s = serde_json::json!({
            "time": "2023-09-16T09:00:03.296545473", "ver_num": 1,
            "raw": {"channel": "l2Book", "data": {"coin": "SOL", "time": 1_694_854_803_000u64, "extra": 1,
                "levels": [[{"px": "19.9", "sz": "5", "n": 2}], [{"px": "20.1", "sz": "3", "n": 1, "x": 0}]]}}
        })
        .to_string();
        let out = LocalFilter
            .filter_lines(Dataset::MarketData, &[s.as_str()])
            .unwrap();
        assert!(!out.lines[0].contains("extra") && !out.lines[0].contains("\"x\""));
        assert!(out.lines[0].starts_with("{\"raw\":{\"channel\":\"l2Book\""));
    }

    /// A synthetic fill without `cloid` (an optional field: the node writes
    /// it only for orders that had one). Values are invented.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn node_fill(
        coin: &str,
        px: &str,
        sz: &str,
        side: &str,
        crossed: bool,
        tid: u64,
        oid: u64,
        time: u64,
    ) -> Value {
        let mut f = fill(coin, px, sz, side, crossed, tid, oid, time);
        if let Some(o) = f[1].as_object_mut() {
            o.remove("cloid");
        }
        f
    }

    #[test]
    fn by_block_objects_are_detected() {
        let block = block_line(1, "2025-07-27T00:00:01.000", vec![]);
        let bare = node_fill("BTC", "1", "1", "B", true, 1, 1, 1).to_string();
        assert!(is_by_block(Dataset::NodeFillsByBlock, &[bare.as_str()]));
        assert!(is_by_block(Dataset::NodeFills, &["", block.as_str()]));
        assert!(!is_by_block(Dataset::NodeFills, &[bare.as_str()]));
        assert!(!is_by_block(Dataset::NodeTrades, &[block.as_str()]));
        assert!(!is_by_block::<&str>(Dataset::NodeFills, &[]));
        // Bare fills after the local filter are objects without `events`
        // (seen on real 2025-06 node_fills): not by-block.
        let other = node_fill("BTC", "1", "1", "A", false, 1, 2, 1).to_string();
        let filtered = LocalFilter
            .filter_lines(Dataset::NodeFills, &[bare.as_str(), other.as_str()])
            .unwrap();
        assert!(!filtered.lines.is_empty());
        assert!(filtered.lines.iter().all(|l| l.starts_with('{')));
        assert!(!is_by_block(Dataset::NodeFills, &filtered.lines));
    }

    #[test]
    fn node_reader_filter_is_the_live_filter() {
        let mut backstop = node_fill("BTC", "100.0", "1", "A", false, 7, 70, 1000);
        backstop[1]["liquidation"] = serde_json::json!({
            "liquidatedUser": "0x02", "markPx": "99.0", "method": "backstop"});
        let line = block_line(
            10,
            "2025-07-27T00:00:01.000",
            vec![
                node_fill("BTC", "100.5", "0.1", "B", true, 1, 11, 1000),
                node_fill("BTC", "100.5", "0.1", "A", false, 1, 12, 1000),
                node_fill("BTC", "100.0", "1", "B", true, 7, 71, 1000),
                backstop,
                // Spot: outside ADM-0001.
                node_fill("@4", "1", "1", "B", true, 3, 31, 1000),
                node_fill("@4", "1", "1", "A", false, 3, 32, 1000),
                // Unmatched: kept for the parser to flag.
                node_fill("ETH", "10", "1", "B", true, 5, 51, 1000),
            ],
        );
        let empty = block_line(11, "2025-07-27T00:00:01.500", vec![]);
        let lines = [line.as_str(), empty.as_str()];
        let out = NodeReaderFilter
            .filter_lines(Dataset::NodeFillsByBlock, &lines)
            .unwrap();
        assert_eq!(
            out.lines,
            [
                hl_node_reader::filter_line(Stream::Fills, &line).unwrap(),
                hl_node_reader::filter_line(Stream::Fills, &empty).unwrap()
            ]
        );
        assert_eq!(out.stats.lines_out, 2, "empty blocks are kept");
        assert_eq!((out.stats.events_kept, out.stats.events_dropped), (3, 4));
        assert_clean(&out.lines);
        assert_eq!(
            DefaultFilter::default().filter_lines(Dataset::NodeFillsByBlock, &lines),
            Ok(out)
        );

        // A field outside the node format is schema drift: the object fails.
        // (`cloid` and `priorityGas` are known and excluded since they were
        // seen in archived node output, so the test uses an invented field.)
        let mut drifted = fill("BTC", "1", "1", "B", true, 9, 1, 1000);
        drifted[1]["inventedField"] = serde_json::json!(1);
        let drift = block_line(12, "2025-07-27T00:00:02.000", vec![drifted]);
        assert!(
            NodeReaderFilter
                .filter_lines(Dataset::NodeFillsByBlock, &[drift.as_str()])
                .is_err()
        );
        assert_eq!(
            NodeReaderFilter.filter_lines(Dataset::MarketData, &[empty.as_str()]),
            Err(FilterError::NotByBlock(Dataset::MarketData))
        );
    }

    #[test]
    fn default_filter_keeps_the_local_filter_for_other_formats() {
        let a = fill("SOL", "20", "2", "B", true, 5, 1, 1000).to_string();
        let b = fill("SOL", "20", "2", "A", false, 5, 2, 1000).to_string();
        let lines = [a.as_str(), b.as_str()];
        assert_eq!(
            DefaultFilter::default().filter_lines(Dataset::NodeFills, &lines),
            LocalFilter.filter_lines(Dataset::NodeFills, &lines)
        );
    }
}
