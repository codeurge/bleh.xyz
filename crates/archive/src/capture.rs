//! Raw capture: two hourly-rotated writers fed by raw node lines.
//!
//! - **Private tier** (`hyperliquid-raw-private`): every line as the node
//!   wrote it, including fields the admission excludes. Never published.
//!   zstd with long-distance matching and, optionally, a trained dictionary
//!   per stream.
//! - **Filtered tier** (`hyperliquid-raw`, the public reproducibility
//!   root): each line passed through the injected [`LineFilter`] for its
//!   stream; streams without a filter are not written. zstd level 19, one
//!   frame, no dictionary, content size and checksum flags on
//!   (`docs/specs/archive-layout-v0.md` § 2), so equal input gives equal
//!   bytes.
//!
//! Both write `stream=<stream>/date=<day>/<HH>.jsonl.zst`, where the day and
//! hour are those of the node file the line came from. Lines are appended to
//! an uncompressed staging file per stream; when a line for a later hour
//! arrives (or on [`HourWriter::seal_before`] / [`HourWriter::seal_all`]) the
//! hour is sealed: compressed in one frame into `ready/`, then uploaded by
//! [`crate::store::upload_ready`]. Sealed hours are never reopened.
//!
//! Staging layout, per archive:
//!
//! ```text
//! <staging>/<archive>/open/<path>.partial   hour being written (uncompressed)
//! <staging>/<archive>/ready/<path>          sealed, awaiting upload
//! <staging>/<archive>/uploaded/<path>       uploaded (if kept locally)
//! <staging>/<archive>/conflicts/<path>      rejected by write-once storage
//! ```
//!
//! After a restart, open hours are found again and appended to; the reader
//! resumes after [`HourWriter::position`] lines.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mesh_types::Sha256Digest;

use crate::error::{Error, IoContext, Result};
use crate::layout::{self, Day, RawPath};
use crate::metrics;

/// Filters one raw line of one stream: returns the filtered line (without
/// a trailing newline), or `None` to drop it. The node reader provides the
/// implementation (the field allowlist of ADM-0001); this crate only
/// applies it.
pub trait LineFilter: Send + Sync {
    /// Filters `line` (no trailing newline).
    fn filter_line(&self, line: &[u8]) -> Option<Vec<u8>>;
}

impl<F> LineFilter for F
where
    F: Fn(&[u8]) -> Option<Vec<u8>> + Send + Sync,
{
    fn filter_line(&self, line: &[u8]) -> Option<Vec<u8>> {
        self(line)
    }
}

/// The filters of the filtered tier, by stream. A stream with no filter is
/// excluded from the filtered tier entirely.
#[derive(Clone, Default)]
pub struct FilterSet {
    filters: BTreeMap<String, Arc<dyn LineFilter>>,
}

impl fmt::Debug for FilterSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FilterSet")
            .field("streams", &self.filters.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl FilterSet {
    /// An empty set (nothing reaches the filtered tier).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the filter for `stream`.
    ///
    /// # Errors
    ///
    /// Fails if `stream` is not a slug.
    pub fn with(mut self, stream: &str, filter: Arc<dyn LineFilter>) -> Result<Self> {
        layout::check_slug("stream", stream)?;
        self.filters.insert(stream.to_owned(), filter);
        Ok(self)
    }

    /// The filter for `stream`.
    #[must_use]
    pub fn get(&self, stream: &str) -> Option<&Arc<dyn LineFilter>> {
        self.filters.get(stream)
    }

    /// Streams with a filter.
    pub fn streams(&self) -> impl Iterator<Item = &str> {
        self.filters.keys().map(String::as_str)
    }

    /// Whether no stream has a filter.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.filters.is_empty()
    }
}

/// A zstd dictionary for one stream of the private tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dictionary {
    /// zstd dictionary ID (recorded in every frame compressed with it).
    pub id: u32,
    /// Dictionary bytes.
    pub bytes: Vec<u8>,
}

