//! End to end on the synthetic fixture day (`tools/hl-synth`, seed 42):
//! capture node files into both tiers with a test filter, replay the
//! filtered raw twice with a test parser and compare content hashes and
//! bytes, check the parsed records against the fixture's ground truth,
//! then build, sign and verify the day's manifests.
//!
//! The filter and parser here are test stand-ins written from
//! `docs/specs/hyperliquid-node-format.md`; the real ones belong to the
//! node reader and are injected by the integrator.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use serde_json::{Map, Value};

use archive::capture::{self, Capture, FilterSet, HourWriter, TierConfig};
use archive::layout::{self, Day};
use archive::manifest;
use archive::replay::{self, RawFile, ReplayOptions};
use archive::verify::{self, DirSource, FileCheck, Status, VerifyOptions};
use archive::{BoxError, cli};
use mesh_types::derived::DerivedHeader;
use mesh_types::{
    AggressorSide, Bar, BarInterval, BookAction, BookEvent, BookSide, MethodId, OrderId, Price,
    Provenance, Record, Registry, Seq, SigningKey, Size, Trade, TradeId, UnixMillis, UnixNanos,
    VenueId,
};

const STREAMS: [(&str, &str); 4] = [
    ("fills", "node_fills_by_block"),
    ("order_statuses", "node_order_statuses_by_block"),
    ("raw_book_diffs", "node_raw_book_diffs_by_block"),
    ("misc_events", "misc_events_by_block"),
];

fn pick(obj: &Map<String, Value>, keys: &[&str]) -> Value {
    Value::Object(
        keys.iter()
            .filter_map(|k| obj.get(*k).map(|v| ((*k).to_owned(), v.clone())))
            .collect(),
    )
}

/// Keeps the block fields and the event fields the reader uses
/// (archive-layout § 2); keys sorted, no whitespace.
fn filter(stream: &'static str) -> Arc<dyn capture::LineFilter> {
    Arc::new(move |line: &[u8]| -> Option<Vec<u8>> {
        let v: Value = serde_json::from_slice(line).ok()?;
        let block = v.as_object()?;
        let events: Vec<Value> = block
            .get("events")?
            .as_array()?
            .iter()
            .filter_map(|e| match stream {
                "fills" => Some(pick(
                    e.as_array()?.get(1)?.as_object()?,
                    &["coin", "crossed", "oid", "px", "side", "sz", "tid", "time"],
                )),
                "order_statuses" => {
                    let o = e.as_object()?;
                    let mut out = pick(o, &["status", "time"]);
                    out["order"] = pick(
                        o.get("order")?.as_object()?,
                        &[
                            "coin",
                            "limitPx",
                            "oid",
                            "origSz",
                            "side",
                            "sz",
                            "timestamp",
                        ],
                    );
                    Some(out)
                }
                _ => Some(pick(
                    e.as_object()?,
                    &["coin", "oid", "px", "raw_book_diff", "side"],
                )),
            })
            .collect();
        let mut out = pick(block, &["block_number", "block_time", "local_time"]);
        out["events"] = Value::Array(events);
        serde_json::to_vec(&out).ok()
    })
}

fn naive_ms(s: &str) -> i64 {
    UnixMillis::parse_rfc3339(&format!("{s}Z")).unwrap().0
}

fn naive_ns(s: &str) -> i64 {
    UnixNanos::parse_rfc3339(&format!("{s}Z")).unwrap().0
}

