//! Building object stores from the environment.
//!
//! Credentials are read only from the environment (or the instance's
//! role), never from arguments, and are never logged.
//!
//! - **Source buckets** (AWS, requester pays): the standard AWS variables
//!   (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`), or
//!   the instance or container role when they are unset. To use a named
//!   profile, export it first:
//!   `eval "$(aws configure export-credentials --profile NAME --format env)"`.
//!   Every request carries `x-amz-request-payer: requester`.
//! - **Raw tier and filtered raw** (any S3-compatible store, such as
//!   Cloudflare R2): `s3://bucket[/prefix]` URLs, with
//!   `BLEH_RAW_ENDPOINT`, `BLEH_RAW_ACCESS_KEY_ID`,
//!   `BLEH_RAW_SECRET_ACCESS_KEY` and optional `BLEH_RAW_REGION` (default
//!   `auto`, as R2 expects). `file:///dir` URLs use a local directory.

use std::sync::Arc;
use std::time::Duration;

use object_store::aws::AmazonS3Builder;
use object_store::local::LocalFileSystem;
use object_store::{BackoffConfig, ObjectStore, RetryConfig};

use crate::buckets::Bucket;

/// Errors building a store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The URL is not `s3://bucket[/prefix]` or `file:///dir`.
    #[error("unsupported store URL {0:?}: use s3://bucket[/prefix] or file:///dir")]
    Url(String),
    /// A required environment variable is missing (its value is never
    /// printed).
    #[error("environment variable {0} is not set")]
    MissingEnv(&'static str),
    /// The store could not be built.
    #[error(transparent)]
    Build(#[from] object_store::Error),
    /// A local directory could not be created.
    #[error("creating {path}: {source}")]
    Io {
        /// The directory.
        path: String,
        /// The error.
        source: std::io::Error,
    },
}

/// The store's own retries are kept short: the copier retries whole
/// objects with its own backoff.
fn retry() -> RetryConfig {
    RetryConfig {
        backoff: BackoffConfig::default(),
        max_retries: 2,
        retry_timeout: Duration::from_secs(120),
    }
}

/// A requester-pays store for one of Hyperliquid's buckets.
///
/// # Errors
///
/// Fails if the store cannot be built.
pub fn source_store(bucket: Bucket, region: &str) -> Result<Arc<dyn ObjectStore>, StoreError> {
    let store = AmazonS3Builder::from_env()
        .with_bucket_name(bucket.name())
        .with_region(region)
        .with_request_payer(true)
        .with_retry(retry())
        .build()?;
    Ok(Arc::new(store))
}

/// A parsed destination URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreUrl {
    /// `s3://bucket[/prefix]`.
    S3 {
        /// Bucket.
        bucket: String,
        /// Key prefix (may be empty).
        prefix: String,
    },
    /// `file:///dir`.
    File {
        /// Directory.
        dir: String,
    },
}

impl StoreUrl {
    /// Parses a URL.
    ///
    /// # Errors
    ///
    /// Fails on other schemes or an empty bucket or directory.
    pub fn parse(url: &str) -> Result<Self, StoreError> {
        if let Some(rest) = url.strip_prefix("s3://") {
            let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
            if bucket.is_empty() {
                return Err(StoreError::Url(url.into()));
            }
            Ok(Self::S3 {
                bucket: bucket.into(),
                prefix: prefix.trim_matches('/').into(),
            })
        } else if let Some(dir) = url.strip_prefix("file://") {
            if dir.is_empty() {
                return Err(StoreError::Url(url.into()));
            }
            Ok(Self::File { dir: dir.into() })
        } else {
            Err(StoreError::Url(url.into()))
        }
    }

    /// The key prefix inside the store (empty for local directories).
    #[must_use]
    pub fn prefix(&self) -> &str {
        match self {
            Self::S3 { prefix, .. } => prefix,
            Self::File { .. } => "",
        }
    }
}

fn env(name: &'static str) -> Result<String, StoreError> {
    std::env::var(name).map_err(|_| StoreError::MissingEnv(name))
}

/// Builds the project's own store (raw tier or filtered raw) for a URL.
///
/// # Errors
///
/// Fails on a bad URL, missing credentials, or a store build failure.
pub fn project_store(url: &StoreUrl) -> Result<Arc<dyn ObjectStore>, StoreError> {
    match url {
        StoreUrl::S3 { bucket, .. } => {
            let store = AmazonS3Builder::new()
                .with_bucket_name(bucket)
                .with_endpoint(env("BLEH_RAW_ENDPOINT")?)
                .with_access_key_id(env("BLEH_RAW_ACCESS_KEY_ID")?)
                .with_secret_access_key(env("BLEH_RAW_SECRET_ACCESS_KEY")?)
                .with_region(std::env::var("BLEH_RAW_REGION").unwrap_or_else(|_| "auto".into()))
                .with_retry(retry())
                .build()?;
            Ok(Arc::new(store))
        }
        StoreUrl::File { dir } => {
            std::fs::create_dir_all(dir).map_err(|source| StoreError::Io {
                path: dir.clone(),
                source,
            })?;
            Ok(Arc::new(LocalFileSystem::new_with_prefix(dir)?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_urls() {
        assert_eq!(
            StoreUrl::parse("s3://raw-tier/hl/archive/").unwrap(),
            StoreUrl::S3 {
                bucket: "raw-tier".into(),
                prefix: "hl/archive".into()
            }
        );
        assert_eq!(StoreUrl::parse("s3://raw-tier").unwrap().prefix(), "");
        assert!(matches!(
            StoreUrl::parse("file:///tmp/x").unwrap(),
            StoreUrl::File { .. }
        ));
        for bad in ["s3://", "file://", "gs://b", "/tmp/x"] {
            assert!(StoreUrl::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn local_store_is_created() {
        let dir = crate::testutil::tempdir().join("sub");
        let url = StoreUrl::parse(&format!("file://{}", dir.display())).unwrap();
        project_store(&url).unwrap();
        assert!(dir.is_dir());
    }

    #[test]
    fn source_store_builds_without_network() {
        // Building never contacts AWS; no request is made in tests.
        assert!(source_store(Bucket::NodeData, "us-east-1").is_ok());
    }
}
