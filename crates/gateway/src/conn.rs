//! One websocket connection: subscriptions, ordering, limits.
//!
//! Each connection reads the fan-out ring with its own cursor and forwards
//! the items that match its channels, in ring order, which is the per-block
//! order of gateway-v0 § 5.1 (batches, records, commitments, heartbeat).
//! Book snapshots come from hub snapshot items on the same ring, so the
//! batches that follow a snapshot are exactly those after its block.
//!
//! Slow consumers (§ 6): when the next unread item is older than the send
//! buffer time, or the ring overran the cursor, or a write blocks for that
//! long, the connection drops everything queued, resynchronizes every book
//! with a `slow_consumer` snapshot and continues; a second strike within
//! the slow-consumer window closes it with 4008. Nothing is dropped
//! selectively.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::IpAddr;
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use bus::{RecvError, RingReceiver};
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use mesh_client::codec;
use mesh_client::protocol::{PROTOCOL, channel_key, close};
use mesh_client::{
    ChannelSpec, ClientMessage, Depth, Encoding, ErrorCode, Hello, InstrumentPattern,
    ServerMessage, SnapshotReason,
};
use mesh_types::{BarInterval, InstrumentId, RecordKind, UnixNanos, VenueId};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use crate::Shared;
use crate::item::{Data, Frame, Item, Route, SnapshotItem, encode_frame};
use crate::limits::{IpGuard, TokenBucket, WindowCounter};

/// Why a connection ends.
#[derive(Debug)]
enum Exit {
    /// Send a close frame with this code and reason.
    Close(u16, &'static str),
    /// The peer is gone or unresponsive: just drop.
    Drop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BookState {
    Pending(SnapshotReason),
    Synced,
}

#[derive(Debug, Default)]
struct KindIndex {
    all: bool,
    prefixes: Vec<String>,
    exact: HashSet<InstrumentId>,
}

impl KindIndex {
    fn add(&mut self, p: &InstrumentPattern) {
        match p {
            InstrumentPattern::All => self.all = true,
            InstrumentPattern::Prefix(s) => self.prefixes.push(s.clone()),
            InstrumentPattern::Exact(i) => {
                self.exact.insert(i.clone());
            }
        }
    }

    fn matches(&self, id: &InstrumentId) -> bool {
        self.all
            || self.exact.contains(id)
            || self.prefixes.iter().any(|p| id.as_str().starts_with(p))
    }
}

fn kind_slot(kind: RecordKind) -> usize {
    RecordKind::ALL.iter().position(|k| *k == kind).unwrap_or(0)
}

/// True if a pattern can cover instruments of `venue`.
fn covers_venue(p: &InstrumentPattern, venue: &VenueId) -> bool {
    match p {
        InstrumentPattern::All => true,
        InstrumentPattern::Exact(i) => i.venue_part() == venue.as_str(),
        InstrumentPattern::Prefix(s) => match s.split_once(':') {
            Some((v, _)) => v == venue.as_str(),
            None => venue.as_str().starts_with(s.as_str()),
        },
    }
}

#[derive(Debug, Default)]
struct Subs {
    channels: Vec<ChannelSpec>,
    index: [KindIndex; RecordKind::ALL.len()],
    commitments: bool,
    books: HashMap<InstrumentId, (BookState, Depth)>,
    last: HashMap<(InstrumentId, RecordKind), u64>,
}

impl Subs {
    fn rebuild(&mut self) {
        self.index = Default::default();
        for c in &self.channels {
            if c.kind != RecordKind::Bar || c.interval.is_none() {
                self.index[kind_slot(c.kind)].add(&c.instrument);
            }
        }
    }

    fn matches(&self, kind: RecordKind, id: &InstrumentId, interval: Option<BarInterval>) -> bool {
        if self.index[kind_slot(kind)].matches(id) {
            return true;
        }
        kind == RecordKind::Bar
            && self.channels.iter().any(|c| {
                c.kind == RecordKind::Bar
                    && c.interval.is_some()
                    && c.interval == interval
                    && c.instrument.matches(id)
            })
    }

    fn book_depth(&self, id: &InstrumentId) -> Option<Depth> {
        self.channels
            .iter()
            .find(|c| c.kind == RecordKind::BookEvent && c.instrument.matches(id))
            .map(|c| c.depth.unwrap_or_default())
    }

    fn covers_commitment(&self, venue: &VenueId, kind: RecordKind) -> bool {
        self.commitments
            && self
                .channels
                .iter()
                .any(|c| c.kind == kind && covers_venue(&c.instrument, venue))
    }
}

struct Conn<S> {
    shared: Arc<Shared>,
    sink: SplitSink<WebSocketStream<S>, Message>,
    enc: Encoding,
    guard: IpGuard,
    subs: Subs,
    msg_bucket: TokenBucket,
    proof_bucket: TokenBucket,
    violations: WindowCounter,
    last_strike: Option<Instant>,
    resnapshot_at: HashMap<InstrumentId, Instant>,
    send_timeout: Duration,
    unflushed: bool,
}

/// Runs a connection to completion. `encoding` is `None` when the client
/// offered only unsupported subprotocols.
pub(crate) async fn run<S>(
    shared: Arc<Shared>,
    ws: WebSocketStream<S>,
    encoding: Option<Encoding>,
    guard: IpGuard,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sink, stream) = ws.split();
    let Some(enc) = encoding else {
        shared.metrics.rejected_subprotocol.inc();
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            sink.send(close_message(close::UNSUPPORTED, "unsupported subprotocol")),
        )
        .await;
        return;
    };
    let ip: IpAddr = guard.ip();
    let limits = shared.config.limits.clone();
    let rx = shared.subscribe_fanout();
    shared.metrics.connections.inc();
    shared.metrics.connections_total.inc();
    let mut conn = Conn {
        msg_bucket: TokenBucket::new(limits.messages_per_second, limits.message_burst),
        proof_bucket: TokenBucket::new(limits.proofs_per_second, limits.proofs_per_second),
        violations: WindowCounter::new(shared.config.violation_window),
        send_timeout: shared.config.send_buffer_time(),
        shared: Arc::clone(&shared),
        sink,
        enc,
        guard,
        subs: Subs::default(),
        last_strike: None,
        resnapshot_at: HashMap::new(),
        unflushed: false,
    };
    tracing::debug!(%ip, ?enc, "connection open");
    let exit = conn.serve(stream, rx).await;
    match exit {
        Exit::Close(code, reason) => {
            tracing::debug!(%ip, code, reason, "closing connection");
            let _ = tokio::time::timeout(
                Duration::from_millis(500),
                conn.sink.send(close_message(code, reason)),
            )
            .await;
        }
        Exit::Drop => tracing::debug!(%ip, "dropping connection"),
    }
    shared.metrics.connections.dec();
}

