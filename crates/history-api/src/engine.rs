//! The DuckDB query engine: a small connection pool with per-query
//! timeouts, a global memory cap, a concurrency cap and locked
//! configuration.
//!
//! Every SQL string is built by this crate from validated inputs; user
//! values travel as bound parameters, and file paths are quoted literals of
//! paths the resolver produced. As defence in depth the engine is
//! configured once at start-up and then `lock_configuration` is set, so no
//! statement can change limits, load extensions or attach databases.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use duckdb::types::Value;
use duckdb::{Connection, Row};
use tokio::sync::Semaphore;

use crate::config::{ArchiveRoot, Config, Limits};
use crate::error::{ApiError, ApiResult, ErrorCode};
use crate::source::sql_literal;

/// The engine. Cheap to clone (shared state behind an `Arc`).
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("limits", &self.inner.limits)
            .finish_non_exhaustive()
    }
}

struct Inner {
    base: Mutex<Connection>,
    pool: Mutex<Vec<Connection>>,
    slots: Semaphore,
    limits: Limits,
    version: String,
}

/// Errors from opening the engine.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// DuckDB refused a setting or extension.
    #[error("duckdb: {0}")]
    Duckdb(#[from] duckdb::Error),
}

impl Engine {
    /// Opens an in-memory DuckDB database configured for `config`.
    ///
    /// # Errors
    ///
    /// Fails if DuckDB cannot apply a setting or load httpfs for a remote
    /// root.
    pub fn open(config: &Config) -> Result<Self, EngineError> {
        let conn = Connection::open_in_memory()?;
        let l = &config.limits;
        let mut setup = vec![
            format!("SET memory_limit = '{}B'", l.memory_limit_bytes),
            format!("SET threads = {}", l.threads.max(1)),
            // No spilling: the memory cap is a hard cap.
            "SET temp_directory = ''".to_owned(),
            "SET enable_progress_bar = false".to_owned(),
            "SET TimeZone = 'UTC'".to_owned(),
        ];
        setup.retain(|s| !s.contains("TimeZone"));
        match &config.root {
            ArchiveRoot::Local(path) => {
                setup.push("SET autoinstall_known_extensions = false".to_owned());
                setup.push("SET autoload_known_extensions = false".to_owned());
                let dir = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
                let mut d = dir.display().to_string();
                if !d.ends_with('/') {
                    d.push('/');
                }
                setup.push(format!("SET allowed_directories = [{}]", sql_literal(&d)));
                setup.push("SET enable_external_access = false".to_owned());
            }
            ArchiveRoot::ObjectStore(_) | ArchiveRoot::Https(_) => {
                if let Some(dir) = &config.remote.extension_dir {
                    setup.push(format!(
                        "SET extension_directory = {}",
                        sql_literal(&dir.display().to_string())
                    ));
                    setup.push("SET autoinstall_known_extensions = false".to_owned());
                } else {
                    setup.push("INSTALL httpfs".to_owned());
                }
                setup.push("LOAD httpfs".to_owned());
                if let Some(e) = &config.remote.s3_endpoint {
                    setup.push(format!("SET s3_endpoint = {}", sql_literal(e)));
                }
                if let Some(r) = &config.remote.s3_region {
                    setup.push(format!("SET s3_region = {}", sql_literal(r)));
                }
                if config.remote.s3_url_style_path {
                    setup.push("SET s3_url_style = 'path'".to_owned());
                }
                if let (Ok(k), Ok(s)) = (
                    std::env::var("AWS_ACCESS_KEY_ID"),
                    std::env::var("AWS_SECRET_ACCESS_KEY"),
                ) {
                    setup.push(format!("SET s3_access_key_id = {}", sql_literal(&k)));
                    setup.push(format!("SET s3_secret_access_key = {}", sql_literal(&s)));
                }
                // Remote roots never read the local disk through SQL.
                setup.push("SET disabled_filesystems = 'LocalFileSystem'".to_owned());
            }
        }
        setup.push("SET lock_configuration = true".to_owned());
        for stmt in &setup {
            conn.execute_batch(stmt)?;
        }
        let version: String = conn
            .query_row("SELECT library_version FROM pragma_version()", [], |r| {
                r.get(0)
            })
            .unwrap_or_else(|_| "unknown".to_owned());
        Ok(Self {
            inner: Arc::new(Inner {
                base: Mutex::new(conn),
                pool: Mutex::new(Vec::new()),
                slots: Semaphore::new(l.max_concurrent_queries.max(1)),
                limits: l.clone(),
                version,
            }),
        })
    }