/// Parses filtered raw files into trades, book events and 1-minute bars,
/// following node-format §§ 4.1 and 6.1.
fn parser(
    registry: Registry,
) -> impl Fn(&[RawFile]) -> Result<Vec<Record>, BoxError> + Send + Sync {
    move |files: &[RawFile]| {
        let venue = VenueId::new("hyperliquid")?;
        // block -> stream -> line
        let mut blocks: BTreeMap<u64, BTreeMap<String, Value>> = BTreeMap::new();
        for f in files {
            for line in std::str::from_utf8(&f.data)?.lines() {
                let v: Value = serde_json::from_str(line)?;
                let b = v["block_number"].as_u64().ok_or("block_number")?;
                blocks.entry(b).or_default().insert(f.stream.clone(), v);
            }
        }
        let mut out = Vec::new();
        // Resting size per order, from the diffs, across blocks.
        let mut resting: BTreeMap<u64, Size> = BTreeMap::new();
        for (block, streams) in &blocks {
            let prov = |coin: &str,
                        index: u32,
                        ts_ms: i64,
                        line: &Value|
             -> Result<(Provenance, u8, u8), BoxError> {
                let spec = registry
                    .by_venue_symbol(&venue, coin)
                    .ok_or("unknown coin")?;
                let p = Provenance::new(
                    spec.id.clone(),
                    venue.clone(),
                    Seq::new(*block, index),
                    UnixMillis(ts_ms),
                    UnixNanos(naive_ns(line["local_time"].as_str().ok_or("local_time")?)),
                    spec.admission.clone(),
                )
                .with_flags(spec.flags);
                Ok((p, spec.price_scale, spec.size_scale))
            };
            // Unconsumed fill sizes per order in this block (as strings at the
            // venue's precision, compared after scaling).
            let mut fills_by_oid: BTreeMap<u64, Vec<String>> = BTreeMap::new();
            if let Some(line) = streams.get("fills") {
                let mut groups: Vec<((String, u64), Vec<&Value>)> = Vec::new();
                for fill in line["events"].as_array().ok_or("events")? {
                    fills_by_oid
                        .entry(fill["oid"].as_u64().ok_or("oid")?)
                        .or_default()
                        .push(fill["sz"].as_str().ok_or("sz")?.to_owned());
                    let key = (
                        fill["coin"].as_str().ok_or("coin")?.to_owned(),
                        fill["tid"].as_u64().ok_or("tid")?,
                    );
                    match groups.iter_mut().find(|(k, _)| *k == key) {
                        Some((_, v)) => v.push(fill),
                        None => groups.push((key, vec![fill])),
                    }
                }
                let mut idx: BTreeMap<String, u32> = BTreeMap::new();
                for ((coin, tid), pair) in groups {
                    let takers: Vec<&&Value> =
                        pair.iter().filter(|f| f["crossed"] == true).collect();
                    if pair.len() != 2 || takers.len() != 1 {
                        continue;
                    }
                    let taker = takers[0];
                    let i = idx.entry(coin.clone()).or_insert(0);
                    let (p, ps, ss) = prov(&coin, *i, taker["time"].as_i64().ok_or("time")?, line)?;
                    *i += 1;
                    out.push(Record::Trade(Trade {
                        prov: p,
                        price: Price::parse_with_scale(taker["px"].as_str().ok_or("px")?, ps)?,
                        size: Size::parse_with_scale(taker["sz"].as_str().ok_or("sz")?, ss)?,
                        aggressor: if taker["side"] == "B" {
                            AggressorSide::Buy
                        } else {
                            AggressorSide::Sell
                        },
                        trade_id: TradeId(tid),
                    }));
                }
            }
            // Orders whose status in this block is `filled`.
            let mut filled_status = BTreeSet::new();
            if let Some(line) = streams.get("order_statuses") {
                for st in line["events"].as_array().ok_or("events")? {
                    if st["status"] == "filled" {
                        filled_status.insert(st["order"]["oid"].as_u64().ok_or("oid")?);
                    }
                }
            }
            if let Some(line) = streams.get("raw_book_diffs") {
                let ts = naive_ms(line["block_time"].as_str().ok_or("block_time")?);
                let mut idx: BTreeMap<String, u32> = BTreeMap::new();
                // A removal of an order whose status is `filled` claims one of
                // its fills, so an earlier update of that order is a fill only
                // if another fill of the right size remains.
                let removed: BTreeSet<u64> = line["events"]
                    .as_array()
                    .ok_or("events")?
                    .iter()
                    .filter(|d| d["raw_book_diff"] == "remove")
                    .filter_map(|d| d["oid"].as_u64())
                    .filter(|oid| filled_status.contains(oid))
                    .collect();
                for d in line["events"].as_array().ok_or("events")? {
                    let coin = d["coin"].as_str().ok_or("coin")?;
                    let oid = d["oid"].as_u64().ok_or("oid")?;
                    let i = idx.entry(coin.to_owned()).or_insert(0);
                    let (p, ps, ss) = prov(coin, *i, ts, line)?;
                    *i += 1;
                    let sz = |v: &Value| -> Result<Size, BoxError> {
                        Ok(Size::parse_with_scale(v.as_str().ok_or("sz")?, ss)?)
                    };
                    let diff = &d["raw_book_diff"];
                    let action = if diff == "remove" {
                        // A fill when a fill of exactly the remaining size
                        // exists for the order in this block.
                        let remaining = resting.remove(&oid);
                        let matched = remaining.and_then(|rem| {
                            let v = fills_by_oid.get_mut(&oid)?;
                            let pos = v.iter().position(|s| {
                                Size::parse_with_scale(s, ss).is_ok_and(|z| z == rem)
                            })?;
                            Some(v.remove(pos))
                        });
                        if matched.is_some() || filled_status.contains(&oid) {
                            BookAction::Fill {
                                orig_size: None,
                                new_size: Size::zero(ss)?,
                            }
                        } else {
                            BookAction::Cancel {}
                        }
                    } else if let Some(n) = diff.get("new") {
                        resting.insert(oid, sz(&n["sz"])?);
                        BookAction::Add {
                            size: sz(&n["sz"])?,
                        }
                    } else {
                        let u = &diff["update"];
                        let (orig_size, new_size) = (Some(sz(&u["origSz"])?), sz(&u["newSz"])?);
                        resting.insert(oid, new_size);
                        // An update is a fill when a fill of exactly the size
                        // change exists for this order; otherwise a modify.
                        let delta = orig_size.ok_or("orig")?.checked_sub(new_size)?;
                        let reserve = usize::from(removed.contains(&oid));
                        let matched = fills_by_oid
                            .get_mut(&oid)
                            .filter(|v| v.len() > reserve)
                            .and_then(|v| {
                                let pos = v.iter().position(|s| {
                                    Size::parse_with_scale(s, ss).is_ok_and(|z| z == delta)
                                })?;
                                Some(v.remove(pos))
                            });
                        if matched.is_some() {
                            BookAction::Fill {
                                orig_size,
                                new_size,
                            }
                        } else {
                            BookAction::Modify {
                                orig_size,
                                new_size,
                            }
                        }
                    };
                    out.push(Record::BookEvent(BookEvent {
                        prov: p,
                        order_id: OrderId(oid),
                        side: if d["side"] == "Bid" {
                            BookSide::Bid
                        } else {
                            BookSide::Ask
                        },
                        price: Price::parse_with_scale(d["px"].as_str().ok_or("px")?, ps)?,
                        action,
                    }));
                }
            }
        }
        // Derived: 1-minute bars per instrument.
        let mut by_bar: BTreeMap<(String, i64), Vec<Trade>> = BTreeMap::new();
        for r in &out {
            if let Record::Trade(t) = r {
                let start = BarInterval::M1.align(t.prov.ts_source_ms).0;
                by_bar
                    .entry((t.prov.instrument.as_str().to_owned(), start))
                    .or_default()
                    .push(t.clone());
            }
        }
        for ((_, start), mut trades) in by_bar {
            trades.sort_by_key(|t| t.prov.seq);
            let recs: Vec<Record> = trades.iter().cloned().map(Record::Trade).collect();
            let last = trades.last().ok_or("empty bar")?;
            let mut prov = last.prov.clone();
            prov.flags = last.prov.flags;
            let zero = Size::zero(last.size.scale())?;
            let (mut vol, mut buy, mut sell) = (zero, zero, zero);
            for t in &trades {
                vol = vol.checked_add(t.size)?;
                match t.aggressor {
                    AggressorSide::Buy => buy = buy.checked_add(t.size)?,
                    AggressorSide::Sell => sell = sell.checked_add(t.size)?,
                    AggressorSide::Unknown => {}
                }
            }
            out.push(Record::Bar(Bar {
                prov,
                derived: DerivedHeader {
                    method: MethodId::new("bars@1")?,
                    inputs_hash: mesh_types::content_hash(&recs)?,
                    input_count: trades.len() as u64,
                },
                interval: BarInterval::M1,
                start_ms: UnixMillis(start),
                open: trades[0].price,
                high: trades.iter().map(|t| t.price).max().ok_or("high")?,
                low: trades.iter().map(|t| t.price).min().ok_or("low")?,
                close: last.price,
                volume: vol,
                buy_volume: buy,
                sell_volume: sell,
                trade_count: u32::try_from(trades.len())?,
            }));
        }
        Ok(out)
    }
}

