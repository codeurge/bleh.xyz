//! The archive buckets, the admitted datasets in them, the hard-coded
//! denylist, and key parsing.
//!
//! Layouts are documented in `docs/specs/hyperliquid-archive-buckets.md`.
//! Everything here is pure (no I/O) so the admission rules can be tested
//! exhaustively.

use std::fmt;

use mesh_types::UnixMillis;

/// One of Hyperliquid's two requester-pays buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Bucket {
    /// `hl-mainnet-node-data`: node output and L1 data.
    NodeData,
    /// `hyperliquid-archive`: asset data (L2 snapshots).
    Archive,
}

impl Bucket {
    /// Both buckets.
    pub const ALL: [Bucket; 2] = [Bucket::NodeData, Bucket::Archive];

    /// The S3 bucket name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::NodeData => "hl-mainnet-node-data",
            Self::Archive => "hyperliquid-archive",
        }
    }

    /// Looks a bucket up by its S3 name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.name() == name)
    }
}

impl fmt::Display for Bucket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// How keys under a dataset prefix are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// `<prefix>/hourly/{YYYYMMDD}/{H}[.lz4]`.
    Hourly,
    /// `market_data/{YYYYMMDD}/{H}/{datatype}/{coin}.lz4`.
    MarketData,
    /// `replica_cmds/{start_time}/{YYYYMMDD}/{height}[.lz4]`, or the
    /// alternative `replica_cmds/{YYYYMMDD}/{height}[.lz4]`.
    ReplicaCmds,
    /// `explorer_blocks/{l1}/{l2}/{height}.rmp.lz4`.
    ExplorerBlocks,
}

/// An admitted dataset (ADM-0003). Excluded prefixes have no variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dataset {
    /// `hl-mainnet-node-data/node_fills_by_block`.
    NodeFillsByBlock,
    /// `hl-mainnet-node-data/node_fills` (older).
    NodeFills,
    /// `hl-mainnet-node-data/node_trades` (older).
    NodeTrades,
    /// `hl-mainnet-node-data/replica_cmds` (replay only).
    ReplicaCmds,
    /// `hl-mainnet-node-data/explorer_blocks` (replay only).
    ExplorerBlocks,
    /// `hyperliquid-archive/market_data` (L2 snapshots).
    MarketData,
}

/// The `market_data` data type admitted by ADM-0003.
pub const MARKET_DATA_ADMITTED_TYPE: &str = "l2Book";

/// Prefixes ADM-0003 excludes. Any key with a path segment that starts
/// with one of these is refused before any request is made for it.
pub const DENIED_PREFIXES: [&str; 2] = ["asset_ctxs", "misc_events_by_block"];

impl Dataset {
    /// Every admitted dataset.
    pub const ALL: [Dataset; 6] = [
        Dataset::NodeFillsByBlock,
        Dataset::NodeFills,
        Dataset::NodeTrades,
        Dataset::ReplicaCmds,
        Dataset::ExplorerBlocks,
        Dataset::MarketData,
    ];

    /// The bucket holding the dataset.
    #[must_use]
    pub const fn bucket(self) -> Bucket {
        match self {
            Self::MarketData => Bucket::Archive,
            _ => Bucket::NodeData,
        }
    }

    /// The top-level key prefix (without a trailing `/`), also the
    /// dataset's name on the command line.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::NodeFillsByBlock => "node_fills_by_block",
            Self::NodeFills => "node_fills",
            Self::NodeTrades => "node_trades",
            Self::ReplicaCmds => "replica_cmds",
            Self::ExplorerBlocks => "explorer_blocks",
            Self::MarketData => "market_data",
        }
    }

    /// Key layout.
    #[must_use]
    pub const fn layout(self) -> Layout {
        match self {
            Self::NodeFillsByBlock | Self::NodeFills | Self::NodeTrades => Layout::Hourly,
            Self::MarketData => Layout::MarketData,
            Self::ReplicaCmds => Layout::ReplicaCmds,
            Self::ExplorerBlocks => Layout::ExplorerBlocks,
        }
    }

    /// True for datasets whose records are published (fills and L2
    /// snapshots). `replica_cmds` and `explorer_blocks` feed the replay
    /// only.
    #[must_use]
    pub const fn is_record_source(self) -> bool {
        matches!(
            self,
            Self::NodeFillsByBlock | Self::NodeFills | Self::NodeTrades | Self::MarketData
        )
    }

    /// Looks a dataset up by its prefix name. Never returns an excluded
    /// prefix.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.prefix() == name)
    }
}

