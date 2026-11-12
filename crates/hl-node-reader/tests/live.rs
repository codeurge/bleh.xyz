//! Live tailing tests: a simulated node writes the synthetic fixtures block
//! by block (with lines split across writes and an hourly rotation) while
//! the pipeline tails, resumes from a checkpoint, and merges two nodes.

#![allow(clippy::unwrap_used)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use hl_node_reader::files::{StartPosition, list_hourly};
use hl_node_reader::filter::StreamDirs;
use hl_node_reader::markets::RegistryHandle;
use hl_node_reader::merge::{IntegrityAlarm, MergeConfig, ReaderId};
use hl_node_reader::metrics::{Metrics, names};
use hl_node_reader::pipeline::{Pipeline, PipelineConfig, PipelineEvent};
use hl_node_reader::raw::Stream;
use hl_node_reader::replay::Replay;
use hl_node_reader::sequence::BlockOutput;
use hl_node_reader::tail::TailerConfig;
use mesh_types::Registry;

const LAST_BLOCK: u64 = 1_000_059;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic/hl")
}

fn registry() -> Registry {
    serde_json::from_slice(&std::fs::read(root().join("markets.json")).unwrap()).unwrap()
}

/// Per stream, the `(relative file, line)` list of a fixture scenario.
fn lines(scenario: &str) -> Vec<Vec<(String, String)>> {
    let dirs = StreamDirs::default();
    Stream::ALL
        .iter()
        .map(|&s| {
            let mut out = Vec::new();
            for f in list_hourly(&root().join(scenario).join(dirs.get(s))).unwrap() {
                for l in std::fs::read_to_string(&f.path).unwrap().lines() {
                    out.push((f.rel(), l.to_owned()));
                }
            }
            out
        })
        .collect()
}

fn append(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    f.write_all(text.as_bytes()).unwrap();
    f.flush().unwrap();
}

