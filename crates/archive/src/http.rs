//! A small HTTP client abstraction, so every network call (OpenTimestamps
//! calendars, Bitcoin header APIs, Ethereum JSON-RPC, the Kubo API, mirror
//! downloads) is optional and replaceable by [`MockHttp`] in tests.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use futures::future::BoxFuture;

use crate::error::{Error, Result};

/// An HTTP response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// Status code.
    pub status: u16,
    /// Body.
    pub body: Bytes,
}

impl Response {
    /// Whether the status is 2xx.
    #[must_use]
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The body if the status is 2xx, else an error naming `what`.
    ///
    /// # Errors
    ///
    /// Fails on a non-2xx status.
    pub fn into_ok(self, what: &str) -> Result<Bytes> {
        if self.ok() {
            Ok(self.body)
        } else {
            Err(Error::Http(format!(
                "{what}: status {}: {}",
                self.status,
                String::from_utf8_lossy(&self.body[..self.body.len().min(200)])
            )))
        }
    }
}

/// The HTTP operations the archive needs.
pub trait HttpClient: Send + Sync + std::fmt::Debug {
    /// `GET url` with extra headers.
    fn get<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(&'a str, &'a str)],
    ) -> BoxFuture<'a, Result<Response>>;
    /// `POST url` with a body and extra headers.
    fn post<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(&'a str, &'a str)],
        body: Bytes,
    ) -> BoxFuture<'a, Result<Response>>;
}

/// [`HttpClient`] over `reqwest` with rustls.
#[derive(Debug, Clone)]
pub struct ReqwestClient {
    client: reqwest::Client,
}

impl ReqwestClient {
    /// A client with the given request timeout.
    ///
    /// # Errors
    ///
    /// Fails if the TLS backend cannot be initialised.
    pub fn new(timeout: Duration) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(concat!("bleh-archive/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::Http(e.to_string()))?;
        Ok(Self { client })
    }
}

async fn finish(req: reqwest::RequestBuilder) -> Result<Response> {
    let resp = req.send().await.map_err(|e| Error::Http(e.to_string()))?;
    let status = resp.status().as_u16();
    let body = resp.bytes().await.map_err(|e| Error::Http(e.to_string()))?;
    Ok(Response { status, body })
}

impl HttpClient for ReqwestClient {
    fn get<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(&'a str, &'a str)],
    ) -> BoxFuture<'a, Result<Response>> {
        Box::pin(async move {
            let mut req = self.client.get(url);
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            finish(req).await
        })
    }

    fn post<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(&'a str, &'a str)],
        body: Bytes,
    ) -> BoxFuture<'a, Result<Response>> {
        Box::pin(async move {
            let mut req = self.client.post(url).body(body);
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            finish(req).await
        })
    }
}

/// A recorded request to [`MockHttp`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// `GET` or `POST`.
    pub method: &'static str,
    /// URL.
    pub url: String,
    /// Body (empty for `GET`).
    pub body: Bytes,
}

type Handler = Arc<dyn Fn(&Request) -> Response + Send + Sync>;

/// An in-memory [`HttpClient`] for tests: fixed responses by method and URL,
/// or handlers by URL prefix. Unknown URLs get 404. Every request is
/// recorded.
#[derive(Clone, Default)]
pub struct MockHttp {
    routes: Arc<Mutex<BTreeMap<(String, String), Response>>>,
    handlers: Arc<Mutex<Vec<(String, Handler)>>>,
    log: Arc<Mutex<Vec<Request>>>,
}

impl std::fmt::Debug for MockHttp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockHttp").finish_non_exhaustive()
    }
}

impl MockHttp {
    /// An empty mock.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Responds to `method url` with `status` and `body`.
    pub fn route(&self, method: &str, url: &str, status: u16, body: impl Into<Bytes>) {
        if let Ok(mut r) = self.routes.lock() {
            r.insert(
                (method.to_owned(), url.to_owned()),
                Response {
                    status,
                    body: body.into(),
                },
            );
        }
    }

    /// Handles every request whose URL starts with `prefix` (checked after
    /// exact routes, most recent handler first).
    pub fn handle(&self, prefix: &str, f: impl Fn(&Request) -> Response + Send + Sync + 'static) {
        if let Ok(mut h) = self.handlers.lock() {
            h.push((prefix.to_owned(), Arc::new(f)));
        }
    }

    /// Requests received so far.
    #[must_use]
    pub fn requests(&self) -> Vec<Request> {
        self.log.lock().map(|l| l.clone()).unwrap_or_default()
    }

    fn respond(&self, req: Request) -> Response {
        if let Ok(mut l) = self.log.lock() {
            l.push(req.clone());
        }
        if let Some(r) = self
            .routes
            .lock()
            .ok()
            .and_then(|r| r.get(&(req.method.to_owned(), req.url.clone())).cloned())
        {
            return r;
        }
        let handler = self.handlers.lock().ok().and_then(|h| {
            h.iter()
                .rev()
                .find(|(p, _)| req.url.starts_with(p.as_str()))
                .map(|(_, f)| f.clone())
        });
        match handler {
            Some(f) => f(&req),
            None => Response {
                status: 404,
                body: Bytes::from_static(b"not found"),
            },
        }
    }
}

impl HttpClient for MockHttp {
    fn get<'a>(
        &'a self,
        url: &'a str,
        _headers: &'a [(&'a str, &'a str)],
    ) -> BoxFuture<'a, Result<Response>> {
        let r = self.respond(Request {
            method: "GET",
            url: url.to_owned(),
            body: Bytes::new(),
        });
        Box::pin(async move { Ok(r) })
    }

    fn post<'a>(
        &'a self,
        url: &'a str,
        _headers: &'a [(&'a str, &'a str)],
        body: Bytes,
    ) -> BoxFuture<'a, Result<Response>> {
        let r = self.respond(Request {
            method: "POST",
            url: url.to_owned(),
            body,
        });
        Box::pin(async move { Ok(r) })
    }
}

/// Percent-encodes each segment of a `/`-separated archive path for use in
/// a URL (RFC 3986 `pchar`s are kept; `#`, `?`, `%`, spaces and non-ASCII
/// bytes are encoded). Instrument IDs may contain `#` (`hyperliquid~outcome~#9001`),
/// which would otherwise start a URL fragment.
#[must_use]
pub fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Joins a base URL and a path with exactly one `/`.
#[must_use]
pub fn join_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_routes_and_handlers() {
        let m = MockHttp::new();
        m.route("GET", "https://a/x", 200, "hello");
        m.handle("https://b/", |r| Response {
            status: 201,
            body: Bytes::from(format!("{} {}", r.method, r.body.len())),
        });
        let r = m.get("https://a/x", &[]).await.unwrap();
        assert_eq!(r.into_ok("x").unwrap(), Bytes::from_static(b"hello"));
        let r = m
            .post("https://b/y", &[], Bytes::from_static(b"abc"))
            .await
            .unwrap();
        assert_eq!((r.status, r.body.as_ref()), (201, &b"POST 3"[..]));
        let r = m.get("https://c", &[]).await.unwrap();
        assert!(r.into_ok("c").is_err());
        assert_eq!(m.requests().len(), 3);
        assert_eq!(join_url("https://a/", "/b"), "https://a/b");
        assert_eq!(
            encode_path("kind=trade/instrument=hyperliquid~outcome~#9001/a b%.parquet"),
            "kind=trade/instrument=hyperliquid~outcome~%239001/a%20b%25.parquet"
        );
    }
}
