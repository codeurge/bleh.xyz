//! Polite HTTP fetching: one request at a time with a minimum spacing,
//! conditional requests (`If-None-Match`, `If-Modified-Since`), and
//! exponential backoff with full jitter on transient failures (network
//! errors, `408`, `429`, `5xx`), honouring `Retry-After`. The user agent is
//! generic and names no person.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use reqwest::StatusCode;
use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED, RETRY_AFTER};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::metrics::Metrics;

/// Fetcher settings.
#[derive(Debug, Clone)]
pub struct HttpConfig {
    /// `User-Agent` header.
    pub user_agent: String,
    /// Minimum time between the starts of two requests.
    pub min_interval: Duration,
    /// Per-request timeout.
    pub timeout: Duration,
    /// Retries after the first attempt for transient failures.
    pub max_retries: u32,
    /// First backoff step.
    pub backoff_base: Duration,
    /// Largest backoff step.
    pub backoff_max: Duration,
    /// Largest body accepted, bytes.
    pub max_body: usize,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            user_agent: concat!("reference-pollers/", env!("CARGO_PKG_VERSION")).to_owned(),
            min_interval: Duration::from_millis(500),
            timeout: Duration::from_secs(300),
            max_retries: 5,
            backoff_base: Duration::from_secs(2),
            backoff_max: Duration::from_secs(600),
            max_body: 512 * 1024 * 1024,
        }
    }
}

/// Cache validators from an earlier response.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Validators {
    /// `ETag`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// `Last-Modified`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
}

impl Validators {
    /// `Last-Modified` as Unix milliseconds.
    #[must_use]
    pub fn last_modified_ms(&self) -> Option<i64> {
        let t = httpdate::parse_http_date(self.last_modified.as_deref()?).ok()?;
        let d = t.duration_since(std::time::UNIX_EPOCH).ok()?;
        i64::try_from(d.as_millis()).ok()
    }
}

/// Result of a conditional GET.
#[derive(Debug)]
pub enum Conditional {
    /// `304 Not Modified`.
    NotModified,
    /// A new body.
    Body {
        /// The body.
        body: Bytes,
        /// Validators for the next request.
        validators: Validators,
    },
}

/// Fetch failures.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// A status that is not retried (for example `403` or `404`).
    #[error("HTTP {status} for {url}")]
    Status {
        /// The status.
        status: StatusCode,
        /// The URL.
        url: String,
    },
    /// Retries exhausted on transient failures.
    #[error("giving up on {url} after {attempts} attempts: {last}")]
    Exhausted {
        /// The URL.
        url: String,
        /// Attempts made.
        attempts: u32,
        /// The last failure.
        last: String,
    },
    /// The body exceeded the size limit.
    #[error("body of {url} exceeds {limit} bytes")]
    TooLarge {
        /// The URL.
        url: String,
        /// The limit.
        limit: usize,
    },
    /// Building the client failed.
    #[error("client: {0}")]
    Client(#[from] reqwest::Error),
}

impl FetchError {
    /// True for `404 Not Found` and `410 Gone`.
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self, FetchError::Status { status, .. } if *status == StatusCode::NOT_FOUND || *status == StatusCode::GONE)
    }
}

/// HTTP client shared by the pollers.
#[derive(Debug, Clone)]
pub struct Fetcher {
    client: reqwest::Client,
    cfg: HttpConfig,
    next_slot: Arc<Mutex<Instant>>,
    metrics: Arc<Metrics>,
}

enum Attempt<T> {
    Done(T),
    Retry {
        reason: String,
        after: Option<Duration>,
    },
    Fail(FetchError),
}

fn retry_after(resp: &reqwest::Response) -> Option<Duration> {
    let v = resp.headers().get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(secs) = v.parse::<u64>() {
        return Some(Duration::from_secs(secs.min(3600)));
    }
    let when = httpdate::parse_http_date(v).ok()?;
    when.duration_since(std::time::SystemTime::now()).ok()
}

