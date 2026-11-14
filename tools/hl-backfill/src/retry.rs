//! Retries with exponential backoff.

use std::future::Future;
use std::time::Duration;

/// Backoff policy: attempt `n` (from 1) waits `base * 2^(n-1)`, capped at
/// `max_delay`, before the next attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total attempts, including the first (at least 1).
    pub max_attempts: u32,
    /// Delay after the first failure.
    pub base_delay: Duration,
    /// Largest delay between attempts.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 6,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    /// No waiting between attempts (tests).
    #[must_use]
    pub const fn immediate(max_attempts: u32) -> Self {
        Self {
            max_attempts,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
        }
    }

    /// Delay after failed attempt `attempt` (1-based).
    #[must_use]
    pub fn delay(&self, attempt: u32) -> Duration {
        let factor = 1u32
            .checked_shl(attempt.saturating_sub(1))
            .unwrap_or(u32::MAX);
        self.base_delay
            .checked_mul(factor)
            .unwrap_or(self.max_delay)
            .min(self.max_delay)
    }
}

/// Outcome of a retried operation.
#[derive(Debug)]
pub struct Retried<T> {
    /// The final result.
    pub value: T,
    /// Attempts made.
    pub attempts: u32,
}

/// Runs `op` until it succeeds, `is_transient` says an error is permanent,
/// or attempts run out. Returns the last error on failure.
///
/// # Errors
///
/// The last error from `op`.
pub async fn retry<T, E, F, Fut>(
    policy: RetryPolicy,
    mut is_transient: impl FnMut(&E) -> bool,
    mut op: F,
) -> Result<Retried<T>, (E, u32)>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let max = policy.max_attempts.max(1);
    let mut attempt = 1;
    loop {
        match op().await {
            Ok(value) => {
                return Ok(Retried {
                    value,
                    attempts: attempt,
                });
            }
            Err(e) if attempt < max && is_transient(&e) => {
                let delay = policy.delay(attempt);
                tracing::debug!(attempt, ?delay, "transient failure, retrying");
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                attempt += 1;
            }
            Err(e) => return Err((e, attempt)),
        }
    }
}

/// True for object-store errors worth retrying. Missing objects,
/// permission errors, bad configuration and requests that cannot succeed
/// as sent (see [`permanent_cause`]) are permanent.
#[must_use]
pub fn object_store_transient(e: &object_store::Error) -> bool {
    let permanent_kind = matches!(
        e,
        object_store::Error::NotFound { .. }
            | object_store::Error::PermissionDenied { .. }
            | object_store::Error::Unauthenticated { .. }
            | object_store::Error::InvalidPath { .. }
            | object_store::Error::NotSupported { .. }
            | object_store::Error::NotImplemented { .. }
            | object_store::Error::UnknownConfigurationKey { .. }
            | object_store::Error::Precondition { .. }
    );
    !permanent_kind && !permanent_cause(e)
}

/// True if anything in the error's source chain will not change on a
/// retry: a request the HTTP client could not build (for example a plain
/// HTTP endpoint the store does not allow, reported as an unknown HTTP
/// error), a redirect without a location (wrong region), or a 4xx status
/// other than 408 and 429. Statuses the store maps to typed errors (401,
/// 403, 404, 412) are handled by [`object_store_transient`] itself.
fn permanent_cause(e: &(dyn std::error::Error + 'static)) -> bool {
    use object_store::client::{HttpError, HttpErrorKind};
    let mut cause = Some(e);
    while let Some(err) = cause {
        if err
            .downcast_ref::<HttpError>()
            .is_some_and(|h| h.kind() == HttpErrorKind::Unknown)
        {
            return true;
        }
        let msg = err.to_string();
        if msg.contains("Received redirect without LOCATION") {
            return true;
        }
        if let Some(code) = status_code(&msg)
            && (400..500).contains(&code)
            && code != 408
            && code != 429
        {
            return true;
        }
        cause = err.source();
    }
    false
}

/// The HTTP status in an object-store "non-2xx status code: NNN" message.
fn status_code(msg: &str) -> Option<u16> {
    let (_, rest) = msg.split_once("status code: ")?;
    rest.get(..3)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn delays_grow_and_cap() {
        let p = RetryPolicy {
            max_attempts: 10,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(1000),
        };
        assert_eq!(p.delay(1), Duration::from_millis(100));
        assert_eq!(p.delay(2), Duration::from_millis(200));
        assert_eq!(p.delay(4), Duration::from_millis(800));
        assert_eq!(p.delay(5), Duration::from_millis(1000));
        assert_eq!(p.delay(64), Duration::from_millis(1000));
    }

    #[tokio::test]
    async fn retries_transient_then_succeeds() {
        let calls = Cell::new(0);
        let r = retry(
            RetryPolicy::immediate(5),
            |_: &&str| true,
            || {
                calls.set(calls.get() + 1);
                let n = calls.get();
                async move { if n < 3 { Err("boom") } else { Ok(n) } }
            },
        )
        .await
        .unwrap();
        assert_eq!(r.value, 3);
        assert_eq!(r.attempts, 3);
    }

    #[test]
    fn classifies_store_errors() {
        use object_store::client::{HttpError, HttpErrorKind};
        let generic =
            |source: Box<dyn std::error::Error + Send + Sync>| object_store::Error::Generic {
                store: "S3",
                source,
            };
        let io = |k| std::io::Error::new(k, "x");
        // A request the client could not build: never retried.
        let builder = HttpError::new(HttpErrorKind::Unknown, io(std::io::ErrorKind::Other));
        assert!(!object_store_transient(&generic(Box::new(builder))));
        // Connection trouble and timeouts: retried.
        for kind in [HttpErrorKind::Connect, HttpErrorKind::Timeout] {
            let e = HttpError::new(kind, io(std::io::ErrorKind::TimedOut));
            assert!(object_store_transient(&generic(Box::new(e))), "{kind:?}");
        }
        // 4xx other than 408/429: permanent; 5xx, 408, 429: transient.
        let status = |code: &str| {
            generic(format!("Server returned non-2xx status code: {code}: body").into())
        };
        assert!(!object_store_transient(&status("400 Bad Request")));
        assert!(!object_store_transient(&status("405 Method Not Allowed")));
        assert!(object_store_transient(&status("408 Request Timeout")));
        assert!(object_store_transient(&status("429 Too Many Requests")));
        assert!(object_store_transient(&status("503 Service Unavailable")));
        assert!(!object_store_transient(
            &object_store::Error::PermissionDenied {
                path: "p".into(),
                source: "AccessDenied".into(),
            }
        ));
        assert!(object_store_transient(&generic("connection reset".into())));
    }

    #[tokio::test]
    async fn stops_on_permanent_and_on_exhaustion() {
        let r = retry(
            RetryPolicy::immediate(5),
            |_: &&str| false,
            || async { Err::<(), _>("perm") },
        )
        .await;
        assert_eq!(r.unwrap_err(), ("perm", 1));
        let r = retry(
            RetryPolicy::immediate(3),
            |_: &&str| true,
            || async { Err::<(), _>("again") },
        )
        .await;
        assert_eq!(r.unwrap_err(), ("again", 3));
    }
}
