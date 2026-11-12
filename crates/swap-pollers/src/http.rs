//! Polite HTTP fetching: one request at a time with a minimum spacing,
//! conditional requests, and exponential backoff with full jitter on
//! transient failures (network errors, `429`, `5xx`), honouring
//! `Retry-After`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use reqwest::StatusCode;
use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED, RETRY_AFTER};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
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
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            user_agent: concat!("swap-pollers/", env!("CARGO_PKG_VERSION")).to_string(),
            min_interval: Duration::from_millis(250),
            timeout: Duration::from_secs(120),
            max_retries: 5,
            backoff_base: Duration::from_secs(1),
            backoff_max: Duration::from_secs(300),
        }
    }
}

/// Cache validators from an earlier response.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Validators {
    /// `ETag`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// `Last-Modified`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
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

/// A completed download to a file.
#[derive(Debug, Clone)]
pub struct Downloaded {
    /// Bytes written.
    pub bytes: u64,
    /// SHA-256 of the body, lowercase hex.
    pub sha256: String,
    /// Response validators.
    pub validators: Validators,
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
    /// Writing the download failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Building the client failed.
    #[error("client: {0}")]
    Client(#[from] reqwest::Error),
}

impl FetchError {
    /// The HTTP status, for non-retried status failures.
    #[must_use]
    pub fn status(&self) -> Option<StatusCode> {
        match self {
            FetchError::Status { status, .. } => Some(*status),
            _ => None,
        }
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
            .map(str::to_string)
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

    /// Waits until the next request slot.
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

    async fn with_retries<T, F, Fut>(
        &self,
        url: &str,
        target: &str,
        mut op: F,
    ) -> Result<T, FetchError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Attempt<T>>,
    {
        let mut attempt = 0u32;
        loop {
            self.pace().await;
            match op().await {
                Attempt::Done(v) => {
                    self.metrics.inc(
                        "swap_fetches_total",
                        &[("target", target), ("outcome", "ok")],
                    );
                    return Ok(v);
                }
                Attempt::Fail(e) => {
                    self.metrics.inc(
                        "swap_fetches_total",
                        &[("target", target), ("outcome", "error")],
                    );
                    return Err(e);
                }
                Attempt::Retry { reason, after } => {
                    self.metrics.inc(
                        "swap_fetches_total",
                        &[("target", target), ("outcome", "retry")],
                    );
                    if attempt >= self.cfg.max_retries {
                        return Err(FetchError::Exhausted {
                            url: url.to_string(),
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

    /// GET with `If-None-Match` / `If-Modified-Since` from `prev`.
    ///
    /// # Errors
    ///
    /// Fails on a non-retried status or when retries run out.
    pub async fn get_conditional(
        &self,
        url: &str,
        target: &str,
        prev: &Validators,
    ) -> Result<Conditional, FetchError> {
        self.with_retries(url, target, || async {
            let mut req = self.client.get(url);
            if let Some(e) = &prev.etag {
                req = req.header(IF_NONE_MATCH, e);
            }
            if let Some(lm) = &prev.last_modified {
                req = req.header(IF_MODIFIED_SINCE, lm);
            }
            let resp = match req.send().await {
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
                    url: url.to_string(),
                });
            }
            let validators = validators_of(&resp);
            match resp.bytes().await {
                Ok(body) => {
                    self.metrics.inc_by(
                        "swap_fetch_bytes_total",
                        &[("target", target)],
                        body.len() as u64,
                    );
                    Attempt::Done(Conditional::Body { body, validators })
                }
                Err(e) => Attempt::Retry {
                    reason: e.to_string(),
                    after: None,
                },
            }
        })
        .await
    }

    /// Plain GET returning the body.
    ///
    /// # Errors
    ///
    /// As [`Fetcher::get_conditional`].
    pub async fn get(&self, url: &str, target: &str) -> Result<Bytes, FetchError> {
        match self
            .get_conditional(url, target, &Validators::default())
            .await?
        {
            Conditional::Body { body, .. } => Ok(body),
            Conditional::NotModified => Err(FetchError::Status {
                status: StatusCode::NOT_MODIFIED,
                url: url.to_string(),
            }),
        }
    }

    /// Streams a GET body to `dest` verbatim: written to `dest.part`,
    /// flushed to disk, then renamed, so `dest` exists only complete.
    ///
    /// # Errors
    ///
    /// As [`Fetcher::get_conditional`], plus I/O failures.
    pub async fn download_to(
        &self,
        url: &str,
        target: &str,
        dest: &Path,
    ) -> Result<Downloaded, FetchError> {
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let part = dest.with_extension(match dest.extension() {
            Some(e) => format!("{}.part", e.to_string_lossy()),
            None => "part".to_string(),
        });
        let done = self
            .with_retries(url, target, || async {
                let resp = match self.client.get(url).send().await {
                    Ok(r) => r,
                    Err(e) => {
                        return Attempt::Retry {
                            reason: e.to_string(),
                            after: None,
                        };
                    }
                };
                let status = resp.status();
                if is_transient(status) {
                    return Attempt::Retry {
                        reason: format!("HTTP {status}"),
                        after: retry_after(&resp),
                    };
                }
                if !status.is_success() {
                    return Attempt::Fail(FetchError::Status {
                        status,
                        url: url.to_string(),
                    });
                }
                let validators = validators_of(&resp);
                match stream_body(resp, &part).await {
                    Ok((bytes, sha256)) => Attempt::Done(Downloaded {
                        bytes,
                        sha256,
                        validators,
                    }),
                    Err(StreamError::Io(e)) => Attempt::Fail(FetchError::Io(e)),
                    Err(StreamError::Net(e)) => Attempt::Retry {
                        reason: e,
                        after: None,
                    },
                }
            })
            .await;
        match done {
            Ok(d) => {
                tokio::fs::rename(&part, dest).await?;
                sync_dir(dest).await;
                self.metrics
                    .inc_by("swap_fetch_bytes_total", &[("target", target)], d.bytes);
                Ok(d)
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(&part).await;
                Err(e)
            }
        }
    }
}

enum StreamError {
    Io(std::io::Error),
    Net(String),
}

async fn stream_body(
    mut resp: reqwest::Response,
    part: &Path,
) -> Result<(u64, String), StreamError> {
    let mut file = tokio::fs::File::create(part)
        .await
        .map_err(StreamError::Io)?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                hasher.update(&chunk);
                total += chunk.len() as u64;
                file.write_all(&chunk).await.map_err(StreamError::Io)?;
            }
            Ok(None) => break,
            Err(e) => return Err(StreamError::Net(e.to_string())),
        }
    }
    file.flush().await.map_err(StreamError::Io)?;
    file.sync_all().await.map_err(StreamError::Io)?;
    let digest = hasher.finalize();
    Ok((total, digest.iter().map(|b| format!("{b:02x}")).collect()))
}

/// Best-effort fsync of the directory holding `path`, so a rename is
/// durable.
pub(crate) async fn sync_dir(path: &Path) {
    if let Some(parent) = path.parent()
        && let Ok(d) = tokio::fs::File::open(parent).await
    {
        let _ = d.sync_all().await;
    }
}