impl Dictionary {
    /// Wraps trained dictionary bytes.
    ///
    /// # Errors
    ///
    /// Fails if the bytes are not a zstd dictionary with an ID.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let id = zstd::zstd_safe::get_dict_id_from_dict(&bytes)
            .ok_or_else(|| Error::invalid("not a zstd dictionary (no dictionary ID)"))?
            .get();
        Ok(Self { id, bytes })
    }

    /// Trains a dictionary of at most `max_size` bytes from sample lines.
    ///
    /// # Errors
    ///
    /// Fails if zstd rejects the samples (too few or too small).
    pub fn train<S: AsRef<[u8]>>(samples: &[S], max_size: usize) -> Result<Self> {
        let bytes = zstd::dict::from_samples(samples, max_size)
            .map_err(|e| Error::invalid(format!("dictionary training failed: {e}")))?;
        Self::from_bytes(bytes)
    }

    /// Path of the dictionary inside the private archive:
    /// `_dictionaries/<stream>/<id>.zdict`.
    #[must_use]
    pub fn path(&self, stream: &str) -> String {
        format!("{}/{stream}/{}.zdict", layout::DICTIONARIES_DIR, self.id)
    }
}

/// zstd settings of a tier.
#[derive(Debug, Clone)]
pub struct Compression {
    /// zstd level.
    pub level: i32,
    /// Long-distance matching window (log2 bytes); `None` disables it.
    pub long_window_log: Option<u32>,
    /// Dictionaries by stream.
    pub dictionaries: BTreeMap<String, Arc<Dictionary>>,
}

impl Compression {
    /// The filtered tier's pinned settings: level 19, no long-distance
    /// matching, no dictionary.
    #[must_use]
    pub fn filtered() -> Self {
        Self {
            level: 19,
            long_window_log: None,
            dictionaries: BTreeMap::new(),
        }
    }

    /// The private tier's default: level 19 with a 128 MiB long-distance
    /// window.
    #[must_use]
    pub fn private() -> Self {
        Self {
            level: 19,
            long_window_log: Some(27),
            dictionaries: BTreeMap::new(),
        }
    }
}

/// Settings of one tier.
#[derive(Debug, Clone)]
pub struct TierConfig {
    /// Archive name.
    pub archive: String,
    /// Local staging directory (shared by tiers; each uses a subdirectory).
    pub staging: PathBuf,
    /// Compression.
    pub compression: Compression,
}

impl TierConfig {
    /// The filtered tier (`hyperliquid-raw`).
    pub fn filtered(staging: impl Into<PathBuf>) -> Self {
        Self {
            archive: layout::RAW_ARCHIVE.to_owned(),
            staging: staging.into(),
            compression: Compression::filtered(),
        }
    }

    /// The private tier (`hyperliquid-raw-private`).
    pub fn private(staging: impl Into<PathBuf>) -> Self {
        Self {
            archive: layout::PRIVATE_RAW_ARCHIVE.to_owned(),
            staging: staging.into(),
            compression: Compression::private(),
        }
    }

    fn dir(&self, state: &str) -> PathBuf {
        self.staging.join(&self.archive).join(state)
    }
}

/// A sealed hour file, ready for upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedFile {
    /// Archive name.
    pub archive: String,
    /// Path relative to the archive root.
    pub path: String,
    /// Local path under `ready/`.
    pub local: PathBuf,
    /// SHA-256 of the compressed file.
    pub sha256: Sha256Digest,
    /// Compressed size.
    pub size: u64,
    /// Uncompressed size.
    pub raw_size: u64,
    /// Lines in the file.
    pub lines: u64,
}

struct OpenHour {
    day: Day,
    hour: u8,
    partial: PathBuf,
    writer: BufWriter<File>,
    lines: u64,
}

/// Compresses `src` (uncompressed lines) into `dst` as one zstd frame.
fn compress_file(src: &Path, dst: &Path, c: &Compression, stream: &str) -> Result<u64> {
    let len = std::fs::metadata(src).at(src)?.len();
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).at(parent)?;
    }
    let out = File::create(dst).at(dst)?;
    let dict = c.dictionaries.get(stream);
    let mut enc = match dict {
        Some(d) => zstd::stream::Encoder::with_dictionary(BufWriter::new(out), c.level, &d.bytes),
        None => zstd::stream::Encoder::new(BufWriter::new(out), c.level),
    }
    .at(dst)?;
    enc.include_checksum(true).at(dst)?;
    enc.include_contentsize(true).at(dst)?;
    enc.set_pledged_src_size(Some(len)).at(dst)?;
    if let Some(w) = c.long_window_log {
        enc.long_distance_matching(true).at(dst)?;
        enc.window_log(w).at(dst)?;
    }
    let mut input = BufReader::new(File::open(src).at(src)?);
    std::io::copy(&mut input, &mut enc).at(dst)?;
    let mut w = enc.finish().at(dst)?;
    w.flush().at(dst)?;
    w.into_inner()
        .map_err(|e| Error::io(dst, e.into_error()))?
        .sync_all()
        .at(dst)?;
    Ok(len)
}

