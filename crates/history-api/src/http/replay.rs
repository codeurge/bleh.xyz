//! Websocket replay of archived records in the gateway-v0 message format
//! (`docs/protocol/gateway-v0.md`), so a client backtests with the same
//! code it runs live.
//!
//! ```text
//! GET /v0/replay?start=2026-01-01T22:00:00Z&end=2026-01-01T23:00:00Z&speed=10
//! ```
//!
//! `speed` is a multiplier of archive time (`1` = real time, `10` = ten
//! times faster, `0` = as fast as the connection allows). After `hello`
//! the client sends a normal `subscribe`; the server then replays, per
//! block, `batch` messages, then `record` messages (bars, emitted when
//! their interval has closed, never earlier), then a `block` heartbeat.
//! Book channels start with a `snapshot` reconstructed at `start`. When the
//! range is exhausted the server closes with code 1000.
//!
//! Differences from the live gateway, all documented in
//! `docs/operations/history-api.md`: heartbeats are sent only for blocks
//! with records in a subscribed channel (the archive does not record empty
//! blocks); `ts_publish_ns` is the replay send time; commitments and
//! proofs are not served (verify against the manifests instead); JSON
//! encoding only.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures::{SinkExt, StreamExt};
use mesh_types::{
    Bar, BarInterval, BlockBatch, InstrumentId, Record, RecordKind, Seq, UnixMillis, UnixNanos,
};
use serde::Deserialize;
use tokio::sync::mpsc;

use super::{AppState, Q, client_ip};
use crate::error::{ApiError, ErrorCode};
use crate::params::{TimeRange, parse_instrument, parse_interval, parse_time};
use crate::service::Service;
use crate::store::SeqKey;

const PAGE: usize = 5_000;
const MAX_CHANNELS: usize = 64;
const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new().route("/v0/replay", get(replay))
}

#[derive(Debug, Deserialize)]
struct ReplayQuery {
    start: String,
    end: Option<String>,
    speed: Option<f64>,
}

/// Decrements the per-IP replay count when the replay ends.
struct Guard {
    map: Arc<Mutex<HashMap<IpAddr, usize>>>,
    ip: IpAddr,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Ok(mut m) = self.map.lock()
            && let Some(n) = m.get_mut(&self.ip)
        {
            *n = n.saturating_sub(1);
            if *n == 0 {
                m.remove(&self.ip);
            }
        }
        crate::metrics::global().replay_delta(false);
    }
}

