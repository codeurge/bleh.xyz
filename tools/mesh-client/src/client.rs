//! The reference client: connect, subscribe, keep books, verify, recover.
//!
//! [`Client::next_event`] drives everything. It connects to the first
//! reachable URL (rotating through the list and backing off after a full
//! round of failures), sends the subscription, and turns server messages
//! into [`ClientEvent`]s while it:
//!
//! - keeps an L4 book per `book_event` instrument from the snapshot plus
//!   contiguous batches (gateway-v0 § 5.2), and on a gap discards the book,
//!   sends `resnapshot` (at most once per instrument per the announced
//!   interval) and ignores the instrument until the new snapshot;
//! - checks contiguity of other batched kinds and reports gaps;
//! - checks heartbeat `last` maps for batches it never received;
//! - verifies every commitment's signature against trusted signers, the
//!   root of every stream it receives in full, and inclusion proofs for a
//!   sample of records;
//! - reconnects (to the next URL) when the connection drops or no block
//!   heartbeat arrives within the stall timeout, and resubscribes.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use mesh_types::{
    BlockBatch, BlockCommitment, EthAddress, InstrumentId, Record, RecordKind, Seq, StreamId,
    UnixMillis, UnixNanos, VenueId,
};
use serde::Serialize;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Bytes, Message, Utf8Bytes};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::book::L4Book;
use crate::codec::{self, CodecError};
use crate::protocol::{
    ChannelSpec, ClientMessage, Encoding, ErrorCode, Hello, InstrumentPattern, ServerMessage,
    SnapshotReason, channel_key,
};
use crate::verify::{BlockCollector, SignerSet, check_proof};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Client settings.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Gateway websocket URLs, tried in order and rotated on failure.
    pub urls: Vec<String>,
    /// Frame encoding.
    pub encoding: Encoding,
    /// Channels to subscribe to.
    pub channels: Vec<ChannelSpec>,
    /// Ask for commitments.
    pub commitments: bool,
    /// Trusted signer addresses; empty trusts the gateway's `hello.signers`.
    pub signers: Vec<EthAddress>,
    /// Verify commitment signatures (on by default).
    pub verify_signatures: bool,
    /// Recompute roots of streams received in full (on by default).
    pub verify_roots: bool,
    /// Ask for an inclusion proof for every Nth record (0: never).
    pub proof_every: u64,
    /// Keep L4 books for `book_event` channels (on by default).
    pub track_books: bool,
    /// Reconnect when no block heartbeat arrives for this long.
    pub stall_timeout: Duration,
    /// Connection and handshake timeout.
    pub connect_timeout: Duration,
    /// First backoff after a full round of failed URLs.
    pub min_backoff: Duration,
    /// Longest backoff.
    pub max_backoff: Duration,
    /// Give up after this many consecutive failed connection attempts.
    pub max_connect_failures: Option<u32>,
    /// Largest accepted server message, bytes.
    pub max_message_bytes: usize,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            urls: Vec::new(),
            encoding: Encoding::Json,
            channels: Vec::new(),
            commitments: true,
            signers: Vec::new(),
            verify_signatures: true,
            verify_roots: true,
            proof_every: 0,
            track_books: true,
            stall_timeout: Duration::from_secs(2),
            connect_timeout: Duration::from_secs(10),
            min_backoff: Duration::from_millis(250),
            max_backoff: Duration::from_secs(10),
            max_connect_failures: None,
            max_message_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Client failure that ends [`Client::next_event`].
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// No URL configured.
    #[error("no gateway URL configured")]
    NoUrls,
    /// Too many consecutive connection failures.
    #[error("gave up after {0} failed connection attempts")]
    GaveUp(u32),
    /// Encoding failure on send.
    #[error(transparent)]
    Codec(#[from] CodecError),
}