/// Decompresses a raw file. `dictionaries` supplies dictionaries by ID for
/// private-tier frames that need one.
///
/// # Errors
///
/// Fails on malformed input or a missing dictionary.
pub fn decompress(data: &[u8], dictionaries: &BTreeMap<u32, Dictionary>) -> Result<Vec<u8>> {
    let dict_id = zstd::zstd_safe::get_dict_id_from_frame(data).map(std::num::NonZeroU32::get);
    let mut out = Vec::new();
    let err = |e: std::io::Error| Error::invalid(format!("zstd: {e}"));
    match dict_id {
        Some(id) => {
            let d = dictionaries
                .get(&id)
                .ok_or_else(|| Error::invalid(format!("zstd frame needs dictionary {id}")))?;
            let mut dec = zstd::stream::Decoder::with_dictionary(data, &d.bytes).map_err(err)?;
            dec.window_log_max(31).map_err(err)?;
            dec.read_to_end(&mut out).map_err(err)?;
        }
        None => {
            let mut dec = zstd::stream::Decoder::new(data).map_err(err)?;
            dec.window_log_max(31).map_err(err)?;
            dec.read_to_end(&mut out).map_err(err)?;
        }
    }
    Ok(out)
}

/// Compresses bytes exactly as the filtered tier seals an hour (for tests
/// and tools that rebuild raw files).
///
/// # Errors
///
/// Fails on zstd errors.
pub fn compress_filtered(data: &[u8]) -> Result<Vec<u8>> {
    let err = |e: std::io::Error| Error::invalid(format!("zstd: {e}"));
    let mut enc =
        zstd::stream::Encoder::new(Vec::new(), Compression::filtered().level).map_err(err)?;
    enc.include_checksum(true).map_err(err)?;
    enc.include_contentsize(true).map_err(err)?;
    enc.set_pledged_src_size(Some(data.len() as u64))
        .map_err(err)?;
    enc.write_all(data).map_err(err)?;
    enc.finish().map_err(err)
}

/// Hourly-rotated writer for one tier.
pub struct HourWriter {
    cfg: TierConfig,
    open: BTreeMap<String, OpenHour>,
    sealed_upto: BTreeMap<String, (Day, u8)>,
}

impl fmt::Debug for HourWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HourWriter")
            .field("archive", &self.cfg.archive)
            .field("open", &self.open.keys().collect::<Vec<_>>())
            .finish()
    }
}

fn count_lines(path: &Path) -> Result<u64> {
    let f = File::open(path).at(path)?;
    let mut n = 0u64;
    let mut r = BufReader::new(f);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let read = r.read_until(b'\n', &mut buf).at(path)?;
        if read == 0 {
            break;
        }
        if buf.last() != Some(&b'\n') {
            // A torn last line from a crash: drop it so the reader rewrites it.
            let len = std::fs::metadata(path).at(path)?.len() - buf.len() as u64;
            OpenOptions::new()
                .write(true)
                .open(path)
                .at(path)?
                .set_len(len)
                .at(path)?;
            break;
        }
        n += 1;
    }
    Ok(n)
}

