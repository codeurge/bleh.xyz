//! DTCC public dissemination files (slice and cumulative reports) to
//! [`SwapPrint`] records.
//!
//! The mapping is specified in `docs/specs/swap-repositories.md` § 4. In
//! short:
//!
//! - `NEWT` with event type `TRAD` (or none) is a `new` print; `CORR` is a
//!   `correct` and `EROR` a `cancel`, both linking the original
//!   dissemination ID. Other action types (`MODI`, `TERM`, `REVI`, ...) and
//!   `NEWT` for non-trade events (clearing, novation, allocation,
//!   compression, exercise) are skipped and counted: observation v0 has no
//!   action for them. The raw file keeps them.
//! - Prices keep the repository's notation: monetary amounts are `price`,
//!   decimal rates become `rate` in percent, decimal and basis-point spreads
//!   become `spread` in basis points. All conversions are exact.
//! - A trailing `+` on the notional (capped by the repository) sets the
//!   `size_capped` flag; a block or large-notional off-facility election
//!   sets `block_trade_delayed`.
//! - Unknown columns are logged and counted, never fatal. A bad row is
//!   counted and skipped; a bad optional cell drops that value only.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek};
use std::path::Path;

use mesh_types::{
    AssetClass, Flag, Flags, InstrumentId, Notional, Provenance, Seq, SwapAction, SwapPrice,
    SwapPrint, UnixMillis, UnixNanos,
};
use sha2::{Digest, Sha256};

use crate::fields::{Column, Field, classify};
use crate::repository::{DtccAsset, DtccBucket, Regime, Repository};
use crate::values;

/// Which kind of report a file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportKind {
    /// Intraday slice report.
    Slice,
    /// End-of-day cumulative report.
    Cumulative,
}

/// Everything about a file that is not in its rows.
#[derive(Debug, Clone)]
pub struct FileContext {
    /// Bucket (regime and asset class).
    pub bucket: DtccBucket,
    /// Disseminating repository.
    pub repository: Repository,
    /// Report kind.
    pub kind: ReportKind,
    /// Source batch number, used as `seq.block_number` (the DTCC slice ID).
    pub batch: u64,
    /// When the file was fetched, from the raw sidecar. Without it,
    /// `ts_ingest_ns` falls back to the print's dissemination time so the
    /// output stays a pure function of the file.
    pub fetched_at: Option<UnixNanos>,
    /// Dissemination time of the whole file from the listing, used only when
    /// rows carry no dissemination timestamp.
    pub listing_dissemination: Option<UnixMillis>,
    /// Set the `backfill` flag on every record (cumulative reports read for
    /// days the live poller did not cover).
    pub backfill: bool,
}

/// Why a row produced no record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SkipReason {
    /// An action type observation v0 cannot represent (`MODI`, `TERM`,
    /// `REVI`, ...).
    LifecycleAction,
    /// `NEWT` for a non-trade event (clearing, novation, ...).
    NonTradeEvent,
}

impl SkipReason {
    /// Metric label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            SkipReason::LifecycleAction => "lifecycle_action",
            SkipReason::NonTradeEvent => "non_trade_event",
        }
    }
}

/// Counters from parsing one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParseStats {
    /// Data rows read.
    pub rows: u64,
    /// Records produced.
    pub records: u64,
    /// Rows skipped, by reason.
    pub skipped: BTreeMap<&'static str, u64>,
    /// Rows rejected as malformed.
    pub row_errors: u64,
    /// Optional cells dropped as malformed, by field.
    pub field_errors: BTreeMap<&'static str, u64>,
    /// Columns not in the documented layout (normalized names).
    pub unknown_columns: Vec<String>,
    /// Records whose dissemination time came from somewhere other than the
    /// row's dissemination timestamp, by source.
    pub dissemination_fallbacks: BTreeMap<&'static str, u64>,
    /// Latest dissemination time seen.
    pub max_dissemination: Option<UnixMillis>,
}