impl fmt::Display for Dataset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.prefix())
    }
}

/// A key refused by the admission policy.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    /// The bucket is not one of Hyperliquid's archive buckets.
    #[error("bucket {0} is not an admitted archive bucket")]
    UnknownBucket(String),
    /// The key touches an excluded prefix.
    #[error("key {key} touches excluded prefix {prefix} (ADM-0003)")]
    Denied {
        /// The key.
        key: String,
        /// The denied prefix it matched.
        prefix: &'static str,
    },
    /// The key is not under an admitted prefix of its bucket.
    #[error("key {0} is not under an admitted prefix of its bucket")]
    NotAdmitted(String),
    /// A `market_data` data type other than `l2Book`.
    #[error("market_data type {0} is not admitted (only l2Book)")]
    MarketDataType(String),
}

/// Returns the denied prefix a key (or listing prefix) touches, if any.
///
/// Matches any path segment that starts with a denied prefix, ignoring
/// ASCII case, so `misc_events_by_block/`, `MISC_EVENTS_BY_BLOCK` and
/// `x/asset_ctxs/` are all refused.
#[must_use]
pub fn denied_prefix(key: &str) -> Option<&'static str> {
    key.split('/').find_map(|seg| {
        let seg = seg.to_ascii_lowercase();
        DENIED_PREFIXES.into_iter().find(|p| seg.starts_with(p))
    })
}

/// Checks that `key` in `bucket` may be listed, read or copied, and
/// returns its dataset.
///
/// # Errors
///
/// Fails for unknown buckets, any key touching a denied prefix, keys
/// outside the admitted prefixes of the bucket, and `market_data` types
/// other than `l2Book`.
pub fn check_key(bucket: &str, key: &str) -> Result<Dataset, PolicyError> {
    if let Some(prefix) = denied_prefix(key) {
        return Err(PolicyError::Denied {
            key: key.to_owned(),
            prefix,
        });
    }
    let bucket =
        Bucket::from_name(bucket).ok_or_else(|| PolicyError::UnknownBucket(bucket.into()))?;
    let first = key.trim_start_matches('/').split('/').next().unwrap_or("");
    let dataset = Dataset::from_name(first)
        .filter(|d| d.bucket() == bucket)
        .ok_or_else(|| PolicyError::NotAdmitted(key.to_owned()))?;
    if dataset == Dataset::MarketData {
        // market_data/{date}/{hour}/{datatype}/...: only l2Book.
        if let Some(datatype) = key.split('/').nth(3)
            && datatype != MARKET_DATA_ADMITTED_TYPE
        {
            return Err(PolicyError::MarketDataType(datatype.to_owned()));
        }
    }
    Ok(dataset)
}

/// A UTC calendar day, stored as `YYYYMMDD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Day(u32);

/// A malformed day.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid day {0:?}: expected YYYYMMDD or YYYY-MM-DD")]
pub struct DayError(pub String);

impl Day {
    /// Parses `YYYYMMDD` or `YYYY-MM-DD`.
    ///
    /// # Errors
    ///
    /// Fails on anything else, or an impossible date.
    pub fn parse(s: &str) -> Result<Self, DayError> {
        let digits: String = s.chars().filter(|c| *c != '-').collect();
        let well_formed = digits.len() == 8
            && digits.bytes().all(|b| b.is_ascii_digit())
            && (s.len() == 8 || (s.len() == 10 && &s[4..5] == "-" && &s[7..8] == "-"));
        if !well_formed {
            return Err(DayError(s.into()));
        }
        let v: u32 = digits.parse().map_err(|_| DayError(s.into()))?;
        let day = Day(v);
        // Validate through the shared calendar code.
        UnixMillis::parse_rfc3339(&format!("{}T00:00:00Z", day.iso()))
            .map_err(|_| DayError(s.into()))?;
        Ok(day)
    }

