//! User feed records and the generic CSV / JSON-lines connector.
//!
//! Every connector yields [`UserSupplied`]`<`[`UserQuote`]`>`. Prices are
//! `f64`: they are used for local comparison only and never become
//! canonical data.

use std::io::BufRead;

use mesh_types::UnixNanos;

use super::boundary::UserSupplied;

/// One observation from the user's feed: a trade price, a top of book, or
/// both.
#[derive(Debug, Clone, PartialEq)]
pub struct UserQuote {
    /// The symbol as the user's feed names it.
    pub symbol: String,
    /// Timestamp, Unix nanoseconds (the feed's event time).
    pub ts_ns: i64,
    /// Trade or last price.
    pub price: Option<f64>,
    /// Best bid.
    pub bid: Option<f64>,
    /// Best ask.
    pub ask: Option<f64>,
}

impl UserQuote {
    /// A trade.
    pub fn trade(symbol: impl Into<String>, ts_ns: i64, price: f64) -> Self {
        Self {
            symbol: symbol.into(),
            ts_ns,
            price: Some(price),
            bid: None,
            ask: None,
        }
    }

    /// A top of book.
    pub fn quote(symbol: impl Into<String>, ts_ns: i64, bid: f64, ask: f64) -> Self {
        Self {
            symbol: symbol.into(),
            ts_ns,
            price: None,
            bid: Some(bid),
            ask: Some(ask),
        }
    }

    /// Mid of bid and ask, when both are present and not crossed.
    pub fn mid(&self) -> Option<f64> {
        match (self.bid, self.ask) {
            (Some(b), Some(a)) if b <= a => Some((b + a) / 2.0),
            _ => None,
        }
    }

    /// The trade price if present, else the mid.
    pub fn price_any(&self) -> Option<f64> {
        self.price.or_else(|| self.mid())
    }
}

/// A connector failure. Messages never quote the user's records beyond
/// the line number and the column at fault.
#[derive(Debug, thiserror::Error)]
pub enum FeedError {
    /// I/O.
    #[error("feed: {0}")]
    Io(#[from] std::io::Error),
    /// A malformed line or row.
    #[error("feed line {line}: {reason}")]
    Parse {
        /// 1-based line (CSV: record) number.
        line: u64,
        /// Why.
        reason: String,
    },
    /// Bad connector settings.
    #[error("feed settings: {0}")]
    Config(String),
    /// The DBN decoder or the live client failed.
    #[error("feed: {0}")]
    Source(String),
}

/// How timestamps are written in a CSV or JSON-lines feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TsFormat {
    /// Integer Unix nanoseconds.
    #[default]
    Ns,
    /// Integer Unix microseconds.
    Us,
    /// Integer Unix milliseconds.
    Ms,
    /// Unix seconds, fractions allowed.
    S,
    /// `YYYY-MM-DDTHH:MM:SS[.f][Z]`, UTC.
    Rfc3339,
}

impl std::str::FromStr for TsFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "ns" => Self::Ns,
            "us" => Self::Us,
            "ms" => Self::Ms,
            "s" => Self::S,
            "rfc3339" => Self::Rfc3339,
            o => {
                return Err(format!(
                    "unknown timestamp format {o:?} (ns, us, ms, s, rfc3339)"
                ));
            }
        })
    }
}

impl TsFormat {
    /// Parses one timestamp to Unix nanoseconds.
    ///
    /// # Errors
    ///
    /// A description of what is wrong (the value is not quoted).
    pub fn parse(self, s: &str) -> Result<i64, String> {
        let s = s.trim();
        let int = |mul: i64| {
            s.parse::<i64>()
                .ok()
                .and_then(|v| v.checked_mul(mul))
                .ok_or_else(|| "timestamp is not an integer in range".to_string())
        };
        match self {
            Self::Ns => int(1),
            Self::Us => int(1_000),
            Self::Ms => int(1_000_000),
            Self::S => {
                let v: f64 = s
                    .parse()
                    .map_err(|_| "timestamp is not a number".to_string())?;
                let ns = (v * 1e9).round();
                if ns.is_finite() && ns.abs() < 9.2e18 {
                    #[allow(clippy::cast_possible_truncation)]
                    Ok(ns as i64)
                } else {
                    Err("timestamp out of range".into())
                }
            }
            Self::Rfc3339 => UnixNanos::parse_rfc3339(s)
                .map(|t| t.0)
                .map_err(|_| "timestamp is not YYYY-MM-DDTHH:MM:SS[.f][Z]".into()),
        }
    }
}

/// Text formats of the generic connector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextFormat {
    /// Delimited text.
    Csv,
    /// One JSON object per line.
    Jsonl,
}