impl ParseStats {
    fn merge(&mut self, o: ParseStats) {
        self.rows += o.rows;
        self.records += o.records;
        self.row_errors += o.row_errors;
        for (k, v) in o.skipped {
            *self.skipped.entry(k).or_default() += v;
        }
        for (k, v) in o.field_errors {
            *self.field_errors.entry(k).or_default() += v;
        }
        for (k, v) in o.dissemination_fallbacks {
            *self.dissemination_fallbacks.entry(k).or_default() += v;
        }
        for c in o.unknown_columns {
            if !self.unknown_columns.contains(&c) {
                self.unknown_columns.push(c);
            }
        }
        self.max_dissemination = self.max_dissemination.max(o.max_dissemination);
    }
}

/// File-level parse failures.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// Reading the file failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The CSV header could not be read.
    #[error("csv: {0}")]
    Csv(#[from] csv::Error),
    /// The zip archive is malformed.
    #[error("zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// A column every row needs is missing.
    #[error("missing required column {0:?}")]
    MissingColumn(&'static str),
    /// The consumer stopped accepting records.
    #[error("record sink closed")]
    SinkClosed,
}

/// Returned by a sink that no longer accepts records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkClosed;

/// Receives each record as it is parsed.
pub type Sink<'a> = dyn FnMut(SwapPrint) -> Result<(), SinkClosed> + 'a;

/// Per-file state shared across the CSV entries of one archive.
#[derive(Debug, Default)]
struct FileState {
    next_index: HashMap<InstrumentId, u32>,
}

/// Parses a report file: a DTCC zip archive (every `.csv` entry, in name
/// order) or a bare CSV file.
///
/// # Errors
///
/// Fails on unreadable files, malformed archives, missing required
/// columns, or a closed sink. Bad rows are counted, not fatal.
pub fn parse_report_file(
    path: &Path,
    ctx: &FileContext,
    sink: &mut Sink<'_>,
) -> Result<ParseStats, ParseError> {
    let mut file = std::fs::File::open(path)?;
    let mut magic = [0u8; 4];
    let n = file.read(&mut magic)?;
    file.rewind()?;
    if n == 4 && magic == *b"PK\x03\x04" {
        parse_zip(file, ctx, sink)
    } else {
        let mut state = FileState::default();
        parse_csv(std::io::BufReader::new(file), ctx, &mut state, sink)
    }
}

/// Parses every `.csv` entry of a zip archive, in name order.
///
/// # Errors
///
/// As [`parse_report_file`].
pub fn parse_zip<R: Read + Seek>(
    reader: R,
    ctx: &FileContext,
    sink: &mut Sink<'_>,
) -> Result<ParseStats, ParseError> {
    let mut archive = zip::ZipArchive::new(reader)?;
    let mut names: Vec<(String, usize)> = Vec::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        let name = entry.name()?.into_owned();
        if entry.is_file() && name.to_ascii_lowercase().ends_with(".csv") {
            names.push((name, i));
        }
    }
    names.sort();
    let mut state = FileState::default();
    let mut stats = ParseStats::default();
    for (_, i) in names {
        let entry = archive.by_index(i)?;
        stats.merge(parse_csv(entry, ctx, &mut state, sink)?);
    }
    Ok(stats)
}

/// Parses CSV bytes from any reader (one file's rows).
///
/// # Errors
///
/// As [`parse_report_file`].
pub fn parse_csv_reader<R: Read>(
    reader: R,
    ctx: &FileContext,
    sink: &mut Sink<'_>,
) -> Result<ParseStats, ParseError> {
    let mut state = FileState::default();
    parse_csv(reader, ctx, &mut state, sink)
}