/// Asserts equality, naming the first differing element.
fn same(got: &[Value], want: &Value, what: &str) {
    let want = want.as_array().unwrap();
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert_eq!(g, w, "{what}: first difference at {i}");
    }
    assert_eq!(got.len(), want.len(), "{what}: count");
}

struct Fixture {
    _dir: tempfile::TempDir,
    node: std::path::PathBuf,
    staging: std::path::PathBuf,
    registry: Registry,
    expected: Value,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let node = dir.path().join("node");
    let files = hl_synth::generate(hl_synth::Config::default()).unwrap();
    for (rel, content) in &files {
        if let Some(rest) = rel.strip_prefix("base/") {
            let p = node.join(rest);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }
    }
    let registry: Registry = serde_json::from_slice(&files["markets.json"]).unwrap();
    let expected: Value = serde_json::from_slice(&files["base/expected.json"]).unwrap();
    let staging = dir.path().join("staging");
    Fixture {
        node,
        staging,
        registry,
        expected,
        _dir: dir,
    }
}

fn capture_fixture(f: &Fixture) -> Vec<capture::SealedFile> {
    let mut filters = FilterSet::new();
    for (s, _) in &STREAMS[..3] {
        filters = filters.with(s, filter(s)).unwrap();
    }
    let mut cap = Capture::new(
        Some(HourWriter::open(TierConfig::private(&f.staging)).unwrap()),
        Some(HourWriter::open(TierConfig::filtered(&f.staging)).unwrap()),
        filters,
    )
    .unwrap();
    let streams: Vec<(String, std::path::PathBuf)> = STREAMS
        .iter()
        .map(|(s, d)| ((*s).to_owned(), f.node.join(d)))
        .collect();
    capture::capture_node_dirs(&mut cap, &streams).unwrap()
}