/// Which columns (CSV: header names, or 0-based indices with
/// `has_header = false`; JSON lines: keys, `a.b` for nested objects) hold
/// what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Columns {
    /// Timestamp column (required).
    pub ts: String,
    /// Symbol column; without it every row has [`Columns::fixed_symbol`].
    pub symbol: Option<String>,
    /// Symbol for every row when there is no symbol column.
    pub fixed_symbol: Option<String>,
    /// Trade or last price column.
    pub price: Option<String>,
    /// Best bid column.
    pub bid: Option<String>,
    /// Best ask column.
    pub ask: Option<String>,
    /// Timestamp format.
    pub ts_format: TsFormat,
    /// CSV delimiter.
    pub delimiter: u8,
    /// CSV has a header row.
    pub has_header: bool,
}

impl Default for Columns {
    fn default() -> Self {
        Self {
            ts: "ts".into(),
            symbol: Some("symbol".into()),
            fixed_symbol: None,
            price: Some("price".into()),
            bid: None,
            ask: None,
            ts_format: TsFormat::Ns,
            delimiter: b',',
            has_header: true,
        }
    }
}

impl Columns {
    /// Checks that the settings can produce prices and symbols.
    ///
    /// # Errors
    ///
    /// [`FeedError::Config`].
    pub fn validate(&self) -> Result<(), FeedError> {
        if self.price.is_none() && (self.bid.is_none() || self.ask.is_none()) {
            return Err(FeedError::Config(
                "name a price column, or both bid and ask columns".into(),
            ));
        }
        if self.symbol.is_none() && self.fixed_symbol.is_none() {
            return Err(FeedError::Config(
                "name a symbol column, or give a fixed symbol".into(),
            ));
        }
        Ok(())
    }
}

fn parse_px(v: &str, col: &str, line: u64) -> Result<Option<f64>, FeedError> {
    let v = v.trim();
    if v.is_empty() {
        return Ok(None);
    }
    let x: f64 = v.parse().map_err(|_| FeedError::Parse {
        line,
        reason: format!("column {col:?} is not a number"),
    })?;
    if x.is_finite() {
        Ok(Some(x))
    } else {
        Err(FeedError::Parse {
            line,
            reason: format!("column {col:?} is not finite"),
        })
    }
}

/// Builds a quote from looked-up cells; `get(col)` returns the cell text.
fn row_to_quote<'a>(
    cols: &Columns,
    line: u64,
    get: impl Fn(&str) -> Option<std::borrow::Cow<'a, str>>,
) -> Result<Option<UserQuote>, FeedError> {
    let missing = |c: &str| FeedError::Parse {
        line,
        reason: format!("missing column {c:?}"),
    };
    let ts_s = get(&cols.ts).ok_or_else(|| missing(&cols.ts))?;
    let ts_ns = cols
        .ts_format
        .parse(&ts_s)
        .map_err(|reason| FeedError::Parse { line, reason })?;
    let symbol = match (&cols.symbol, &cols.fixed_symbol) {
        (Some(c), _) => get(c).ok_or_else(|| missing(c))?.trim().to_string(),
        (None, Some(s)) => s.clone(),
        (None, None) => return Err(FeedError::Config("no symbol column".into())),
    };
    let px = |c: &Option<String>| -> Result<Option<f64>, FeedError> {
        match c {
            None => Ok(None),
            Some(c) => match get(c) {
                None => Err(missing(c)),
                Some(v) => parse_px(&v, c, line),
            },
        }
    };
    let q = UserQuote {
        symbol,
        ts_ns,
        price: px(&cols.price)?,
        bid: px(&cols.bid)?,
        ask: px(&cols.ask)?,
    };
    Ok((q.price.is_some() || q.bid.is_some() || q.ask.is_some()).then_some(q))
}

/// Reads CSV or JSON lines from any reader (a file the user chose, or
/// standard input) and yields user-supplied quotes. Rows with no price at
/// all are skipped.
pub struct TextFeed {
    inner: TextInner,
    cols: Columns,
}

enum TextInner {
    Csv {
        reader: csv::Reader<Box<dyn std::io::Read + Send>>,
        index: Option<Vec<(String, usize)>>,
        record: csv::StringRecord,
    },
    Jsonl {
        reader: Box<dyn BufRead + Send>,
        line: u64,
        buf: String,
    },
}

impl std::fmt::Debug for TextFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextFeed")
            .field("cols", &self.cols)
            .finish()
    }
}

