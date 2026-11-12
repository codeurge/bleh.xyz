//! One error type for every surface (JSON API, compatibility layers, MCP).
//!
//! JSON responses carry `{"error": {"code": "...", "message": "..."}}` with
//! a stable, lowercase snake_case `code` and the HTTP status that goes with
//! it.

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// Stable error codes. Clients branch on these, never on messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Malformed or missing parameter.
    BadRequest,
    /// No such route or resource.
    NotFound,
    /// The instrument is not in the registry or the archive.
    UnknownInstrument,
    /// The bar interval (or other variant) is not offered.
    UnsupportedInterval,
    /// The schema, encoding or kind is not offered.
    Unsupported,
    /// The time range exceeds the configured maximum.
    RangeTooLarge,
    /// The pagination cursor is malformed or belongs to another query.
    InvalidCursor,
    /// Per-IP request rate exceeded.
    RateLimited,
    /// Too many queries are running; retry later.
    Overloaded,
    /// The query exceeded its time budget.
    QueryTimeout,
    /// The query exceeded the memory cap or a result-size cap.
    TooLarge,
    /// The archive could not be read.
    ArchiveUnavailable,
    /// Anything else.
    Internal,
}

impl ErrorCode {
    /// HTTP status for the code.
    #[must_use]
    pub const fn status(self) -> StatusCode {
        match self {
            Self::BadRequest
            | Self::UnsupportedInterval
            | Self::Unsupported
            | Self::RangeTooLarge
            | Self::InvalidCursor => StatusCode::BAD_REQUEST,
            Self::NotFound | Self::UnknownInstrument => StatusCode::NOT_FOUND,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::Overloaded | Self::ArchiveUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::QueryTimeout => StatusCode::GATEWAY_TIMEOUT,
            Self::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Text form, as serialized.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BadRequest => "bad_request",
            Self::NotFound => "not_found",
            Self::UnknownInstrument => "unknown_instrument",
            Self::UnsupportedInterval => "unsupported_interval",
            Self::Unsupported => "unsupported",
            Self::RangeTooLarge => "range_too_large",
            Self::InvalidCursor => "invalid_cursor",
            Self::RateLimited => "rate_limited",
            Self::Overloaded => "overloaded",
            Self::QueryTimeout => "query_timeout",
            Self::TooLarge => "too_large",
            Self::ArchiveUnavailable => "archive_unavailable",
            Self::Internal => "internal",
        }
    }
}

/// An API error: a code and a human-readable message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}: {message}", code.as_str())]
pub struct ApiError {
    /// Stable code.
    pub code: ErrorCode,
    /// Explanation for humans.
    pub message: String,
    /// Seconds to wait before retrying, for rate limits.
    pub retry_after: Option<u64>,
}

impl ApiError {
    /// A new error.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retry_after: None,
        }
    }

    /// `bad_request`.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::BadRequest, message)
    }

    /// `internal`; the detail is logged, not returned.
    pub fn internal(detail: impl std::fmt::Display) -> Self {
        tracing::error!(error = %detail, "internal error");
        Self::new(ErrorCode::Internal, "internal error")
    }

    /// The JSON body.
    #[must_use]
    pub fn body(&self) -> serde_json::Value {
        serde_json::json!({ "error": { "code": self.code, "message": self.message } })
    }
}

/// Result alias.
pub type ApiResult<T> = Result<T, ApiError>;

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut resp = (self.code.status(), axum::Json(self.body())).into_response();
        if let Some(secs) = self.retry_after
            && let Ok(v) = HeaderValue::from_str(&secs.to_string())
        {
            resp.headers_mut().insert(header::RETRY_AFTER, v);
        }
        resp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_shape_and_status() {
        let e = ApiError::new(ErrorCode::RangeTooLarge, "too wide");
        assert_eq!(
            e.body(),
            serde_json::json!({"error": {"code": "range_too_large", "message": "too wide"}})
        );
        assert_eq!(e.code.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            ErrorCode::QueryTimeout.status(),
            StatusCode::GATEWAY_TIMEOUT
        );
        assert_eq!(ErrorCode::RateLimited.as_str(), "rate_limited");
        let mut r = ApiError::new(ErrorCode::RateLimited, "slow down");
        r.retry_after = Some(2);
        let resp = r.into_response();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(resp.headers()[header::RETRY_AFTER], "2");
    }
}
