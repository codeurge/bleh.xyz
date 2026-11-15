//! Every node-format fact the reader assumes but that is still marked
//! **to verify** in `docs/specs/hyperliquid-node-format.md` § 11.
//!
//! Each assumption is one named constant or function, used everywhere the
//! reader depends on it, so that fixing it once a real node runs is a
//! one-place change (plus the spec, `tools/hl-synth` and the fixtures, which
//! encode the same assumptions). The numbers in the headings refer to the
//! spec's § 11 list. `docs/operations/node-reader.md` repeats this list.

use mesh_types::{AggressorSide, BookSide, UnixMillis, UnixNanos};

// ---------------------------------------------------------------------------
// 1. Hourly file paths: `<stream>/hourly/{date}/{hour}`, `{date}` is
//    `YYYYMMDD`, `{hour}` is unpadded `0`-`23`, UTC.
// ---------------------------------------------------------------------------

/// Directory between a stream directory and the date directories.
pub const HOURLY_SUBDIR: &str = "hourly";

/// Parses a date directory name (`YYYYMMDD`). Assumption 1.
#[must_use]
pub fn parse_date_dir(name: &str) -> Option<u32> {
    if name.len() != 8 || !name.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let v: u32 = name.parse().ok()?;
    let (month, day) = ((v / 100) % 100, v % 100);
    ((1..=12).contains(&month) && (1..=31).contains(&day)).then_some(v)
}

/// Parses an hour file name (unpadded `0`-`23`). Assumption 1.
#[must_use]
pub fn parse_hour_file(name: &str) -> Option<u8> {
    if name.is_empty() || name.len() > 2 || !name.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if name.len() == 2 && name.starts_with('0') {
        return None;
    }
    let h: u8 = name.parse().ok()?;
    (h <= 23).then_some(h)
}

/// Relative path of an hourly file inside a stream directory
/// (`hourly/{date}/{hour}`). Assumption 1.
#[must_use]
pub fn hourly_rel_path(date: u32, hour: u8) -> String {
    format!("{HOURLY_SUBDIR}/{date:08}/{hour}")
}

// ---------------------------------------------------------------------------
// 2. Stream directory names under `--batch-by-block`. The reader takes them
//    from configuration; these are the defaults (what the fixtures use).
// ---------------------------------------------------------------------------

/// Default fills directory. Assumption 2 (alternative: `node_fills`).
pub const DEFAULT_FILLS_DIR: &str = "node_fills_by_block";
/// Default order-status directory. Assumption 2.
pub const DEFAULT_ORDER_STATUSES_DIR: &str = "node_order_statuses_by_block";
/// Default raw-book-diff directory. Assumption 2.
pub const DEFAULT_RAW_BOOK_DIFFS_DIR: &str = "node_raw_book_diffs_by_block";

// ---------------------------------------------------------------------------
// 3. `local_time` and `block_time` are naive UTC strings
//    (`YYYY-MM-DDTHH:MM:SS.fff...`); a line is written for blocks with no
//    events.
// ---------------------------------------------------------------------------

/// Parses `block_time` to milliseconds, flooring any sub-millisecond
/// digits. Assumption 3: naive UTC with up to nine fractional digits.
/// Archived node output (2025-07-27 and 2026-11-13, sampled 2026-11-14)
/// writes nine, so the earlier "at most millisecond precision" reading was
/// wrong; `ts_source_ms` is the block time floored to the millisecond.
///
/// # Errors
///
/// Fails on any other format.
pub fn parse_block_time(s: &str) -> Result<UnixMillis, String> {
    UnixNanos::parse_rfc3339(s)
        .map(UnixNanos::to_millis)
        .map_err(|e| e.to_string())
}

/// Parses `local_time` to nanoseconds. Assumption 3: naive UTC with up to
/// nine fractional digits.
///
/// # Errors
///
/// Fails on any other format.
pub fn parse_local_time(s: &str) -> Result<UnixNanos, String> {
    UnixNanos::parse_rfc3339(s).map_err(|e| e.to_string())
}

/// Parses an order-status `time` (same naive UTC format). Assumption 3.
///
/// # Errors
///
/// Fails on any other format.
pub fn parse_status_time(s: &str) -> Result<UnixNanos, String> {
    UnixNanos::parse_rfc3339(s).map_err(|e| e.to_string())
}

/// Assumption 3: the node writes a line in every stream for every block,
/// with an empty `events` array when the block had none. If this is false,
/// block-number jumps are normal and [`is_block_gap`] must return false for
/// them (gaps are then only detectable across streams).
pub const EMPTY_BLOCK_LINES_WRITTEN: bool = true;