fn parse_csv<R: Read>(
    reader: R,
    ctx: &FileContext,
    state: &mut FileState,
    sink: &mut Sink<'_>,
) -> Result<ParseStats, ParseError> {
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .has_headers(true)
        .trim(csv::Trim::None)
        .from_reader(reader);
    let headers = rdr.byte_headers()?.clone();
    let mut index: HashMap<Field, usize> = HashMap::new();
    let mut stats = ParseStats::default();
    for (i, h) in headers.iter().enumerate() {
        let h = String::from_utf8_lossy(h);
        match classify(&h) {
            Column::Used(f) => {
                index.entry(f).or_insert(i);
            }
            Column::KnownUnused => {}
            Column::Unknown(n) if n.is_empty() => {}
            Column::Unknown(n) => {
                tracing::warn!(column = %h, bucket = %ctx.bucket, "unknown column in swap report; ignoring");
                if !stats.unknown_columns.contains(&n) {
                    stats.unknown_columns.push(n);
                }
            }
        }
    }
    for required in [Field::DisseminationId, Field::ExecutionTimestamp] {
        if !index.contains_key(&required) {
            return Err(ParseError::MissingColumn(required.csv_name()));
        }
    }
    let mut record = csv::ByteRecord::new();
    loop {
        match rdr.read_byte_record(&mut record) {
            Ok(false) => break,
            Ok(true) => {}
            Err(e) => {
                // A malformed line (for example, invalid quoting). Count and
                // carry on with the next line.
                stats.rows += 1;
                stats.row_errors += 1;
                tracing::warn!(error = %e, bucket = %ctx.bucket, "malformed CSV line");
                if e.is_io_error() {
                    return Err(ParseError::Csv(e));
                }
                continue;
            }
        }
        if record.iter().all(|c| c.iter().all(u8::is_ascii_whitespace)) {
            continue;
        }
        stats.rows += 1;
        let row = Row {
            record: &record,
            index: &index,
        };
        match map_row(&row, ctx, state, &mut stats) {
            Ok(Outcome::Print(p)) => {
                stats.records += 1;
                stats.max_dissemination = stats.max_dissemination.max(Some(p.ts_dissemination_ms));
                sink(*p).map_err(|SinkClosed| ParseError::SinkClosed)?;
            }
            Ok(Outcome::Skip(reason)) => {
                *stats.skipped.entry(reason.label()).or_default() += 1;
            }
            Err(e) => {
                stats.row_errors += 1;
                tracing::debug!(error = %e, bucket = %ctx.bucket, "rejected swap report row");
            }
        }
    }
    Ok(stats)
}

struct Row<'a> {
    record: &'a csv::ByteRecord,
    index: &'a HashMap<Field, usize>,
}

impl Row<'_> {
    fn get(&self, f: Field) -> String {
        self.index
            .get(&f)
            .and_then(|&i| self.record.get(i))
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default()
    }
}

enum Outcome {
    Print(Box<SwapPrint>),
    Skip(SkipReason),
}