/// Something the client observed.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ClientEvent {
    /// Connected and subscribed.
    Connected {
        /// URL.
        url: String,
        /// The gateway's hello.
        hello: Hello,
    },
    /// A connection attempt failed.
    ConnectFailed {
        /// URL.
        url: String,
        /// Error.
        error: String,
    },
    /// The connection ended; books were discarded.
    Disconnected {
        /// URL.
        url: String,
        /// Why.
        reason: String,
    },
    /// Subscription acknowledged.
    Subscribed {
        /// Request ID.
        id: String,
        /// Channels.
        channels: Vec<ChannelSpec>,
    },
    /// Unsubscription acknowledged.
    Unsubscribed {
        /// Request ID.
        id: String,
        /// Channels.
        channels: Vec<ChannelSpec>,
    },
    /// A book (re)started from a snapshot.
    Snapshot {
        /// Instrument.
        instrument: InstrumentId,
        /// End-of-block marker.
        as_of: Seq,
        /// Why.
        reason: SnapshotReason,
        /// The snapshot record.
        record: Arc<Record>,
    },
    /// A contiguous batch, already applied to the book for `book_event`.
    Batch {
        /// The batch.
        batch: Arc<BlockBatch>,
    },
    /// A single record.
    Record {
        /// The record.
        record: Arc<Record>,
    },
    /// A commitment whose signature verified (or verification is off).
    Commitment {
        /// The commitment.
        commitment: Arc<BlockCommitment>,
        /// `Some(true)` if the root was recomputed from the full stream.
        root_verified: Option<bool>,
    },
    /// A block heartbeat.
    Block {
        /// Venue whose block closed.
        venue: VenueId,
        /// Block number.
        block_number: u64,
        /// Block time.
        block_time_ms: UnixMillis,
        /// Upstream close time.
        ts_publish_ns: UnixNanos,
        /// Last block with a batch per channel.
        last: BTreeMap<String, u64>,
    },
    /// Data is missing for a channel.
    Gap {
        /// Instrument.
        instrument: InstrumentId,
        /// Kind.
        kind: RecordKind,
        /// Last block applied, if any.
        last_applied: Option<u64>,
        /// The block that revealed the gap.
        block: u64,
        /// Detail.
        detail: String,
    },
    /// An inclusion proof verified.
    ProofVerified {
        /// Instrument.
        instrument: InstrumentId,
        /// Kind.
        kind: RecordKind,
        /// Position.
        seq: Seq,
    },
    /// A signature, root or proof failed.
    VerificationFailed {
        /// Detail.
        detail: String,
    },
    /// The server reported an error.
    ServerError {
        /// Request ID.
        id: Option<String>,
        /// Code.
        code: ErrorCode,
        /// Detail.
        message: String,
    },
    /// Reply to a ping.
    Pong {
        /// Request ID.
        id: String,
        /// Server clock.
        server_time_ns: UnixNanos,
    },
}

/// Counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ClientStats {
    /// Server messages received.
    pub messages: u64,
    /// Batches applied or forwarded.
    pub batches: u64,
    /// Snapshots received.
    pub snapshots: u64,
    /// Gaps detected.
    pub gaps: u64,
    /// `resnapshot` requests sent.
    pub resnapshots: u64,
    /// Commitments with a valid, trusted signature.
    pub signatures_ok: u64,
    /// Commitments that failed signature checks.
    pub signatures_failed: u64,
    /// Roots recomputed and matched.
    pub roots_ok: u64,
    /// Roots that did not match.
    pub roots_failed: u64,
    /// Proofs that verified.
    pub proofs_ok: u64,
    /// Proofs that failed.
    pub proofs_failed: u64,
    /// Proof samples skipped to stay within the gateway's rate limits.
    pub proofs_skipped: u64,
    /// Connections established.
    pub connects: u64,
    /// Block heartbeats received.
    pub blocks: u64,
}

#[derive(Debug, Default)]
struct BookSync {
    book: Option<L4Book>,
    fresh: bool,
    awaiting: bool,
    want_request: bool,
    last_request: Option<Instant>,
}