    /// DuckDB library version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.inner.version
    }

    /// The limits in force.
    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.inner.limits
    }

    /// Queries waiting or running (for metrics).
    #[must_use]
    pub fn busy_slots(&self) -> usize {
        self.inner
            .limits
            .max_concurrent_queries
            .max(1)
            .saturating_sub(self.inner.slots.available_permits())
    }

    fn checkout(&self) -> ApiResult<Connection> {
        if let Some(c) = self.inner.pool.lock().map_err(ApiError::internal)?.pop() {
            return Ok(c);
        }
        self.inner
            .base
            .lock()
            .map_err(ApiError::internal)?
            .try_clone()
            .map_err(ApiError::internal)
    }

    fn checkin(&self, conn: Connection) {
        if let Ok(mut pool) = self.inner.pool.lock()
            && pool.len() < self.inner.limits.max_concurrent_queries.max(1)
        {
            pool.push(conn);
        }
    }

    /// Runs `sql` with bound `params`, mapping each row with `map`, under
    /// the concurrency cap and the query timeout.
    ///
    /// # Errors
    ///
    /// `overloaded` when no slot frees up within the queue timeout,
    /// `query_timeout` when the query runs too long, `too_large` when
    /// DuckDB runs out of its memory budget, `archive_unavailable` on I/O
    /// errors, `internal` otherwise.
    pub async fn query<T, F>(&self, sql: String, params: Vec<Value>, map: F) -> ApiResult<Vec<T>>
    where
        T: Send + 'static,
        F: FnMut(&Row<'_>) -> duckdb::Result<T> + Send + 'static,
    {
        self.query_with_timeout(sql, params, map, self.inner.limits.query_timeout)
            .await
    }

    /// As [`Engine::query`] with an explicit timeout.
    ///
    /// # Errors
    ///
    /// As [`Engine::query`].
    pub async fn query_with_timeout<T, F>(
        &self,
        sql: String,
        params: Vec<Value>,
        mut map: F,
        timeout: Duration,
    ) -> ApiResult<Vec<T>>
    where
        T: Send + 'static,
        F: FnMut(&Row<'_>) -> duckdb::Result<T> + Send + 'static,
    {
        let permit =
            tokio::time::timeout(self.inner.limits.queue_timeout, self.inner.slots.acquire())
                .await
                .map_err(|_| {
                    let mut e = ApiError::new(
                        ErrorCode::Overloaded,
                        "too many queries are running; retry shortly",
                    );
                    e.retry_after = Some(1);
                    e
                })?
                .map_err(ApiError::internal)?;
        let conn = self.checkout()?;
        let interrupt = conn.interrupt_handle();
        let started = std::time::Instant::now();
        let task = tokio::task::spawn_blocking(move || {
            let result = (|| -> duckdb::Result<Vec<T>> {
                let mut stmt = conn.prepare(&sql)?;
                let rows = stmt.query_map(duckdb::params_from_iter(params), |r| map(r))?;
                rows.collect()
            })();
            (conn, result)
        });
        tokio::pin!(task);
        let outcome = tokio::select! {
            joined = &mut task => Some(joined),
            () = tokio::time::sleep(timeout) => None,
        };
        let (timed_out, joined) = match outcome {
            Some(j) => (false, j),
            None => {
                // An interrupt sent before DuckDB starts executing is lost,
                // so keep interrupting until the task ends.
                loop {
                    interrupt.interrupt();
                    tokio::select! {
                        joined = &mut task => break (true, joined),
                        () = tokio::time::sleep(Duration::from_millis(20)) => {}
                    }
                }
            }
        };
        drop(permit);
        crate::metrics::global().observe_query(started.elapsed());
        let (conn, result) = joined.map_err(ApiError::internal)?;
        self.checkin(conn);
        match result {
            _ if timed_out => Err(ApiError::new(
                ErrorCode::QueryTimeout,
                format!(
                    "query exceeded the {} ms budget; narrow the range or lower the limit",
                    timeout.as_millis()
                ),
            )),
            Ok(rows) => Ok(rows),
            Err(e) => Err(map_duckdb_error(&e)),
        }
    }
}

/// Maps a DuckDB error to an API error.
#[must_use]
pub fn map_duckdb_error(e: &duckdb::Error) -> ApiError {
    let msg = e.to_string();
    if msg.contains("Out of Memory") || msg.contains("memory limit") {
        ApiError::new(
            ErrorCode::TooLarge,
            "query exceeded the memory cap; narrow the range or lower the limit",
        )
    } else if msg.contains("INTERRUPT") || msg.contains("Interrupted") {
        ApiError::new(ErrorCode::QueryTimeout, "query was interrupted")
    } else if msg.contains("HTTP") || msg.contains("IO Error") || msg.contains("No files found") {
        tracing::warn!(error = %msg, "archive read failed");
        ApiError::new(
            ErrorCode::ArchiveUnavailable,
            "the archive could not be read",
        )
    } else {
        ApiError::internal(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(limits: Limits) -> Engine {
        let dir = std::env::temp_dir();
        let mut cfg = Config::local(dir);
        cfg.limits = limits;
        Engine::open(&cfg).unwrap()
    }

    #[tokio::test]
    async fn runs_queries_with_params() {
        let e = engine(Limits::default());
        let rows = e
            .query(
                "SELECT ?::BIGINT + 1 AS x".into(),
                vec![Value::BigInt(41)],
                |r| r.get::<_, i64>(0),
            )
            .await
            .unwrap();
        assert_eq!(rows, vec![42]);
        assert!(!e.version().is_empty());
    }

    #[tokio::test]
    async fn configuration_is_locked() {
        let e = engine(Limits::default());
        let err = e
            .query("SET memory_limit = '100GB'".into(), vec![], |_| Ok(()))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal);
    }

    #[tokio::test]
    async fn long_queries_time_out() {
        let e = engine(Limits::default());
        let err = e
            .query_with_timeout(
                "SELECT count(*) FROM range(10000000000) a".into(),
                vec![],
                |r| r.get::<_, i64>(0),
                Duration::from_millis(100),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::QueryTimeout);
        // The connection is still usable afterwards.
        let ok = e
            .query("SELECT 1".into(), vec![], |r| r.get::<_, i32>(0))
            .await
            .unwrap();
        assert_eq!(ok, vec![1]);
    }

    #[tokio::test]
    async fn memory_cap_is_enforced() {
        let e = engine(Limits {
            memory_limit_bytes: 32 << 20,
            ..Limits::default()
        });
        let err = e
            .query(
                "SELECT list(range) FROM range(100000000) GROUP BY range % 7".into(),
                vec![],
                |_| Ok(()),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::TooLarge, "{err:?}");
    }

    #[tokio::test]
    async fn local_roots_cannot_read_outside_the_archive() {
        let e = engine(Limits::default());
        let err = e
            .query(
                "SELECT * FROM read_text('/etc/hosts')".into(),
                vec![],
                |_| Ok(()),
            )
            .await;
        assert!(err.is_err());
    }
}