fn close_message(code: u16, reason: &'static str) -> Message {
    Message::Close(Some(CloseFrame {
        code: CloseCode::from(code),
        reason: reason.into(),
    }))
}

impl<S> Conn<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    async fn serve(
        &mut self,
        mut stream: SplitStream<WebSocketStream<S>>,
        mut rx: RingReceiver<Item>,
    ) -> Exit {
        let hello = ServerMessage::Hello(Hello {
            protocol: PROTOCOL.into(),
            server_time_ns: UnixNanos::now(),
            signers: self.shared.signers(),
            limits: self.shared.config.limits.clone(),
        });
        if let Err(e) = self.send(&hello).await {
            return e;
        }
        if let Err(e) = self.flush().await {
            return e;
        }
        let mut shutdown = self.shared.shutdown.clone();
        loop {
            let r = tokio::select! {
                biased;
                _ = shutdown.changed() => Err(Exit::Close(close::GOING_AWAY, "server going away")),
                incoming = stream.next() => match incoming {
                    Some(Ok(Message::Text(t))) => self.on_client(t.as_bytes(), Encoding::Json).await,
                    Some(Ok(Message::Binary(b))) => self.on_client(&b, Encoding::Cbor).await,
                    Some(Ok(Message::Close(_))) | None => Err(Exit::Drop),
                    Some(Ok(_)) => Ok(()),
                    Some(Err(e)) => Err(match e {
                        tokio_tungstenite::tungstenite::Error::Capacity(_) => {
                            Exit::Close(close::TOO_BIG, "message too big")
                        }
                        _ => Exit::Drop,
                    }),
                },
                item = rx.recv() => self.on_ring(item, &mut rx).await,
            };
            if let Err(exit) = r {
                return exit;
            }
        }
    }

    async fn on_ring(
        &mut self,
        first: Result<Arc<Item>, RecvError>,
        rx: &mut RingReceiver<Item>,
    ) -> Result<(), Exit> {
        let mut next = Some(first);
        let mut budget = 256;
        while let Some(r) = next.take() {
            match r {
                Ok(item) => {
                    if rx.next_age().is_some_and(|age| age > self.send_timeout) {
                        return self.strike(rx);
                    }
                    self.on_item(&item).await?;
                }
                Err(RecvError::Lagged(_)) => return self.strike(rx),
                Err(RecvError::Closed) => {
                    self.flush().await?;
                    return Err(Exit::Close(close::GOING_AWAY, "server going away"));
                }
            }
            budget -= 1;
            if budget > 0 {
                next = rx.try_recv().transpose();
            }
        }
        self.flush().await
    }

    fn strike(&mut self, rx: &mut RingReceiver<Item>) -> Result<(), Exit> {
        let now = Instant::now();
        let window = self.shared.config.slow_consumer_window;
        if self
            .last_strike
            .is_some_and(|t| now.duration_since(t) < window)
        {
            self.shared.metrics.slow_consumer_disconnects.inc();
            return Err(Exit::Close(close::SLOW_CONSUMER, "slow consumer"));
        }
        self.last_strike = Some(now);
        self.shared.metrics.slow_consumer_resnapshots.inc();
        rx.skip_to_head();
        let books: Vec<InstrumentId> = self.subs.books.keys().cloned().collect();
        for inst in books {
            if let Some(entry) = self.subs.books.get_mut(&inst) {
                entry.0 = BookState::Pending(SnapshotReason::SlowConsumer);
            }
            self.shared.request_snapshot(inst);
        }
        Ok(())
    }

    async fn on_item(&mut self, item: &Item) -> Result<(), Exit> {
        match item {
            Item::Data(d) => self.on_data(d).await,
            Item::Snapshot(s) => self.on_snapshot(s).await,
            Item::Block(b) => {
                let venue = b.venue.as_str();
                let last: BTreeMap<String, u64> = self
                    .subs
                    .last
                    .iter()
                    .filter(|((i, _), _)| i.venue_part() == venue)
                    .map(|((i, k), b)| (channel_key(i, *k), *b))
                    .collect();
                let msg = ServerMessage::Block {
                    venue: b.venue.clone(),
                    block_number: b.block_number,
                    block_time_ms: b.block_time_ms,
                    ts_publish_ns: b.ts_publish_ns,
                    last,
                };
                self.send(&msg).await
            }
        }
    }

    async fn on_data(&mut self, d: &Data) -> Result<(), Exit> {
        let wanted = match &d.route {
            Route::Batch(inst, kind) => {
                if !self.subs.matches(*kind, inst, None) {
                    return Ok(());
                }
                if *kind == RecordKind::BookEvent && !self.subs.books.contains_key(inst) {
                    // A new instrument under a wildcard channel.
                    let depth = self.subs.book_depth(inst).unwrap_or_default();
                    self.subs.books.insert(
                        inst.clone(),
                        (BookState::Pending(SnapshotReason::Subscribe), depth),
                    );
                    self.shared.request_snapshot(inst.clone());
                }
                let e = self.subs.last.entry((inst.clone(), *kind)).or_insert(0);
                *e = (*e).max(d.block_number);
                true
            }
            Route::Record(inst, kind, interval) => self.subs.matches(*kind, inst, *interval),
            Route::Commitment(venue, kind) => self.subs.covers_commitment(venue, *kind),
        };
        if !wanted {
            return Ok(());
        }
        let frame = d.frame(self.enc).map_err(|e| {
            tracing::error!(error = %e, "cannot encode message");
            Exit::Close(close::PROTOCOL_ERROR, "internal encoding error")
        })?;
        self.write(frame).await?;
        let now = UnixNanos::now().0;
        if let Some(t) = d.ts_ingest_ns {
            self.shared.metrics.send_latency.observe_nanos(now - t.0);
        }
        if let Some(t) = d.ts_publish_ns {
            self.shared.metrics.publish_to_send.observe_nanos(now - t.0);
        }
        Ok(())
    }

    async fn on_snapshot(&mut self, s: &SnapshotItem) -> Result<(), Exit> {
        let Some(depth) = self.subs.book_depth(&s.instrument) else {
            return Ok(());
        };
        let state = self
            .subs
            .books
            .get(&s.instrument)
            .map_or(BookState::Pending(SnapshotReason::Subscribe), |e| e.0);
        let reason = match (state, s.forced) {
            (BookState::Pending(SnapshotReason::Subscribe), _) => SnapshotReason::Subscribe,
            (BookState::Pending(r), forced) => forced.unwrap_or(r),
            (BookState::Synced, Some(r)) => r,
            (BookState::Synced, None) => return Ok(()),
        };
        let Some(record) = s.record(depth) else {
            return Ok(());
        };
        let msg = ServerMessage::Snapshot {
            instrument: s.instrument.clone(),
            as_of: s.as_of,
            reason,
            record,
        };
        self.send(&msg).await?;
        self.shared.metrics.snapshots_sent.inc();
        if !matches!(
            reason,
            SnapshotReason::Subscribe | SnapshotReason::Resnapshot
        ) {
            self.shared.metrics.forced_snapshots_sent.inc();
        }
        self.subs
            .books
            .insert(s.instrument.clone(), (BookState::Synced, depth));
        if let Some(b) = s.last_batch_block {
            let e = self
                .subs
                .last
                .entry((s.instrument.clone(), RecordKind::BookEvent))
                .or_insert(0);
            *e = (*e).max(b);
        }
        Ok(())
    }

    async fn send(&mut self, msg: &ServerMessage) -> Result<(), Exit> {
        let frame = encode_frame(msg, self.enc).map_err(|e| {
            tracing::error!(error = %e, "cannot encode message");
            Exit::Close(close::PROTOCOL_ERROR, "internal encoding error")
        })?;
        self.write(frame).await
    }

    async fn write(&mut self, frame: Frame) -> Result<(), Exit> {
        let n = frame.len();
        match tokio::time::timeout(self.send_timeout, self.sink.feed(frame.into_message())).await {
            Ok(Ok(())) => {
                self.unflushed = true;
                self.shared.metrics.messages_sent.inc();
                self.shared
                    .metrics
                    .bytes_sent
                    .add(u64::try_from(n).unwrap_or(u64::MAX));
                Ok(())
            }
            Ok(Err(_)) => Err(Exit::Drop),
            Err(_) => {
                self.shared.metrics.slow_consumer_disconnects.inc();
                Err(Exit::Drop)
            }
        }
    }

    async fn flush(&mut self) -> Result<(), Exit> {
        if !self.unflushed {
            return Ok(());
        }
        match tokio::time::timeout(self.send_timeout, self.sink.flush()).await {
            Ok(Ok(())) => {
                self.unflushed = false;
                Ok(())
            }
            Ok(Err(_)) => Err(Exit::Drop),
            Err(_) => {
                self.shared.metrics.slow_consumer_disconnects.inc();
                Err(Exit::Drop)
            }
        }
    }

    async fn error(
        &mut self,
        id: Option<String>,
        code: ErrorCode,
        message: impl Into<String>,
    ) -> Result<(), Exit> {
        // Rate limits and malformed requests count towards the 4029 close;
        // refusals that are not the client misbehaving (caps, unknown
        // instruments, unavailable proofs) do not.
        if matches!(code, ErrorCode::RateLimited | ErrorCode::BadRequest) {
            if code == ErrorCode::RateLimited {
                self.shared.metrics.rate_limited.inc();
            } else {
                self.shared.metrics.bad_requests.inc();
            }
            let n = self.violations.hit();
            if n >= usize::try_from(self.shared.config.violations_before_close).unwrap_or(10) {
                self.shared.metrics.rate_limit_disconnects.inc();
                return Err(Exit::Close(close::RATE_LIMITED, "rate limited"));
            }
        }
        let msg = ServerMessage::Error {
            id,
            code,
            message: message.into(),
        };
        self.send(&msg).await?;
        self.flush().await
    }

    async fn on_client(&mut self, bytes: &[u8], frame_enc: Encoding) -> Result<(), Exit> {
        self.shared.metrics.client_messages.inc();
        if !self.msg_bucket.try_take() {
            return self
                .error(None, ErrorCode::RateLimited, "too many messages")
                .await;
        }
        if frame_enc != self.enc {
            return self
                .error(
                    None,
                    ErrorCode::BadRequest,
                    "frame type does not match the negotiated encoding",
                )
                .await;
        }
        let msg: ClientMessage = match codec::decode(bytes, self.enc) {
            Ok(m) => m,
            Err(e) => {
                return self
                    .error(
                        None,
                        ErrorCode::BadRequest,
                        format!("malformed message: {e}"),
                    )
                    .await;
            }
        };
        match msg {
            ClientMessage::Ping { id } => {
                self.send(&ServerMessage::Pong {
                    id,
                    server_time_ns: UnixNanos::now(),
                })
                .await?;
                self.flush().await
            }
            ClientMessage::Subscribe {
                id,
                channels,
                commitments,
            } => self.subscribe(id, channels, commitments).await,
            ClientMessage::Unsubscribe { id, channels } => self.unsubscribe(id, channels).await,
            ClientMessage::Resnapshot { id, instrument, .. } => {
                self.resnapshot(id, instrument).await
            }
            ClientMessage::Proof {
                id,
                stream,
                block_number,
                instrument,
                event_index,
            } => {
                if !self.proof_bucket.try_take() {
                    return self
                        .error(Some(id), ErrorCode::RateLimited, "too many proof requests")
                        .await;
                }
                match self
                    .shared
                    .proofs
                    .lookup(&stream, block_number, &instrument, event_index)
                {
                    Ok(p) => {
                        self.shared.metrics.proofs_served.inc();
                        self.send(&ServerMessage::Proof {
                            id,
                            commitment: p.commitment,
                            proof: p.proof,
                            record: Arc::new(p.record),
                        })
                        .await?;
                        self.flush().await
                    }
                    Err(e) => {
                        self.shared.metrics.proofs_unavailable.inc();
                        self.error(Some(id), ErrorCode::ProofUnavailable, e.to_string())
                            .await
                    }
                }
            }
        }
    }

    fn check_channel(&self, c: &ChannelSpec) -> Result<(), (ErrorCode, String)> {
        if c.depth.is_some() && c.kind != RecordKind::BookEvent {
            return Err((
                ErrorCode::UnsupportedKind,
                format!("depth applies to book_event only, not {}", c.kind),
            ));
        }
        if c.interval.is_some() && c.kind != RecordKind::Bar {
            return Err((
                ErrorCode::UnsupportedKind,
                format!("interval applies to bar only, not {}", c.kind),
            ));
        }
        if let (Some(reg), InstrumentPattern::Exact(i)) =
            (&self.shared.config.registry, &c.instrument)
            && reg.get(i).is_none()
        {
            return Err((
                ErrorCode::UnknownInstrument,
                format!("{i} is not in the registry"),
            ));
        }
        Ok(())
    }

    async fn subscribe(
        &mut self,
        id: String,
        channels: Vec<ChannelSpec>,
        commitments: bool,
    ) -> Result<(), Exit> {
        for c in &channels {
            if let Err((code, m)) = self.check_channel(c) {
                return self.error(Some(id), code, m).await;
            }
        }
        let new: Vec<ChannelSpec> = channels
            .iter()
            .filter(|c| !self.subs.channels.contains(c))
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let limits = &self.shared.config.limits;
        let total = self.subs.channels.len() + new.len();
        if total > usize::try_from(limits.channels_per_connection).unwrap_or(usize::MAX) {
            let m = format!(
                "at most {} channels per connection",
                limits.channels_per_connection
            );
            return self.error(Some(id), ErrorCode::LimitExceeded, m).await;
        }
        let n = u32::try_from(new.len()).unwrap_or(u32::MAX);
        if !self
            .guard
            .add_channels(n, self.shared.config.channels_per_ip)
        {
            let m = format!(
                "at most {} channels per client IP",
                self.shared.config.channels_per_ip
            );
            return self.error(Some(id), ErrorCode::LimitExceeded, m).await;
        }
        self.subs.commitments |= commitments;
        self.subs.channels.extend(new.iter().cloned());
        self.subs.rebuild();
        self.send(&ServerMessage::Subscribed { id, channels })
            .await?;
        // Snapshots for the book channels: known instruments now, others
        // when they first appear.
        let mut wanted = Vec::new();
        {
            let st = self
                .shared
                .state
                .read()
                .unwrap_or_else(PoisonError::into_inner);
            for c in new.iter().filter(|c| c.kind == RecordKind::BookEvent) {
                let insts = match &c.instrument {
                    InstrumentPattern::Exact(i) => vec![i.clone()],
                    p => st.book_instruments(|i| p.matches(i)),
                };
                wanted.extend(insts);
            }
        }
        for inst in wanted {
            if self.subs.books.contains_key(&inst) {
                continue;
            }
            let depth = self.subs.book_depth(&inst).unwrap_or_default();
            self.subs.books.insert(
                inst.clone(),
                (BookState::Pending(SnapshotReason::Subscribe), depth),
            );
            self.shared.request_snapshot(inst);
        }
        self.flush().await
    }

    async fn unsubscribe(&mut self, id: String, channels: Vec<ChannelSpec>) -> Result<(), Exit> {
        let before = self.subs.channels.len();
        self.subs.channels.retain(|c| !channels.contains(c));
        let removed = before - self.subs.channels.len();
        self.guard
            .remove_channels(u32::try_from(removed).unwrap_or(u32::MAX));
        self.subs.rebuild();
        let subs = &self.subs;
        let keep_books: HashSet<InstrumentId> = subs
            .books
            .keys()
            .filter(|i| subs.book_depth(i).is_some())
            .cloned()
            .collect();
        self.subs.books.retain(|i, _| keep_books.contains(i));
        let subs = &self.subs;
        let keep_last: HashSet<(InstrumentId, RecordKind)> = subs
            .last
            .keys()
            .filter(|(i, k)| subs.matches(*k, i, None))
            .cloned()
            .collect();
        self.subs.last.retain(|k, _| keep_last.contains(k));
        self.send(&ServerMessage::Unsubscribed { id, channels })
            .await?;
        self.flush().await
    }

    async fn resnapshot(&mut self, id: String, instrument: InstrumentId) -> Result<(), Exit> {
        let Some(depth) = self.subs.book_depth(&instrument) else {
            return self
                .error(
                    Some(id),
                    ErrorCode::BadRequest,
                    format!("not subscribed to {instrument} book_event"),
                )
                .await;
        };
        let now = Instant::now();
        let interval = self.shared.config.resnapshot_interval();
        if let Some(t) = self.resnapshot_at.get(&instrument)
            && now.duration_since(*t) < interval
        {
            return self
                .error(
                    Some(id),
                    ErrorCode::RateLimited,
                    format!("resnapshot at most once per {interval:?} per instrument"),
                )
                .await;
        }
        self.resnapshot_at.insert(instrument.clone(), now);
        let state = self.subs.books.get(&instrument).map(|e| e.0);
        if !matches!(state, Some(BookState::Pending(_))) {
            self.subs.books.insert(
                instrument.clone(),
                (BookState::Pending(SnapshotReason::Resnapshot), depth),
            );
        }
        self.shared.request_snapshot(instrument);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_matching() {
        let mut s = Subs {
            channels: vec![
                "hyperliquid:perp:*|trade".parse().unwrap(),
                "hyperliquid:spot:@1|book_event|l2".parse().unwrap(),
                "*|bar|1m".parse().unwrap(),
            ],
            commitments: true,
            ..Subs::default()
        };
        s.rebuild();
        let btc = InstrumentId::new("hyperliquid:perp:BTC").unwrap();
        let spot = InstrumentId::new("hyperliquid:spot:@1").unwrap();
        assert!(s.matches(RecordKind::Trade, &btc, None));
        assert!(!s.matches(RecordKind::Trade, &spot, None));
        assert!(s.matches(RecordKind::BookEvent, &spot, None));
        assert!(!s.matches(RecordKind::BookEvent, &btc, None));
        assert!(s.matches(RecordKind::Bar, &btc, Some(BarInterval::M1)));
        assert!(!s.matches(RecordKind::Bar, &btc, Some(BarInterval::S1)));
        assert_eq!(s.book_depth(&spot), Some(Depth::L2));
        let hl = VenueId::new("hyperliquid").unwrap();
        let other = VenueId::new("other").unwrap();
        assert!(s.covers_commitment(&hl, RecordKind::Trade));
        assert!(!s.covers_commitment(&other, RecordKind::Trade));
        assert!(!s.covers_commitment(&hl, RecordKind::BookSnapshot));
        assert!(covers_venue(
            &InstrumentPattern::Prefix("hyper".into()),
            &hl
        ));
    }
}