#[derive(Debug, Default)]
struct Session {
    books: HashMap<InstrumentId, BookSync>,
    batch_last: HashMap<(InstrumentId, RecordKind), u64>,
    pending_proofs: HashMap<String, Record>,
    proof_queue: Vec<Record>,
    record_counter: u64,
    resnapshot_interval: Duration,
    budget: Budget,
}

/// Client-side token bucket that keeps requests under the limits the
/// gateway announced (with headroom), so the client is never rate limited.
#[derive(Debug)]
struct Budget {
    rate: f64,
    burst: f64,
    tokens: f64,
    last: Instant,
}

impl Default for Budget {
    fn default() -> Self {
        Self::new(10.0, 10.0)
    }
}

impl Budget {
    fn new(rate: f64, burst: f64) -> Self {
        Self {
            rate,
            burst: burst.max(1.0),
            tokens: burst.max(1.0),
            last: Instant::now(),
        }
    }

    fn from_limits(l: &crate::protocol::Limits) -> Self {
        let rate = f64::from(l.messages_per_second.min(l.proofs_per_second)) * 0.8;
        Self::new(rate, f64::from(l.message_burst) / 2.0)
    }

    fn take(&mut self) -> bool {
        let now = Instant::now();
        self.tokens =
            (self.tokens + now.duration_since(self.last).as_secs_f64() * self.rate).min(self.burst);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

struct Conn {
    ws: Ws,
    url: String,
    last_heartbeat: Instant,
}

/// The reference client.
pub struct Client {
    cfg: ClientConfig,
    url_index: usize,
    failures: u32,
    backoff: Duration,
    conn: Option<Conn>,
    queue: VecDeque<ClientEvent>,
    outbox: Vec<ClientMessage>,
    session: Session,
    signers: SignerSet,
    collector: BlockCollector,
    stats: ClientStats,
    next_id: u64,
    /// Roots are checked only from this block on: earlier blocks of the
    /// connection may be partial (joined mid-block, or dropped as a slow
    /// consumer).
    verify_from: u64,
    max_seen_block: u64,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("urls", &self.cfg.urls)
            .field("connected", &self.conn.as_ref().map(|c| &c.url))
            .field("stats", &self.stats)
            .finish()
    }
}

fn full_kinds(channels: &[ChannelSpec]) -> HashSet<RecordKind> {
    channels
        .iter()
        .filter(|c| c.instrument == InstrumentPattern::All && c.interval.is_none())
        .map(|c| c.kind)
        .collect()
}

impl Client {
    /// A client; nothing happens until [`Client::next_event`].
    #[must_use]
    pub fn new(cfg: ClientConfig) -> Self {
        let full = if cfg.verify_roots {
            full_kinds(&cfg.channels)
        } else {
            HashSet::new()
        };
        Self {
            signers: SignerSet::new(cfg.signers.iter().copied()),
            collector: BlockCollector::new(full),
            backoff: cfg.min_backoff,
            cfg,
            url_index: 0,
            failures: 0,
            conn: None,
            queue: VecDeque::new(),
            outbox: Vec::new(),
            session: Session::default(),
            stats: ClientStats::default(),
            next_id: 0,
            verify_from: u64::MAX,
            max_seen_block: 0,
        }
    }

    /// Counters.
    #[must_use]
    pub fn stats(&self) -> ClientStats {
        self.stats
    }

    /// The book of an instrument, when synchronized.
    #[must_use]
    pub fn book(&self, instrument: &InstrumentId) -> Option<&L4Book> {
        self.session
            .books
            .get(instrument)
            .and_then(|b| b.book.as_ref())
    }

    /// Every synchronized book.
    pub fn books(&self) -> impl Iterator<Item = &L4Book> {
        self.session.books.values().filter_map(|b| b.book.as_ref())
    }

    /// URL of the current connection.
    #[must_use]
    pub fn connected_url(&self) -> Option<&str> {
        self.conn.as_ref().map(|c| c.url.as_str())
    }

