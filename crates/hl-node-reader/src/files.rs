//! Hourly node files: listing, one-pass reading and incremental tailing.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::assumptions::{HOURLY_SUBDIR, hourly_rel_path, parse_date_dir, parse_hour_file};
use crate::checkpoint::Position;

/// One hourly file of a stream.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct HourlyFile {
    /// `YYYYMMDD`.
    pub date: u32,
    /// `0`-`23`.
    pub hour: u8,
    /// Full path.
    pub path: PathBuf,
}

impl HourlyFile {
    /// Path relative to the stream directory (`hourly/{date}/{hour}`).
    #[must_use]
    pub fn rel(&self) -> String {
        hourly_rel_path(self.date, self.hour)
    }
}

/// Lists a stream directory's hourly files in time order. A missing
/// directory is an empty list. Names that are not dates or hours are
/// ignored.
///
/// # Errors
///
/// Fails on I/O errors other than a missing directory.
pub fn list_hourly(stream_dir: &Path) -> io::Result<Vec<HourlyFile>> {
    let root = stream_dir.join(HOURLY_SUBDIR);
    let dates = match std::fs::read_dir(&root) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for d in dates {
        let d = d?;
        let Some(date) = d.file_name().to_str().and_then(parse_date_dir) else {
            continue;
        };
        if !d.file_type()?.is_dir() {
            continue;
        }
        for f in std::fs::read_dir(d.path())? {
            let f = f?;
            let Some(hour) = f.file_name().to_str().and_then(parse_hour_file) else {
                continue;
            };
            if f.file_type()?.is_file() {
                out.push(HourlyFile {
                    date,
                    hour,
                    path: f.path(),
                });
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Parses a relative hourly path back to `(date, hour)`.
#[must_use]
pub fn parse_rel(rel: &str) -> Option<(u32, u8)> {
    let mut parts = rel.split('/');
    if parts.next()? != HOURLY_SUBDIR {
        return None;
    }
    let date = parse_date_dir(parts.next()?)?;
    let hour = parse_hour_file(parts.next()?)?;
    parts.next().is_none().then_some((date, hour))
}

/// A line read from a stream, with its start position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Where the line starts.
    pub pos: Position,
    /// The text, without the newline.
    pub text: String,
}

/// Reads every complete line of a stream's hourly files once, in order.
/// A final line without a newline is returned too (a replay reads closed
/// files).
#[derive(Debug)]
pub struct LineReader {
    files: std::vec::IntoIter<HourlyFile>,
    current: Option<(HourlyFile, BufReader<File>, u64)>,
}

impl LineReader {
    /// Opens a stream directory.
    ///
    /// # Errors
    ///
    /// Fails if the directory cannot be listed.
    pub fn open(stream_dir: &Path) -> io::Result<Self> {
        Ok(Self {
            files: list_hourly(stream_dir)?.into_iter(),
            current: None,
        })
    }
}

impl Iterator for LineReader {
    type Item = io::Result<Line>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.current.is_none() {
                let f = self.files.next()?;
                match File::open(&f.path) {
                    Ok(file) => self.current = Some((f, BufReader::new(file), 0)),
                    Err(e) => return Some(Err(e)),
                }
            }
            let (f, reader, offset) = self.current.as_mut()?;
            let mut buf = Vec::new();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => self.current = None,
                Ok(n) => {
                    let pos = Position {
                        file: f.rel(),
                        offset: *offset,
                    };
                    *offset += n as u64;
                    if buf.last() == Some(&b'\n') {
                        buf.pop();
                    }
                    if buf.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    return Some(
                        String::from_utf8(buf)
                            .map(|text| Line { pos, text })
                            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
                    );
                }
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

/// Where a fresh tail (no checkpoint) starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StartPosition {
    /// The oldest hourly file, from its start.
    Earliest,
    /// The newest hourly file, from its start.
    #[default]
    LatestFile,
    /// The end of the newest hourly file (only new blocks).
    End,
}

/// Follows one stream's hourly files: reads appended complete lines and
/// moves to the next hourly file once the current one is finished.
#[derive(Debug)]
pub struct StreamCursor {
    dir: PathBuf,
    current: Option<HourlyFile>,
    offset: u64,
    start: StartPosition,
    resume: Option<Position>,
}

/// What one poll found besides lines.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PollReport {
    /// Bytes of an unterminated last line left behind when rotating to the
    /// next hourly file (the line was cut off).
    pub truncated_bytes: u64,
    /// Number of files rotated past.
    pub rotations: u32,
}

impl StreamCursor {
    /// A cursor on `stream_dir`, resuming at `resume` if given, else at
    /// `start`.
    #[must_use]
    pub fn new(stream_dir: PathBuf, start: StartPosition, resume: Option<Position>) -> Self {
        Self {
            dir: stream_dir,
            current: None,
            offset: 0,
            start,
            resume,
        }
    }

    /// The stream directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Position of the next unread byte, once a file is open.
    #[must_use]
    pub fn position(&self) -> Option<Position> {
        self.current.as_ref().map(|f| Position {
            file: f.rel(),
            offset: self.offset,
        })
    }

    fn open_initial(&mut self, files: &[HourlyFile]) -> io::Result<()> {
        if let Some(r) = self.resume.take() {
            if let Some((date, hour)) = parse_rel(&r.file) {
                let path = self.dir.join(&r.file);
                if let Some(f) = files.iter().find(|f| (f.date, f.hour) == (date, hour)) {
                    self.current = Some(f.clone());
                    self.offset = r.offset;
                    return Ok(());
                }
                // The resume file is gone (pruned): continue at the next
                // newer file, from its start.
                tracing::warn!(file = %path.display(), "checkpoint file missing; resuming at the next file");
                if let Some(f) = files.iter().find(|f| (f.date, f.hour) > (date, hour)) {
                    self.current = Some(f.clone());
                    self.offset = 0;
                }
                return Ok(());
            }
            tracing::warn!(file = %r.file, "unparseable checkpoint position; using start position");
        }
        let pick = match self.start {
            StartPosition::Earliest => files.first(),
            StartPosition::LatestFile | StartPosition::End => files.last(),
        };
        if let Some(f) = pick {
            self.offset = if self.start == StartPosition::End {
                std::fs::metadata(&f.path)?.len()
            } else {
                0
            };
            self.current = Some(f.clone());
        }
        Ok(())
    }

    /// Reads every complete line appended since the last poll, following
    /// hourly rotation. Never returns a partial line.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn poll(&mut self) -> io::Result<(Vec<Line>, PollReport)> {
        let mut lines = Vec::new();
        let mut report = PollReport::default();
        loop {
            if self.current.is_none() {
                let files = list_hourly(&self.dir)?;
                self.open_initial(&files)?;
                if self.current.is_none() {
                    return Ok((lines, report));
                }
            }
            let Some(cur) = self.current.clone() else {
                return Ok((lines, report));
            };
            self.read_complete(&cur, &mut lines)?;
            // Rotate only when a newer file exists; re-read the current one
            // first so lines written just before the switch are not lost.
            let files = list_hourly(&self.dir)?;
            let Some(next) = files
                .iter()
                .find(|f| (f.date, f.hour) > (cur.date, cur.hour))
                .cloned()
            else {
                return Ok((lines, report));
            };
            let leftover = self.read_complete(&cur, &mut lines)?;
            if leftover > 0 {
                report.truncated_bytes += leftover;
            }
            report.rotations += 1;
            self.current = Some(next);
            self.offset = 0;
        }
    }

    /// Reads complete lines from `self.offset`; returns the number of bytes
    /// of a trailing partial line (left unread).
    fn read_complete(&mut self, f: &HourlyFile, out: &mut Vec<Line>) -> io::Result<u64> {
        let mut file = match File::open(&f.path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };
        let len = file.metadata()?.len();
        if len < self.offset {
            // Truncated or replaced: start over at the beginning.
            tracing::warn!(file = %f.path.display(), "file shrank; re-reading from the start");
            self.offset = 0;
        }
        if len == self.offset {
            return Ok(0);
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut buf = Vec::with_capacity(usize::try_from(len - self.offset).unwrap_or(0));
        file.take(len - self.offset).read_to_end(&mut buf)?;
        let mut start = 0usize;
        while let Some(i) = buf[start..].iter().position(|&b| b == b'\n') {
            let end = start + i;
            let text = &buf[start..end];
            let pos = Position {
                file: f.rel(),
                offset: self.offset + start as u64,
            };
            if !text.iter().all(u8::is_ascii_whitespace) {
                let text = String::from_utf8(text.to_vec())
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                out.push(Line { pos, text });
            }
            start = end + 1;
        }
        self.offset += start as u64;
        Ok((buf.len() - start) as u64)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, rel: &str, text: &str, append: bool) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(append)
            .write(true)
            .truncate(!append)
            .open(p)
            .unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn listing_and_reading() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "hourly/20260101/10", "c\n", false);
        write(d.path(), "hourly/20260101/9", "a\n\nb", false);
        write(d.path(), "hourly/20260102/0", "d\n", false);
        write(d.path(), "hourly/notadate/0", "x\n", false);
        write(d.path(), "hourly/20260101/09", "x\n", false);
        let files = list_hourly(d.path()).unwrap();
        let rels: Vec<String> = files.iter().map(HourlyFile::rel).collect();
        assert_eq!(
            rels,
            [
                "hourly/20260101/9",
                "hourly/20260101/10",
                "hourly/20260102/0"
            ]
        );
        let lines: Vec<Line> = LineReader::open(d.path())
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["a", "b", "c", "d"]);
        assert_eq!(lines[1].pos.offset, 3);
        assert_eq!(parse_rel("hourly/20260101/9"), Some((20_260_101, 9)));
        assert_eq!(parse_rel("hourly/20260101/9/x"), None);
        assert!(list_hourly(&d.path().join("missing")).unwrap().is_empty());
    }

    #[test]
    fn cursor_tails_and_rotates() {
        let d = tempfile::tempdir().unwrap();
        let mut c = StreamCursor::new(d.path().to_path_buf(), StartPosition::Earliest, None);
        assert!(c.poll().unwrap().0.is_empty());
        write(d.path(), "hourly/20260101/0", "one\ntw", false);
        let (l, _) = c.poll().unwrap();
        assert_eq!(l.len(), 1);
        assert!(c.poll().unwrap().0.is_empty());
        write(d.path(), "hourly/20260101/0", "o\nthr", true);
        write(d.path(), "hourly/20260101/1", "four\n", false);
        let (l, r) = c.poll().unwrap();
        let texts: Vec<&str> = l.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["two", "four"]);
        assert_eq!(
            r,
            PollReport {
                truncated_bytes: 3,
                rotations: 1
            }
        );
        assert_eq!(c.position().unwrap().file, "hourly/20260101/1");

        // Resume at a saved position.
        let mut c2 = StreamCursor::new(
            d.path().to_path_buf(),
            StartPosition::Earliest,
            Some(Position {
                file: "hourly/20260101/0".into(),
                offset: 4,
            }),
        );
        let texts: Vec<String> = c2.poll().unwrap().0.into_iter().map(|l| l.text).collect();
        assert_eq!(texts, ["two", "four"]);

        // End starts after existing data.
        let mut c3 = StreamCursor::new(d.path().to_path_buf(), StartPosition::End, None);
        assert!(c3.poll().unwrap().0.is_empty());
        write(d.path(), "hourly/20260101/1", "five\n", true);
        assert_eq!(c3.poll().unwrap().0[0].text, "five");
    }
}