impl TextFeed {
    /// Opens a connector over `reader`.
    ///
    /// # Errors
    ///
    /// Bad column settings, or a CSV header without a named column.
    pub fn new(
        format: TextFormat,
        reader: Box<dyn BufRead + Send>,
        cols: Columns,
    ) -> Result<Self, FeedError> {
        cols.validate()?;
        let inner = match format {
            TextFormat::Jsonl => TextInner::Jsonl {
                reader,
                line: 0,
                buf: String::new(),
            },
            TextFormat::Csv => {
                let mut reader = csv::ReaderBuilder::new()
                    .delimiter(cols.delimiter)
                    .has_headers(cols.has_header)
                    .flexible(true)
                    .trim(csv::Trim::All)
                    .from_reader(Box::new(reader) as Box<dyn std::io::Read + Send>);
                let index = if cols.has_header {
                    let headers = reader
                        .headers()
                        .map_err(|e| FeedError::Source(format!("CSV header: {e}")))?
                        .clone();
                    let mut idx = Vec::new();
                    for c in wanted(&cols) {
                        let i = headers.iter().position(|h| h == c).ok_or_else(|| {
                            FeedError::Config(format!("CSV header has no column {c:?}"))
                        })?;
                        idx.push((c.to_string(), i));
                    }
                    Some(idx)
                } else {
                    for c in wanted(&cols) {
                        c.parse::<usize>().map_err(|_| {
                            FeedError::Config(format!(
                                "without a header, columns are 0-based indices; got {c:?}"
                            ))
                        })?;
                    }
                    None
                };
                TextInner::Csv {
                    reader,
                    index,
                    record: csv::StringRecord::new(),
                }
            }
        };
        Ok(Self { inner, cols })
    }

    /// The next quote, `None` at the end of input.
    ///
    /// # Errors
    ///
    /// I/O or a malformed row.
    pub fn next_quote(&mut self) -> Result<Option<UserSupplied<UserQuote>>, FeedError> {
        loop {
            let q = match &mut self.inner {
                TextInner::Csv {
                    reader,
                    index,
                    record,
                } => {
                    let more = reader
                        .read_record(record)
                        .map_err(|e| FeedError::Source(format!("CSV: {e}")))?;
                    if !more {
                        return Ok(None);
                    }
                    let line = record.position().map_or(0, csv::Position::line);
                    let index = &*index;
                    let lookup = |c: &str| -> Option<usize> {
                        match index {
                            Some(ix) => ix.iter().find(|(n, _)| n == c).map(|(_, i)| *i),
                            None => c.parse().ok(),
                        }
                    };
                    let rec = &*record;
                    row_to_quote(&self.cols, line, |c| {
                        lookup(c)
                            .and_then(|i| rec.get(i))
                            .map(|s| std::borrow::Cow::Owned(s.to_string()))
                    })?
                }
                TextInner::Jsonl { reader, line, buf } => {
                    buf.clear();
                    if reader.read_line(buf)? == 0 {
                        return Ok(None);
                    }
                    *line += 1;
                    let text = buf.trim();
                    if text.is_empty() {
                        continue;
                    }
                    let v: serde_json::Value =
                        serde_json::from_str(text).map_err(|_| FeedError::Parse {
                            line: *line,
                            reason: "not a JSON object".into(),
                        })?;
                    row_to_quote(&self.cols, *line, |c| json_cell(&v, c))?
                }
            };
            if let Some(q) = q {
                return Ok(Some(UserSupplied::new(q)));
            }
        }
    }
}

impl Iterator for TextFeed {
    type Item = Result<UserSupplied<UserQuote>, FeedError>;
    fn next(&mut self) -> Option<Self::Item> {
        self.next_quote().transpose()
    }
}

fn wanted(cols: &Columns) -> Vec<&str> {
    [
        Some(&cols.ts),
        cols.symbol.as_ref(),
        cols.price.as_ref(),
        cols.bid.as_ref(),
        cols.ask.as_ref(),
    ]
    .into_iter()
    .flatten()
    .map(String::as_str)
    .collect()
}