impl HourWriter {
    /// Opens the tier, recovering any open hours left by a previous run.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors or if two open hours exist for one stream.
    pub fn open(cfg: TierConfig) -> Result<Self> {
        layout::check_slug("archive", &cfg.archive)?;
        let mut w = Self {
            cfg,
            open: BTreeMap::new(),
            sealed_upto: BTreeMap::new(),
        };
        let open_dir = w.cfg.dir("open");
        for rel in layout::list_files(&open_dir)? {
            let Some(raw_rel) = rel.strip_suffix(".partial") else {
                continue;
            };
            let Some(RawPath { stream, day, hour }) = layout::parse_raw_path(raw_rel) else {
                continue;
            };
            let partial = open_dir.join(&rel);
            if let Some(prev) = w.open.get(&stream) {
                return Err(Error::invalid(format!(
                    "{}: two open hours for stream {stream} ({} {} and {day} {hour})",
                    w.cfg.archive, prev.day, prev.hour
                )));
            }
            let lines = count_lines(&partial)?;
            let file = OpenOptions::new()
                .append(true)
                .open(&partial)
                .at(&partial)?;
            tracing::info!(archive = %w.cfg.archive, stream, %day, hour, lines, "recovered open hour");
            w.open.insert(
                stream,
                OpenHour {
                    day,
                    hour,
                    partial,
                    writer: BufWriter::new(file),
                    lines,
                },
            );
        }
        Ok(w)
    }

    /// The archive name.
    #[must_use]
    pub fn archive(&self) -> &str {
        &self.cfg.archive
    }

    /// The open hour of `stream` and the lines written to it so far; a
    /// restarted reader skips that many lines of that node file.
    #[must_use]
    pub fn position(&self, stream: &str) -> Option<(Day, u8, u64)> {
        self.open.get(stream).map(|o| (o.day, o.hour, o.lines))
    }

    /// Appends a line (without trailing newline) to the hour file of
    /// `stream`, sealing the stream's previous hour first if it differs.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors, an invalid stream or hour, a line containing a
    /// newline, or a line for an hour that is already sealed.
    pub fn push(
        &mut self,
        stream: &str,
        day: Day,
        hour: u8,
        line: &[u8],
    ) -> Result<Option<SealedFile>> {
        layout::check_slug("stream", stream)?;
        if hour > 23 {
            return Err(Error::invalid(format!("hour {hour}")));
        }
        if line.contains(&b'\n') {
            return Err(Error::invalid("a raw line contains a newline"));
        }
        let mut sealed = None;
        if let Some(o) = self.open.get(stream) {
            match (o.day, o.hour).cmp(&(day, hour)) {
                std::cmp::Ordering::Less => sealed = self.seal_stream(stream)?,
                std::cmp::Ordering::Greater => {
                    return Err(Error::invalid(format!(
                        "{}: line for {day} hour {hour} of {stream} after hour {} {} was opened",
                        self.cfg.archive, o.day, o.hour
                    )));
                }
                std::cmp::Ordering::Equal => {}
            }
        }
        if !self.open.contains_key(stream) {
            if self
                .sealed_upto
                .get(stream)
                .is_some_and(|last| *last >= (day, hour))
            {
                return Err(Error::invalid(format!(
                    "{}: {stream} {day} hour {hour} is already sealed",
                    self.cfg.archive
                )));
            }
            let rel = layout::raw_path(stream, day, hour);
            let ready = self.cfg.dir("ready").join(&rel);
            if ready.exists() {
                return Err(Error::invalid(format!(
                    "{}: {rel} is already sealed",
                    self.cfg.archive
                )));
            }
            let partial = self.cfg.dir("open").join(format!("{rel}.partial"));
            if let Some(parent) = partial.parent() {
                std::fs::create_dir_all(parent).at(parent)?;
            }
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&partial)
                .at(&partial)?;
            self.open.insert(
                stream.to_owned(),
                OpenHour {
                    day,
                    hour,
                    partial,
                    writer: BufWriter::new(file),
                    lines: 0,
                },
            );
        }
        if let Some(o) = self.open.get_mut(stream) {
            o.writer.write_all(line).at(&o.partial)?;
            o.writer.write_all(b"\n").at(&o.partial)?;
            o.lines += 1;
        }
        metrics::inc(
            "archive_capture_lines_total",
            &[("archive", &self.cfg.archive), ("stream", stream)],
            1,
        );
        metrics::inc(
            "archive_capture_bytes_total",
            &[("archive", &self.cfg.archive), ("stream", stream)],
            line.len() as u64 + 1,
        );
        Ok(sealed)
    }

    /// Flushes buffered lines of every open hour to the staging files.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn flush(&mut self) -> Result<()> {
        for o in self.open.values_mut() {
            o.writer.flush().at(&o.partial)?;
        }
        Ok(())
    }

    /// Seals the open hour of `stream`, if any.
    ///
    /// # Errors
    ///
    /// Fails on I/O or compression errors.
    pub fn seal_stream(&mut self, stream: &str) -> Result<Option<SealedFile>> {
        let Some(mut o) = self.open.remove(stream) else {
            return Ok(None);
        };
        o.writer.flush().at(&o.partial)?;
        o.writer.get_ref().sync_all().at(&o.partial)?;
        drop(o.writer);
        let rel = layout::raw_path(stream, o.day, o.hour);
        let ready = self.cfg.dir("ready").join(&rel);
        let mut tmp = ready.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let raw_size = compress_file(&o.partial, &tmp, &self.cfg.compression, stream)?;
        std::fs::rename(&tmp, &ready).at(&ready)?;
        std::fs::remove_file(&o.partial).at(&o.partial)?;
        let sha256 = crate::digest::sha256_file(&ready)?;
        let size = std::fs::metadata(&ready).at(&ready)?.len();
        self.sealed_upto.insert(stream.to_owned(), (o.day, o.hour));
        metrics::inc(
            "archive_capture_files_sealed_total",
            &[("archive", &self.cfg.archive), ("stream", stream)],
            1,
        );
        tracing::info!(archive = %self.cfg.archive, path = %rel, lines = o.lines, raw_size, size, "sealed hour");
        Ok(Some(SealedFile {
            archive: self.cfg.archive.clone(),
            path: rel,
            local: ready,
            sha256,
            size,
            raw_size,
            lines: o.lines,
        }))
    }

    /// Seals every open hour strictly before `(day, hour)` (call once per
    /// wall-clock hour so quiet streams rotate too).
    ///
    /// # Errors
    ///
    /// Fails on I/O or compression errors.
    pub fn seal_before(&mut self, day: Day, hour: u8) -> Result<Vec<SealedFile>> {
        let due: Vec<String> = self
            .open
            .iter()
            .filter(|(_, o)| (o.day, o.hour) < (day, hour))
            .map(|(s, _)| s.clone())
            .collect();
        let mut out = Vec::new();
        for s in due {
            out.extend(self.seal_stream(&s)?);
        }
        Ok(out)
    }

    /// Seals every open hour.
    ///
    /// # Errors
    ///
    /// Fails on I/O or compression errors.
    pub fn seal_all(&mut self) -> Result<Vec<SealedFile>> {
        let streams: Vec<String> = self.open.keys().cloned().collect();
        let mut out = Vec::new();
        for s in streams {
            out.extend(self.seal_stream(&s)?);
        }
        Ok(out)
    }
}

