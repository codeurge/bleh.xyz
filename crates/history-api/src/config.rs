//! Service configuration and limits.
//!
//! The binary fills [`Config`] from command-line flags and environment
//! variables (see `main.rs`); tests build it directly.

use std::path::PathBuf;
use std::time::Duration;

/// Default archive name (see `docs/specs/archive-layout-v0.md`).
pub const DEFAULT_ARCHIVE: &str = "hyperliquid-normalized";

/// Query and request limits. Every value is enforced server-side; the
/// effective values are published at `/v0/limits`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Limits {
    /// Wall-clock budget of one query; the query is interrupted after it.
    #[serde(serialize_with = "ser_ms")]
    pub query_timeout: Duration,
    /// DuckDB memory cap for the whole engine, in bytes.
    pub memory_limit_bytes: u64,
    /// DuckDB worker threads.
    pub threads: u32,
    /// Queries allowed to run at once; more wait up to `queue_timeout`.
    pub max_concurrent_queries: usize,
    /// How long a query waits for a slot before `overloaded`.
    #[serde(serialize_with = "ser_ms")]
    pub queue_timeout: Duration,
    /// Default page size when `limit` is omitted.
    pub default_rows: usize,
    /// Largest page size a client may ask for.
    pub max_rows: usize,
    /// Widest time range for record queries (trades, book events, bars).
    #[serde(serialize_with = "ser_ms")]
    pub max_range: Duration,
    /// Widest range for coverage queries.
    #[serde(serialize_with = "ser_ms")]
    pub max_coverage_range: Duration,
    /// How far back `book` looks for a checkpoint snapshot.
    #[serde(serialize_with = "ser_ms")]
    pub max_checkpoint_lookback: Duration,
    /// Most book events applied on top of a checkpoint.
    pub max_book_events: usize,
    /// Widest range a websocket replay may cover.
    #[serde(serialize_with = "ser_ms")]
    pub max_replay_range: Duration,
    /// Fastest replay multiplier (0 in requests means "as fast as possible").
    pub max_replay_speed: f64,
    /// Concurrent replays per IP.
    pub max_replays_per_ip: usize,
    /// Most records one Databento-shaped request returns.
    pub max_dbn_records: usize,
    /// Requests per second per IP (token refill rate).
    pub rate_per_sec: f64,
    /// Burst size per IP.
    pub rate_burst: f64,
}

fn ser_ms<S: serde::Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u64(u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

impl Default for Limits {
    fn default() -> Self {
        const DAY: u64 = 86_400;
        Self {
            query_timeout: Duration::from_secs(10),
            memory_limit_bytes: 1 << 30,
            threads: 4,
            max_concurrent_queries: 8,
            queue_timeout: Duration::from_secs(5),
            default_rows: 1_000,
            max_rows: 10_000,
            max_range: Duration::from_secs(31 * DAY),
            max_coverage_range: Duration::from_secs(31 * DAY),
            max_checkpoint_lookback: Duration::from_secs(DAY),
            max_book_events: 2_000_000,
            max_replay_range: Duration::from_secs(7 * DAY),
            max_replay_speed: 1_000.0,
            max_replays_per_ip: 2,
            max_dbn_records: 1_000_000,
            rate_per_sec: 20.0,
            rate_burst: 50.0,
        }
    }
}

impl Limits {
    /// Clamps a requested page size.
    #[must_use]
    pub fn page(&self, requested: Option<usize>) -> usize {
        requested
            .unwrap_or(self.default_rows)
            .clamp(1, self.max_rows)
    }
}

/// Where the archive lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveRoot {
    /// A local directory holding the archives (`<root>/<archive>/kind=...`).
    Local(PathBuf),
    /// An object store reachable by DuckDB's httpfs extension, which can
    /// list keys: `s3://bucket/prefix`, `r2://...`, `gcs://...`.
    ObjectStore(String),
    /// Plain HTTPS (a CDN or web seed). It cannot list, so files are found
    /// through the daily manifests.
    Https(String),
}