async fn replay(
    State(s): State<AppState>,
    headers: HeaderMap,
    connect: Option<axum::Extension<ConnectInfo<std::net::SocketAddr>>>,
    Q(q): Q<ReplayQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let limits = &s.svc.config.limits;
    let max = i64::try_from(limits.max_replay_range.as_millis()).unwrap_or(i64::MAX);
    let start = match parse_time("start", &q.start) {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    let end = match q.end.as_deref().map(|e| parse_time("end", e)).transpose() {
        Ok(e) => e.unwrap_or(UnixMillis(start.0.saturating_add(crate::params::HOUR_MS))),
        Err(e) => return e.into_response(),
    };
    let range = match TimeRange::new(start, end, max) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let speed = q.speed.unwrap_or(1.0);
    if !speed.is_finite() || speed < 0.0 || speed > limits.max_replay_speed {
        return ApiError::bad_request(format!(
            "speed must be between 0 (as fast as possible) and {}",
            limits.max_replay_speed
        ))
        .into_response();
    }
    let ip = client_ip(
        &headers,
        connect.map(|c| c.0.0),
        s.svc.config.trust_forwarded_for,
    );
    {
        let Ok(mut m) = s.replays.lock() else {
            return ApiError::internal("replay registry poisoned").into_response();
        };
        let n = m.entry(ip).or_insert(0);
        if *n >= limits.max_replays_per_ip {
            let mut e = ApiError::new(
                ErrorCode::RateLimited,
                format!(
                    "at most {} concurrent replays per address",
                    limits.max_replays_per_ip
                ),
            );
            e.retry_after = Some(5);
            return e.into_response();
        }
        *n += 1;
    }
    crate::metrics::global().replay_delta(true);
    let guard = Guard {
        map: s.replays.clone(),
        ip,
    };
    let svc = s.svc.clone();
    ws.protocols(["bleh.v0.json"])
        .max_message_size(64 * 1024)
        .on_upgrade(move |socket| async move {
            let _guard = guard;
            run(socket, svc, range, speed).await;
        })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChannelReq {
    instrument: String,
    kind: String,
    depth: Option<String>,
    interval: Option<String>,
}

// Every gateway-v0 field is parsed (unknown fields are rejected), even
// those replay does not act on.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum ClientMsg {
    Subscribe {
        id: String,
        channels: Vec<ChannelReq>,
        #[serde(default)]
        commitments: bool,
    },
    Unsubscribe {
        id: String,
        channels: Vec<ChannelReq>,
    },
    Resnapshot {
        id: String,
        instrument: String,
        #[serde(default)]
        reason: Option<String>,
    },
    Proof {
        id: String,
        #[serde(default)]
        stream: serde_json::Value,
        #[serde(default)]
        block_number: u64,
        #[serde(default)]
        instrument: String,
        #[serde(default)]
        event_index: u32,
    },
    Ping {
        id: String,
    },
}

enum Ctl {
    Msg(ClientMsg),
    Invalid(String),
    Closed,
}

fn error_msg(id: Option<&str>, code: &str, message: &str) -> serde_json::Value {
    let mut v = serde_json::json!({"type": "error", "code": code, "message": message});
    if let Some(id) = id {
        v["id"] = serde_json::Value::String(id.to_owned());
    }
    v
}

fn now_ns() -> String {
    UnixNanos::now().to_string()
}

struct Sender {
    tx: futures::stream::SplitSink<WebSocket, Message>,
    sent: u64,
}

impl Sender {
    async fn send(&mut self, v: &serde_json::Value) -> bool {
        self.sent += 1;
        self.tx
            .send(Message::Text(v.to_string().into()))
            .await
            .is_ok()
    }

    async fn close(&mut self, code: u16, reason: &str) {
        let _ = self
            .tx
            .send(Message::Close(Some(CloseFrame {
                code,
                reason: reason.into(),
            })))
            .await;
    }
}

/// A batched channel (`trade` or `book_event`).
struct Source {
    id: InstrumentId,
    kind: RecordKind,
    depth: Option<String>,
    key: String,
    buf: VecDeque<Record>,
    after: Option<SeqKey>,
    exhausted: bool,
    last_block: Option<u64>,
    active: bool,
}

impl Source {
    async fn fill(&mut self, svc: &Service, range: TimeRange) -> Result<(), ApiError> {
        if self.exhausted {
            return Ok(());
        }
        let page: Vec<Record> = match self.kind {
            RecordKind::Trade => svc
                .store
                .trades(&self.id, range.start_ms, range.end_ms, self.after, PAGE)
                .await?
                .into_iter()
                .map(Record::Trade)
                .collect(),
            _ => svc
                .store
                .book_events(&self.id, range.start_ms, range.end_ms, self.after, PAGE)
                .await?
                .into_iter()
                .map(Record::BookEvent)
                .collect(),
        };
        if page.len() < PAGE {
            self.exhausted = true;
        }
        if let Some(last) = page.last() {
            let s = last.prov().seq;
            self.after = Some((s.block_number, s.event_index));
        }
        self.buf.extend(page);
        Ok(())
    }

    fn front_block(&self) -> Option<(u64, i64)> {
        self.buf
            .front()
            .map(|r| (r.prov().seq.block_number, r.prov().ts_source_ms.0))
    }

    /// Takes every record of `block`, fetching more pages if the buffer
    /// ends inside the block.
    async fn take_block(
        &mut self,
        svc: &Service,
        range: TimeRange,
        block: u64,
    ) -> Result<Vec<Record>, ApiError> {
        loop {
            let ends_inside = self
                .buf
                .back()
                .is_some_and(|r| r.prov().seq.block_number == block);
            if ends_inside && !self.exhausted {
                self.fill(svc, range).await?;
            } else {
                break;
            }
        }
        let mut out = Vec::new();
        while self
            .buf
            .front()
            .is_some_and(|r| r.prov().seq.block_number == block)
        {
            if let Some(r) = self.buf.pop_front() {
                out.push(r);
            }
        }
        Ok(out)
    }
}

/// A bar channel: bars are emitted once their interval has closed.
struct BarSource {
    id: InstrumentId,
    interval: BarInterval,
    key: String,
    buf: VecDeque<Bar>,
    after_start: Option<i64>,
    exhausted: bool,
    active: bool,
}

impl BarSource {
    async fn fill(&mut self, svc: &Service, range: TimeRange) -> Result<(), ApiError> {
        if self.exhausted || !self.buf.is_empty() {
            return Ok(());
        }
        let len = self.interval.millis();
        // Bars that close inside the replay range.
        let lo = range.start_ms.saturating_sub(len);
        let hi = range.end_ms.saturating_sub(len).saturating_add(1);
        if hi <= lo {
            self.exhausted = true;
            return Ok(());
        }
        let page = svc
            .store
            .bars(&self.id, self.interval, lo, hi, self.after_start, PAGE)
            .await?;
        if page.len() < PAGE {
            self.exhausted = true;
        }
        if let Some(last) = page.last() {
            self.after_start = Some(last.start_ms.0);
        }
        self.buf.extend(page);
        Ok(())
    }

    fn close_ms(&self, b: &Bar) -> i64 {
        b.start_ms.0 + self.interval.millis()
    }
}

#[allow(clippy::too_many_lines)]
async fn run(socket: WebSocket, svc: Service, range: TimeRange, speed: f64) {
    let (tx, mut rx) = socket.split();
    let mut out = Sender { tx, sent: 0 };
    let (ctl_tx, mut ctl) = mpsc::channel::<Ctl>(64);
    let reader = tokio::spawn(async move {
        while let Some(msg) = rx.next().await {
            let ctl = match msg {
                Ok(Message::Text(t)) => match serde_json::from_str::<ClientMsg>(t.as_str()) {
                    Ok(m) => Ctl::Msg(m),
                    Err(e) => Ctl::Invalid(e.to_string()),
                },
                Ok(Message::Binary(_)) => Ctl::Invalid("binary frames are not accepted".into()),
                Ok(Message::Close(_)) | Err(_) => Ctl::Closed,
                Ok(_) => continue,
            };
            let closed = matches!(ctl, Ctl::Closed);
            if ctl_tx.send(ctl).await.is_err() || closed {
                break;
            }
        }
        let _ = ctl_tx.send(Ctl::Closed).await;
    });

    let l = &svc.config.limits;
    let hello = serde_json::json!({
        "type": "hello",
        "protocol": "bleh-gateway-v0",
        "server_time_ns": now_ns(),
        "signers": [],
        "limits": {
            "connections_per_ip": l.max_replays_per_ip,
            "channels_per_connection": MAX_CHANNELS,
        },
    });
    if !out.send(&hello).await {
        reader.abort();
        return;
    }

    // Wait for the subscription.
    let (sub_id, channels) = loop {
        match tokio::time::timeout(SUBSCRIBE_TIMEOUT, ctl.recv()).await {
            Ok(Some(Ctl::Msg(ClientMsg::Subscribe {
                id,
                channels,
                commitments,
            }))) => {
                if commitments {
                    let _ = out
                        .send(&error_msg(
                            Some(&id),
                            "unsupported_kind",
                            "replay does not send commitments; verify files against the signed manifests",
                        ))
                        .await;
                }
                break (id, channels);
            }
            Ok(Some(Ctl::Msg(ClientMsg::Ping { id }))) => {
                let _ = out
                    .send(
                        &serde_json::json!({"type": "pong", "id": id, "server_time_ns": now_ns()}),
                    )
                    .await;
            }
            Ok(Some(Ctl::Msg(_))) => {
                let _ = out
                    .send(&error_msg(None, "bad_request", "send subscribe first"))
                    .await;
            }
            Ok(Some(Ctl::Invalid(e))) => {
                let _ = out.send(&error_msg(None, "bad_request", &e)).await;
            }
            Ok(Some(Ctl::Closed) | None) => {
                reader.abort();
                return;
            }
            Err(_) => {
                out.close(4000, "no subscribe within 30 seconds").await;
                reader.abort();
                return;
            }
        }
    };
    if channels.is_empty() || channels.len() > MAX_CHANNELS {
        let _ = out
            .send(&error_msg(
                Some(&sub_id),
                "bad_request",
                &format!("subscribe to 1 to {MAX_CHANNELS} channels"),
            ))
            .await;
        out.close(4000, "bad subscription").await;
        reader.abort();
        return;
    }

    let mut sources: Vec<Source> = Vec::new();
    let mut bar_sources: Vec<BarSource> = Vec::new();
    let mut accepted = Vec::new();
    for c in channels {
        let id = match parse_instrument(&c.instrument) {
            Ok(id) => id,
            Err(e) => {
                let _ = out
                    .send(&error_msg(Some(&sub_id), "bad_request", &e.message))
                    .await;
                continue;
            }
        };
        if svc.store.require_instrument(&id).await.is_err() {
            let _ = out
                .send(&error_msg(
                    Some(&sub_id),
                    "unknown_instrument",
                    &format!("{id} is not in the registry or the archive"),
                ))
                .await;
            continue;
        }
        let key_kind = c.kind.clone();
        match c.kind.as_str() {
            "trade" | "book_event" => {
                let kind = if c.kind == "trade" {
                    RecordKind::Trade
                } else {
                    RecordKind::BookEvent
                };
                if let Some(d) = &c.depth
                    && (kind == RecordKind::Trade || !matches!(d.as_str(), "l4" | "l2"))
                {
                    let _ = out
                        .send(&error_msg(
                            Some(&sub_id),
                            "unsupported_kind",
                            "depth is l4 or l2 on book_event",
                        ))
                        .await;
                    continue;
                }
                sources.push(Source {
                    key: format!("{id}|{key_kind}"),
                    id,
                    kind,
                    depth: c.depth.clone(),
                    buf: VecDeque::new(),
                    after: None,
                    exhausted: false,
                    last_block: None,
                    active: true,
                });
            }
            "bar" => {
                let interval = match parse_interval(c.interval.as_deref().unwrap_or("1m")) {
                    Ok(i) => i,
                    Err(e) => {
                        let _ = out
                            .send(&error_msg(Some(&sub_id), "unsupported_kind", &e.message))
                            .await;
                        continue;
                    }
                };
                bar_sources.push(BarSource {
                    key: format!("{id}|bar"),
                    id,
                    interval,
                    buf: VecDeque::new(),
                    after_start: None,
                    exhausted: false,
                    active: true,
                });
            }
            other => {
                let _ = out
                    .send(&error_msg(
                        Some(&sub_id),
                        "unsupported_kind",
                        &format!("replay offers trade, book_event and bar, not {other}"),
                    ))
                    .await;
                continue;
            }
        }
        accepted.push(serde_json::json!({
            "instrument": c.instrument,
            "kind": c.kind,
            "depth": c.depth,
            "interval": c.interval,
        }));
    }
    if !out
        .send(&serde_json::json!({"type": "subscribed", "id": sub_id, "channels": accepted}))
        .await
    {
        reader.abort();
        return;
    }

    // Snapshots for book channels, reconstructed at the replay start.
    for src in sources
        .iter_mut()
        .filter(|s| s.kind == RecordKind::BookEvent)
    {
        if !send_snapshot(&svc, &mut out, src, range.start_ms - 1, "subscribe").await {
            src.active = false;
        }
    }

    let wall0 = Instant::now();
    let mut t0: Option<i64> = None;
    let mut last_time = range.start_ms;
    let mut aborted = false;
    'blocks: loop {
        // Control messages.
        while let Ok(c) = ctl.try_recv() {
            if !handle_ctl(c, &svc, &mut out, &mut sources, &mut bar_sources, last_time).await {
                aborted = true;
                break 'blocks;
            }
        }
        // Next block across channels.
        let mut next: Option<(u64, i64)> = None;
        for src in sources.iter_mut().filter(|s| s.active) {
            if src.buf.is_empty()
                && let Err(e) = src.fill(&svc, range).await
            {
                let _ = out
                    .send(&error_msg(None, e.code.as_str(), &e.message))
                    .await;
                aborted = true;
                break 'blocks;
            }
            if let Some(fb) = src.front_block()
                && next.is_none_or(|n| fb.0 < n.0)
            {
                next = Some(fb);
            }
        }
        for b in bar_sources.iter_mut().filter(|b| b.active) {
            if let Err(e) = b.fill(&svc, range).await {
                let _ = out
                    .send(&error_msg(None, e.code.as_str(), &e.message))
                    .await;
                aborted = true;
                break 'blocks;
            }
        }
        let Some((block, block_time)) = next else {
            break;
        };

        // Pacing.
        if speed > 0.0 {
            let base = *t0.get_or_insert(block_time);
            #[allow(clippy::cast_precision_loss)]
            let offset = (block_time - base).max(0) as f64 / 1000.0 / speed;
            let target = wall0 + Duration::from_secs_f64(offset);
            loop {
                tokio::select! {
                    () = tokio::time::sleep_until(target.into()) => break,
                    c = ctl.recv() => {
                        let c = c.unwrap_or(Ctl::Closed);
                        if !handle_ctl(c, &svc, &mut out, &mut sources, &mut bar_sources, last_time).await {
                            aborted = true;
                            break 'blocks;
                        }
                    }
                }
            }
        }

        let publish = UnixNanos::now();
        let mut last_map = BTreeMap::new();
        for src in sources.iter_mut().filter(|s| s.active) {
            if src.front_block().is_some_and(|(b, _)| b == block) {
                let mut records = match src.take_block(&svc, range, block).await {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = out
                            .send(&error_msg(None, e.code.as_str(), &e.message))
                            .await;
                        aborted = true;
                        break 'blocks;
                    }
                };
                for r in &mut records {
                    stamp(r, publish);
                }
                let Some(venue) = records.first().map(|r| r.prov().venue.clone()) else {
                    continue;
                };
                let batch = BlockBatch {
                    instrument: src.id.clone(),
                    venue,
                    kind: src.kind,
                    block_number: block,
                    block_time_ms: UnixMillis(block_time),
                    prev_block_number: src.last_block,
                    records,
                };
                if !out
                    .send(&serde_json::json!({"type": "batch", "batch": batch}))
                    .await
                {
                    aborted = true;
                    break 'blocks;
                }
                src.last_block = Some(block);
            }
            if let Some(b) = src.last_block {
                last_map.insert(src.key.clone(), b);
            }
        }
        for b in bar_sources.iter_mut().filter(|b| b.active) {
            while b
                .buf
                .front()
                .is_some_and(|bar| b.close_ms(bar) <= block_time)
            {
                if let Some(mut bar) = b.buf.pop_front() {
                    bar.prov.stamp_publish(publish);
                    if !out
                        .send(&serde_json::json!({"type": "record", "record": Record::Bar(bar)}))
                        .await
                    {
                        aborted = true;
                        break 'blocks;
                    }
                }
                if b.buf.is_empty()
                    && let Err(e) = b.fill(&svc, range).await
                {
                    let _ = out
                        .send(&error_msg(None, e.code.as_str(), &e.message))
                        .await;
                    aborted = true;
                    break 'blocks;
                }
            }
        }
        let hb = serde_json::json!({
            "type": "block",
            "block_number": block,
            "block_time_ms": block_time,
            "ts_publish_ns": publish.to_string(),
            "last": last_map,
        });
        if !out.send(&hb).await {
            aborted = true;
            break;
        }
        last_time = block_time;
    }
    if !aborted {
        // Bars that close by the end of the range.
        let publish = UnixNanos::now();
        for b in bar_sources.iter_mut().filter(|b| b.active) {
            loop {
                while let Some(bar) = b.buf.front() {
                    if b.close_ms(bar) > range.end_ms {
                        b.buf.clear();
                        b.exhausted = true;
                        break;
                    }
                    if let Some(mut bar) = b.buf.pop_front() {
                        bar.prov.stamp_publish(publish);
                        let _ = out
                            .send(
                                &serde_json::json!({"type": "record", "record": Record::Bar(bar)}),
                            )
                            .await;
                    }
                }
                if b.exhausted || b.fill(&svc, range).await.is_err() || b.buf.is_empty() {
                    break;
                }
            }
        }
        out.close(1000, "replay complete").await;
    }
    crate::metrics::global().add_replay_messages(out.sent);
    reader.abort();
}

