//! A minimal blocking HTTP/1.1 JSON client for the node's local info
//! server (`--serve-info`, spec § 7.2).
//!
//! The info server listens on the node host (`http://localhost:3001/info`)
//! and speaks plain HTTP, so a small client over `std::net` is enough and
//! keeps TLS stacks out of the reader. Only `http://host[:port]/path` URLs
//! are accepted. Responses are read to the end (`Connection: close`), with
//! `Content-Length` or chunked bodies and a size cap.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde_json::Value;

/// Largest response body accepted.
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024 * 1024;

/// Errors from the info client.
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// The URL is not `http://host[:port]/path`.
    #[error("unsupported info URL {0:?} (expected http://host[:port]/path)")]
    Url(String),
    /// Connection or I/O failure.
    #[error("info server I/O: {0}")]
    Io(#[from] std::io::Error),
    /// The response is not valid HTTP.
    #[error("info server response: {0}")]
    Protocol(String),
    /// A non-success status.
    #[error("info server returned HTTP {status}: {body}")]
    Status {
        /// Status code.
        status: u16,
        /// Start of the body.
        body: String,
    },
    /// The body is not JSON.
    #[error("info server JSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// A client for one info endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoClient {
    host: String,
    port: u16,
    path: String,
    timeout: Duration,
}

impl InfoClient {
    /// Parses an endpoint URL.
    ///
    /// # Errors
    ///
    /// Fails unless the URL is `http://host[:port][/path]`.
    pub fn new(url: &str, timeout: Duration) -> Result<Self, HttpError> {
        let bad = || HttpError::Url(url.to_owned());
        let rest = url.strip_prefix("http://").ok_or_else(bad)?;
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let (host, port_text) = if let Some(v6) = authority.strip_prefix('[') {
            let (h, after) = v6.split_once(']').ok_or_else(bad)?;
            match after {
                "" => (h, None),
                p => (h, Some(p.strip_prefix(':').ok_or_else(bad)?)),
            }
        } else {
            match authority.split_once(':') {
                Some((h, p)) => (h, Some(p)),
                None => (authority, None),
            }
        };
        let port = match port_text {
            Some(p) => p.parse::<u16>().map_err(|_| bad())?,
            None => 80,
        };
        if host.is_empty() || path.contains(char::is_whitespace) {
            return Err(bad());
        }
        Ok(Self {
            host: host.to_owned(),
            port,
            path: path.to_owned(),
            timeout,
        })
    }

    /// POSTs a JSON body and returns the JSON response (`Value::Null` for
    /// an empty body).
    ///
    /// # Errors
    ///
    /// Fails on connection errors, timeouts, a non-2xx status or a body
    /// that is not JSON.
    pub fn post_json(&self, body: &Value) -> Result<Value, HttpError> {
        let payload = serde_json::to_vec(body)?;
        let addr = (self.host.as_str(), self.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| HttpError::Url(self.host.clone()))?;
        let mut stream = TcpStream::connect_timeout(&addr, self.timeout)?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;
        let host_header = if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        };
        let head = format!(
            "POST {} HTTP/1.1\r\nHost: {host_header}\r\nContent-Type: application/json\r\n\
             Accept: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.path,
            payload.len()
        );
        stream.write_all(head.as_bytes())?;
        stream.write_all(&payload)?;
        stream.flush()?;
        let mut raw = Vec::new();
        stream
            .take(u64::try_from(MAX_RESPONSE_BYTES).unwrap_or(u64::MAX) + 1)
            .read_to_end(&mut raw)?;
        if raw.len() > MAX_RESPONSE_BYTES {
            return Err(HttpError::Protocol("response too large".into()));
        }
        let (status, body) = parse_response(&raw)?;
        if !(200..300).contains(&status) {
            let text = String::from_utf8_lossy(&body);
            return Err(HttpError::Status {
                status,
                body: text.chars().take(200).collect(),
            });
        }
        if body.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_slice(&body)?)
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Splits a full HTTP/1.1 response into status and decoded body.
fn parse_response(raw: &[u8]) -> Result<(u16, Vec<u8>), HttpError> {
    let proto = |m: &str| HttpError::Protocol(m.to_owned());
    let end = find(raw, b"\r\n\r\n").ok_or_else(|| proto("no header terminator"))?;
    let head = std::str::from_utf8(&raw[..end]).map_err(|_| proto("non-UTF-8 headers"))?;
    let body = &raw[end + 4..];
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or_else(|| proto("empty response"))?;
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or_default();
    if !version.starts_with("HTTP/1.") {
        return Err(proto("not HTTP/1.x"));
    }
    let status: u16 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| proto("bad status code"))?;
    let mut chunked = false;
    let mut length: Option<usize> = None;
    for h in lines {
        let Some((name, value)) = h.split_once(':') else {
            continue;
        };
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.to_ascii_lowercase().contains("chunked");
        } else if name.eq_ignore_ascii_case("content-length") {
            length = Some(value.parse().map_err(|_| proto("bad content-length"))?);
        }
    }
    let body = if chunked {
        decode_chunked(body)?
    } else if let Some(n) = length {
        body.get(..n)
            .ok_or_else(|| proto("body shorter than content-length"))?
            .to_vec()
    } else {
        body.to_vec()
    };
    Ok((status, body))
}