/// One raw line with its origin.
#[derive(Debug, Clone, Copy)]
pub struct RawLine<'a> {
    /// Stream slug (`fills`, `order_statuses`, `raw_book_diffs`, ...).
    pub stream: &'a str,
    /// UTC day of the node file.
    pub day: Day,
    /// UTC hour of the node file.
    pub hour: u8,
    /// The line, with or without a trailing newline.
    pub line: &'a [u8],
}

/// Both tiers together.
#[derive(Debug)]
pub struct Capture {
    private: Option<HourWriter>,
    filtered: Option<HourWriter>,
    filters: FilterSet,
}

impl Capture {
    /// A capture writing the tiers that are given. The filtered tier needs
    /// at least one filter.
    ///
    /// # Errors
    ///
    /// Fails if neither tier is given, or the filtered tier has no filters.
    pub fn new(
        private: Option<HourWriter>,
        filtered: Option<HourWriter>,
        filters: FilterSet,
    ) -> Result<Self> {
        if private.is_none() && filtered.is_none() {
            return Err(Error::invalid("capture needs at least one tier"));
        }
        if filtered.is_some() && filters.is_empty() {
            return Err(Error::NotWired(
                "the filtered tier needs the node reader's line filters; none are wired",
            ));
        }
        Ok(Self {
            private,
            filtered,
            filters,
        })
    }

    /// The private tier writer.
    #[must_use]
    pub fn private(&self) -> Option<&HourWriter> {
        self.private.as_ref()
    }

