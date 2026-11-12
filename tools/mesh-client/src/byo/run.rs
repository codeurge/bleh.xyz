//! The `mesh-client byo merge` driver: reads the user's feed and the
//! bleh.xyz side, aligns them, and writes local outputs.
//!
//! The gateway connection is read-only: the client sends only its
//! subscription and protocol requests (`ClientMessage`), which have no field
//! that could carry user-supplied data, and this module never calls
//! `Client::send`.

use std::io::BufRead;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use super::align::{AlignConfig, Aligner, Sample};
use super::bleh::{BlehInput, BlehTracker};
use super::boundary::UserSupplied;
use super::feed::{FeedError, UserQuote};
use super::mapping::Mapping;
use super::sink::{JsonlSink, LocalWs, report_line, sample_line};
use super::stats::{StatsConfig, TrackingReport, TrackingStats};
use crate::client::{Client, ClientConfig};

/// Which clock places observations on the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Clock {
    /// The feeds' own timestamps (venue time on the bleh.xyz side).
    #[default]
    Event,
    /// Local arrival time (live sources only): measures what this machine
    /// sees, delivery latency included.
    Receive,
}

impl std::str::FromStr for Clock {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "event" => Ok(Self::Event),
            "receive" => Ok(Self::Receive),
            o => Err(format!("unknown clock {o:?} (event or receive)")),
        }
    }
}

/// Merge settings.
#[derive(Debug, Clone)]
pub struct MergeConfig {
    /// The mapping.
    pub mapping: Mapping,
    /// Grid interval.
    pub interval: Duration,
    /// Staleness limit.
    pub max_stale: Duration,
    /// Largest lead or lag tested.
    pub max_lag: Duration,
    /// Samples kept for lead/lag.
    pub window: usize,
    /// Clock.
    pub clock: Clock,
    /// Emit statistics this often (live only); always at the end.
    pub stats_every: Option<Duration>,
    /// Stop after this long (live only).
    pub duration: Option<Duration>,
}

/// A blocking source of user quotes (a file or standard input).
pub type QuoteIter = Box<dyn Iterator<Item = Result<UserSupplied<UserQuote>, FeedError>> + Send>;

/// The user's feed.
pub enum UserSource {
    /// A finite reader (CSV, JSON lines or DBN file, or standard input).
    Reader(QuoteIter),
    /// The vendor's live client.
    #[cfg(feature = "databento-live")]
    DatabentoLive(super::dbn_feed::live::LiveConfig),
    /// Quotes from a channel (tests, embedding).
    Channel(mpsc::Receiver<Result<UserSupplied<UserQuote>, FeedError>>),
}

impl std::fmt::Debug for UserSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Reader(_) => "Reader",
            #[cfg(feature = "databento-live")]
            Self::DatabentoLive(_) => "DatabentoLive",
            Self::Channel(_) => "Channel",
        })
    }
}

/// The bleh.xyz side.
pub enum BlehSource {
    /// A live gateway.
    Gateway(ClientConfig),
    /// A recording made with `mesh-client --record`.
    Recording(Box<dyn BufRead + Send>),
}

impl std::fmt::Debug for BlehSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gateway(c) => f.debug_tuple("Gateway").field(&c.urls).finish(),
            Self::Recording(_) => f.write_str("Recording"),
        }
    }
}

/// Where outputs go.
#[derive(Debug, Default)]
pub struct Outputs {
    /// The merged side-by-side stream.
    pub merged: Option<JsonlSink>,
    /// Tracking statistics (standard error when `None`).
    pub stats: Option<JsonlSink>,
    /// The local websocket.
    pub ws: Option<LocalWs>,
}