/// True if going from block `prev` to block `next` skips data.
#[must_use]
pub const fn is_block_gap(prev: u64, next: u64) -> bool {
    EMPTY_BLOCK_LINES_WRITTEN && next > prev.saturating_add(1)
}

// ---------------------------------------------------------------------------
// 4. `side` is `B` (buy) or `A` (sell) in fills and order statuses; raw
//    book diffs use `Bid` / `Ask`.
// ---------------------------------------------------------------------------

/// Aggressor of a trade from the `side` of its `crossed = true` fill.
/// Assumption 4.
#[must_use]
pub fn fill_side_aggressor(side: &str) -> Option<AggressorSide> {
    match side {
        "B" => Some(AggressorSide::Buy),
        "A" => Some(AggressorSide::Sell),
        _ => None,
    }
}

/// Book side of an order (`order.side` in statuses and L4 snapshots).
/// Assumption 4.
#[must_use]
pub fn order_side(side: &str) -> Option<BookSide> {
    match side {
        "B" => Some(BookSide::Bid),
        "A" => Some(BookSide::Ask),
        _ => None,
    }
}

/// Book side of a raw book diff (`Bid` / `Ask`, per the schema docs).
#[must_use]
pub fn diff_side(side: &str) -> Option<BookSide> {
    match side {
        "Bid" => Some(BookSide::Bid),
        "Ask" => Some(BookSide::Ask),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 5. Order-status `sz` (remaining or filled size). The reader never reads
//    it: book sizes come only from raw book diffs, so the assumption cannot
//    change any record. The filter keeps it in filtered raw as `sz`.
// ---------------------------------------------------------------------------

/// The order status that, with a `remove` diff for the same order in the
/// same block, classifies the removal as a fill (spec § 6.1).
pub const FILLED_STATUS: &str = "filled";

// ---------------------------------------------------------------------------
// 6. How user modifies appear in raw book diffs. The fixtures model a
//    size-reducing modify as an in-place `update` (status `open`); a
//    `remove` plus `new` with a new `oid` maps to `cancel` plus `add`
//    either way. The classification itself lives in
//    `crate::normalize::classify_diff`, which this assumption feeds.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 7. How halt-settlement fills are marked. The fixtures model them as a
//    pair with no `crossed = true` fill (and `dir = "Settlement"`); the rule
//    below does not read `dir`.
// ---------------------------------------------------------------------------

/// Liquidation `method` of a fill whose liquidation order matched the book:
/// the trade is kept, the `liquidation` object dropped.
pub const LIQUIDATION_METHOD_MARKET: &str = "market";
/// Liquidation `method` of a backstop transfer at the mark price: the pair
/// is dropped.
pub const LIQUIDATION_METHOD_BACKSTOP: &str = "backstop";

/// Assumption 7: a fill pair is an order-book trade only if exactly one of
/// its two fills has `crossed = true`. Pairs with none (backstop
/// liquidations, halt settlements) are not trades.
#[must_use]
pub const fn crossed_count_is_book_trade(crossed: usize) -> bool {
    crossed == 1
}

// ---------------------------------------------------------------------------
// 8. L4 snapshot top level with `includeHeightInOutput`:
//    `{"height": H, "snapshot": [[coin, [bids, asks]], ...]}`. Isolated in
//    `crate::snapshot::parse_l4_envelope`.
// ---------------------------------------------------------------------------

/// Key of the snapshot height in the L4 envelope. Assumption 8.
pub const L4_HEIGHT_KEY: &str = "height";
/// Key of the per-coin books in the L4 envelope. Assumption 8.
pub const L4_SNAPSHOT_KEY: &str = "snapshot";

// ---------------------------------------------------------------------------
// 9. Price scales for HIP-3 and outcome markets, the coin used for merged
//    HIP-4 books, and `@index` spot coin names.
// ---------------------------------------------------------------------------

/// Assumption 9: HIP-3 deployer perpetuals follow the core perpetual price
/// rule (`6 - szDecimals`). `mesh_types::InstrumentSpec::hyperliquid`
/// applies that rule to every perpetual; this constant documents it.
pub const HIP3_USES_PERP_PRICE_RULE: bool = true;

/// Assumption 9: HIP-4 outcome markets follow the spot price rule: at most
/// `8 - szDecimals` price decimals (and five significant figures). The
/// venue documents outcomes as sharing "most implementation details with
/// spot" and gives no separate rule; observed prices (probabilities between
/// 0 and 1) have five decimals, which a fixed scale of 4 could not hold
/// (ADR 0021).
pub const OUTCOME_MAX_DECIMALS: u8 = 8;

/// Assumption 9: price scale of a HIP-4 outcome market with `sz_decimals`
/// size decimals (`8 - szDecimals`, ADR 0021).
#[must_use]
pub const fn outcome_price_scale(sz_decimals: u8) -> u8 {
    OUTCOME_MAX_DECIMALS.saturating_sub(sz_decimals)
}

/// Assumption 9: size decimals of HIP-4 outcome markets when metadata does
/// not give `szDecimals`.
pub const OUTCOME_DEFAULT_SZ_DECIMALS: u8 = 0;

/// Assumption 9: the node coins of one HIP-4 outcome: `#{10 * outcome +
/// side}` for side 0 and 1. Both are registered, because which coin the
/// node reports for a merged Yes/No book is still to verify.
#[must_use]
pub fn outcome_coins(outcome: u64) -> [String; 2] {
    let base = outcome.saturating_mul(10);
    [format!("#{base}"), format!("#{}", base.saturating_add(1))]
}

/// Market class implied by a node `coin` string alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoinClass {
    /// Core perpetual, such as `BTC`.
    CorePerp,
    /// HIP-3 deployer perpetual, `{dex}:{coin}`.
    Hip3Perp,
    /// HIP-4 outcome, `#{n}`.
    Outcome,
    /// Spot, `@{index}` or a pair name with `/`. Assumption 9.
    Spot,
}

/// Classifies a node `coin` by its syntax (spec § 8). Assumption 9 for the
/// spot and outcome forms.
#[must_use]
pub fn classify_coin(coin: &str) -> CoinClass {
    if coin.starts_with('#') {
        CoinClass::Outcome
    } else if coin.starts_with('@') || coin.contains('/') {
        CoinClass::Spot
    } else if coin.contains(':') {
        CoinClass::Hip3Perp
    } else {
        CoinClass::CorePerp
    }
}

/// True if ADM-0001 admits the market: core perpetuals, HIP-3 deployer
/// perpetuals and HIP-4 outcome markets. Spot markets are outside the
/// admission's scope, so their events are dropped from records and from
/// filtered raw.
#[must_use]
pub fn is_admitted_coin(coin: &str) -> bool {
    !matches!(classify_coin(coin), CoinClass::Spot)
}

// ---------------------------------------------------------------------------
// Info server (spec § 7.2).
// ---------------------------------------------------------------------------

/// Default info server endpoint of a node started with `--serve-info`.
pub const DEFAULT_INFO_URL: &str = "http://127.0.0.1:3001/info";

/// The `fileSnapshot` request body for an L4 snapshot written to
/// `out_path`. Always without users and trigger orders.
#[must_use]
pub fn l4_snapshot_request(out_path: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "fileSnapshot",
        "request": {"type": "l4Snapshots", "includeUsers": false, "includeTriggerOrders": false},
        "outPath": out_path,
        "includeHeightInOutput": true,
    })
}