fn decode_chunked(mut data: &[u8]) -> Result<Vec<u8>, HttpError> {
    let proto = |m: &str| HttpError::Protocol(m.to_owned());
    let mut out = Vec::new();
    loop {
        let eol = find(data, b"\r\n").ok_or_else(|| proto("truncated chunk size"))?;
        let size_text = std::str::from_utf8(&data[..eol]).map_err(|_| proto("bad chunk size"))?;
        let size_text = size_text.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_text, 16).map_err(|_| proto("bad chunk size"))?;
        data = &data[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        let chunk = data.get(..size).ok_or_else(|| proto("truncated chunk"))?;
        out.extend_from_slice(chunk);
        data = data
            .get(size..)
            .and_then(|d| d.strip_prefix(b"\r\n"))
            .ok_or_else(|| proto("missing chunk terminator"))?;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// Serves canned responses, one per connection, and returns the
    /// received request bodies.
    pub(crate) fn serve(responses: Vec<String>) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/info", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for resp in responses {
                let (mut s, _) = listener.accept().unwrap();
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    let n = s.read(&mut tmp).unwrap();
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(end) = find(&buf, b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..end]).to_string();
                        let len: usize = head
                            .lines()
                            .find_map(|l| l.strip_prefix("Content-Length: "))
                            .map_or(0, |v| v.trim().parse().unwrap());
                        if buf.len() >= end + 4 + len {
                            tx.send(
                                String::from_utf8_lossy(&buf[end + 4..end + 4 + len]).to_string(),
                            )
                            .unwrap();
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                s.write_all(resp.as_bytes()).unwrap();
            }
        });
        (url, rx)
    }

    #[test]
    fn url_parsing() {
        let c = InfoClient::new("http://127.0.0.1:3001/info", Duration::from_secs(1)).unwrap();
        assert_eq!(
            (c.host.as_str(), c.port, c.path.as_str()),
            ("127.0.0.1", 3001, "/info")
        );
        let c = InfoClient::new("http://node", Duration::from_secs(1)).unwrap();
        assert_eq!((c.port, c.path.as_str()), (80, "/"));
        let c = InfoClient::new("http://[::1]:3001/info", Duration::from_secs(1)).unwrap();
        assert_eq!((c.host.as_str(), c.port), ("::1", 3001));
        assert!(InfoClient::new("https://x/info", Duration::from_secs(1)).is_err());
        assert!(InfoClient::new("http://x:notaport/", Duration::from_secs(1)).is_err());
    }

    #[test]
    fn post_with_length_chunked_empty_and_error() {
        let (url, rx) = serve(vec![
            "HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n{\"ok\":true}".into(),
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n".into(),
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".into(),
            "HTTP/1.1 500 Internal\r\nContent-Length: 4\r\n\r\nnope".into(),
        ]);
        let c = InfoClient::new(&url, Duration::from_secs(5)).unwrap();
        let req = serde_json::json!({"type": "meta"});
        assert_eq!(c.post_json(&req).unwrap(), serde_json::json!({"ok": true}));
        assert_eq!(
            serde_json::from_str::<Value>(&rx.recv().unwrap()).unwrap(),
            req
        );
        assert_eq!(c.post_json(&req).unwrap(), serde_json::json!({"a": 1}));
        assert_eq!(c.post_json(&req).unwrap(), Value::Null);
        assert!(matches!(
            c.post_json(&req),
            Err(HttpError::Status { status: 500, .. })
        ));
    }
}
