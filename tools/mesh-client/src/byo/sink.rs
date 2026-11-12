//! Local outputs: JSON lines to standard output or a checked file, and a
//! websocket on a loopback address. Every line built from user-supplied
//! data carries `"provenance":"user_supplied"`.
//!
//! The serde structures below are private: rendering is the only way out
//! of [`UserSupplied`], and it only reaches these local sinks.

use std::io::Write;
use std::net::SocketAddr;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;

use super::align::Sample;
use super::boundary::{LocalPath, LoopbackAddr, USER_SUPPLIED, UserSupplied};
use super::stats::TrackingReport;

#[derive(Serialize)]
struct SampleWire<'a> {
    provenance: &'static str,
    #[serde(rename = "type")]
    ty: &'static str,
    pair: &'a str,
    ts_ns: String,
    user: UserWire<'a>,
    bleh: BlehWire<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    multiplier: Option<f64>,
    basis: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    basis_bps: Option<f64>,
}

#[derive(Serialize)]
struct UserWire<'a> {
    symbol: &'a str,
    price: f64,
    ts_ns: String,
}

#[derive(Serialize)]
struct BlehWire<'a> {
    series: &'a str,
    price: f64,
    ts_ns: String,
}

/// Renders a sample as one JSON line (no newline).
pub fn sample_line(s: &Sample) -> String {
    let u = s.user.get();
    let basis = s.basis();
    let w = SampleWire {
        provenance: USER_SUPPLIED,
        ty: "sample",
        pair: &s.pair_name,
        ts_ns: s.ts_ns.to_string(),
        user: UserWire {
            symbol: &u.symbol,
            price: u.price,
            ts_ns: u.ts_ns.to_string(),
        },
        bleh: BlehWire {
            series: &s.bleh.label,
            price: s.bleh.price,
            ts_ns: s.bleh.ts_ns.to_string(),
        },
        multiplier: (s.multiplier != 1.0).then_some(s.multiplier),
        basis,
        basis_bps: (u.adjusted != 0.0).then(|| basis / u.adjusted.abs() * 1e4),
    };
    serde_json::to_string(&w).unwrap_or_default()
}

#[derive(Serialize)]
struct ReportWire<'a> {
    provenance: &'static str,
    #[serde(rename = "type")]
    ty: &'static str,
    pair: &'a str,
    samples: u64,
    interval_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_ts_ns: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_ts_ns: Option<String>,
    basis: Moments,
    basis_bps: Moments,
    returns: u64,
    tracking_error_bps: Option<f64>,
    return_corr: Option<f64>,
    lead_lag: Option<LeadLagWire>,
    inputs: InputsWire,
}

#[derive(Serialize)]
struct Moments {
    mean: Option<f64>,
    std: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last: Option<f64>,
}

#[derive(Serialize)]
struct LeadLagWire {
    bleh_lead_ms: i64,
    corr: f64,
    corr_at_zero: Option<f64>,
    pairs: u64,
    max_lag_ms: i64,
}

#[derive(Serialize)]
struct InputsWire {
    user: u64,
    bleh: u64,
    late: u64,
    stale: u64,
    dropped: u64,
}

/// Renders tracking statistics as one JSON line (no newline).
pub fn report_line(r: &UserSupplied<TrackingReport>) -> String {
    let r = r.get();
    let w = ReportWire {
        provenance: USER_SUPPLIED,
        ty: "tracking",
        pair: &r.pair,
        samples: r.samples,
        interval_ms: r.interval_ms,
        first_ts_ns: r.span_ns.map(|s| s.0.to_string()),
        last_ts_ns: r.span_ns.map(|s| s.1.to_string()),
        basis: Moments {
            mean: r.basis_mean,
            std: r.basis_std,
            min: r.basis_min,
            max: r.basis_max,
            last: r.basis_last,
        },
        basis_bps: Moments {
            mean: r.basis_bps_mean,
            std: r.basis_bps_std,
            min: None,
            max: None,
            last: None,
        },
        returns: r.returns,
        tracking_error_bps: r.tracking_error_bps,
        return_corr: r.return_corr,
        lead_lag: r.lead_lag.map(|l| LeadLagWire {
            bleh_lead_ms: l.bleh_lead_ms,
            corr: l.corr,
            corr_at_zero: l.corr_at_zero,
            pairs: l.pairs,
            max_lag_ms: l.max_lag_ms,
        }),
        inputs: InputsWire {
            user: r.inputs.user,
            bleh: r.inputs.bleh,
            late: r.inputs.late,
            stale: r.inputs.stale,
            dropped: r.inputs.dropped,
        },
    };
    serde_json::to_string(&w).unwrap_or_default()
}