/// A merge failure.
#[derive(Debug, thiserror::Error)]
pub enum MergeError {
    /// The user's feed failed.
    #[error(transparent)]
    Feed(#[from] FeedError),
    /// The bleh.xyz side failed.
    #[error("bleh.xyz side: {0}")]
    Bleh(String),
    /// An output failed.
    #[error("output: {0}")]
    Output(#[from] std::io::Error),
}

struct State {
    aligner: Aligner,
    tracker: BlehTracker,
    stats: Vec<TrackingStats>,
    out: Outputs,
    clock: Clock,
    first_user: Option<i64>,
    first_bleh: Option<i64>,
    warned_far: bool,
}

const FAR_APART_NS: i64 = 86_400 * 1_000_000_000;

impl State {
    fn emit(&mut self, samples: Vec<Sample>) -> Result<(), MergeError> {
        for s in samples {
            if let Some(st) = self.stats.get_mut(s.pair) {
                st.push(s.ts_ns, s.user.get().adjusted, s.bleh.price);
            }
            if self.out.merged.is_none() && self.out.ws.is_none() {
                continue;
            }
            let line = sample_line(&s);
            if let Some(m) = self.out.merged.as_mut() {
                m.line(&line)?;
            }
            if let Some(ws) = &self.out.ws {
                ws.send(line.into());
            }
        }
        Ok(())
    }

    fn check_far_apart(&mut self) {
        if let (Some(u), Some(b), false) = (self.first_user, self.first_bleh, self.warned_far)
            && (u - b).abs() > FAR_APART_NS
        {
            self.warned_far = true;
            tracing::warn!(
                "the user feed and the bleh.xyz side start more than a day apart: no samples will align (pair files with recordings, live feeds with a live gateway)"
            );
        }
    }

    fn on_user(&mut self, q: UserSupplied<UserQuote>) -> Result<(), MergeError> {
        let q = if self.clock == Clock::Receive {
            let now = mesh_types::UnixNanos::now().0;
            q.map(|mut q| {
                q.ts_ns = now;
                q
            })
        } else {
            q
        };
        if self.first_user.is_none() && self.aligner.wants_symbol(&q.get().symbol) {
            self.first_user = Some(q.get().ts_ns);
            self.check_far_apart();
        }
        let s = self.aligner.push_user(&q);
        self.emit(s)
    }

    fn on_bleh(&mut self, input: &BlehInput) -> Result<(), MergeError> {
        let (obs, beat) = self.tracker.on_input(input);
        let now = mesh_types::UnixNanos::now().0;
        for mut o in obs {
            if self.clock == Clock::Receive {
                o.ts_ns = now;
            }
            if self.first_bleh.is_none() {
                self.first_bleh = Some(o.ts_ns);
                self.check_far_apart();
            }
            let s = self.aligner.push_bleh(&o);
            self.emit(s)?;
        }
        if let (Some(t), Clock::Event) = (beat, self.clock) {
            let s = self.aligner.bleh_clock(t);
            self.emit(s)?;
        }
        Ok(())
    }

    fn reports(&self) -> Vec<UserSupplied<TrackingReport>> {
        self.stats
            .iter()
            .enumerate()
            .filter(|(i, _)| self.aligner.counts(*i) != super::stats::InputCounts::default())
            .map(|(i, s)| UserSupplied::new(s.report(self.aligner.counts(i))))
            .collect()
    }

    fn write_stats(&mut self) -> Result<(), MergeError> {
        let reports = self.reports();
        for r in &reports {
            let line = report_line(r);
            match self.out.stats.as_mut() {
                Some(s) => {
                    s.line(&line)?;
                    s.flush()?;
                }
                None => eprintln!("{line}"),
            }
            if let Some(ws) = &self.out.ws {
                ws.send(Arc::from(line));
            }
        }
        Ok(())
    }
}

/// Runs a merge until the inputs end (files), the duration elapses, or
/// Ctrl-C (live). Returns the final statistics, also written to the stats
/// output.
///
/// # Errors
///
/// A source or output failure.
pub async fn run(
    cfg: MergeConfig,
    user: UserSource,
    bleh: BlehSource,
    out: Outputs,
) -> Result<Vec<UserSupplied<TrackingReport>>, MergeError> {
    let interval_ns = i64::try_from(cfg.interval.as_nanos())
        .unwrap_or(i64::MAX)
        .max(1);
    let aligner = Aligner::new(
        &cfg.mapping,
        AlignConfig {
            interval_ns,
            max_stale_ns: i64::try_from(cfg.max_stale.as_nanos()).unwrap_or(i64::MAX),
        },
    );
    let scfg = StatsConfig {
        interval_ns,
        max_lag_steps: usize::try_from(
            cfg.max_lag.as_nanos() / u128::try_from(interval_ns).unwrap_or(1),
        )
        .unwrap_or(0),
        window: cfg.window.max(1),
    };
    let stats = cfg
        .mapping
        .pairs
        .iter()
        .map(|p| TrackingStats::new(p.name.clone(), scfg))
        .collect();
    let mut st = State {
        aligner,
        tracker: BlehTracker::default(),
        stats,
        out,
        clock: cfg.clock,
        first_user: None,
        first_bleh: None,
        warned_far: false,
    };

    let finite = matches!(user, UserSource::Reader(_)) && matches!(bleh, BlehSource::Recording(_));
    let mut user_rx = spawn_user(user);
    let (mut bleh_rx, gateway) = spawn_bleh(bleh);

    let result = if finite {
        ordered(&mut st, &mut user_rx, &mut bleh_rx).await
    } else {
        live(&mut st, &cfg, &mut user_rx, &mut bleh_rx).await
    };
    if let Some(g) = gateway {
        g.abort();
    }
    result?;
    let s = st.aligner.flush();
    st.emit(s)?;
    if let Some(m) = st.out.merged.as_mut() {
        m.flush()?;
    }
    st.write_stats()?;
    Ok(st.reports())
}

type UserRx = mpsc::Receiver<Result<UserSupplied<UserQuote>, FeedError>>;
type BlehRx = mpsc::Receiver<Result<BlehInput, String>>;

fn spawn_user(user: UserSource) -> UserRx {
    match user {
        UserSource::Channel(rx) => rx,
        UserSource::Reader(iter) => {
            let (tx, rx) = mpsc::channel(4096);
            tokio::task::spawn_blocking(move || {
                for q in iter {
                    let stop = q.is_err();
                    if tx.blocking_send(q).is_err() || stop {
                        break;
                    }
                }
            });
            rx
        }
        #[cfg(feature = "databento-live")]
        UserSource::DatabentoLive(cfg) => {
            let (tx, rx) = mpsc::channel(4096);
            tokio::spawn(async move {
                if let Err(e) = super::dbn_feed::live::run(cfg, tx.clone()).await {
                    let _ = tx.send(Err(e)).await;
                }
            });
            rx
        }
    }
}

fn spawn_bleh(bleh: BlehSource) -> (BlehRx, Option<tokio::task::JoinHandle<()>>) {
    let (tx, rx) = mpsc::channel(4096);
    match bleh {
        BlehSource::Recording(reader) => {
            tokio::task::spawn_blocking(move || {
                for (n, line) in reader.lines().enumerate() {
                    let item = match line {
                        Ok(l) => match BlehInput::from_recorded_line(&l) {
                            Ok(None) => continue,
                            Ok(Some(i)) => Ok(i),
                            Err(e) => Err(format!("recording line {}: {e}", n + 1)),
                        },
                        Err(e) => Err(format!("recording: {e}")),
                    };
                    let stop = item.is_err();
                    if tx.blocking_send(item).is_err() || stop {
                        break;
                    }
                }
            });
            (rx, None)
        }
        BlehSource::Gateway(ccfg) => {
            // The client runs in its own task: `next_event` is not
            // cancel-safe, so it must not sit in a `select!`.
            let task = tokio::spawn(async move {
                let mut client = Client::new(ccfg);
                loop {
                    match client.next_event().await {
                        Ok(e) => {
                            log_event(&e);
                            if let Some(i) = BlehInput::from_event(&e)
                                && tx.send(Ok(i)).await.is_err()
                            {
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e.to_string())).await;
                            break;
                        }
                    }
                }
                client.close().await;
            });
            (rx, Some(task))
        }
    }
}

fn log_event(e: &crate::client::ClientEvent) {
    use crate::client::ClientEvent as E;
    match e {
        E::Connected { url, .. } => tracing::info!(%url, "connected to gateway"),
        E::ConnectFailed { url, error } => tracing::warn!(%url, %error, "connect failed"),
        E::Disconnected { url, reason } => tracing::warn!(%url, %reason, "disconnected"),
        E::VerificationFailed { detail } => tracing::error!(%detail, "verification failed"),
        E::ServerError { code, message, .. } => tracing::warn!(?code, %message, "server error"),
        _ => {}
    }
}

/// Both sources are finite: merge them in event-time order so memory stays
/// bounded however long the files are.
async fn ordered(
    st: &mut State,
    user_rx: &mut UserRx,
    bleh_rx: &mut BlehRx,
) -> Result<(), MergeError> {
    let mut pu: Option<UserSupplied<UserQuote>> = None;
    let mut pb: Option<BlehInput> = None;
    let (mut udone, mut bdone) = (false, false);
    loop {
        if pu.is_none() && !udone {
            match user_rx.recv().await {
                Some(q) => pu = Some(q?),
                None => udone = true,
            }
        }
        if pb.is_none() && !bdone {
            match bleh_rx.recv().await {
                Some(b) => pb = Some(b.map_err(MergeError::Bleh)?),
                None => bdone = true,
            }
        }
        match (pu.take(), pb.take()) {
            (None, None) => return Ok(()),
            (Some(u), None) => st.on_user(u)?,
            (None, Some(b)) => st.on_bleh(&b)?,
            (Some(u), Some(b)) => {
                if b.ts_ns().is_none_or(|bt| bt < u.get().ts_ns) {
                    st.on_bleh(&b)?;
                    pu = Some(u);
                } else {
                    st.on_user(u)?;
                    pb = Some(b);
                }
            }
        }
    }
}

/// At least one source is live: process inputs as they arrive.
async fn live(
    st: &mut State,
    cfg: &MergeConfig,
    user_rx: &mut UserRx,
    bleh_rx: &mut BlehRx,
) -> Result<(), MergeError> {
    let deadline = cfg.duration.map(|d| tokio::time::Instant::now() + d);
    let mut tick = tokio::time::interval(cfg.interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let stats_every = cfg
        .stats_every
        .unwrap_or(Duration::from_secs(3600 * 24 * 365));
    let mut stats_tick =
        tokio::time::interval_at(tokio::time::Instant::now() + stats_every, stats_every);
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    // Either side ending ends the session: nothing more can align.
    loop {
        tokio::select! {
            q = user_rx.recv() => match q {
                Some(q) => st.on_user(q?)?,
                None => {
                    tracing::info!("user feed ended");
                    return Ok(());
                }
            },
            b = bleh_rx.recv() => match b {
                Some(b) => st.on_bleh(&b.map_err(MergeError::Bleh)?)?,
                None => {
                    tracing::info!("bleh.xyz side ended");
                    return Ok(());
                }
            },
            _ = tick.tick(), if st.clock == Clock::Receive => {
                let s = st.aligner.both_clocks(mesh_types::UnixNanos::now().0);
                st.emit(s)?;
            },
            _ = stats_tick.tick(), if cfg.stats_every.is_some() => st.write_stats()?,
            () = async {
                match deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending().await,
                }
            } => return Ok(()),
            _ = &mut ctrl_c => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesh_types::{
        AdmissionId, AggressorSide, BlockBatch, InstrumentId, Price, Provenance, Record,
        RecordKind, Seq, Size, Trade, TradeId, UnixMillis, UnixNanos, VenueId,
    };

    const S: i64 = 1_000_000_000;
    const T0_MS: i64 = 1_767_225_600_000;

    fn mapping() -> Mapping {
        Mapping::parse(
            r#"
[[pair]]
name = "btc"
user_symbol = "MY-BTC"
[pair.bleh]
market = "hyperliquid:perp:BTC"
"#,
        )
        .unwrap()
    }

    fn cfg() -> MergeConfig {
        MergeConfig {
            mapping: mapping(),
            interval: Duration::from_secs(1),
            max_stale: Duration::from_secs(30),
            max_lag: Duration::from_secs(3),
            window: 1000,
            clock: Clock::Event,
            stats_every: None,
            duration: None,
        }
    }

    /// One synthetic recorded line: a trade batch for block `b` at time
    /// `T0 + b` seconds.
    fn batch_line(b: u64, px: &str) -> String {
        let inst: InstrumentId = "hyperliquid:perp:BTC".parse().unwrap();
        let venue: VenueId = "hyperliquid".parse().unwrap();
        let ms = T0_MS + i64::try_from(b).unwrap() * 1000;
        let prov = Provenance::new(
            inst.clone(),
            venue.clone(),
            Seq::new(b, 0),
            UnixMillis(ms),
            UnixNanos(ms * 1_000_000),
            AdmissionId::new("ADM-0001").unwrap(),
        );
        let trade = Trade {
            prov,
            price: Price::parse_exact(px).unwrap(),
            size: Size::parse_exact("1").unwrap(),
            aggressor: AggressorSide::Buy,
            trade_id: TradeId(b),
        };
        let batch = BlockBatch {
            instrument: inst,
            venue,
            kind: RecordKind::Trade,
            block_number: b,
            block_time_ms: UnixMillis(ms),
            prev_block_number: b.checked_sub(1),
            records: vec![Record::Trade(trade)],
        };
        serde_json::json!({"event": "batch", "batch": batch}).to_string()
    }

    #[tokio::test]
    async fn merges_a_synthetic_csv_with_a_synthetic_recording() {
        // The user's feed sits 0.5 above bleh.xyz, one second behind.
        let mut csv = String::from("ts,symbol,price\n");
        let mut rec = String::new();
        let px = |i: i64| {
            100.0
                + f64::from(i32::try_from(i % 7).unwrap()) * 0.25
                + f64::from(i32::try_from(i % 3).unwrap()) * 0.1
        };
        for i in 0..120i64 {
            csv.push_str(&format!(
                "{},MY-BTC,{}\n",
                (T0_MS / 1000 + i + 1) * S,
                px(i) + 0.5
            ));
            csv.push_str(&format!("{},OTHER,1\n", (T0_MS / 1000 + i) * S));
            rec.push_str(&batch_line(
                u64::try_from(i).unwrap(),
                &format!("{:.2}", px(i)),
            ));
            rec.push('\n');
        }
        rec.push_str("{\"event\":\"connected\",\"url\":\"x\"}\n");
        let feed = super::super::feed::TextFeed::new(
            super::super::feed::TextFormat::Csv,
            Box::new(std::io::Cursor::new(csv.into_bytes())),
            super::super::feed::Columns::default(),
        )
        .unwrap();
        let reports = run(
            cfg(),
            UserSource::Reader(Box::new(feed)),
            BlehSource::Recording(Box::new(std::io::Cursor::new(rec.into_bytes()))),
            Outputs::default(),
        )
        .await
        .unwrap();
        assert_eq!(reports.len(), 1);
        let r = reports[0].get();
        assert!(r.samples > 100, "{r:?}");
        let ll = r.lead_lag.unwrap();
        assert_eq!(
            ll.bleh_lead_ms, 1000,
            "bleh.xyz moves one second first: {ll:?}"
        );
        assert!(ll.corr > 0.99);
        assert_eq!(r.inputs.late, 0);
    }

    #[tokio::test]
    async fn bad_recording_lines_stop_the_merge() {
        let feed: QuoteIter = Box::new(std::iter::empty());
        let e = run(
            cfg(),
            UserSource::Reader(feed),
            BlehSource::Recording(Box::new(std::io::Cursor::new(b"not json\n".to_vec()))),
            Outputs::default(),
        )
        .await
        .unwrap_err();
        assert!(e.to_string().contains("recording line 1"), "{e}");
    }

    #[tokio::test]
    async fn live_mode_stops_when_a_side_ends() {
        let (tx, rx) = mpsc::channel(8);
        let ms = T0_MS;
        tx.send(Ok(UserSupplied::new(UserQuote::trade(
            "MY-BTC",
            ms * 1_000_000,
            100.0,
        ))))
        .await
        .unwrap();
        drop(tx);
        let rec = format!("{}\n{}\n", batch_line(0, "100"), batch_line(3, "101"));
        let reports = run(
            cfg(),
            UserSource::Channel(rx),
            BlehSource::Recording(Box::new(std::io::Cursor::new(rec.into_bytes()))),
            Outputs::default(),
        )
        .await
        .unwrap();
        assert_eq!(reports[0].get().inputs.user, 1);
    }
}