fn day() -> Day {
    "2026-01-01".parse().unwrap()
}

fn replay_opts() -> ReplayOptions {
    ReplayOptions {
        require_next_hour: false,
        ..ReplayOptions::default()
    }
}

fn key() -> SigningKey {
    SigningKey::from_bytes(&[0x21; 32]).unwrap()
}

fn build_sign(root: &Path, archive: &str) -> mesh_types::manifest::SignedManifest {
    let opts = manifest::BuildOptions {
        archive: archive.into(),
        day: day(),
        web_seeds: vec!["https://mirror.example.org/".into()],
        trackers: vec![],
        created_ns: UnixNanos(1_767_312_000_000_000_000),
        signer: key().address(),
        next_signer: None,
        previous: None,
    };
    let built = manifest::build(root, &opts).unwrap();
    let signed = manifest::sign(built.manifest, &key()).unwrap();
    manifest::write_signed(root, &signed, &built.torrent.metainfo).unwrap();
    signed
}

#[test]
fn capture_replay_manifest_verify() {
    let f = fixture();
    let sealed = capture_fixture(&f);

    // Private tier: all four streams, verbatim. Filtered tier: three streams,
    // no excluded field names.
    let private: Vec<_> = sealed
        .iter()
        .filter(|s| s.archive == layout::PRIVATE_RAW_ARCHIVE)
        .collect();
    let filtered: Vec<_> = sealed
        .iter()
        .filter(|s| s.archive == layout::RAW_ARCHIVE)
        .collect();
    assert_eq!(private.len(), 8);
    assert_eq!(filtered.len(), 6);
    let original = std::fs::read(f.node.join("node_fills_by_block/hourly/20260101/0")).unwrap();
    let p0 = private
        .iter()
        .find(|s| s.path == "stream=fills/date=2026-01-01/00.jsonl.zst")
        .unwrap();
    assert_eq!(
        capture::decompress(&std::fs::read(&p0.local).unwrap(), &BTreeMap::new()).unwrap(),
        original
    );
    for s in &filtered {
        let raw = capture::decompress(&std::fs::read(&s.local).unwrap(), &BTreeMap::new()).unwrap();
        let text = String::from_utf8(raw).unwrap();
        for banned in [
            "\"user\"",
            "\"hash\"",
            "closedPnl",
            "liquidation",
            "markPx",
            "cloid",
            "\"fee\"",
            "0x0000",
        ] {
            assert!(!text.contains(banned), "{} contains {banned}", s.path);
        }
        // Same bytes as the pinned single-frame encoding.
        assert_eq!(
            std::fs::read(&s.local).unwrap(),
            capture::compress_filtered(text.as_bytes()).unwrap()
        );
    }

    // The filtered tier's ready/ tree is laid out as the archive root.
    let raw_root = f.staging.join(layout::RAW_ARCHIVE).join("ready");
    let parse = parser(f.registry.clone());

    // Replay twice into separate roots: identical content hashes and bytes.
    let out = f.staging.join("normalized");
    let out2 = f.staging.join("normalized-2");
    let r1 = replay::replay_day(&raw_root, day(), &parse, &out, &replay_opts()).unwrap();
    let r2 = replay::replay_day(&raw_root, day(), &parse, &out2, &replay_opts()).unwrap();
    assert_eq!(r1.files, r2.files);
    assert_eq!(r1.day_content_hash, r2.day_content_hash);
    assert!(replay::compare_with(&r1, &out2, true).unwrap().is_empty());
    assert_eq!(r1.inputs.len(), 6);
    assert_eq!(r1.missing_next_hour.len(), 3);
    assert_eq!(r1.out_of_day, 0);
    let kinds: BTreeSet<_> = r1.files.iter().map(|w| w.key.kind).collect();
    assert_eq!(kinds.len(), 3, "trade, book_event and bar partitions");

    // Parsed records match the fixture's ground truth.
    let parsed = replay::replay_records(&raw_root, day(), &parse, false).unwrap();
    for (coin, truth) in f.expected["coins"].as_object().unwrap() {
        let inst = truth["instrument"].as_str().unwrap();
        let trades: Vec<Value> = parsed
            .records
            .iter()
            .filter_map(|r| match r {
                Record::Trade(t) if t.prov.instrument.as_str() == inst => Some(serde_json::json!({
                    "aggressor": t.aggressor,
                    "price": t.price.to_string(),
                    "seq": [t.prov.seq.block_number, t.prov.seq.event_index],
                    "size": t.size.to_string(),
                    "trade_id": t.trade_id.0,
                    "ts_source_ms": t.prov.ts_source_ms.0,
                })),
                _ => None,
            })
            .collect();
        same(&trades, &truth["trades"], &format!("{coin} trades"));
        let events: Vec<Value> = parsed
            .records
            .iter()
            .filter_map(|r| match r {
                Record::BookEvent(e) if e.prov.instrument.as_str() == inst => {
                    let mut v = serde_json::json!({
                        "action": e.action.as_str(),
                        "order_id": e.order_id.0,
                        "price": e.price.to_string(),
                        "seq": [e.prov.seq.block_number, e.prov.seq.event_index],
                        "side": e.side.as_str(),
                    });
                    match e.action {
                        BookAction::Add { size } => v["size"] = size.to_string().into(),
                        BookAction::Modify {
                            orig_size,
                            new_size,
                        }
                        | BookAction::Fill {
                            orig_size,
                            new_size,
                        } => {
                            if let Some(o) = orig_size {
                                v["orig_size"] = o.to_string().into();
                            }
                            v["new_size"] = new_size.to_string().into();
                        }
                        BookAction::Cancel {} => {}
                    }
                    Some(v)
                }
                _ => None,
            })
            .collect();
        same(
            &events,
            &truth["book_events"],
            &format!("{coin} book events"),
        );
    }

    // Manifests for both public archives, signed and verified deeply.
    let signed_norm = build_sign(&out, layout::NORMALIZED_ARCHIVE);
    let signed_raw = build_sign(&raw_root, layout::RAW_ARCHIVE);
    assert_eq!(signed_raw.manifest.files.len(), 6);
    assert!(
        signed_raw
            .manifest
            .files
            .iter()
            .all(|m| m.content_hash.is_none())
    );
    assert!(
        signed_norm
            .manifest
            .files
            .iter()
            .all(|m| m.content_hash.is_some())
    );
    for m in &signed_norm.manifest.files {
        let w = r1.files.iter().find(|w| w.path == m.path).unwrap();
        assert_eq!(
            m.content_hash,
            Some(w.content_hash),
            "manifest gate for {}",
            m.path
        );
        assert_eq!(m.sha256, w.sha256);
    }
    let mut opts = VerifyOptions::new(
        vec![key().address()],
        UnixNanos(1_767_312_000_000_000_000 + 3_600_000_000_000),
    );
    opts.files = FileCheck::Deep;
    for (root, archive) in [
        (&out, layout::NORMALIZED_ARCHIVE),
        (&raw_root, layout::RAW_ARCHIVE),
    ] {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let report = rt
            .block_on(verify::verify_day(
                &DirSource::new(root),
                archive,
                day(),
                &opts,
            ))
            .unwrap();
        assert_eq!(report.count(Status::Fail), 0, "{report}");
        assert_eq!(report.status(), Status::Warn, "{report}");
    }

    // The CLI verifies the same day (anchors still in their window would be
    // warnings; now they are overdue, so --no-files still reports failure).
    let code = cli::run_from(
        [
            "archive",
            "verify",
            "--dir",
            out.to_str().unwrap(),
            "--archive",
            layout::NORMALIZED_ARCHIVE,
            "--date",
            "2026-01-01",
            "--trusted",
            &key().address().to_string(),
            "--deep",
        ],
        cli::Hooks::default(),
    );
    assert_eq!(
        code,
        ExitCode::FAILURE,
        "anchors are overdue for a day in the past"
    );
}