    /// Queues a message for the server (sent on the next
    /// [`Client::next_event`] call while connected).
    pub fn send(&mut self, msg: ClientMessage) {
        self.outbox.push(msg);
    }

    /// Closes the connection, if any.
    pub async fn close(&mut self) {
        if let Some(mut c) = self.conn.take() {
            let _ = c.ws.close(None).await;
        }
    }

    fn fresh_id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    /// Waits for the next event. Reconnects as needed; returns an error
    /// only when no URL is configured or the failure limit is reached.
    ///
    /// # Errors
    ///
    /// [`ClientError`].
    pub async fn next_event(&mut self) -> Result<ClientEvent, ClientError> {
        if self.cfg.urls.is_empty() {
            return Err(ClientError::NoUrls);
        }
        loop {
            if !self.outbox.is_empty() && self.conn.is_some() {
                self.flush_outbox().await?;
            }
            if let Some(e) = self.queue.pop_front() {
                return Ok(e);
            }
            if self.conn.is_none() {
                return self.connect_next().await;
            }
            let stall = self.cfg.stall_timeout;
            let Some(conn) = self.conn.as_mut() else {
                continue;
            };
            let deadline = conn.last_heartbeat + stall;
            let msg = tokio::select! {
                m = conn.ws.next() => m,
                () = tokio::time::sleep_until(deadline.into()) => {
                    self.disconnect(format!("stalled: no block heartbeat for {stall:?}")).await;
                    continue;
                }
            };
            match msg {
                Some(Ok(Message::Text(t))) => self.on_frame(t.as_bytes(), Encoding::Json),
                Some(Ok(Message::Binary(b))) => self.on_frame(&b, Encoding::Cbor),
                Some(Ok(Message::Close(frame))) => {
                    let reason = frame.map_or_else(
                        || "closed by server".to_owned(),
                        |f| format!("closed by server: {} {}", u16::from(f.code), f.reason),
                    );
                    self.disconnect(reason).await;
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => self.disconnect(e.to_string()).await,
                None => self.disconnect("connection ended".into()).await,
            }
        }
    }

    async fn flush_outbox(&mut self) -> Result<(), ClientError> {
        let msgs = std::mem::take(&mut self.outbox);
        let enc = self.cfg.encoding;
        let Some(conn) = self.conn.as_mut() else {
            return Ok(());
        };
        for m in &msgs {
            if let Err(e) = send_msg(&mut conn.ws, m, enc).await {
                let reason = format!("send failed: {e}");
                self.disconnect(reason).await;
                break;
            }
        }
        Ok(())
    }

    async fn connect_next(&mut self) -> Result<ClientEvent, ClientError> {
        let url = self.cfg.urls[self.url_index % self.cfg.urls.len()].clone();
        match self.try_connect(&url).await {
            Ok((ws, hello)) => {
                self.failures = 0;
                self.backoff = self.cfg.min_backoff;
                self.stats.connects += 1;
                self.signers.set_announced(&hello.signers);
                self.session = Session {
                    resnapshot_interval: Duration::from_millis(hello.limits.resnapshot_interval_ms),
                    budget: Budget::from_limits(&hello.limits),
                    ..Session::default()
                };
                self.collector = BlockCollector::new(if self.cfg.verify_roots {
                    full_kinds(&self.cfg.channels)
                } else {
                    HashSet::new()
                });
                self.conn = Some(Conn {
                    ws,
                    url: url.clone(),
                    last_heartbeat: Instant::now(),
                });
                self.verify_from = u64::MAX;
                self.max_seen_block = 0;
                let id = self.fresh_id("sub");
                self.outbox.insert(
                    0,
                    ClientMessage::Subscribe {
                        id,
                        channels: self.cfg.channels.clone(),
                        commitments: self.cfg.commitments,
                    },
                );
                Ok(ClientEvent::Connected { url, hello })
            }
            Err(error) => {
                self.failures += 1;
                self.url_index += 1;
                if let Some(max) = self.cfg.max_connect_failures
                    && self.failures >= max
                {
                    return Err(ClientError::GaveUp(self.failures));
                }
                let round = usize::try_from(self.failures).unwrap_or(usize::MAX);
                if round % self.cfg.urls.len() == 0 {
                    tokio::time::sleep(self.backoff).await;
                    self.backoff = (self.backoff * 2).min(self.cfg.max_backoff);
                }
                Ok(ClientEvent::ConnectFailed { url, error })
            }
        }
    }

    async fn try_connect(&self, url: &str) -> Result<(Ws, Hello), String> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut req = url.into_client_request().map_err(|e| e.to_string())?;
        req.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_static(self.cfg.encoding.subprotocol()),
        );
        let config = WebSocketConfig::default()
            .max_message_size(Some(self.cfg.max_message_bytes))
            .max_frame_size(Some(self.cfg.max_message_bytes));
        let fut = tokio_tungstenite::connect_async_with_config(req, Some(config), true);
        let (mut ws, _resp) = tokio::time::timeout(self.cfg.connect_timeout, fut)
            .await
            .map_err(|_| "connect timed out".to_owned())?
            .map_err(|e| e.to_string())?;
        let first = tokio::time::timeout(self.cfg.connect_timeout, ws.next())
            .await
            .map_err(|_| "no hello".to_owned())?;
        let bytes: Vec<u8> = match first {
            Some(Ok(Message::Text(t))) => t.as_bytes().to_vec(),
            Some(Ok(Message::Binary(b))) => b.to_vec(),
            Some(Ok(Message::Close(f))) => {
                return Err(format!(
                    "closed before hello: {}",
                    f.map(|f| format!("{} {}", u16::from(f.code), f.reason))
                        .unwrap_or_default()
                ));
            }
            Some(Ok(other)) => return Err(format!("unexpected first frame {other:?}")),
            Some(Err(e)) => return Err(e.to_string()),
            None => return Err("closed before hello".into()),
        };
        match codec::decode::<ServerMessage>(&bytes, self.cfg.encoding) {
            Ok(ServerMessage::Hello(h)) if h.protocol == crate::protocol::PROTOCOL => Ok((ws, h)),
            Ok(other) => Err(format!("expected hello, got {other:?}")),
            Err(e) => Err(format!("bad hello: {e}")),
        }
    }