fn stamp(r: &mut Record, at: UnixNanos) {
    match r {
        Record::Trade(t) => t.prov.stamp_publish(at),
        Record::BookEvent(e) => e.prov.stamp_publish(at),
        Record::BookSnapshot(s) => s.prov.stamp_publish(at),
        Record::Bar(b) => b.prov.stamp_publish(at),
        Record::Footprint(f) => f.prov.stamp_publish(at),
        Record::VolumeProfile(v) => v.prov.stamp_publish(at),
        Record::SwapPrint(p) => p.prov.stamp_publish(at),
    }
}

/// Sends a snapshot for a book channel as of `at_ms`; on failure sends an
/// error and returns false.
async fn send_snapshot(
    svc: &Service,
    out: &mut Sender,
    src: &mut Source,
    at_ms: i64,
    reason: &str,
) -> bool {
    match svc.build_book(&src.id, at_ms).await {
        Ok((book, snap)) => {
            let l2 = src.depth.as_deref() == Some("l2");
            let record = if matches!(snap.depth, mesh_types::SnapshotDepth::L2 { .. }) {
                Ok(snap.clone())
            } else {
                book.to_snapshot(&snap.prov, l2, None)
            };
            match record {
                Ok(mut rec) => {
                    rec.prov.stamp_publish(UnixNanos::now());
                    let as_of = rec.prov.seq;
                    // The last block that had a book event for this
                    // instrument at or before the snapshot.
                    let prev = if book.events_applied > 0 {
                        book.last_seq.block_number
                    } else {
                        snap.prov.seq.block_number
                    };
                    src.last_block = Some(prev.min(as_of.block_number));
                    out.send(&serde_json::json!({
                        "type": "snapshot",
                        "instrument": src.id.to_string(),
                        "as_of": Seq::end_of_block(as_of.block_number),
                        "reason": reason,
                        "record": Record::BookSnapshot(rec),
                    }))
                    .await
                }
                Err(e) => {
                    let _ = out
                        .send(&error_msg(None, e.code.as_str(), &e.message))
                        .await;
                    false
                }
            }
        }
        Err(e) => {
            let _ = out
                .send(&error_msg(
                    None,
                    "bad_request",
                    &format!("{}: {}", src.id, e.message),
                ))
                .await;
            false
        }
    }
}

