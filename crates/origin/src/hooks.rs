//! The archive's injected components (`archive::cli::Hooks`), built from
//! the node reader and the book builder:
//!
//! - **Filters**: `hl_node_reader::filter_line` per stream slug (`fills`,
//!   `order_statuses`, `raw_book_diffs`). Lines that do not parse are
//!   dropped from the filtered tier (and counted by the archive); the
//!   private tier keeps them verbatim.
//! - **Parser**: the day's filtered raw files go through
//!   `hl_node_reader::replay::Replay` (the same join, normalization and
//!   sequencing code as live tailing), then every block through the same
//!   [`Deriver`] as the origin. The result is the records of every
//!   published batch plus the derived records and coverage, exactly as the
//!   origin publishes them.

use std::path::Path;
use std::sync::Arc;

use archive::BoxError;
use archive::capture::{FilterSet, LineFilter};
use archive::cli::Hooks;
use archive::replay::{RawFile, RawParser};
use hl_node_reader::replay::{Replay, sources_from_texts};
use hl_node_reader::{Stream, filter_line};
use mesh_types::{Record, Registry};

use crate::derive::{BookStart, DeriveConfig, Deriver};

/// The node reader's line filter for one stream.
#[must_use]
pub fn line_filter(stream: Stream) -> Arc<dyn LineFilter> {
    Arc::new(move |line: &[u8]| -> Option<Vec<u8>> {
        let text = std::str::from_utf8(line).ok()?;
        if text.trim().is_empty() {
            return None;
        }
        match filter_line(stream, text) {
            Ok(out) => Some(out.into_bytes()),
            Err(e) => {
                tracing::warn!(stream = stream.as_str(), error = %e, "line dropped from the filtered tier");
                None
            }
        }
    })
}

/// Filters for every admitted stream, keyed by the archive's stream slugs.
///
/// # Errors
///
/// Never in practice (fixed slugs).
pub fn filters() -> archive::Result<FilterSet> {
    let mut set = FilterSet::new();
    for s in Stream::ALL {
        set = set.with(s.as_str(), line_filter(s))?;
    }
    Ok(set)
}

/// Turns a day's filtered raw files into records.
#[derive(Debug, Clone)]
pub struct NodeParser {
    registry: Registry,
    derive: DeriveConfig,
}

impl NodeParser {
    /// A parser for markets in `registry`, deriving with `derive`.
    #[must_use]
    pub fn new(registry: Registry, derive: DeriveConfig) -> Self {
        Self { registry, derive }
    }

    /// Parses files sorted by (day, hour, stream), as the archive passes
    /// them.
    ///
    /// # Errors
    ///
    /// Fails on a line that does not parse or a file of an unknown stream.
    pub fn parse_files(&self, files: &[RawFile]) -> Result<Vec<Record>, BoxError> {
        let mut texts: [String; 3] = Default::default();
        for f in files {
            let Some(stream) = Stream::ALL.into_iter().find(|s| s.as_str() == f.stream) else {
                return Err(format!("{}: unknown stream {:?}", f.path, f.stream).into());
            };
            let text = std::str::from_utf8(&f.data).map_err(|e| format!("{}: {e}", f.path))?;
            texts[stream.index()].push_str(text);
            if !text.is_empty() && !text.ends_with('\n') {
                texts[stream.index()].push('\n');
            }
        }
        let mut deriver = Deriver::new(self.derive.clone());
        let mut out = Vec::new();
        for item in Replay::new(sources_from_texts(texts), self.registry.clone()).sequenced() {
            let block = item.map_err(|e| format!("replay: {e}"))?;
            for batch in &block.batches {
                out.extend(batch.records.iter().cloned());
            }
            out.extend(deriver.block(&block).records);
        }
        Ok(out)
    }
}

impl RawParser for NodeParser {
    fn parse(&self, files: &[RawFile]) -> Result<Vec<Record>, BoxError> {
        self.parse_files(files)
    }
}

/// Loads a registry file (`mesh_types::Registry` JSON).
///
/// # Errors
///
/// Fails if the file cannot be read or parsed.
pub fn load_registry(path: &Path) -> Result<Registry, BoxError> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?)
}

/// A parser that reads its registry from `BLEH_ARCHIVE_REGISTRY` when it
/// first runs (the archive binary builds its hooks before parsing
/// arguments), starts books as `BLEH_ARCHIVE_BOOK_START` says (`empty`
/// for a first recorded day; default `snapshot`) and checkpoints books every
/// `BLEH_BOOK_CHECKPOINT_SECS` of block time (default 600, as the origin;
/// the two must agree for archived checkpoints to equal live ones).
#[derive(Debug, Clone, Copy, Default)]
pub struct EnvParser;

impl RawParser for EnvParser {
    fn parse(&self, files: &[RawFile]) -> Result<Vec<Record>, BoxError> {
        let path = std::env::var_os("BLEH_ARCHIVE_REGISTRY").ok_or(
            "set BLEH_ARCHIVE_REGISTRY to the instrument registry (registry.json) to replay",
        )?;
        let registry = load_registry(Path::new(&path))?;
        let mut derive = DeriveConfig::hyperliquid()?;
        if let Ok(secs) = std::env::var("BLEH_BOOK_CHECKPOINT_SECS") {
            let secs: i64 = secs
                .parse()
                .map_err(|e| format!("BLEH_BOOK_CHECKPOINT_SECS: {e}"))?;
            derive.processor.checkpoint = if secs <= 0 {
                book::checkpoint::CheckpointPolicy::Never
            } else {
                book::checkpoint::CheckpointPolicy::EveryMillis(secs.saturating_mul(1_000))
            };
        }
        derive.book_start = match std::env::var("BLEH_ARCHIVE_BOOK_START").as_deref() {
            Ok("empty") => BookStart::Empty,
            Ok("snapshot") | Err(_) => BookStart::Snapshot,
            Ok(other) => {
                return Err(format!(
                    "BLEH_ARCHIVE_BOOK_START: expected empty or snapshot, got {other:?}"
                )
                .into());
            }
        };
        NodeParser::new(registry, derive).parse_files(files)
    }
}

/// The hooks the `archive` binary runs with.
///
/// # Errors
///
/// Never in practice.
pub fn env_hooks() -> archive::Result<Hooks> {
    Ok(Hooks {
        filters: filters()?,
        parser: Some(Arc::new(EnvParser)),
    })
}

/// Hooks with an explicit registry and derive settings (tests, embedding).
///
/// # Errors
///
/// Never in practice.
pub fn hooks(registry: Registry, derive: DeriveConfig) -> archive::Result<Hooks> {
    Ok(Hooks {
        filters: filters()?,
        parser: Some(Arc::new(NodeParser::new(registry, derive))),
    })
}