    async fn disconnect(&mut self, reason: String) {
        if let Some(mut c) = self.conn.take() {
            let _ = tokio::time::timeout(Duration::from_millis(200), c.ws.close(None)).await;
            self.url_index += 1;
            self.queue
                .push_back(ClientEvent::Disconnected { url: c.url, reason });
        }
        self.session.books.clear();
        self.outbox.clear();
    }

    fn on_frame(&mut self, bytes: &[u8], frame_encoding: Encoding) {
        self.stats.messages += 1;
        if frame_encoding != self.cfg.encoding {
            self.queue.push_back(ClientEvent::VerificationFailed {
                detail: "frame type does not match the negotiated encoding".into(),
            });
            return;
        }
        match codec::decode::<ServerMessage>(bytes, self.cfg.encoding) {
            Ok(m) => self.on_message(m),
            Err(e) => self.queue.push_back(ClientEvent::VerificationFailed {
                detail: format!("undecodable server message: {e}"),
            }),
        }
    }

    fn on_message(&mut self, msg: ServerMessage) {
        match msg {
            ServerMessage::Hello(h) => self.signers.set_announced(&h.signers),
            ServerMessage::Subscribed { id, channels } => {
                self.queue
                    .push_back(ClientEvent::Subscribed { id, channels });
            }
            ServerMessage::Unsubscribed { id, channels } => {
                for c in &channels {
                    if c.kind == RecordKind::BookEvent {
                        self.session.books.retain(|i, _| !c.instrument.matches(i));
                    }
                }
                self.queue
                    .push_back(ClientEvent::Unsubscribed { id, channels });
            }
            ServerMessage::Snapshot {
                instrument,
                as_of,
                reason,
                record,
            } => self.on_snapshot(instrument, as_of, reason, record),
            ServerMessage::Batch { batch } => self.on_batch(batch),
            ServerMessage::Record { record } => {
                self.collector.add_record(&record);
                self.queue.push_back(ClientEvent::Record { record });
            }
            ServerMessage::Commitment { commitment } => self.on_commitment(commitment),
            ServerMessage::Block {
                venue,
                block_number,
                block_time_ms,
                ts_publish_ns,
                last,
            } => self.on_block(venue, block_number, block_time_ms, ts_publish_ns, last),
            ServerMessage::Proof {
                id,
                commitment,
                proof,
                record,
            } => {
                let Some(expected) = self.session.pending_proofs.remove(&id) else {
                    return;
                };
                match check_proof(&self.signers, &expected, &commitment, &proof, &record) {
                    Ok(()) => {
                        self.stats.proofs_ok += 1;
                        self.queue.push_back(ClientEvent::ProofVerified {
                            instrument: record.prov().instrument.clone(),
                            kind: record.kind(),
                            seq: record.prov().seq,
                        });
                    }
                    Err(e) => {
                        self.stats.proofs_failed += 1;
                        self.queue.push_back(ClientEvent::VerificationFailed {
                            detail: e.to_string(),
                        });
                    }
                }
            }
            ServerMessage::Pong { id, server_time_ns } => {
                self.queue
                    .push_back(ClientEvent::Pong { id, server_time_ns });
            }
            ServerMessage::Error { id, code, message } => {
                if let Some(pid) = &id {
                    self.session.pending_proofs.remove(pid);
                    if code == ErrorCode::RateLimited
                        && let Some(inst) = pid.strip_prefix("rs:")
                        && let Ok(inst) = InstrumentId::new(inst)
                        && let Some(b) = self.session.books.get_mut(&inst)
                        && b.awaiting
                    {
                        b.want_request = true;
                    }
                }
                self.queue
                    .push_back(ClientEvent::ServerError { id, code, message });
            }
        }
    }

