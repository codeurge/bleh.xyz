//! The crate's error type.

use std::path::PathBuf;

/// Boxed error from an injected component (filter, parser, signer).
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Errors from archive operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A filesystem operation failed.
    #[error("{path}: {source}")]
    Io {
        /// File or directory involved.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// JSON encoding or decoding failed.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// Canonical encoding failed.
    #[error("canonical encoding: {0}")]
    Canonical(#[from] mesh_types::hash::CanonicalError),
    /// A manifest rule or signature check failed.
    #[error(transparent)]
    Manifest(#[from] mesh_types::manifest::ManifestError),
    /// Parquet encoding or decoding failed.
    #[error("parquet: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),
    /// Arrow array construction failed.
    #[error("arrow: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    /// Object storage failed.
    #[error("object store: {0}")]
    Store(#[from] object_store::Error),
    /// An HTTP request failed or returned an unexpected status.
    #[error("http: {0}")]
    Http(String),
    /// Input data is malformed or violates the archive layout.
    #[error("invalid: {0}")]
    Invalid(String),
    /// A write-once object already exists with different content.
    #[error(
        "write-once conflict at {key}: existing object differs (existing sha256 {existing}, new {new})"
    )]
    Conflict {
        /// Object key or path.
        key: String,
        /// SHA-256 of the existing object.
        existing: String,
        /// SHA-256 of the rejected content.
        new: String,
    },
    /// An injected component (filter, parser, signer, submitter) failed.
    #[error("{context}: {source}")]
    Component {
        /// Which component.
        context: &'static str,
        /// Its error.
        source: BoxError,
    },
    /// An OpenTimestamps proof is malformed or does not verify.
    #[error("opentimestamps: {0}")]
    Ots(String),
    /// An Ethereum anchor could not be built, submitted or verified.
    #[error("ethereum anchor: {0}")]
    Evm(String),
    /// A required hook is not wired into this build.
    #[error("{0}")]
    NotWired(&'static str),
}

/// Result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// Builds an [`Error::Invalid`].
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }

    /// Builds an [`Error::Io`] for `path`.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

/// Adds a path to I/O errors.
pub(crate) trait IoContext<T> {
    /// Maps an I/O error to [`Error::Io`] naming `path`.
    fn at(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T> {
        self.map_err(|e| Error::io(path, e))
    }
}