fn validators_of(resp: &reqwest::Response) -> Validators {
    let get = |h| {
        resp.headers()
            .get(h)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    Validators {
        etag: get(ETAG),
        last_modified: get(LAST_MODIFIED),
    }
}

fn is_transient(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::REQUEST_TIMEOUT
        || status.is_server_error()
}

impl Fetcher {
    /// Builds a fetcher.
    ///
    /// # Errors
    ///
    /// Fails if the TLS client cannot be built.
    pub fn new(cfg: HttpConfig, metrics: Arc<Metrics>) -> Result<Self, FetchError> {
        let client = reqwest::Client::builder()
            .user_agent(cfg.user_agent.clone())
            .timeout(cfg.timeout)
            .connect_timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self {
            client,
            cfg,
            next_slot: Arc::new(Mutex::new(Instant::now())),
            metrics,
        })
    }

    async fn pace(&self) {
        let mut slot = self.next_slot.lock().await;
        let now = Instant::now();
        if *slot > now {
            tokio::time::sleep_until(*slot).await;
        }
        *slot = Instant::now() + self.cfg.min_interval;
    }

    fn backoff(&self, attempt: u32, hint: Option<Duration>) -> Duration {
        let exp = self
            .cfg
            .backoff_base
            .saturating_mul(2u32.saturating_pow(attempt.min(20)))
            .min(self.cfg.backoff_max);
        let millis = u64::try_from(exp.as_millis()).unwrap_or(u64::MAX);
        let jittered = Duration::from_millis(fastrand::u64(0..=millis));
        hint.map_or(jittered, |h| h.max(jittered))
    }

    /// GET with validators from `prev`. `source` labels metrics.
    ///
    /// # Errors
    ///
    /// Fails on a non-retried status, an oversized body, or when retries
    /// run out.
    pub async fn get_conditional(
        &self,
        url: &str,
        source: &str,
        prev: &Validators,
    ) -> Result<Conditional, FetchError> {
        let mut attempt = 0u32;
        loop {
            self.pace().await;
            let result = self.try_once(url, prev).await;
            let outcome = match &result {
                Attempt::Done(_) => "ok",
                Attempt::Fail(_) => "error",
                Attempt::Retry { .. } => "retry",
            };
            self.metrics.inc(
                "ref_fetches_total",
                &[("source", source), ("outcome", outcome)],
            );
            match result {
                Attempt::Done(c) => {
                    if let Conditional::Body { body, .. } = &c {
                        self.metrics.inc_by(
                            "ref_fetch_bytes_total",
                            &[("source", source)],
                            body.len() as u64,
                        );
                    }
                    return Ok(c);
                }
                Attempt::Fail(e) => return Err(e),
                Attempt::Retry { reason, after } => {
                    if attempt >= self.cfg.max_retries {
                        return Err(FetchError::Exhausted {
                            url: url.to_owned(),
                            attempts: attempt + 1,
                            last: reason,
                        });
                    }
                    let wait = self.backoff(attempt, after);
                    tracing::warn!(%url, %reason, wait_ms = wait.as_millis() as u64, "transient fetch failure; backing off");
                    tokio::time::sleep(wait).await;
                    attempt += 1;
                }
            }
        }
    }

    async fn try_once(&self, url: &str, prev: &Validators) -> Attempt<Conditional> {
        let mut req = self.client.get(url);
        if let Some(e) = &prev.etag {
            req = req.header(IF_NONE_MATCH, e);
        }
        if let Some(lm) = &prev.last_modified {
            req = req.header(IF_MODIFIED_SINCE, lm);
        }
        let mut resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                return Attempt::Retry {
                    reason: e.to_string(),
                    after: None,
                };
            }
        };
        let status = resp.status();
        if status == StatusCode::NOT_MODIFIED {
            return Attempt::Done(Conditional::NotModified);
        }
        if is_transient(status) {
            return Attempt::Retry {
                reason: format!("HTTP {status}"),
                after: retry_after(&resp),
            };
        }
        if !status.is_success() {
            return Attempt::Fail(FetchError::Status {
                status,
                url: url.to_owned(),
            });
        }
        let validators = validators_of(&resp);
        let mut body = Vec::new();
        loop {
            match resp.chunk().await {
                Ok(Some(c)) => {
                    if body.len() + c.len() > self.cfg.max_body {
                        return Attempt::Fail(FetchError::TooLarge {
                            url: url.to_owned(),
                            limit: self.cfg.max_body,
                        });
                    }
                    body.extend_from_slice(&c);
                }
                Ok(None) => break,
                Err(e) => {
                    return Attempt::Retry {
                        reason: e.to_string(),
                        after: None,
                    };
                }
            }
        }
        Attempt::Done(Conditional::Body {
            body: Bytes::from(body),
            validators,
        })
    }

    /// Plain GET returning the body and validators.
    ///
    /// # Errors
    ///
    /// As [`Fetcher::get_conditional`].
    pub async fn get(&self, url: &str, source: &str) -> Result<(Bytes, Validators), FetchError> {
        match self
            .get_conditional(url, source, &Validators::default())
            .await?
        {
            Conditional::Body { body, validators } => Ok((body, validators)),
            Conditional::NotModified => Err(FetchError::Status {
                status: StatusCode::NOT_MODIFIED,
                url: url.to_owned(),
            }),
        }
    }
}