    fn on_snapshot(
        &mut self,
        instrument: InstrumentId,
        as_of: Seq,
        reason: SnapshotReason,
        record: Arc<Record>,
    ) {
        self.stats.snapshots += 1;
        if reason == SnapshotReason::SlowConsumer {
            // The server dropped queued data: the current block is partial.
            self.verify_from = self.verify_from.max(self.max_seen_block.saturating_add(2));
        }
        if self.cfg.track_books {
            let entry = self.session.books.entry(instrument.clone()).or_default();
            match L4Book::from_record(&record) {
                Ok(book) => {
                    entry.book = Some(book);
                    entry.fresh = true;
                    entry.awaiting = false;
                    entry.want_request = false;
                }
                Err(_) => {
                    // L2 snapshots cannot take order-level deltas; keep no book.
                    entry.book = None;
                    entry.awaiting = false;
                }
            }
        }
        self.queue.push_back(ClientEvent::Snapshot {
            instrument,
            as_of,
            reason,
            record,
        });
    }

    fn gap(&mut self, instrument: InstrumentId, kind: RecordKind, block: u64, detail: String) {
        self.stats.gaps += 1;
        let mut last_applied = self
            .session
            .batch_last
            .get(&(instrument.clone(), kind))
            .copied();
        if kind == RecordKind::BookEvent
            && self.cfg.track_books
            && let Some(b) = self.session.books.get_mut(&instrument)
        {
            last_applied = b.book.as_ref().and_then(L4Book::last_block);
            b.book = None;
            b.awaiting = true;
            b.want_request = true;
        }
        self.queue.push_back(ClientEvent::Gap {
            instrument,
            kind,
            last_applied,
            block,
            detail,
        });
    }

