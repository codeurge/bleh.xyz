//! The `gateway` binary configured from `BLEH_GATEWAY_*` environment
//! variables alone, with no command-line arguments.

#![allow(clippy::unwrap_used)]

use std::net::{SocketAddr, TcpListener};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Kills the child process on drop.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_addr() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap()
}

fn gateway(env: &[(&str, String)]) -> Running {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gateway"));
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("BLEH_GATEWAY_") {
            cmd.env_remove(k);
        }
    }
    cmd.envs(env.iter().map(|(k, v)| (k, v)))
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Running(cmd.spawn().unwrap())
}

async fn get(addr: SocketAddr, path: &str) -> Option<(u16, String)> {
    let mut s = TcpStream::connect(addr).await.ok()?;
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n").as_bytes())
        .await
        .ok()?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.ok()?;
    let text = String::from_utf8(buf).ok()?;
    let status = text.get(9..12)?.parse().ok()?;
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_owned())?;
    Some((status, body))
}

/// Polls `path` until `want` holds for the response.
async fn wait_for(addr: SocketAddr, path: &str, want: impl Fn(u16, &str) -> bool) -> String {
    let mut last = None;
    for _ in 0..300 {
        if let Some((status, body)) = get(addr, path).await {
            if want(status, &body) {
                return body;
            }
            last = Some((status, body));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{path} never matched; last response {last:?}");
}

#[tokio::test]
async fn demo_mode_from_the_environment() {
    let addr = free_addr();
    let _gw = gateway(&[
        ("BLEH_GATEWAY_MODE", "demo".into()),
        ("BLEH_GATEWAY_LISTEN", addr.to_string()),
        ("BLEH_GATEWAY_EPHEMERAL_KEY", "true".into()),
        ("BLEH_GATEWAY_INTERVAL_MS", "20".into()),
        ("BLEH_GATEWAY_CHANNELS_PER_CONNECTION", "7".into()),
    ]);
    // Healthy once blocks flow (only with the interval and key from the
    // environment), signing with the ephemeral key.
    wait_for(addr, "/health", |s, _| s == 200).await;
    let signers = wait_for(addr, "/signers", |s, _| s == 200).await;
    assert!(signers.contains("ephemeral"), "{signers}");
    let metrics = wait_for(addr, "/metrics", |s, _| s == 200).await;
    assert!(metrics.contains("gateway_blocks_total"), "{metrics}");
}

#[tokio::test]
async fn relay_mode_from_the_environment() {
    let addr = free_addr();
    let dead = free_addr();
    let _gw = gateway(&[
        ("BLEH_GATEWAY_MODE", "relay".into()),
        ("BLEH_GATEWAY_LISTEN", addr.to_string()),
        (
            "BLEH_GATEWAY_UPSTREAM",
            format!("ws://{dead}/v0/ws,ws://{dead}/v0/ws"),
        ),
        (
            "BLEH_GATEWAY_SIGNER",
            "0x0000000000000000000000000000000000000001,0x0000000000000000000000000000000000000002"
                .into(),
        ),
    ]);
    let body = wait_for(addr, "/health", |s, _| s == 503).await;
    assert!(body.contains("\"upstream_connected\":false"), "{body}");
    let signers = wait_for(addr, "/signers", |s, _| s == 200).await;
    assert!(
        signers.contains("0x0000000000000000000000000000000000000002"),
        "{signers}"
    );
}