#[derive(Debug, thiserror::Error)]
enum RowError {
    #[error("missing {0}")]
    Missing(&'static str),
    #[error("bad {0}: {1}")]
    Bad(&'static str, String),
    #[error("invalid record: {0}")]
    Invalid(String),
}

/// Text cleaned for a `SwapPrint` field: control characters removed,
/// trimmed, at most 128 bytes (cut on a character boundary).
fn clean_text(s: &str) -> Option<String> {
    let t: String = s.chars().filter(|c| !c.is_control()).collect();
    let t = t.trim();
    if t.is_empty() {
        return None;
    }
    let mut end = t.len().min(128);
    while !t.is_char_boundary(end) {
        end -= 1;
    }
    Some(t[..end].trim_end().to_string())
}

fn field_error(stats: &mut ParseStats, f: Field) {
    *stats.field_errors.entry(f.csv_name()).or_default() += 1;
}

fn map_action(s: &str) -> Option<Result<SwapAction, SkipReason>> {
    match s.trim().to_ascii_uppercase().as_str() {
        "NEWT" | "NEW" => Some(Ok(SwapAction::New)),
        "CORR" | "CORRECT" | "CORRECTION" => Some(Ok(SwapAction::Correct)),
        "EROR" | "CANCEL" | "CANCELLATION" => Some(Ok(SwapAction::Cancel)),
        "" => None,
        _ => Some(Err(SkipReason::LifecycleAction)),
    }
}

fn is_trade_event(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_uppercase().as_str(),
        "" | "TRAD" | "TRADE"
    )
}

/// Which price candidate to read and how.
#[derive(Debug, Clone, Copy)]
enum PriceSource {
    /// `Price` with `Price notation`.
    Price,
    /// `Spread-Leg n` with its notation.
    Spread(u8),
    /// `Fixed rate-Leg n` (always decimal).
    FixedRate(u8),
    /// `Exchange rate` (a price).
    ExchangeRate,
    /// `Strike Price` with its notation (CFTC options only).
    Strike,
}

fn decimal_to(p: mesh_types::Price, notation: Option<&str>, spread: bool) -> Option<SwapPrice> {
    match (notation, spread) {
        (Some("1"), _) => Some(SwapPrice::Price { value: p }),
        (Some("2" | "3"), false) => values::shift(p, 2)
            .ok()
            .map(|v| SwapPrice::Rate { value: v }),
        (Some("2" | "3"), true) => values::shift(p, 4)
            .ok()
            .map(|v| SwapPrice::Spread { value: v }),
        (Some("4"), _) => Some(SwapPrice::Spread { value: p }),
        (None, false) => Some(SwapPrice::Price { value: p }),
        _ => None,
    }
}

fn read_price(row: &Row<'_>, src: PriceSource, stats: &mut ParseStats) -> Option<SwapPrice> {
    let (value_field, notation_field, spread) = match src {
        PriceSource::Price => (Field::Price, Some(Field::PriceNotation), false),
        PriceSource::Spread(1) => (Field::SpreadLeg1, Some(Field::SpreadNotationLeg1), true),
        PriceSource::Spread(_) => (Field::SpreadLeg2, Some(Field::SpreadNotationLeg2), true),
        PriceSource::FixedRate(1) => (Field::FixedRateLeg1, None, false),
        PriceSource::FixedRate(_) => (Field::FixedRateLeg2, None, false),
        PriceSource::ExchangeRate => (Field::ExchangeRate, None, false),
        PriceSource::Strike => (Field::StrikePrice, Some(Field::StrikePriceNotation), false),
    };
    let num = match values::number(&row.get(value_field)) {
        Ok(Some(n)) => n,
        Ok(None) => return None,
        Err(_) => {
            field_error(stats, value_field);
            return None;
        }
    };
    let Ok(p) = values::to_price(&num.text) else {
        field_error(stats, value_field);
        return None;
    };
    let notation = notation_field.and_then(|f| values::text(&row.get(f)));
    let out = match src {
        PriceSource::FixedRate(_) => values::shift(p, 2)
            .ok()
            .map(|v| SwapPrice::Rate { value: v }),
        PriceSource::ExchangeRate => Some(SwapPrice::Price { value: p }),
        _ => decimal_to(p, notation.as_deref(), spread),
    };
    if out.is_none() {
        field_error(stats, notation_field.unwrap_or(value_field));
    }
    out
}

fn price_order(asset: AssetClass, regime: Regime, is_option: bool) -> Vec<PriceSource> {
    use PriceSource as S;
    // Strike prices are admitted for CFTC Part 43 option terms (ADM-0002)
    // only; ADM-0004 admits price, spread and notional.
    let strike = regime == Regime::CftcPart43 && is_option;
    let mut order = Vec::new();
    if strike {
        order.push(S::Strike);
    }
    match asset {
        AssetClass::InterestRate => {
            order.extend([
                S::FixedRate(1),
                S::FixedRate(2),
                S::Spread(1),
                S::Spread(2),
                S::Price,
            ]);
        }
        AssetClass::Credit => {
            order.extend([
                S::Spread(1),
                S::Spread(2),
                S::Price,
                S::FixedRate(1),
                S::FixedRate(2),
            ]);
        }
        AssetClass::Equity => {
            order.extend([
                S::Price,
                S::Spread(1),
                S::Spread(2),
                S::FixedRate(1),
                S::FixedRate(2),
            ]);
        }
        AssetClass::ForeignExchange => {
            order.extend([S::ExchangeRate, S::Price, S::Spread(1), S::Spread(2)]);
        }
        AssetClass::Commodity => {
            order.extend([
                S::Price,
                S::Spread(1),
                S::Spread(2),
                S::FixedRate(1),
                S::FixedRate(2),
            ]);
        }
    }
    order
}

fn read_notional(row: &Row<'_>, stats: &mut ParseStats) -> (Option<Notional>, bool) {
    let mut capped = false;
    let mut notional = None;
    for (amount_f, ccy_f) in [
        (Field::NotionalAmountLeg1, Field::NotionalCurrencyLeg1),
        (Field::NotionalAmountLeg2, Field::NotionalCurrencyLeg2),
    ] {
        let amount = match values::number(&row.get(amount_f)) {
            Ok(Some(n)) => n,
            Ok(None) => continue,
            Err(_) => {
                field_error(stats, amount_f);
                continue;
            }
        };
        let Ok(size) = values::to_size(&amount.text) else {
            field_error(stats, amount_f);
            continue;
        };
        let ccy = values::text(&row.get(ccy_f)).map(|c| c.to_ascii_uppercase());
        match ccy {
            Some(c) if c.len() == 3 && c.bytes().all(|b| b.is_ascii_uppercase()) => {
                capped = amount.capped;
                notional = Some(Notional {
                    amount: size,
                    currency: c,
                });
                break;
            }
            _ => {
                field_error(stats, ccy_f);
            }
        }
    }
    // Commodity quantities carry the cap marker too, with no currency.
    if let Ok(Some(q)) = values::number(&row.get(Field::TotalNotionalQuantityLeg1)) {
        capped |= q.capped;
    }
    (notional, capped)
}

/// Tenor from unadjusted effective and expiration dates, only when they are
/// an exact whole number of months apart on the same day of the month
/// (`5Y`, `18M`). Nothing is rounded or guessed.
fn exact_tenor(effective: &str, expiry: &str) -> Option<String> {
    let parse = |s: &str| -> Option<(i64, i64, i64)> {
        let b = s.as_bytes();
        (b.len() == 10).then_some(())?;
        Some((
            s[..4].parse().ok()?,
            s[5..7].parse().ok()?,
            s[8..].parse().ok()?,
        ))
    };
    let (y1, m1, d1) = parse(effective)?;
    let (y2, m2, d2) = parse(expiry)?;
    if d1 != d2 {
        return None;
    }
    let months = (y2 - y1) * 12 + (m2 - m1);
    if months <= 0 {
        None
    } else if months % 12 == 0 {
        Some(format!("{}Y", months / 12))
    } else {
        Some(format!("{months}M"))
    }
}

const SYMBOL_OK: fn(char) -> bool =
    |c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '@' | '#' | '+' | '-');