// ---------------------------------------------------------------------------
// 10. Floating-point artifacts in venue decimal strings.
// ---------------------------------------------------------------------------

/// Assumption 10: significant digits a venue decimal string can carry
/// faithfully. The venue computes some values (liquidation and
/// auto-deleveraging fills, for example) in binary floating point and can
/// print more digits than a 64-bit float holds, such as
/// `"2946657.1000000001"` for `2946657.1`. A double holds 15 significant
/// decimal digits exactly (ADR 0022).
pub const VENUE_SIGNIFICANT_DIGITS: usize = 15;

/// Assumption 10: a venue decimal string with float noise removed. A string
/// with more than [`VENUE_SIGNIFICANT_DIGITS`] significant digits is rounded
/// (half up, in decimal, without floating point) to that many and written
/// without trailing fractional zeros; every other string, including
/// malformed ones, is returned unchanged for the caller to parse or reject.
/// After this, a value that still has more decimals than the instrument's
/// scale is a real violation.
#[must_use]
pub fn venue_decimal(s: &str) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, s),
    };
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    let digits_ok = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    if int.is_empty() || !digits_ok(int) || !digits_ok(frac) {
        return Cow::Borrowed(s);
    }
    let frac = frac.trim_end_matches('0');
    let mut d: Vec<u8> = int.bytes().chain(frac.bytes()).map(|b| b - b'0').collect();
    let Some(first) = d.iter().position(|&x| x != 0) else {
        return Cow::Borrowed(s);
    };
    if d.len() - first <= VENUE_SIGNIFICANT_DIGITS {
        return Cow::Borrowed(s);
    }
    let mut int_len = int.len();
    let keep = first + VENUE_SIGNIFICANT_DIGITS;
    let round_up = d[keep] >= 5;
    d.truncate(keep);
    if round_up {
        let mut i = keep;
        loop {
            if i == 0 {
                d.insert(0, 1);
                int_len += 1;
                break;
            }
            i -= 1;
            if d[i] == 9 {
                d[i] = 0;
            } else {
                d[i] += 1;
                break;
            }
        }
    }
    // Integer digits dropped by the rounding become zeros.
    while d.len() < int_len {
        d.push(0);
    }
    let to_str = |x: &[u8]| x.iter().map(|&v| char::from(b'0' + v)).collect::<String>();
    let int_s = to_str(&d[..int_len]);
    let int_s = int_s.trim_start_matches('0');
    let frac_s = to_str(&d[int_len..]);
    let frac_s = frac_s.trim_end_matches('0');
    let mut out = String::with_capacity(s.len());
    if neg {
        out.push('-');
    }
    out.push_str(if int_s.is_empty() { "0" } else { int_s });
    if !frac_s.is_empty() {
        out.push('.');
        out.push_str(frac_s);
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn hourly_paths() {
        assert_eq!(parse_date_dir("20260101"), Some(20_260_101));
        assert_eq!(parse_date_dir("2026011"), None);
        assert_eq!(parse_date_dir("20261301"), None);
        assert_eq!(parse_hour_file("0"), Some(0));
        assert_eq!(parse_hour_file("23"), Some(23));
        assert_eq!(parse_hour_file("24"), None);
        assert_eq!(parse_hour_file("05"), None);
        assert_eq!(parse_hour_file("x"), None);
        assert_eq!(hourly_rel_path(20_260_101, 7), "hourly/20260101/7");
    }

    #[test]
    fn times_and_gaps() {
        assert_eq!(
            parse_block_time("2026-01-01T00:59:57.089").unwrap(),
            UnixMillis(1_767_229_197_089)
        );
        // Nine digits, as archived node output writes them: floored.
        assert_eq!(
            parse_block_time("2026-01-01T00:59:57.089999999").unwrap(),
            UnixMillis(1_767_229_197_089)
        );
        assert!(parse_block_time("2026-01-01 00:59:57").is_err());
        assert_eq!(
            parse_local_time("2026-01-01T00:59:57.008642787").unwrap(),
            UnixNanos(1_767_229_197_008_642_787)
        );
        assert!(!is_block_gap(5, 6));
        assert!(is_block_gap(5, 7));
        assert!(!is_block_gap(5, 5));
    }

    #[test]
    fn sides_and_coins() {
        assert_eq!(fill_side_aggressor("B"), Some(AggressorSide::Buy));
        assert_eq!(fill_side_aggressor("A"), Some(AggressorSide::Sell));
        assert_eq!(fill_side_aggressor("Bid"), None);
        assert_eq!(diff_side("Ask"), Some(BookSide::Ask));
        assert_eq!(order_side("B"), Some(BookSide::Bid));
        assert_eq!(classify_coin("SYN"), CoinClass::CorePerp);
        assert_eq!(classify_coin("syn:IDX"), CoinClass::Hip3Perp);
        assert_eq!(classify_coin("#9001"), CoinClass::Outcome);
        assert_eq!(classify_coin("@107"), CoinClass::Spot);
        assert_eq!(classify_coin("AAA/USDC"), CoinClass::Spot);
        assert!(!is_admitted_coin("@1"));
        assert!(is_admitted_coin("#10"));
        assert_eq!(outcome_coins(900), ["#9000".to_owned(), "#9001".to_owned()]);
    }

    #[test]
    fn venue_decimal_removes_float_noise_only() {
        // Archived liquidation fill, 2025-10-10: 17 significant digits.
        assert_eq!(venue_decimal("2946657.1000000001"), "2946657.1");
        assert_eq!(venue_decimal("0.30000000000000004"), "0.3");
        assert_eq!(venue_decimal("-1945471.4171330004"), "-1945471.417133");
        assert_eq!(venue_decimal("99.9999999999999999"), "100");
        assert_eq!(
            venue_decimal("0.000012345678901234567"),
            "0.0000123456789012346"
        );
        assert_eq!(venue_decimal("123456789012345678"), "123456789012346000");
        // Fifteen or fewer significant digits: unchanged, borrowed.
        for s in [
            "1.99203",
            "100000",
            "0.00012",
            "123456789012345",
            "1.50",
            "0",
            "0.000",
            "-3",
        ] {
            assert!(
                matches!(venue_decimal(s), std::borrow::Cow::Borrowed(_)),
                "{s}"
            );
        }
        // Malformed input is left for the parser to reject.
        for s in ["", ".5", "1e5", "abc", "1.2.3"] {
            assert_eq!(venue_decimal(s), s);
        }
    }
}