/// A JSON-lines sink: standard output, standard error, or a file whose
/// path passed [`LocalPath`]'s checks.
pub struct JsonlSink {
    w: Box<dyn Write + Send>,
}

impl std::fmt::Debug for JsonlSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JsonlSink")
    }
}

impl JsonlSink {
    /// Standard output.
    pub fn stdout() -> Self {
        Self {
            w: Box::new(std::io::LineWriter::new(std::io::stdout())),
        }
    }

    /// Standard error.
    pub fn stderr() -> Self {
        Self {
            w: Box::new(std::io::stderr()),
        }
    }

    /// Creates (truncates) a checked file.
    ///
    /// # Errors
    ///
    /// I/O.
    pub fn create(path: &LocalPath) -> std::io::Result<Self> {
        Ok(Self {
            w: Box::new(std::io::BufWriter::new(std::fs::File::create(
                path.as_path(),
            )?)),
        })
    }

    /// Any writer (tests).
    #[cfg(test)]
    pub(crate) fn writer(w: Box<dyn Write + Send>) -> Self {
        Self { w }
    }

    /// Writes one line.
    ///
    /// # Errors
    ///
    /// I/O.
    pub fn line(&mut self, line: &str) -> std::io::Result<()> {
        self.w.write_all(line.as_bytes())?;
        self.w.write_all(b"\n")
    }

    /// Flushes.
    ///
    /// # Errors
    ///
    /// I/O.
    pub fn flush(&mut self) -> std::io::Result<()> {
        self.w.flush()
    }
}

/// A websocket server on a loopback address that sends every merged and
/// statistics line to each connected local client. Incoming messages are
/// ignored. Browsers send an `Origin` header; connections from pages not
/// served from a loopback host (or an origin allowed explicitly) are refused
/// with 403, so a web page cannot read the user's data through it.
#[derive(Debug)]
pub struct LocalWs {
    tx: broadcast::Sender<Arc<str>>,
    addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for LocalWs {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// True if a browser `Origin` may connect.
pub fn origin_allowed(origin: &str, allowed: &[String]) -> bool {
    if allowed.iter().any(|a| a == origin) {
        return true;
    }
    let Some(rest) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    let host = if let Some(v6) = rest.strip_prefix('[') {
        v6.split(']').next().unwrap_or_default()
    } else {
        rest.split([':', '/']).next().unwrap_or_default()
    };
    matches!(host, "localhost" | "::1")
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback())
}

impl LocalWs {
    /// Binds `addr` (loopback only, by type) and starts accepting.
    ///
    /// # Errors
    ///
    /// The bind failed.
    pub async fn bind(addr: LoopbackAddr, allowed_origins: Vec<String>) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr.addr()).await?;
        let local = listener.local_addr()?;
        let (tx, _) = broadcast::channel::<Arc<str>>(4096);
        let txc = tx.clone();
        let allowed = Arc::new(allowed_origins);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, peer)) = listener.accept().await else {
                    continue;
                };
                if !peer.ip().is_loopback() {
                    continue;
                }
                let rx = txc.subscribe();
                let allowed = allowed.clone();
                tokio::spawn(async move {
                    // The handshake callback's signature is tungstenite's.
                    #[allow(clippy::result_large_err)]
                    let check = |req: &Request,
                                 resp: Response|
                     -> Result<Response, ErrorResponse> {
                        match req
                            .headers()
                            .get("origin")
                            .map(|o| o.to_str().unwrap_or(""))
                        {
                            Some(o) if !origin_allowed(o, &allowed) => {
                                let mut r = ErrorResponse::new(Some("origin not allowed".into()));
                                *r.status_mut() = StatusCode::FORBIDDEN;
                                Err(r)
                            }
                            _ => Ok(resp),
                        }
                    };
                    let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, check).await else {
                        return;
                    };
                    serve(ws, rx).await;
                });
            }
        });
        Ok(Self {
            tx,
            addr: local,
            task,
        })
    }

    /// The bound address.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Sends a line to every connected client (dropped if none).
    pub fn send(&self, line: Arc<str>) {
        let _ = self.tx.send(line);
    }
}