    fn on_batch(&mut self, batch: Arc<BlockBatch>) {
        self.max_seen_block = self.max_seen_block.max(batch.block_number);
        // Every received record counts for the root, applied or not.
        self.collector.add_batch(&batch);
        if let Err(e) = batch.validate() {
            self.gap(
                batch.instrument.clone(),
                batch.kind,
                batch.block_number,
                format!("invalid batch: {e}"),
            );
            return;
        }
        if batch.kind == RecordKind::BookEvent && self.cfg.track_books {
            // No snapshot yet, waiting for one, or L2 depth (no order-level
            // book): nothing to apply.
            let Some(sync) = self.session.books.get_mut(&batch.instrument) else {
                return;
            };
            if sync.awaiting {
                return;
            }
            let Some(book) = sync.book.as_mut() else {
                return;
            };
            let b = book.last_block().unwrap_or(0);
            if batch.block_number <= b {
                return; // already in the snapshot
            }
            let contiguous = if sync.fresh {
                batch.prev_block_number.is_none_or(|p| p <= b)
            } else {
                batch.prev_block_number == Some(b)
            };
            let verdict = if !contiguous {
                Err(format!(
                    "prev_block_number {:?} after block {b}",
                    batch.prev_block_number
                ))
            } else if let Err(e) = book.apply_batch(&batch) {
                Err(format!("book: {e}"))
            } else {
                sync.fresh = false;
                Ok(())
            };
            if let Err(detail) = verdict {
                self.gap(
                    batch.instrument.clone(),
                    batch.kind,
                    batch.block_number,
                    detail,
                );
                return;
            }
        } else {
            let key = (batch.instrument.clone(), batch.kind);
            if let Some(&last) = self.session.batch_last.get(&key) {
                if batch.block_number <= last {
                    return; // duplicate
                }
                if !batch.follows(Some(last)) {
                    let detail = format!(
                        "prev_block_number {:?} after block {last}",
                        batch.prev_block_number
                    );
                    self.gap(
                        batch.instrument.clone(),
                        batch.kind,
                        batch.block_number,
                        detail,
                    );
                }
            }
            self.session.batch_last.insert(key, batch.block_number);
        }
        self.stats.batches += 1;
        if self.cfg.proof_every > 0 {
            for r in &batch.records {
                self.session.record_counter += 1;
                if self
                    .session
                    .record_counter
                    .is_multiple_of(self.cfg.proof_every)
                {
                    self.session.proof_queue.push(r.clone());
                }
            }
        }
        self.queue.push_back(ClientEvent::Batch { batch });
    }

    fn on_commitment(&mut self, commitment: Arc<BlockCommitment>) {
        if self.cfg.verify_signatures {
            if let Err(e) = self.signers.verify(&commitment) {
                self.stats.signatures_failed += 1;
                self.queue.push_back(ClientEvent::VerificationFailed {
                    detail: e.to_string(),
                });
                return;
            }
            self.stats.signatures_ok += 1;
        }
        let root_verified = match self.collector.check(&commitment) {
            Some(_) if commitment.block_number < self.verify_from => None,
            None => None,
            Some(Ok(())) => {
                self.stats.roots_ok += 1;
                Some(true)
            }
            Some(Err(e)) => {
                self.stats.roots_failed += 1;
                self.queue.push_back(ClientEvent::VerificationFailed {
                    detail: e.to_string(),
                });
                Some(false)
            }
        };
        self.queue.push_back(ClientEvent::Commitment {
            commitment,
            root_verified,
        });
    }