    /// The filtered tier writer.
    #[must_use]
    pub fn filtered(&self) -> Option<&HourWriter> {
        self.filtered.as_ref()
    }

    /// Writes one raw line to the private tier and, filtered, to the
    /// filtered tier. Returns any hours this sealed.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors, out-of-order hours, or a filter output that
    /// contains a newline.
    pub fn push(&mut self, raw: &RawLine<'_>) -> Result<Vec<SealedFile>> {
        let line = raw.line.strip_suffix(b"\n").unwrap_or(raw.line);
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let mut sealed = Vec::new();
        if let Some(p) = &mut self.private {
            sealed.extend(p.push(raw.stream, raw.day, raw.hour, line)?);
        }
        if let Some(f) = &mut self.filtered {
            match self.filters.get(raw.stream) {
                Some(filter) => match filter.filter_line(line) {
                    Some(out) => sealed.extend(f.push(raw.stream, raw.day, raw.hour, &out)?),
                    None => metrics::inc(
                        "archive_capture_filtered_out_total",
                        &[("stream", raw.stream)],
                        1,
                    ),
                },
                None => metrics::inc(
                    "archive_capture_stream_excluded_total",
                    &[("stream", raw.stream)],
                    1,
                ),
            }
        }
        Ok(sealed)
    }

    /// Seals every open hour before `(day, hour)` in both tiers.
    ///
    /// # Errors
    ///
    /// Fails on I/O or compression errors.
    pub fn seal_before(&mut self, day: Day, hour: u8) -> Result<Vec<SealedFile>> {
        let mut out = Vec::new();
        for w in [&mut self.private, &mut self.filtered]
            .into_iter()
            .flatten()
        {
            out.extend(w.seal_before(day, hour)?);
        }
        Ok(out)
    }

    /// Seals every open hour in both tiers.
    ///
    /// # Errors
    ///
    /// Fails on I/O or compression errors.
    pub fn seal_all(&mut self) -> Result<Vec<SealedFile>> {
        let mut out = Vec::new();
        for w in [&mut self.private, &mut self.filtered]
            .into_iter()
            .flatten()
        {
            out.extend(w.seal_all()?);
        }
        Ok(out)
    }

    /// Flushes both tiers.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn flush(&mut self) -> Result<()> {
        for w in [&mut self.private, &mut self.filtered]
            .into_iter()
            .flatten()
        {
            w.flush()?;
        }
        Ok(())
    }
}

/// Lists a node stream directory's hourly files
/// (`<dir>/hourly/<YYYYMMDD>/<H>`), sorted by day and hour.
///
/// # Errors
///
/// Fails on I/O errors.
pub fn node_hour_files(dir: &Path) -> Result<Vec<(Day, u8, PathBuf)>> {
    let mut out = Vec::new();
    for rel in layout::list_files(&dir.join("hourly"))? {
        let mut it = rel.split('/');
        let (Some(d), Some(h), None) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let (Ok(day), Ok(hour)) = (Day::parse_compact(d), h.parse::<u8>()) else {
            continue;
        };
        if hour <= 23 {
            out.push((day, hour, dir.join("hourly").join(d).join(h)));
        }
    }
    out.sort();
    Ok(out)
}

