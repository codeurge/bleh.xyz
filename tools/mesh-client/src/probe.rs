//! Availability probe: connect, subscribe one channel, wait for the first
//! block heartbeat (and, for `book_event`, the snapshot), report one result.
//!
//! Used by `mesh-client probe`, which prints the [`ProbeResult`] as one
//! JSON line for monitoring.

use std::time::{Duration, Instant};

use mesh_types::{EthAddress, RecordKind};
use serde::Serialize;

use crate::client::{Client, ClientConfig, ClientEvent};
use crate::protocol::{ChannelSpec, Encoding};

/// What to probe.
#[derive(Debug, Clone)]
pub struct ProbeConfig {
    /// Gateway websocket URL.
    pub url: String,
    /// The channel to subscribe.
    pub channel: ChannelSpec,
    /// Signers the gateway must announce in `hello` (none: not checked).
    pub signers: Vec<EthAddress>,
    /// Give up after this long, connection included.
    pub timeout: Duration,
    /// Frame encoding.
    pub encoding: Encoding,
}

/// The probe outcome.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ProbeResult {
    /// Heartbeat (and snapshot, for `book_event`) received in time.
    pub ok: bool,
    /// Seconds from the start (before connecting) to the first block
    /// heartbeat.
    pub first_block_seconds: Option<f64>,
    /// Block time of the last heartbeat received, milliseconds since the
    /// epoch.
    pub block_time_ms: Option<i64>,
    /// `book_event` only: seconds from the start to the book snapshot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_seconds: Option<f64>,
    /// Why the probe failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Runs the probe. Never fails: problems are reported in the result.
pub async fn run(cfg: ProbeConfig) -> ProbeResult {
    let start = Instant::now();
    let want_snapshot = cfg.channel.kind == RecordKind::BookEvent;
    let mut client = Client::new(ClientConfig {
        urls: vec![cfg.url.clone()],
        encoding: cfg.encoding,
        channels: vec![cfg.channel.clone()],
        commitments: false,
        verify_signatures: false,
        verify_roots: false,
        // The probe's own timeout governs; never reconnect within it.
        stall_timeout: cfg.timeout + Duration::from_secs(1),
        connect_timeout: cfg.timeout,
        min_backoff: Duration::ZERO,
        max_backoff: Duration::ZERO,
        ..ClientConfig::default()
    });
    let mut res = ProbeResult::default();
    let outcome = tokio::time::timeout(
        cfg.timeout,
        watch(&mut client, &cfg, start, want_snapshot, &mut res),
    )
    .await;
    client.close().await;
    match outcome {
        Ok(Ok(())) => res.ok = true,
        Ok(Err(e)) => res.error = Some(e),
        Err(_) => {
            let what = if res.first_block_seconds.is_none() {
                "block heartbeat"
            } else {
                "book snapshot"
            };
            res.error = Some(format!("no {what} within {:?}", cfg.timeout));
        }
    }
    res
}

async fn watch(
    client: &mut Client,
    cfg: &ProbeConfig,
    start: Instant,
    want_snapshot: bool,
    res: &mut ProbeResult,
) -> Result<(), String> {
    loop {
        match client.next_event().await.map_err(|e| e.to_string())? {
            ClientEvent::Connected { hello, .. } => {
                if let Some(s) = cfg.signers.iter().find(|s| !hello.signers.contains(s)) {
                    return Err(format!("gateway does not announce signer {s}"));
                }
            }
            ClientEvent::ConnectFailed { error, .. } => {
                return Err(format!("connect failed: {error}"));
            }
            ClientEvent::Disconnected { reason, .. } => {
                return Err(format!("disconnected: {reason}"));
            }
            ClientEvent::ServerError { code, message, .. } => {
                let code = serde_json::to_value(code)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                return Err(format!("server error {code}: {message}"));
            }
            ClientEvent::Snapshot { .. } => {
                res.snapshot_seconds
                    .get_or_insert(start.elapsed().as_secs_f64());
            }
            ClientEvent::Block { block_time_ms, .. } => {
                res.first_block_seconds
                    .get_or_insert(start.elapsed().as_secs_f64());
                res.block_time_ms = Some(block_time_ms.0);
            }
            _ => {}
        }
        if res.first_block_seconds.is_some() && (!want_snapshot || res.snapshot_seconds.is_some()) {
            return Ok(());
        }
    }
}

/// Parses a duration: `5s`, `500ms`, `2m`, or a plain number of seconds.
///
/// # Errors
///
/// A message for anything else.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let (num, unit) = match s.find(|c: char| !(c.is_ascii_digit() || c == '.')) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, "s"),
    };
    let n: f64 = num
        .parse()
        .map_err(|_| format!("bad duration {s:?} (examples: 5s, 500ms, 2m)"))?;
    let secs = match unit {
        "ms" => n / 1000.0,
        "s" => n,
        "m" => n * 60.0,
        _ => return Err(format!("bad duration unit in {s:?} (ms, s or m)")),
    };
    Duration::try_from_secs_f64(secs).map_err(|e| format!("bad duration {s:?}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("5s").unwrap(), Duration::from_secs(5));
        assert_eq!(parse_duration("5").unwrap(), Duration::from_secs(5));
        assert_eq!(parse_duration("250ms").unwrap(), Duration::from_millis(250));
        assert_eq!(parse_duration("1.5s").unwrap(), Duration::from_millis(1500));
        assert_eq!(parse_duration("2m").unwrap(), Duration::from_secs(120));
        assert!(parse_duration("5h").is_err());
        assert!(parse_duration("s").is_err());
        assert!(parse_duration("-1s").is_err());
    }

    #[test]
    fn result_json_shape() {
        let ok = ProbeResult {
            ok: true,
            first_block_seconds: Some(0.25),
            block_time_ms: Some(1_767_229_197_000),
            ..ProbeResult::default()
        };
        assert_eq!(
            serde_json::to_string(&ok).unwrap(),
            r#"{"ok":true,"first_block_seconds":0.25,"block_time_ms":1767229197000}"#
        );
        let bad = ProbeResult {
            error: Some("connect failed".into()),
            ..ProbeResult::default()
        };
        assert_eq!(
            serde_json::to_string(&bad).unwrap(),
            r#"{"ok":false,"first_block_seconds":null,"block_time_ms":null,"error":"connect failed"}"#
        );
    }

    #[tokio::test]
    async fn unreachable_gateway_fails_fast_with_an_error() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        let res = run(ProbeConfig {
            url: format!("ws://{addr}/v0/ws"),
            channel: "*|trade".parse().unwrap(),
            signers: vec![],
            timeout: Duration::from_secs(5),
            encoding: Encoding::Json,
        })
        .await;
        assert!(!res.ok);
        assert!(
            res.error.as_deref().unwrap().starts_with("connect failed"),
            "{res:?}"
        );
    }
}