fn json_cell<'a>(v: &serde_json::Value, path: &str) -> Option<std::borrow::Cow<'a, str>> {
    let mut cur = v;
    for part in path.split('.') {
        cur = cur.get(part)?;
    }
    match cur {
        serde_json::Value::String(s) => Some(std::borrow::Cow::Owned(s.clone())),
        serde_json::Value::Number(n) => Some(std::borrow::Cow::Owned(n.to_string())),
        serde_json::Value::Null => Some(std::borrow::Cow::Borrowed("")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(format: TextFormat, text: &'static str, cols: Columns) -> Vec<UserQuote> {
        TextFeed::new(format, Box::new(text.as_bytes()), cols)
            .unwrap()
            .map(|q| q.unwrap().get().clone())
            .collect()
    }

    #[test]
    fn timestamps_parse_in_every_format() {
        assert_eq!(
            TsFormat::Ns.parse("1767225600000000001"),
            Ok(1_767_225_600_000_000_001)
        );
        assert_eq!(
            TsFormat::Us.parse("1767225600000000"),
            Ok(1_767_225_600_000_000_000)
        );
        assert_eq!(
            TsFormat::Ms.parse(" 1767225600000 "),
            Ok(1_767_225_600_000_000_000)
        );
        assert_eq!(
            TsFormat::S.parse("1767225600.5"),
            Ok(1_767_225_600_500_000_000)
        );
        assert_eq!(
            TsFormat::Rfc3339.parse("2026-01-01T00:00:00.25Z"),
            Ok(1_767_225_600_250_000_000)
        );
        assert!(TsFormat::Ms.parse("x").is_err());
        assert!(TsFormat::Ns.parse("99999999999999999999").is_err());
    }

    #[test]
    fn csv_with_headers_and_quotes() {
        let cols = Columns {
            ts: "time".into(),
            symbol: Some("sym".into()),
            price: Some("last".into()),
            bid: Some("bid".into()),
            ask: Some("ask".into()),
            ts_format: TsFormat::Rfc3339,
            ..Columns::default()
        };
        let q = feed(
            TextFormat::Csv,
            "time,sym,last,bid,ask,extra\n\
             2026-01-01T00:00:00Z,IDX-DEC,100.5,,,x\n\
             2026-01-01T00:00:01Z,\"IDX-DEC\",,100.25,100.75,y\n\
             2026-01-01T00:00:02Z,IDX-DEC,,,,z\n",
            cols,
        );
        assert_eq!(q.len(), 2, "rows with no price are skipped");
        assert_eq!(q[0].price, Some(100.5));
        assert_eq!(q[1].price_any(), Some(100.5));
        assert_eq!(q[1].ts_ns - q[0].ts_ns, 1_000_000_000);
    }

    #[test]
    fn csv_without_headers_uses_indices_and_a_fixed_symbol() {
        let cols = Columns {
            ts: "0".into(),
            symbol: None,
            fixed_symbol: Some("MINE".into()),
            price: Some("2".into()),
            ts_format: TsFormat::Ms,
            delimiter: b';',
            has_header: false,
            ..Columns::default()
        };
        let q = feed(TextFormat::Csv, "1000;ignored;5.5\n2000;x;6\n", cols);
        assert_eq!(q.len(), 2);
        assert_eq!(
            (q[1].symbol.as_str(), q[1].ts_ns, q[1].price),
            ("MINE", 2_000_000_000, Some(6.0))
        );
    }

    #[test]
    fn csv_settings_are_checked_up_front() {
        let bad = |cols: Columns, text: &'static str| {
            TextFeed::new(TextFormat::Csv, Box::new(text.as_bytes()), cols).unwrap_err()
        };
        let e = bad(Columns::default(), "ts,symbol,px\n1,a,2\n");
        assert!(e.to_string().contains("\"price\""), "{e}");
        let e = bad(
            Columns {
                price: None,
                bid: Some("bid".into()),
                ..Columns::default()
            },
            "",
        );
        assert!(e.to_string().contains("bid and ask"), "{e}");
        let e = bad(
            Columns {
                has_header: false,
                ..Columns::default()
            },
            "",
        );
        assert!(e.to_string().contains("0-based"), "{e}");
    }

    #[test]
    fn malformed_rows_name_the_line_but_not_the_value() {
        let mut f = TextFeed::new(
            TextFormat::Csv,
            Box::new("ts,symbol,price\n1,a,2\n2,a,SECRETVALUE\n".as_bytes()),
            Columns::default(),
        )
        .unwrap();
        assert!(f.next_quote().unwrap().is_some());
        let e = f.next_quote().unwrap_err().to_string();
        assert!(e.contains("line 3") && !e.contains("SECRETVALUE"), "{e}");
    }

    #[test]
    fn jsonl_with_nested_keys_and_numbers() {
        let cols = Columns {
            ts: "t".into(),
            symbol: Some("instrument.symbol".into()),
            price: None,
            bid: Some("top.bid".into()),
            ask: Some("top.ask".into()),
            ts_format: TsFormat::S,
            ..Columns::default()
        };
        let q = feed(
            TextFormat::Jsonl,
            "{\"t\":1767225600,\"instrument\":{\"symbol\":\"X\"},\"top\":{\"bid\":99,\"ask\":\"101\"}}\n\n\
             {\"t\":1767225601.5,\"instrument\":{\"symbol\":\"X\"},\"top\":{\"bid\":null,\"ask\":null}}\n",
            cols,
        );
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].mid(), Some(100.0));
        let e = TextFeed::new(
            TextFormat::Jsonl,
            Box::new("[1]\nnope\n".as_bytes()),
            Columns::default(),
        )
        .unwrap()
        .nth(1)
        .unwrap()
        .unwrap_err();
        assert!(e.to_string().contains("line 2"), "{e}");
    }

    #[test]
    fn crossed_books_have_no_mid() {
        assert_eq!(UserQuote::quote("a", 0, 2.0, 1.0).mid(), None);
        assert_eq!(UserQuote::quote("a", 0, 1.0, 1.0).mid(), Some(1.0));
    }
}