    /// `YYYYMMDD`, as in bucket keys.
    #[must_use]
    pub fn compact(self) -> String {
        format!("{:08}", self.0)
    }

    /// `YYYY-MM-DD`, as in archive partitions.
    #[must_use]
    pub fn iso(self) -> String {
        let v = self.0;
        format!("{:04}-{:02}-{:02}", v / 10_000, (v / 100) % 100, v % 100)
    }

    /// Midnight UTC at the start of the day.
    #[must_use]
    pub fn start_ms(self) -> UnixMillis {
        UnixMillis::parse_rfc3339(&format!("{}T00:00:00Z", self.iso())).unwrap_or(UnixMillis(0))
    }

    /// Days since the Unix epoch (used for deterministic sharding).
    #[must_use]
    pub fn ordinal(self) -> i64 {
        self.start_ms().0.div_euclid(86_400_000)
    }

    /// The UTC day containing `ts`.
    #[must_use]
    pub fn of(ts: UnixMillis) -> Self {
        let iso = ts.to_rfc3339();
        // to_rfc3339 always starts with YYYY-MM-DD.
        Day::parse(&iso[..10]).unwrap_or(Day(19_700_101))
    }

    /// The next day.
    #[must_use]
    pub fn succ(self) -> Self {
        Day::of(UnixMillis(self.start_ms().0 + 86_400_000))
    }

    /// Every day from `self` to `to`, inclusive.
    #[must_use]
    pub fn through(self, to: Day) -> Vec<Day> {
        let mut out = Vec::new();
        let mut d = self;
        while d <= to {
            out.push(d);
            d = d.succ();
        }
        out
    }
}

impl fmt::Display for Day {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.iso())
    }
}

/// What a key says about its contents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyInfo {
    /// UTC day, where the layout has one.
    pub day: Option<Day>,
    /// UTC hour (0-23), where the layout has one.
    pub hour: Option<u8>,
    /// First block height in the file (`replica_cmds`, `explorer_blocks`).
    pub height: Option<u64>,
    /// Coin (`market_data`).
    pub coin: Option<String>,
}

impl KeyInfo {
    /// A stable shard label: `YYYY-MM-DD/HH`, `YYYY-MM-DD`, or
    /// `h<height floored to 100,000>`.
    #[must_use]
    pub fn shard_label(&self) -> String {
        match (self.day, self.hour, self.height) {
            (Some(d), Some(h), _) => format!("{d}/{h:02}"),
            (Some(d), None, _) => d.to_string(),
            (None, _, Some(h)) => format!("h{}", h / 100_000 * 100_000),
            _ => "unsharded".into(),
        }
    }

    /// Deterministic shard bucket used to split work across hosts: whole
    /// days (or 100,000-block ranges) go to the same host.
    #[must_use]
    pub fn shard_ordinal(&self) -> u64 {
        match (self.day, self.height) {
            (Some(d), _) => u64::try_from(d.ordinal()).unwrap_or(0),
            (None, Some(h)) => h / 100_000,
            _ => 0,
        }
    }
}

/// A key that does not match its dataset's layout.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("key {key} does not match the {dataset} layout: {reason}")]
pub struct KeyError {
    /// The key.
    pub key: String,
    /// The dataset.
    pub dataset: Dataset,
    /// What is wrong.
    pub reason: &'static str,
}

fn parse_hour(s: &str) -> Option<u8> {
    // Unpadded 0-23; tolerate zero padding.
    if s.is_empty() || s.len() > 2 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u8>().ok().filter(|h| *h < 24)
}

fn strip_ext(name: &str) -> &str {
    let name = name.strip_suffix(".lz4").unwrap_or(name);
    name.strip_suffix(".rmp").unwrap_or(name)
}