/// Handles one control message during the replay. Returns false when the
/// replay must stop.
async fn handle_ctl(
    c: Ctl,
    svc: &Service,
    out: &mut Sender,
    sources: &mut [Source],
    bars: &mut [BarSource],
    last_time: i64,
) -> bool {
    match c {
        Ctl::Closed => false,
        Ctl::Invalid(e) => out.send(&error_msg(None, "bad_request", &e)).await,
        Ctl::Msg(ClientMsg::Ping { id }) => {
            out.send(&serde_json::json!({"type": "pong", "id": id, "server_time_ns": now_ns()}))
                .await
        }
        Ctl::Msg(ClientMsg::Subscribe { id, .. }) => {
            out.send(&error_msg(
                Some(&id),
                "bad_request",
                "replay channels are fixed at the start; open another replay",
            ))
            .await
        }
        Ctl::Msg(ClientMsg::Unsubscribe { id, channels }) => {
            for c in &channels {
                let key = format!("{}|{}", c.instrument, c.kind);
                for s in sources.iter_mut().filter(|s| s.key == key) {
                    s.active = false;
                }
                for b in bars.iter_mut().filter(|b| b.key == key) {
                    b.active = false;
                }
            }
            let names: Vec<serde_json::Value> = channels
                .iter()
                .map(|c| serde_json::json!({"instrument": c.instrument, "kind": c.kind}))
                .collect();
            out.send(&serde_json::json!({"type": "unsubscribed", "id": id, "channels": names}))
                .await
        }
        Ctl::Msg(ClientMsg::Resnapshot { id, instrument, .. }) => {
            let key = format!("{instrument}|book_event");
            let Some(src) = sources.iter_mut().find(|s| s.key == key && s.active) else {
                return out
                    .send(&error_msg(
                        Some(&id),
                        "bad_request",
                        "not subscribed to that book",
                    ))
                    .await;
            };
            // The stream position is the end of the last block sent.
            let _ = send_snapshot(svc, out, src, last_time, "resnapshot").await;
            true
        }
        Ctl::Msg(ClientMsg::Proof { id, .. }) => {
            out.send(&error_msg(
                Some(&id),
                "proof_unavailable",
                "replay does not serve proofs; verify files against the signed manifests",
            ))
            .await
        }
    }
}