/// Writes lines `range` of every stream into `dir`, each line in two writes.
async fn write_blocks(dir: &Path, lines: &[Vec<(String, String)>], range: std::ops::Range<usize>) {
    let dirs = StreamDirs::default();
    for i in range {
        for (s, stream_lines) in Stream::ALL.iter().zip(lines) {
            let Some((rel, line)) = stream_lines.get(i) else {
                continue;
            };
            let path = dir.join(dirs.get(*s)).join(rel);
            let (a, b) = line.split_at(line.len() / 2);
            append(&path, a);
            tokio::task::yield_now().await;
            append(&path, &format!("{b}\n"));
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

fn expected_outputs() -> Vec<BlockOutput> {
    Replay::open(&root().join("base"), &StreamDirs::default(), registry())
        .unwrap()
        .sequenced()
        .map(Result::unwrap)
        .collect()
}

fn tailer(name: &str, dir: &Path) -> TailerConfig {
    let mut t = TailerConfig::new(name, dir.to_path_buf());
    t.start = StartPosition::Earliest;
    t.poll_interval = Duration::from_millis(10);
    t.notify_poll_interval = Duration::from_millis(20);
    t.registry_refresh = None;
    t
}

fn config(readers: Vec<TailerConfig>, checkpoint: Option<PathBuf>) -> PipelineConfig {
    PipelineConfig {
        readers,
        merge: MergeConfig {
            gap_wait: Duration::from_secs(5),
            confirm_window_blocks: 1_000,
            stall_after: Duration::from_secs(120),
        },
        checkpoint,
        checkpoint_interval: Duration::ZERO,
        channel_capacity: 64,
    }
}

/// Collects events until block `until` is published (or times out).
async fn collect(p: &mut Pipeline, until: u64) -> (Vec<BlockOutput>, Vec<IntegrityAlarm>) {
    let mut blocks = Vec::new();
    let mut alarms = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while blocks
        .last()
        .is_none_or(|b: &BlockOutput| b.block_number < until)
    {
        match tokio::time::timeout_at(deadline, p.events.recv()).await {
            Ok(Some(PipelineEvent::Block(b))) => blocks.push(b),
            Ok(Some(PipelineEvent::Alarm(a))) => alarms.push(a),
            Ok(None) => break,
            Err(_) => panic!("timed out at {:?}", blocks.last().map(|b| b.block_number)),
        }
    }
    (blocks, alarms)
}

async fn shutdown(mut p: Pipeline) -> (Vec<BlockOutput>, Vec<IntegrityAlarm>) {
    p.stop();
    let mut blocks = Vec::new();
    let mut alarms = Vec::new();
    while let Some(e) = p.events.recv().await {
        match e {
            PipelineEvent::Block(b) => blocks.push(b),
            PipelineEvent::Alarm(a) => alarms.push(a),
        }
    }
    p.join().await;
    (blocks, alarms)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tails_a_node_writing_live() {
    let dir = tempfile::tempdir().unwrap();
    let metrics = Arc::new(Metrics::new());
    let reg = Arc::new(RegistryHandle::fixed(registry(), metrics.clone()));
    let mut p = Pipeline::spawn(
        config(vec![tailer("a", dir.path())], None),
        reg,
        metrics.clone(),
    )
    .unwrap();
    let all = lines("base");
    write_blocks(dir.path(), &all, 0..all[0].len()).await;
    let (blocks, alarms) = collect(&mut p, LAST_BLOCK).await;
    assert!(alarms.is_empty());
    assert_eq!(blocks, expected_outputs());
    assert!(metrics.get(names::READER_ROTATIONS, &[("reader", "a")]) >= 3.0);
    assert!((metrics.get(names::OUTPUT_LAST_BLOCK, &[]) - LAST_BLOCK as f64).abs() < 0.5);
    assert!(
        metrics
            .render()
            .contains("hl_reader_blocks_total{reader=\"a\"} 60")
    );
    let (rest, _) = shutdown(p).await;
    assert!(rest.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resumes_from_checkpoint_without_breaking_links() {
    let dir = tempfile::tempdir().unwrap();
    let cp = dir.path().join("checkpoint.json");
    let node = dir.path().join("node");
    let all = lines("base");
    let metrics = Arc::new(Metrics::new());
    let reg = Arc::new(RegistryHandle::fixed(registry(), metrics.clone()));

    let mut p = Pipeline::spawn(
        config(vec![tailer("a", &node)], Some(cp.clone())),
        reg.clone(),
        metrics.clone(),
    )
    .unwrap();
    write_blocks(&node, &all, 0..30).await;
    let (mut first, _) = collect(&mut p, 1_000_029).await;
    let (rest, _) = shutdown(p).await;
    first.extend(rest);
    assert_eq!(first.last().unwrap().block_number, 1_000_029);
    assert!(cp.exists());

    // The node keeps writing while the reader is down.
    write_blocks(&node, &all, 30..45).await;
    // Restart in polling mode (no filesystem notifications).
    let mut polling = tailer("a", &node);
    polling.use_notify = false;
    let mut p = Pipeline::spawn(config(vec![polling], Some(cp.clone())), reg, metrics).unwrap();
    write_blocks(&node, &all, 45..all[0].len()).await;
    let (second, _) = collect(&mut p, LAST_BLOCK).await;
    shutdown(p).await;
    assert_eq!(second.first().unwrap().block_number, 1_000_030);
    first.extend(second);
    assert_eq!(first, expected_outputs());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_active_fills_one_nodes_gap_and_alarms() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a"), dir.path().join("b"));
    let metrics = Arc::new(Metrics::new());
    let reg = Arc::new(RegistryHandle::fixed(registry(), metrics.clone()));
    let mut p = Pipeline::spawn(
        config(vec![tailer("a", &a), tailer("b", &b)], None),
        reg,
        metrics.clone(),
    )
    .unwrap();
    let (base, gap) = (lines("base"), lines("gap"));
    // Node B (missing block 1000015) runs ahead of node A.
    write_blocks(&b, &gap, 0..gap[0].len()).await;
    write_blocks(&a, &base, 0..base[0].len()).await;
    let (blocks, mut alarms) = collect(&mut p, LAST_BLOCK).await;
    let (rest, more) = shutdown(p).await;
    assert!(rest.is_empty());
    alarms.extend(more);
    // Every block exactly once, identical to a single clean node.
    assert_eq!(blocks, expected_outputs());
    assert!(
        alarms.contains(&IntegrityAlarm::OneSidedGap {
            missing: ReaderId::B,
            first_block: 1_000_015,
            last_block: 1_000_015,
        }),
        "{alarms:?}"
    );
    assert!(
        !alarms
            .iter()
            .any(|a| matches!(a, IntegrityAlarm::Mismatch { .. }))
    );
    assert!(metrics.get(names::MERGE_CONFIRMED, &[]) >= 50.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_active_detects_a_content_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a"), dir.path().join("b"));
    let metrics = Arc::new(Metrics::new());
    let reg = Arc::new(RegistryHandle::fixed(registry(), metrics.clone()));
    let mut p = Pipeline::spawn(
        config(vec![tailer("a", &a), tailer("b", &b)], None),
        reg,
        metrics.clone(),
    )
    .unwrap();
    let base = lines("base");
    // Node B reports one different trade price in block 1000020.
    let mut tampered = base.clone();
    let line = &mut tampered[0][20].1;
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    assert!(!v["events"].as_array().unwrap().is_empty());
    let px = v["events"][0][1]["px"].as_str().unwrap().to_owned();
    *line = line.replace(&format!("\"px\":\"{px}\""), "\"px\":\"0.001\"");
    write_blocks(&a, &base, 0..base[0].len()).await;
    write_blocks(&b, &tampered, 0..tampered[0].len()).await;
    let (_, mut alarms) = collect(&mut p, LAST_BLOCK).await;
    let (_, more) = shutdown(p).await;
    alarms.extend(more);
    assert!(
        alarms.iter().any(|a| matches!(
            a,
            IntegrityAlarm::Mismatch {
                block_number: 1_000_020,
                ..
            }
        )),
        "{alarms:?}"
    );
    assert!(metrics.get(names::MERGE_MISMATCHES, &[]) >= 1.0);
}