fn sanitize_segment(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let c = if SYMBOL_OK(c) { c } else { '-' };
        if !(c == '-' && out.ends_with('-')) {
            out.push(c);
        }
    }
    out.trim_matches('-').to_string()
}

/// Mesh instrument for a print: `<venue>:swap:<symbol>`, where the symbol is
/// the UPI, or else the sanitized product name and underlier.
fn instrument_for(
    regime: Regime,
    asset: DtccAsset,
    upi: Option<&str>,
    product: Option<&str>,
    underlier: Option<&str>,
) -> Option<InstrumentId> {
    let mut segments: Vec<String> = Vec::new();
    if let Some(u) = upi.map(sanitize_segment).filter(|s| !s.is_empty()) {
        segments.push(u);
    } else {
        for part in [product, underlier].into_iter().flatten() {
            segments.extend(
                part.split(':')
                    .map(sanitize_segment)
                    .filter(|s| !s.is_empty()),
            );
        }
        if segments.is_empty() {
            segments.push(format!("UNKNOWN-{}", asset.code()));
        }
    }
    let symbol = segments.join(":");
    let prefix = format!("{}:swap:", regime.venue_str());
    let full = format!("{prefix}{symbol}");
    if full.len() <= 128 {
        return InstrumentId::new(full).ok();
    }
    // Too long: keep a prefix and add a digest so distinct products stay
    // distinct.
    let digest = Sha256::digest(symbol.as_bytes());
    let tag: String = digest[..4].iter().map(|b| format!("{b:02x}")).collect();
    let room = 128 - prefix.len() - 1 - tag.len();
    let mut cut = symbol[..room.min(symbol.len())]
        .trim_end_matches([':', '-'])
        .to_string();
    if cut.is_empty() {
        cut.push('X');
    }
    InstrumentId::new(format!("{prefix}{cut}#{tag}")).ok()
}