#[test]
fn cli_replay_with_wired_parser_is_deterministic() {
    let f = fixture();
    capture_fixture(&f);
    let raw_root = f.staging.join(layout::RAW_ARCHIVE).join("ready");
    let hooks = cli::Hooks {
        filters: FilterSet::new(),
        parser: Some(Arc::new(parser(f.registry.clone()))),
    };
    let a = f.staging.join("a");
    let b = f.staging.join("b");
    for out in [&a, &b] {
        let code = cli::run_from(
            [
                "archive",
                "replay",
                "--raw",
                raw_root.to_str().unwrap(),
                "--date",
                "2026-01-01",
                "--out",
                out.to_str().unwrap(),
                "--allow-missing-next-hour",
            ],
            hooks.clone(),
        );
        assert_eq!(code, ExitCode::SUCCESS);
    }
    let code = cli::run_from(
        [
            "archive",
            "replay",
            "--raw",
            raw_root.to_str().unwrap(),
            "--date",
            "2026-01-01",
            "--out",
            f.staging.join("c").to_str().unwrap(),
            "--allow-missing-next-hour",
            "--check-against",
            a.to_str().unwrap(),
            "--bytes",
        ],
        hooks,
    );
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(
        layout::list_files(&a).unwrap(),
        layout::list_files(&b).unwrap()
    );
    for p in layout::list_files(&a).unwrap() {
        assert_eq!(
            std::fs::read(a.join(&p)).unwrap(),
            std::fs::read(b.join(&p)).unwrap(),
            "{p}"
        );
    }
}

