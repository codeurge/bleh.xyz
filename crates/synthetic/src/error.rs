//! Errors.

/// An engine error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The registry file is unreadable or inconsistent.
    #[error("registry: {0}")]
    Registry(String),
    /// An input record is malformed.
    #[error("input: {0}")]
    Input(String),
    /// Reading or writing failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