fn map_row(
    row: &Row<'_>,
    ctx: &FileContext,
    state: &mut FileState,
    stats: &mut ParseStats,
) -> Result<Outcome, RowError> {
    let regime = ctx.bucket.regime;

    let dissemination_id = clean_text(&row.get(Field::DisseminationId))
        .ok_or(RowError::Missing("dissemination identifier"))?;
    let action = match map_action(&row.get(Field::ActionType)) {
        None => return Err(RowError::Missing("action type")),
        Some(Err(skip)) => return Ok(Outcome::Skip(skip)),
        Some(Ok(a)) => a,
    };
    if action == SwapAction::New && !is_trade_event(&row.get(Field::EventType)) {
        return Ok(Outcome::Skip(SkipReason::NonTradeEvent));
    }
    let original_dissemination_id = clean_text(&row.get(Field::OriginalDisseminationId));
    if action != SwapAction::New && original_dissemination_id.is_none() {
        return Err(RowError::Missing("original dissemination identifier"));
    }

    let asset = values::text(&row.get(Field::AssetClass))
        .and_then(|c| DtccAsset::parse(&c))
        .unwrap_or(ctx.bucket.asset);
    let asset_class = asset.asset_class();

    let ts_execution = values::timestamp(&row.get(Field::ExecutionTimestamp))
        .map_err(|e| RowError::Bad("execution timestamp", e.to_string()))?
        .ok_or(RowError::Missing("execution timestamp"))?;

    let row_dissemination = match values::timestamp(&row.get(Field::DisseminationTimestamp)) {
        Ok(v) => v,
        Err(_) => {
            field_error(stats, Field::DisseminationTimestamp);
            None
        }
    };
    let event_ts = values::timestamp(&row.get(Field::EventTimestamp))
        .ok()
        .flatten();
    let (ts_dissemination, fallback) =
        match (row_dissemination, ctx.listing_dissemination, event_ts) {
            (Some(t), _, _) => (t, None),
            (None, Some(t), _) => (t, Some("listing")),
            (None, None, Some(t)) => (t, Some("event_timestamp")),
            (None, None, None) => (ts_execution, Some("execution_timestamp")),
        };
    if let Some(src) = fallback {
        *stats.dissemination_fallbacks.entry(src).or_default() += 1;
    }

    let upi = clean_text(&row.get(Field::Upi));
    let product =
        clean_text(&row.get(Field::UpiFisn)).or_else(|| clean_text(&row.get(Field::ProductName)));
    let underlier = clean_text(&row.get(Field::UpiUnderlierName))
        .or_else(|| clean_text(&row.get(Field::UnderlierIdLeg1)))
        .or_else(|| clean_text(&row.get(Field::UnderlyingAssetName)));

    let expiry = match values::date(&row.get(Field::ExpirationDate)) {
        Ok(v) => v,
        Err(_) => {
            field_error(stats, Field::ExpirationDate);
            None
        }
    };
    let effective = values::date(&row.get(Field::EffectiveDate)).ok().flatten();
    let tenor = match (&effective, &expiry) {
        (Some(e), Some(x)) => exact_tenor(e, x),
        _ => None,
    };

    let is_option = values::text(&row.get(Field::OptionType)).is_some()
        || values::text(&row.get(Field::StrikePrice)).is_some();
    let price = price_order(asset_class, regime, is_option)
        .into_iter()
        .find_map(|src| read_price(row, src, stats));

    let (notional, capped) = read_notional(row, stats);

    let mut flags = Flags::NONE;
    if capped {
        flags.insert(Flag::SizeCapped);
    }
    let block = values::boolean(&row.get(Field::BlockTradeElection)) == Some(true)
        || values::boolean(&row.get(Field::LargeNotionalOffFacilityElection)) == Some(true);
    if block {
        flags.insert(Flag::BlockTradeDelayed);
    }
    if ctx.backfill {
        flags.insert(Flag::Backfill);
    }

    // For corrections and cancels the instrument comes from the row too;
    // DTCC repeats the full terms on every message.
    let instrument = instrument_for(
        regime,
        asset,
        upi.as_deref(),
        product.as_deref(),
        underlier.as_deref(),
    )
    .ok_or_else(|| RowError::Invalid("cannot build an instrument id".into()))?;
    let idx = state.next_index.entry(instrument.clone()).or_insert(0);
    let event_index = *idx;
    *idx = idx
        .checked_add(1)
        .filter(|&i| i != u32::MAX)
        .ok_or_else(|| RowError::Invalid("too many records in one batch".into()))?;

    let ingest = ctx
        .fetched_at
        .or_else(|| ts_dissemination.to_nanos())
        .ok_or_else(|| RowError::Invalid("dissemination time out of range".into()))?;
    let prov = Provenance::new(
        instrument,
        regime.venue(),
        Seq::new(ctx.batch, event_index),
        ts_dissemination,
        ingest,
        regime.admission(),
    )
    .with_flags(flags);
    let print = SwapPrint {
        prov,
        repository: ctx.repository.id(),
        dissemination_id,
        original_dissemination_id,
        action,
        asset_class,
        upi,
        product,
        underlier,
        tenor,
        expiry,
        price,
        notional,
        ts_execution_ms: ts_execution,
        ts_dissemination_ms: ts_dissemination,
    };
    print
        .validate()
        .map_err(|e| RowError::Invalid(e.to_string()))?;
    Ok(Outcome::Print(Box::new(print)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenors() {
        assert_eq!(
            exact_tenor("2026-11-16", "2031-11-16").as_deref(),
            Some("5Y")
        );
        assert_eq!(
            exact_tenor("2026-11-16", "2027-05-16").as_deref(),
            Some("6M")
        );
        assert_eq!(
            exact_tenor("2026-11-16", "2028-05-16").as_deref(),
            Some("18M")
        );
        assert_eq!(exact_tenor("2026-11-16", "2031-11-17"), None);
        assert_eq!(exact_tenor("2026-11-16", "2026-11-16"), None);
    }

    #[test]
    fn instruments() {
        let i = instrument_for(
            Regime::CftcPart43,
            DtccAsset::Rates,
            Some("QZ12AB34CD56"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(i.as_str(), "cftc-part43:swap:QZ12AB34CD56");
        let i = instrument_for(
            Regime::CftcPart43,
            DtccAsset::Commodities,
            None,
            Some("Commodity:Energy:NatGas:Swap:Cash"),
            Some("Henry Hub / Index"),
        )
        .unwrap();
        assert_eq!(
            i.as_str(),
            "cftc-part43:swap:Commodity:Energy:NatGas:Swap:Cash:Henry-Hub-Index"
        );
        let i = instrument_for(Regime::SecSbsr, DtccAsset::Equities, None, None, None).unwrap();
        assert_eq!(i.as_str(), "sec-sbsr:swap:UNKNOWN-EQ");
        let long = "X".repeat(300);
        let i = instrument_for(
            Regime::SecSbsr,
            DtccAsset::Equities,
            None,
            Some(&long),
            None,
        )
        .unwrap();
        assert!(i.as_str().len() <= 128);
        assert!(i.as_str().contains('#'));
    }

    #[test]
    fn clean() {
        assert_eq!(clean_text("  a\tb  ").as_deref(), Some("ab"));
        assert_eq!(clean_text("").as_deref(), None);
        assert_eq!(clean_text(&"é".repeat(100)).unwrap().len(), 128);
    }
}