    fn on_block(
        &mut self,
        venue: VenueId,
        block_number: u64,
        block_time_ms: UnixMillis,
        ts_publish_ns: UnixNanos,
        last: BTreeMap<String, u64>,
    ) {
        self.stats.blocks += 1;
        self.max_seen_block = self.max_seen_block.max(block_number);
        if self.verify_from == u64::MAX {
            self.verify_from = block_number.saturating_add(1);
        }
        if let Some(c) = self.conn.as_mut() {
            c.last_heartbeat = Instant::now();
        }
        // Batches announced in `last` that never arrived.
        let mut gaps = Vec::new();
        for (key, &announced) in &last {
            let Some((inst, kind)) = key.split_once('|') else {
                continue;
            };
            let Ok(inst) = InstrumentId::new(inst) else {
                continue;
            };
            if kind == RecordKind::BookEvent.as_str() && self.cfg.track_books {
                if let Some(sync) = self.session.books.get(&inst)
                    && !sync.awaiting
                    && let Some(b) = sync.book.as_ref().and_then(L4Book::last_block)
                    && announced > b
                {
                    gaps.push((inst, RecordKind::BookEvent, announced, b));
                }
            } else if let Ok(kind) =
                serde_json::from_value::<RecordKind>(serde_json::Value::String(kind.to_owned()))
                && let Some(&applied) = self.session.batch_last.get(&(inst.clone(), kind))
                && announced > applied
            {
                gaps.push((inst, kind, announced, applied));
            }
        }
        for (inst, kind, announced, applied) in gaps {
            if kind != RecordKind::BookEvent {
                self.session
                    .batch_last
                    .insert((inst.clone(), kind), announced);
            }
            self.gap(
                inst,
                kind,
                block_number,
                format!("heartbeat announces block {announced}, last applied {applied}"),
            );
        }
        // Resnapshot requests, rate limited per instrument.
        let interval = self.session.resnapshot_interval;
        let now = Instant::now();
        let mut requests = Vec::new();
        let budget = &mut self.session.budget;
        for (inst, sync) in &mut self.session.books {
            if sync.want_request
                && sync.last_request.is_none_or(|t| now - t >= interval)
                && budget.take()
            {
                sync.want_request = false;
                sync.last_request = Some(now);
                requests.push(inst.clone());
            }
        }
        for inst in requests {
            self.stats.resnapshots += 1;
            self.outbox.push(ClientMessage::Resnapshot {
                id: format!("rs:{inst}"),
                instrument: inst,
                reason: "gap".into(),
            });
        }
        // Proof requests for records of closed blocks.
        let ready: Vec<Record> = {
            let (ready, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.session.proof_queue)
                .into_iter()
                .partition(|r| r.prov().seq.block_number <= block_number);
            self.session.proof_queue = keep;
            ready
        };
        for r in ready {
            if !self.session.budget.take() {
                self.stats.proofs_skipped += 1;
                continue;
            }
            let id = self.fresh_id("pf");
            let p = r.prov();
            self.outbox.push(ClientMessage::Proof {
                id: id.clone(),
                stream: StreamId {
                    venue: p.venue.clone(),
                    kind: r.kind(),
                },
                block_number: p.seq.block_number,
                instrument: p.instrument.clone(),
                event_index: p.seq.event_index,
            });
            self.session.pending_proofs.insert(id, r);
        }
        self.collector.end_block(block_number);
        self.queue.push_back(ClientEvent::Block {
            venue,
            block_number,
            block_time_ms,
            ts_publish_ns,
            last,
        });
    }
}

async fn send_msg(ws: &mut Ws, msg: &ClientMessage, enc: Encoding) -> Result<(), String> {
    let bytes = codec::encode(msg, enc).map_err(|e| e.to_string())?;
    let frame = match enc {
        Encoding::Json => Message::Text(Utf8Bytes::try_from(bytes).map_err(|e| e.to_string())?),
        Encoding::Cbor => Message::Binary(Bytes::from(bytes)),
    };
    ws.send(frame).await.map_err(|e| e.to_string())
}

/// The channel key of a batch, as used in heartbeats.
#[must_use]
pub fn batch_key(batch: &BlockBatch) -> String {
    channel_key(&batch.instrument, batch.kind)
}