#[test]
fn cli_partition_build_sign_and_verify() {
    let dir = tempfile::tempdir().unwrap();
    let records = dir.path().join("records.jsonl");
    let f = fixture();
    capture_fixture(&f);
    let raw_root = f.staging.join(layout::RAW_ARCHIVE).join("ready");
    let parsed =
        replay::replay_records(&raw_root, day(), &parser(f.registry.clone()), false).unwrap();
    let lines: Vec<String> = parsed
        .records
        .iter()
        .map(|r| serde_json::to_string(r).unwrap())
        .collect();
    std::fs::write(&records, lines.join("\n")).unwrap();
    let root = dir.path().join("hyperliquid-normalized");
    let key_file = dir.path().join("key");
    std::fs::write(&key_file, format!("0x{}\n", "21".repeat(32))).unwrap();
    let run = |args: &[&str]| {
        cli::run_from(
            std::iter::once("archive").chain(args.iter().copied()),
            cli::Hooks::default(),
        )
    };
    assert_eq!(
        run(&[
            "partition",
            "--input",
            records.to_str().unwrap(),
            "--out",
            root.to_str().unwrap()
        ]),
        ExitCode::SUCCESS
    );
    // Unsigned build, then sign.
    let r = root.to_str().unwrap();
    assert_eq!(
        run(&[
            "manifest",
            "build",
            "--root",
            r,
            "--archive",
            layout::NORMALIZED_ARCHIVE,
            "--date",
            "2026-01-01",
            "--web-seed",
            "https://mirror.example.org/",
            "--signer",
            &key().address().to_string(),
            "--created-ns",
            "1767312000000000000"
        ]),
        ExitCode::SUCCESS
    );
    assert!(
        root.join("_manifests/2026-01-01/manifest.unsigned.json")
            .exists()
    );
    assert_eq!(
        run(&[
            "manifest",
            "sign",
            "--root",
            r,
            "--archive",
            layout::NORMALIZED_ARCHIVE,
            "--date",
            "2026-01-01",
            "--key-file",
            key_file.to_str().unwrap()
        ]),
        ExitCode::SUCCESS
    );
    // Dry-run L2 anchor and Iceberg metadata.
    assert_eq!(
        run(&[
            "anchor",
            "evm-submit",
            "--root",
            r,
            "--date",
            "2026-01-01",
            "--chain-id",
            "10",
            "--key-file",
            key_file.to_str().unwrap(),
            "--dry-run"
        ]),
        ExitCode::SUCCESS
    );
    assert!(
        root.join("_manifests/2026-01-01/manifest.l2-tx.json")
            .exists()
    );
    assert_eq!(
        run(&[
            "iceberg",
            "--root",
            r,
            "--location",
            "s3://bucket/hyperliquid-normalized"
        ]),
        ExitCode::SUCCESS
    );
    assert!(
        root.join("_iceberg/trade/metadata/version-hint.text")
            .exists()
    );
    // Local publish check without upload or IPFS.
    assert_eq!(
        run(&[
            "publish",
            "--root",
            r,
            "--archive",
            layout::NORMALIZED_ARCHIVE,
            "--date",
            "2026-01-01"
        ]),
        ExitCode::SUCCESS
    );
    // manifest verify (no files) passes signature and chain; anchors are overdue.
    let code = run(&[
        "manifest",
        "verify",
        "--dir",
        r,
        "--archive",
        layout::NORMALIZED_ARCHIVE,
        "--date",
        "2026-01-01",
        "--trusted",
        &key().address().to_string(),
        "--json",
    ]);
    assert_eq!(code, ExitCode::FAILURE);
    let signed = manifest::read_signed(&root, day()).unwrap().unwrap();
    assert_eq!(signed.manifest.signer, key().address());
}