fn parse_height(s: &str) -> Option<u64> {
    let s = strip_ext(s);
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn compact_day(s: &str) -> Option<Day> {
    if s.len() == 8 {
        Day::parse(s).ok()
    } else {
        None
    }
}

/// Parses what a key says about its contents.
///
/// # Errors
///
/// Fails if the key does not match the dataset's layout.
pub fn parse_key(dataset: Dataset, key: &str) -> Result<KeyInfo, KeyError> {
    let err = |reason| KeyError {
        key: key.to_owned(),
        dataset,
        reason,
    };
    let segs: Vec<&str> = key.trim_start_matches('/').split('/').collect();
    if segs.first() != Some(&dataset.prefix()) {
        return Err(err("wrong prefix"));
    }
    match dataset.layout() {
        Layout::Hourly => {
            let [_, hourly, day, hour] = segs.as_slice() else {
                return Err(err("expected <prefix>/hourly/YYYYMMDD/H"));
            };
            if *hourly != "hourly" {
                return Err(err("expected an hourly/ directory"));
            }
            Ok(KeyInfo {
                day: Some(compact_day(day).ok_or_else(|| err("bad date"))?),
                hour: Some(parse_hour(strip_ext(hour)).ok_or_else(|| err("bad hour"))?),
                ..KeyInfo::default()
            })
        }
        Layout::MarketData => {
            let [_, day, hour, datatype, file] = segs.as_slice() else {
                return Err(err("expected market_data/YYYYMMDD/H/l2Book/COIN.lz4"));
            };
            if *datatype != MARKET_DATA_ADMITTED_TYPE {
                return Err(err("data type is not l2Book"));
            }
            let coin = file.strip_suffix(".lz4").unwrap_or(file);
            if coin.is_empty() {
                return Err(err("empty coin"));
            }
            Ok(KeyInfo {
                day: Some(compact_day(day).ok_or_else(|| err("bad date"))?),
                hour: Some(parse_hour(hour).ok_or_else(|| err("bad hour"))?),
                coin: Some(coin.to_owned()),
                ..KeyInfo::default()
            })
        }
        Layout::ReplicaCmds => {
            // replica_cmds/{start_time}/{date}/{height} or replica_cmds/{date}/{height}.
            let (day, file) = match segs.as_slice() {
                [_, _, day, file] | [_, day, file] => (*day, *file),
                _ => return Err(err("expected replica_cmds/[start_time/]YYYYMMDD/HEIGHT")),
            };
            Ok(KeyInfo {
                day: Some(compact_day(day).ok_or_else(|| err("bad date"))?),
                height: Some(parse_height(file).ok_or_else(|| err("bad height"))?),
                ..KeyInfo::default()
            })
        }
        Layout::ExplorerBlocks => {
            let [_, l1, l2, file] = segs.as_slice() else {
                return Err(err("expected explorer_blocks/L1/L2/HEIGHT.rmp.lz4"));
            };
            if parse_height(l1).is_none() || parse_height(l2).is_none() {
                return Err(err("directory levels must be numeric"));
            }
            Ok(KeyInfo {
                height: Some(parse_height(file).ok_or_else(|| err("bad height"))?),
                ..KeyInfo::default()
            })
        }
    }
}

/// Numeric value of a directory name, for range filtering of
/// `explorer_blocks` levels.
#[must_use]
pub fn numeric_dir(name: &str) -> Option<u64> {
    parse_height(name.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denylist_refuses_excluded_prefixes_everywhere() {
        for (bucket, key) in [
            ("hyperliquid-archive", "asset_ctxs/20250101.csv.lz4"),
            ("hyperliquid-archive", "asset_ctxs"),
            (
                "hl-mainnet-node-data",
                "misc_events_by_block/hourly/20250101/0.lz4",
            ),
            (
                "hl-mainnet-node-data",
                "MISC_EVENTS_BY_BLOCK/hourly/20250101/0.lz4",
            ),
            (
                "hl-mainnet-node-data",
                "node_fills_by_block/misc_events_by_block/0.lz4",
            ),
            ("hyperliquid-archive", "market_data/asset_ctxs/x"),
            ("hl-mainnet-node-data", "asset_ctxs_v2/x"),
            ("unknown-bucket", "asset_ctxs/x"),
        ] {
            assert!(
                matches!(check_key(bucket, key), Err(PolicyError::Denied { .. })),
                "{bucket}/{key} must be denied"
            );
        }
    }

    #[test]
    fn admitted_prefixes_pass_only_in_their_bucket() {
        assert_eq!(
            check_key(
                "hl-mainnet-node-data",
                "node_fills_by_block/hourly/20250727/9.lz4"
            ),
            Ok(Dataset::NodeFillsByBlock)
        );
        assert_eq!(
            check_key(
                "hyperliquid-archive",
                "market_data/20230916/9/l2Book/SOL.lz4"
            ),
            Ok(Dataset::MarketData)
        );
        assert!(matches!(
            check_key("hyperliquid-archive", "node_fills/hourly/20250101/0.lz4"),
            Err(PolicyError::NotAdmitted(_))
        ));
        assert!(matches!(
            check_key("hl-mainnet-node-data", "misc_events/hourly/20250101/0.lz4"),
            Err(PolicyError::NotAdmitted(_))
        ));
        assert!(matches!(
            check_key(
                "hyperliquid-archive",
                "market_data/20230916/9/trades/SOL.lz4"
            ),
            Err(PolicyError::MarketDataType(_))
        ));
        assert!(matches!(
            check_key("some-bucket", "node_fills/hourly/20250101/0.lz4"),
            Err(PolicyError::UnknownBucket(_))
        ));
    }

    #[test]
    fn no_dataset_names_a_denied_prefix() {
        for d in Dataset::ALL {
            assert!(denied_prefix(d.prefix()).is_none());
            assert!(Dataset::from_name(d.prefix()).is_some());
        }
        for p in DENIED_PREFIXES {
            assert!(Dataset::from_name(p).is_none());
        }
    }

    #[test]
    fn parses_every_layout() {
        let k = parse_key(
            Dataset::NodeFillsByBlock,
            "node_fills_by_block/hourly/20250727/9.lz4",
        )
        .unwrap();
        assert_eq!(k.day, Some(Day::parse("2025-07-27").unwrap()));
        assert_eq!(k.hour, Some(9));
        assert_eq!(k.shard_label(), "2025-07-27/09");

        let k = parse_key(
            Dataset::MarketData,
            "market_data/20230916/23/l2Book/#10.lz4",
        )
        .unwrap();
        assert_eq!(k.coin.as_deref(), Some("#10"));
        assert_eq!(k.hour, Some(23));

        let k = parse_key(
            Dataset::ReplicaCmds,
            "replica_cmds/2025-04-06T14:33:08Z/20250411/559130000.lz4",
        )
        .unwrap();
        assert_eq!(k.height, Some(559_130_000));
        assert_eq!(k.day.unwrap().iso(), "2025-04-11");
        let k = parse_key(Dataset::ReplicaCmds, "replica_cmds/20250411/559130000").unwrap();
        assert_eq!(k.height, Some(559_130_000));

        let k = parse_key(
            Dataset::ExplorerBlocks,
            "explorer_blocks/500000000/559100000/559148100.rmp.lz4",
        )
        .unwrap();
        assert_eq!(k.height, Some(559_148_100));
        assert_eq!(k.shard_label(), "h559100000");
    }

    #[test]
    fn rejects_bad_keys() {
        for (d, key) in [
            (Dataset::NodeFills, "node_fills/hourly/2025072/9.lz4"),
            (Dataset::NodeFills, "node_fills/hourly/20250727/24.lz4"),
            (Dataset::NodeFills, "node_fills/daily/20250727/1.lz4"),
            (Dataset::NodeFills, "node_trades/hourly/20250727/1.lz4"),
            (Dataset::MarketData, "market_data/20250727/1/l2Book/.lz4"),
            (Dataset::ExplorerBlocks, "explorer_blocks/a/b/1.rmp.lz4"),
            (Dataset::ReplicaCmds, "replica_cmds/x/y/z/w"),
        ] {
            assert!(parse_key(d, key).is_err(), "{key}");
        }
    }

    #[test]
    fn days_iterate_and_validate() {
        let a = Day::parse("20240228").unwrap();
        let b = Day::parse("2024-03-01").unwrap();
        let days: Vec<String> = a.through(b).iter().map(|d| d.compact()).collect();
        assert_eq!(days, ["20240228", "20240229", "20240301"]);
        assert!(Day::parse("20230229").is_err());
        assert!(Day::parse("2023-2-01").is_err());
        assert_eq!(Day::of(UnixMillis(0)).iso(), "1970-01-01");
    }
}