/// Captures complete node hour files (batch mode, for backfills, tests and
/// catching up). `streams` maps a stream slug to its node directory. Hours
/// are fed in time order across streams, then everything is sealed.
///
/// # Errors
///
/// Fails on I/O or capture errors.
pub fn capture_node_dirs(
    capture: &mut Capture,
    streams: &[(String, PathBuf)],
) -> Result<Vec<SealedFile>> {
    let mut hours = Vec::new();
    for (stream, dir) in streams {
        for (day, hour, path) in node_hour_files(dir)? {
            hours.push((day, hour, stream.clone(), path));
        }
    }
    hours.sort();
    let mut sealed = Vec::new();
    for (day, hour, stream, path) in hours {
        let skip = [capture.private(), capture.filtered()]
            .into_iter()
            .flatten()
            .filter_map(|w| w.position(&stream))
            .filter(|(d, h, _)| (*d, *h) == (day, hour))
            .map(|(_, _, n)| n)
            .max()
            .unwrap_or(0);
        let f = File::open(&path).at(&path)?;
        let mut r = BufReader::new(f);
        let mut buf = Vec::new();
        let mut n = 0u64;
        loop {
            buf.clear();
            if r.read_until(b'\n', &mut buf).at(&path)? == 0 {
                break;
            }
            n += 1;
            if n <= skip {
                continue;
            }
            sealed.extend(capture.push(&RawLine {
                stream: &stream,
                day,
                hour,
                line: &buf,
            })?);
        }
    }
    sealed.extend(capture.seal_all()?);
    Ok(sealed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upper(line: &[u8]) -> Option<Vec<u8>> {
        if line.starts_with(b"drop") {
            None
        } else {
            Some(line.to_ascii_uppercase())
        }
    }

    fn d(s: &str) -> Day {
        s.parse().unwrap()
    }

    #[test]
    fn rotates_by_hour_and_filters() {
        let dir = tempfile::tempdir().unwrap();
        let filters = FilterSet::new().with("fills", Arc::new(upper)).unwrap();
        let mut c = Capture::new(
            Some(HourWriter::open(TierConfig::private(dir.path())).unwrap()),
            Some(HourWriter::open(TierConfig::filtered(dir.path())).unwrap()),
            filters,
        )
        .unwrap();
        let day = d("2026-01-01");
        let push = |c: &mut Capture, stream: &str, hour: u8, line: &[u8]| {
            c.push(&RawLine {
                stream,
                day,
                hour,
                line,
            })
            .unwrap()
        };
        assert!(push(&mut c, "fills", 0, b"a\n").is_empty());
        assert!(push(&mut c, "fills", 0, b"drop me").is_empty());
        assert!(push(&mut c, "misc", 0, b"secret").is_empty());
        // A later hour seals hour 0 in both tiers.
        let sealed = push(&mut c, "fills", 1, b"b\r\n");
        assert_eq!(sealed.len(), 2);
        let private = &sealed[0];
        assert_eq!(private.archive, "hyperliquid-raw-private");
        assert_eq!(private.path, "stream=fills/date=2026-01-01/00.jsonl.zst");
        assert_eq!(private.lines, 2);
        let raw = decompress(&std::fs::read(&private.local).unwrap(), &BTreeMap::new()).unwrap();
        assert_eq!(raw, b"a\ndrop me\n");
        let filtered = &sealed[1];
        let raw = decompress(&std::fs::read(&filtered.local).unwrap(), &BTreeMap::new()).unwrap();
        assert_eq!(raw, b"A\n");
        // Spec: one frame, checksum and content size on, no dictionary;
        // byte-identical to compress_filtered.
        let bytes = std::fs::read(&filtered.local).unwrap();
        assert_eq!(bytes, compress_filtered(b"A\n").unwrap());
        assert_eq!(
            zstd::zstd_safe::get_frame_content_size(&bytes).unwrap(),
            Some(2)
        );
        assert_eq!(bytes[4] & 0x04, 0x04, "checksum flag");
        assert!(zstd::zstd_safe::get_dict_id_from_frame(&bytes).is_none());

        // Out-of-order and sealed hours are rejected.
        assert!(
            c.push(&RawLine {
                stream: "fills",
                day,
                hour: 0,
                line: b"late"
            })
            .is_err()
        );
        let rest = c.seal_all().unwrap();
        // fills hour 1 in both tiers, misc hour 0 in private only.
        assert_eq!(rest.len(), 3);
        assert!(
            rest.iter()
                .all(|s| s.archive != "hyperliquid-raw" || !s.path.contains("misc"))
        );
    }

    #[test]
    fn recovers_open_hours_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let day = d("2026-01-01");
        {
            let mut w = HourWriter::open(TierConfig::filtered(dir.path())).unwrap();
            w.push("fills", day, 5, b"one").unwrap();
            w.push("fills", day, 5, b"two").unwrap();
            w.flush().unwrap();
        }
        // Simulate a torn line from a crash.
        let partial = dir
            .path()
            .join("hyperliquid-raw/open/stream=fills/date=2026-01-01/05.jsonl.zst.partial");
        let mut f = OpenOptions::new().append(true).open(&partial).unwrap();
        f.write_all(b"thr").unwrap();
        drop(f);
        let mut w = HourWriter::open(TierConfig::filtered(dir.path())).unwrap();
        assert_eq!(w.position("fills"), Some((day, 5, 2)));
        w.push("fills", day, 5, b"three").unwrap();
        let sealed = w.seal_all().unwrap();
        let raw = decompress(&std::fs::read(&sealed[0].local).unwrap(), &BTreeMap::new()).unwrap();
        assert_eq!(raw, b"one\ntwo\nthree\n");
        assert!(!partial.exists());
        // The sealed hour cannot be reopened.
        assert!(w.push("fills", day, 5, b"four").is_err());
        let mut w2 = HourWriter::open(TierConfig::filtered(dir.path())).unwrap();
        assert!(w2.push("fills", day, 5, b"four").is_err());
    }

    #[test]
    fn private_tier_uses_long_window_and_dictionary() {
        let dir = tempfile::tempdir().unwrap();
        let samples: Vec<Vec<u8>> = (0..2000)
            .map(|i| {
                format!(
                    "{{\"coin\":\"SYN\",\"px\":\"{}.{}\",\"sz\":\"{}\",\"side\":\"B\"}}",
                    100 + i % 7,
                    i % 10,
                    i % 13
                )
                .into_bytes()
            })
            .collect();
        let dict = Dictionary::train(&samples, 4096).unwrap();
        assert_eq!(
            dict.path("fills"),
            format!("_dictionaries/fills/{}.zdict", dict.id)
        );
        let mut cfg = TierConfig::private(dir.path());
        cfg.compression
            .dictionaries
            .insert("fills".into(), Arc::new(dict.clone()));
        let mut w = HourWriter::open(cfg).unwrap();
        let day = d("2026-01-01");
        for s in &samples[..100] {
            w.push("fills", day, 3, s).unwrap();
        }
        let sealed = w.seal_all().unwrap();
        let bytes = std::fs::read(&sealed[0].local).unwrap();
        assert_eq!(
            zstd::zstd_safe::get_dict_id_from_frame(&bytes).map(|n| n.get()),
            Some(dict.id)
        );
        assert!(decompress(&bytes, &BTreeMap::new()).is_err());
        let dicts = BTreeMap::from([(dict.id, dict)]);
        let raw = decompress(&bytes, &dicts).unwrap();
        assert_eq!(raw.iter().filter(|&&b| b == b'\n').count(), 100);
        assert!(Dictionary::from_bytes(b"not a dictionary".to_vec()).is_err());
    }

    #[test]
    fn filtered_tier_requires_filters() {
        let dir = tempfile::tempdir().unwrap();
        let w = HourWriter::open(TierConfig::filtered(dir.path())).unwrap();
        assert!(matches!(
            Capture::new(None, Some(w), FilterSet::new()),
            Err(Error::NotWired(_))
        ));
        assert!(Capture::new(None, None, FilterSet::new()).is_err());
    }

    #[test]
    fn captures_node_directories() {
        let node = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        for (h, body) in [("0", "x1\nx2\n"), ("1", "y1\n"), ("23", "z\n")] {
            let p = node
                .path()
                .join("node_fills_by_block/hourly/20260101")
                .join(h);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
        }
        let files = node_hour_files(&node.path().join("node_fills_by_block")).unwrap();
        assert_eq!(
            files.iter().map(|f| f.1).collect::<Vec<_>>(),
            vec![0, 1, 23]
        );
        let mut c = Capture::new(
            None,
            Some(HourWriter::open(TierConfig::filtered(staging.path())).unwrap()),
            FilterSet::new()
                .with("fills", Arc::new(|l: &[u8]| Some(l.to_vec())))
                .unwrap(),
        )
        .unwrap();
        let sealed = capture_node_dirs(
            &mut c,
            &[("fills".into(), node.path().join("node_fills_by_block"))],
        )
        .unwrap();
        let paths: Vec<&str> = sealed.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "stream=fills/date=2026-01-01/00.jsonl.zst",
                "stream=fills/date=2026-01-01/01.jsonl.zst",
                "stream=fills/date=2026-01-01/23.jsonl.zst"
            ]
        );
    }
}
