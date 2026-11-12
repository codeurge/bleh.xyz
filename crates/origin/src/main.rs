//! `origin`: the bleh.xyz origin service. See `docs/operations/origin.md`.

use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use origin::config::{Cli, LogFormat};

fn init_logs(format: LogFormat) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let b = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    match format {
        LogFormat::Text => b.init(),
        LogFormat::Json => b.json().init(),
    }
}

async fn terminate() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = term.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Stops within the orchestrator's 30 s grace period.
const SHUTDOWN: Duration = Duration::from_secs(25);

async fn run(cli: Cli) -> Result<()> {
    let cfg = cli.config()?;
    let mut running = origin::start(cfg).await?;
    let failed = tokio::select! {
        () = terminate() => {
            tracing::info!("stopping");
            None
        }
        r = running.next_exit() => match r {
            Some(Err(e)) => {
                tracing::error!(error = %e, "a part of the origin failed; stopping");
                Some(e)
            }
            _ => None,
        },
    };
    running.stop();
    let joined = tokio::time::timeout(SHUTDOWN, running.join()).await;
    if let Some(e) = failed {
        return Err(e);
    }
    match joined {
        Ok(r) => r,
        Err(_) => {
            tracing::warn!("shutdown took longer than {SHUTDOWN:?}");
            Ok(())
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logs(cli.log_format);
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(cli))
}