impl ArchiveRoot {
    /// Parses a path or URL.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        let trimmed = s.trim_end_matches('/');
        if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
            Self::Https(trimmed.to_owned())
        } else if ["s3://", "s3a://", "r2://", "gcs://", "gs://"]
            .iter()
            .any(|p| trimmed.starts_with(p))
        {
            Self::ObjectStore(trimmed.to_owned())
        } else {
            Self::Local(PathBuf::from(s))
        }
    }

    /// Whether DuckDB needs the httpfs extension for this root.
    #[must_use]
    pub const fn is_remote(&self) -> bool {
        !matches!(self, Self::Local(_))
    }

    /// Display form (paths and URLs).
    #[must_use]
    pub fn display(&self) -> String {
        match self {
            Self::Local(p) => p.display().to_string(),
            Self::ObjectStore(u) | Self::Https(u) => u.clone(),
        }
    }
}

/// Settings for remote archives.
#[derive(Debug, Clone, Default)]
pub struct RemoteSettings {
    /// Directory holding pre-downloaded DuckDB extensions (offline,
    /// reproducible deployments). When unset DuckDB installs httpfs from its
    /// extension repository on first use.
    pub extension_dir: Option<PathBuf>,
    /// S3 endpoint override (R2, MinIO), without scheme.
    pub s3_endpoint: Option<String>,
    /// S3 region.
    pub s3_region: Option<String>,
    /// Use path-style S3 URLs.
    pub s3_url_style_path: bool,
}

/// Full service configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Archive root.
    pub root: ArchiveRoot,
    /// Archive names under the root whose files are unioned (for example a
    /// live archive and a backfill archive).
    pub archives: Vec<String>,
    /// Optional registry file (`mesh_types::Registry` JSON). Without it the
    /// instrument list is derived from the partitions.
    pub registry_file: Option<PathBuf>,
    /// Public base URL used to build links in `/v0/manifests` (defaults to
    /// the archive root for HTTPS roots).
    pub public_base_url: Option<String>,
    /// Remote access settings.
    pub remote: RemoteSettings,
    /// Limits.
    pub limits: Limits,
    /// Trust the first `X-Forwarded-For` address for rate limiting (only
    /// behind a proxy that sets it).
    pub trust_forwarded_for: bool,
    /// Hosts the MCP streamable HTTP endpoint accepts (DNS-rebinding
    /// protection); empty accepts any.
    pub mcp_allowed_hosts: Vec<String>,
}

impl Config {
    /// A configuration for a local archive with default limits.
    #[must_use]
    pub fn local(root: impl Into<PathBuf>) -> Self {
        Self {
            root: ArchiveRoot::Local(root.into()),
            archives: vec![DEFAULT_ARCHIVE.to_owned()],
            registry_file: None,
            public_base_url: None,
            remote: RemoteSettings::default(),
            limits: Limits::default(),
            trust_forwarded_for: false,
            mcp_allowed_hosts: vec![
                "localhost".to_owned(),
                "127.0.0.1".to_owned(),
                "::1".to_owned(),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_roots() {
        assert_eq!(
            ArchiveRoot::parse("s3://b/p/"),
            ArchiveRoot::ObjectStore("s3://b/p".into())
        );
        assert_eq!(
            ArchiveRoot::parse("https://cdn.example/x"),
            ArchiveRoot::Https("https://cdn.example/x".into())
        );
        assert_eq!(
            ArchiveRoot::parse("/data/archive"),
            ArchiveRoot::Local("/data/archive".into())
        );
        assert!(ArchiveRoot::parse("r2://bucket").is_remote());
        assert!(!ArchiveRoot::parse("./a").is_remote());
    }

    #[test]
    fn page_is_clamped() {
        let l = Limits::default();
        assert_eq!(l.page(None), 1_000);
        assert_eq!(l.page(Some(0)), 1);
        assert_eq!(l.page(Some(1_000_000)), 10_000);
        let v = serde_json::to_value(&l).unwrap();
        assert_eq!(v["query_timeout"], 10_000);
    }
}