async fn serve(
    ws: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    mut rx: broadcast::Receiver<Arc<str>>,
) {
    let (mut tx_ws, mut rx_ws) = ws.split();
    loop {
        tokio::select! {
            m = rx.recv() => match m {
                Ok(line) => {
                    if tx_ws.send(Message::text(line.as_ref())).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
            m = rx_ws.next() => match m {
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                Some(Ok(_)) => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::byo::align::{BlehPoint, UserPoint};
    use crate::byo::stats::{InputCounts, StatsConfig, TrackingStats};

    fn sample() -> Sample {
        Sample {
            pair: 0,
            pair_name: "p".into(),
            ts_ns: 2_000_000_000,
            user: UserSupplied::new(UserPoint {
                symbol: "U".into(),
                price: 50.0,
                adjusted: 100.0,
                ts_ns: 1_500_000_000,
            }),
            bleh: BlehPoint {
                label: "hyperliquid:perp:BTC".into(),
                price: 101.0,
                ts_ns: 1_900_000_000,
            },
            multiplier: 2.0,
        }
    }

    #[test]
    fn sample_lines_are_tagged_user_supplied() {
        let v: serde_json::Value = serde_json::from_str(&sample_line(&sample())).unwrap();
        assert_eq!(v["provenance"], "user_supplied");
        assert_eq!(v["type"], "sample");
        assert_eq!(v["ts_ns"], "2000000000");
        assert_eq!(v["user"]["price"], 50.0);
        assert_eq!(v["bleh"]["series"], "hyperliquid:perp:BTC");
        assert_eq!(v["multiplier"], 2.0);
        assert_eq!(v["basis"], 1.0);
        assert_eq!(v["basis_bps"], 100.0);
    }

    #[test]
    fn report_lines_are_tagged_user_supplied() {
        let mut st = TrackingStats::new("p", StatsConfig::default());
        st.push(0, 1.0, 1.0);
        let r = UserSupplied::new(st.report(InputCounts::default()));
        let line = report_line(&r);
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["provenance"], "user_supplied");
        assert_eq!(v["type"], "tracking");
        assert_eq!(v["samples"], 1);
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        struct W(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl Write for W {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut sink = JsonlSink::writer(Box::new(W(buf.clone())));
        sink.line(&line).unwrap();
        sink.flush().unwrap();
        assert_eq!(
            buf.lock().unwrap().as_slice(),
            format!("{line}\n").as_bytes()
        );
    }

    #[test]
    fn browser_origins_must_be_loopback() {
        for ok in [
            "http://localhost:3000",
            "http://127.0.0.1",
            "https://127.0.0.1:8443/x",
            "http://[::1]:5173",
        ] {
            assert!(origin_allowed(ok, &[]), "{ok}");
        }
        for bad in [
            "https://example.com",
            "http://localhost.example.com",
            "http://192.168.0.2:3000",
            "null",
            "file://",
        ] {
            assert!(!origin_allowed(bad, &[]), "{bad}");
        }
        assert!(origin_allowed(
            "https://dash.internal",
            &["https://dash.internal".into()]
        ));
    }

    #[tokio::test]
    async fn local_websocket_serves_lines_and_refuses_foreign_origins() {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let ws = LocalWs::bind(LoopbackAddr::parse("127.0.0.1:0").unwrap(), vec![])
            .await
            .unwrap();
        let url = format!("ws://{}/", ws.local_addr());
        let (mut client, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        // Wait until the server subscribed this client.
        let line: Arc<str> = sample_line(&sample()).into();
        let got = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                ws.send(line.clone());
                tokio::select! {
                    m = client.next() => return m,
                    () = tokio::time::sleep(std::time::Duration::from_millis(20)) => {}
                }
            }
        })
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert_eq!(got.into_text().unwrap().as_str(), line.as_ref());

        let mut req = url.as_str().into_client_request().unwrap();
        req.headers_mut()
            .insert("origin", "https://evil.example".parse().unwrap());
        let e = tokio_tungstenite::connect_async(req).await.unwrap_err();
        assert!(e.to_string().contains("403"), "{e}");
    }
}
