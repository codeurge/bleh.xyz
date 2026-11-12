//! One-pass reading of a node data directory (raw or filtered raw).
//!
//! The replay path used by tests, by `hl-node-reader --replay` and by the
//! archive's rebuilds: the same join, normalization and sequencing code as
//! live tailing, reading closed files once. The three streams are read in
//! lockstep (always advancing the stream that is furthest behind), so
//! memory stays bounded by the lag between streams, not by file size.

use std::io;
use std::path::Path;

use mesh_types::Registry;

use crate::checkpoint::Position;
use crate::files::{Line, LineReader};
use crate::filter::StreamDirs;
use crate::join::{Joiner, PushOutcome};
use crate::normalize::{ReaderBlock, normalize};
use crate::raw::{ParsedLine, Stream};
use crate::sequence::{BlockOutput, Sequencer};

/// A source of lines for one stream.
pub type LineSource = Box<dyn Iterator<Item = io::Result<Line>> + Send>;

/// A non-fatal problem met during a replay. Iteration continues after it.
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    /// A file could not be read.
    #[error("{stream}: {source}")]
    Io {
        /// Stream.
        stream: Stream,
        /// Error.
        source: io::Error,
    },
    /// A line could not be parsed (the block will be incomplete).
    #[error("{stream} {}@{}: {source}", pos.file, pos.offset)]
    Parse {
        /// Stream.
        stream: Stream,
        /// Line position.
        pos: Position,
        /// Error.
        source: serde_json::Error,
    },
    /// A line repeated or went back in block number (dropped).
    #[error("{stream} {}@{}: block {block} out of order", pos.file, pos.offset)]
    OutOfOrder {
        /// Stream.
        stream: Stream,
        /// Line position.
        pos: Position,
        /// The line's block.
        block: u64,
    },
}

/// Replays three line sources into [`ReaderBlock`]s.
pub struct Replay {
    sources: [Option<LineSource>; 3],
    joiner: Joiner,
    registry: Registry,
}

impl std::fmt::Debug for Replay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Replay")
            .field("joiner", &self.joiner)
            .field("instruments", &self.registry.len())
            .finish_non_exhaustive()
    }
}

/// Largest gap between streams the replay tolerates before releasing a
/// block without the lagging stream.
pub const REPLAY_MAX_LAG_BLOCKS: u64 = 10_000;

impl Replay {
    /// A replay over arbitrary line sources, in [`Stream::ALL`] order.
    #[must_use]
    pub fn new(sources: [LineSource; 3], registry: Registry) -> Self {
        let [a, b, c] = sources;
        Self {
            sources: [Some(a), Some(b), Some(c)],
            joiner: Joiner::new(None, REPLAY_MAX_LAG_BLOCKS),
            registry,
        }
    }

    /// A replay of a node data directory (or a filtered-raw tree with the
    /// same layout).
    ///
    /// # Errors
    ///
    /// Fails if a stream directory cannot be listed.
    pub fn open(root: &Path, dirs: &StreamDirs, registry: Registry) -> io::Result<Self> {
        let open = |s: Stream| -> io::Result<LineSource> {
            Ok(Box::new(LineReader::open(&root.join(dirs.get(s)))?))
        };
        Ok(Self::new(
            [
                open(Stream::Fills)?,
                open(Stream::OrderStatuses)?,
                open(Stream::RawBookDiffs)?,
            ],
            registry,
        ))
    }

    /// Wraps the replay with a [`Sequencer`], yielding published outputs.
    #[must_use]
    pub fn sequenced(self) -> SequencedReplay {
        SequencedReplay {
            replay: self,
            sequencer: Sequencer::default(),
        }
    }

    fn next_stream(&self) -> Option<usize> {
        let high = self.joiner.high();
        (0..3)
            .filter(|&i| self.sources[i].is_some())
            .min_by_key(|&i| high[i].map_or(0, |h| h.saturating_add(1)))
    }
}

impl Iterator for Replay {
    type Item = Result<ReaderBlock, ReplayError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(b) = self.joiner.pop_ready() {
                return Some(Ok(normalize(&b, &self.registry)));
            }
            let Some(i) = self.next_stream() else {
                return self
                    .joiner
                    .flush()
                    .map(|b| Ok(normalize(&b, &self.registry)));
            };
            let stream = Stream::ALL[i];
            let line = match self.sources[i].as_mut().and_then(Iterator::next) {
                None => {
                    self.sources[i] = None;
                    continue;
                }
                Some(Err(source)) => {
                    self.sources[i] = None;
                    return Some(Err(ReplayError::Io { stream, source }));
                }
                Some(Ok(line)) => line,
            };
            let parsed = match ParsedLine::parse(stream, &line.text) {
                Ok(p) => p,
                Err(source) => {
                    return Some(Err(ReplayError::Parse {
                        stream,
                        pos: line.pos,
                        source,
                    }));
                }
            };
            let block = parsed.block_number();
            if self.joiner.push(parsed, None) == PushOutcome::OutOfOrder {
                return Some(Err(ReplayError::OutOfOrder {
                    stream,
                    pos: line.pos,
                    block,
                }));
            }
        }
    }
}

/// A [`Replay`] followed by a [`Sequencer`].
#[derive(Debug)]
pub struct SequencedReplay {
    replay: Replay,
    sequencer: Sequencer,
}

impl Iterator for SequencedReplay {
    type Item = Result<BlockOutput, ReplayError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.replay.next()? {
                Err(e) => return Some(Err(e)),
                Ok(b) => {
                    if let Some(out) = self.sequencer.sequence(b) {
                        return Some(Ok(out));
                    }
                }
            }
        }
    }
}

/// Line sources from in-memory texts (one string per stream, possibly
/// several concatenated hourly files), for tests and generators.
pub fn sources_from_texts(texts: [String; 3]) -> [LineSource; 3] {
    texts.map(|t| {
        let lines: Vec<io::Result<Line>> = t
            .lines()
            .filter(|l| !l.trim().is_empty())
            .enumerate()
            .map(|(i, l)| {
                Ok(Line {
                    pos: Position {
                        file: "memory".into(),
                        offset: i as u64,
                    },
                    text: l.to_owned(),
                })
            })
            .collect();
        Box::new(lines.into_iter()) as LineSource
    })
}
