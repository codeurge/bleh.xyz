//! Synthetic demo feed: `tools/hl-synth` records on a loop.
//!
//! Lets the whole gateway run locally without a node. The feed generates
//! the hl-synth golden set in memory (seeded, deterministic, entirely
//! synthetic), takes its ground-truth trades and book events (the
//! `expected.json` a correct reader produces) and replays them as live
//! blocks with current timestamps:
//!
//! - every source block becomes one live block, numbered consecutively
//!   from the hl-synth first block;
//! - after the last source block, a reset block cancels every resting
//!   order, and the next cycle starts with order IDs offset by
//!   `cycle * 1_000_000_000`, so books stay consistent forever;
//! - every `checkpoint_every` blocks (and on the first block) the feed
//!   publishes an L4 `book_snapshot` per instrument, as the book builder's
//!   checkpoints would, which seeds and reconciles gateway books;
//! - with `gap_every`, every Nth block withholds one instrument's
//!   `book_event` batch from publication (simulating an upstream skip) to
//!   exercise forced re-snapshots.
//!
//! No value comes from real market data; see
//! `fixtures/synthetic/hl/README.md`.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use bus::{BlockPublisher, PublishError};
use mesh_client::{Depth, L4Book};
use mesh_types::{
    AdmissionId, AggressorSide, BlockBatch, BookAction, BookEvent, BookSide, InstrumentId,
    InstrumentSpec, OrderId, Price, Provenance, Record, RecordKind, Registry, Seq, Size,
    SnapshotDepth, Trade, TradeId, UnixMillis, UnixNanos, VenueId,
};
use serde_json::Value;
use tokio::sync::watch;

/// Demo feed settings.
#[derive(Debug, Clone)]
pub struct DemoConfig {
    /// hl-synth seed.
    pub seed: u64,
    /// hl-synth blocks per cycle (at least 45).
    pub blocks: u64,
    /// Time between blocks.
    pub block_interval: Duration,
    /// Publish book checkpoints every N blocks (at least 1).
    pub checkpoint_every: u64,
    /// Withhold one `book_event` batch every N blocks.
    pub gap_every: Option<u64>,
}

impl Default for DemoConfig {
    fn default() -> Self {
        Self {
            seed: hl_synth::DEFAULT_SEED,
            blocks: hl_synth::DEFAULT_BLOCKS,
            block_interval: Duration::from_millis(100),
            checkpoint_every: 10,
            gap_every: None,
        }
    }
}

/// Error building the demo feed.
#[derive(Debug, thiserror::Error)]
pub enum DemoError {
    /// hl-synth failed.
    #[error("hl-synth: {0}")]
    Synth(String),
    /// The generated ground truth has an unexpected shape.
    #[error("unexpected hl-synth output: {0}")]
    Shape(String),
}

fn shape(msg: impl Into<String>) -> DemoError {
    DemoError::Shape(msg.into())
}

#[derive(Debug, Clone)]
struct TradeIn {
    price: Price,
    size: Size,
    aggressor: AggressorSide,
    trade_id: u64,
}

#[derive(Debug, Clone)]
struct EventIn {
    order_id: u64,
    side: BookSide,
    price: Price,
    action: BookAction,
}

#[derive(Debug, Clone)]
struct Coin {
    spec: InstrumentSpec,
    trades: BTreeMap<u64, Vec<TradeIn>>,
    events: BTreeMap<u64, Vec<EventIn>>,
}

/// One block produced by the feed.
#[derive(Debug, Clone)]
pub struct DemoBlock {
    /// Venue.
    pub venue: VenueId,
    /// Block number.
    pub block_number: u64,
    /// Block time.
    pub block_time_ms: UnixMillis,
    /// Batches to publish.
    pub batches: Vec<BlockBatch>,
    /// Single records to publish (checkpoints).
    pub records: Vec<Record>,
    /// Batches withheld to simulate an upstream gap.
    pub withheld: Vec<BlockBatch>,
}

/// The synthetic feed.
#[derive(Debug, Clone)]
pub struct DemoFeed {
    cfg: DemoConfig,
    venue: VenueId,
    registry: Registry,
    coins: Vec<Coin>,
    first_block: u64,
    source_blocks: u64,
    counter: u64,
    books: BTreeMap<InstrumentId, L4Book>,
    prev: HashMap<(InstrumentId, RecordKind), u64>,
}

fn s<'a>(v: &'a Value, k: &str) -> Result<&'a str, DemoError> {
    v.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| shape(format!("missing string {k}")))
}

fn u(v: &Value, k: &str) -> Result<u64, DemoError> {
    v.get(k)
        .and_then(Value::as_u64)
        .ok_or_else(|| shape(format!("missing integer {k}")))
}

fn seq_of(v: &Value) -> Result<(u64, u32), DemoError> {
    let a = v
        .get("seq")
        .and_then(Value::as_array)
        .ok_or_else(|| shape("missing seq"))?;
    let b = a
        .first()
        .and_then(Value::as_u64)
        .ok_or_else(|| shape("seq block"))?;
    let i = a
        .get(1)
        .and_then(Value::as_u64)
        .and_then(|i| u32::try_from(i).ok())
        .ok_or_else(|| shape("seq index"))?;
    Ok((b, i))
}

fn size(v: &Value, k: &str) -> Result<Size, DemoError> {
    Size::parse_exact(s(v, k)?).map_err(|e| shape(e.to_string()))
}

fn opt_size(v: &Value, k: &str) -> Result<Option<Size>, DemoError> {
    v.get(k).map(|_| size(v, k)).transpose()
}

impl DemoFeed {
    /// Generates the hl-synth set and prepares the feed.
    ///
    /// # Errors
    ///
    /// hl-synth failure or unexpected output.
    pub fn new(cfg: DemoConfig) -> Result<Self, DemoError> {
        let files = hl_synth::generate(hl_synth::Config {
            seed: cfg.seed,
            blocks: cfg.blocks,
        })
        .map_err(|e| DemoError::Synth(e.to_string()))?;
        let registry: Registry = serde_json::from_slice(
            files
                .get("markets.json")
                .ok_or_else(|| shape("no markets.json"))?,
        )
        .map_err(|e| shape(e.to_string()))?;
        let expected: Value = serde_json::from_slice(
            files
                .get("base/expected.json")
                .ok_or_else(|| shape("no base/expected.json"))?,
        )
        .map_err(|e| shape(e.to_string()))?;
        let first_block = u(&expected, "first_block")?;
        let last_block = u(&expected, "last_block")?;
        let coins_v = expected
            .get("coins")
            .and_then(Value::as_object)
            .ok_or_else(|| shape("no coins"))?;
        let mut coins = Vec::new();
        for (_, c) in coins_v {
            let id = InstrumentId::new(s(c, "instrument")?).map_err(|e| shape(e.to_string()))?;
            let spec = registry
                .get(&id)
                .cloned()
                .ok_or_else(|| shape(format!("{id} not in markets.json")))?;
            let mut trades: BTreeMap<u64, Vec<TradeIn>> = BTreeMap::new();
            for t in c
                .get("trades")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let (b, _) = seq_of(t)?;
                let aggressor = match s(t, "aggressor")? {
                    "buy" => AggressorSide::Buy,
                    "sell" => AggressorSide::Sell,
                    _ => AggressorSide::Unknown,
                };
                trades.entry(b).or_default().push(TradeIn {
                    price: Price::parse_exact(s(t, "price")?).map_err(|e| shape(e.to_string()))?,
                    size: size(t, "size")?,
                    aggressor,
                    trade_id: u(t, "trade_id")?,
                });
            }
            let mut events: BTreeMap<u64, Vec<EventIn>> = BTreeMap::new();
            for e in c
                .get("book_events")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let (b, _) = seq_of(e)?;
                let action = match s(e, "action")? {
                    "add" => BookAction::Add {
                        size: size(e, "size")?,
                    },
                    "modify" => BookAction::Modify {
                        orig_size: opt_size(e, "orig_size")?,
                        new_size: size(e, "new_size")?,
                    },
                    "fill" => BookAction::Fill {
                        orig_size: opt_size(e, "orig_size")?,
                        new_size: size(e, "new_size")?,
                    },
                    "cancel" => BookAction::Cancel {},
                    other => return Err(shape(format!("unknown action {other}"))),
                };
                events.entry(b).or_default().push(EventIn {
                    order_id: u(e, "order_id")?,
                    side: if s(e, "side")? == "bid" {
                        BookSide::Bid
                    } else {
                        BookSide::Ask
                    },
                    price: Price::parse_exact(s(e, "price")?).map_err(|e| shape(e.to_string()))?,
                    action,
                });
            }
            coins.push(Coin {
                spec,
                trades,
                events,
            });
        }
        coins.sort_by(|a, b| a.spec.id.cmp(&b.spec.id));
        let venue = coins
            .first()
            .map(|c| c.spec.venue.clone())
            .ok_or_else(|| shape("no coins"))?;
        let books = coins
            .iter()
            .map(|c| (c.spec.id.clone(), L4Book::new(c.spec.id.clone())))
            .collect();
        Ok(Self {
            cfg: DemoConfig {
                checkpoint_every: cfg.checkpoint_every.max(1),
                ..cfg
            },
            venue,
            registry,
            coins,
            first_block,
            source_blocks: last_block - first_block + 1,
            counter: 0,
            books,
            prev: HashMap::new(),
        })
    }

    /// The synthetic markets.
    #[must_use]
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The venue.
    #[must_use]
    pub fn venue(&self) -> &VenueId {
        &self.venue
    }

    /// The feed's own book of an instrument (the source of truth).
    #[must_use]
    pub fn book(&self, instrument: &InstrumentId) -> Option<&L4Book> {
        self.books.get(instrument)
    }

    /// Every instrument.
    pub fn instruments(&self) -> impl Iterator<Item = &InstrumentId> {
        self.books.keys()
    }

    /// The number of the next block.
    #[must_use]
    pub fn next_block_number(&self) -> u64 {
        self.first_block + self.counter
    }

    /// Blocks per cycle, including the reset block.
    #[must_use]
    pub fn cycle_len(&self) -> u64 {
        self.source_blocks + 1
    }

    fn prov(
        &self,
        spec: &InstrumentSpec,
        seq: Seq,
        now_ms: UnixMillis,
        now_ns: UnixNanos,
    ) -> Provenance {
        Provenance::new(
            spec.id.clone(),
            spec.venue.clone(),
            seq,
            now_ms,
            now_ns,
            spec.admission.clone(),
        )
        .with_flags(spec.flags)
    }

    /// Builds the next block and applies it to the feed's books.
    #[must_use]
    pub fn next_block(&mut self) -> DemoBlock {
        let block = self.first_block + self.counter;
        let cycle = self.counter / self.cycle_len();
        let pos = self.counter % self.cycle_len();
        let now_ns = UnixNanos::now();
        let now_ms = now_ns.to_millis();
        let offset = cycle * 1_000_000_000;
        let mut batches = Vec::new();
        let mut withheld = Vec::new();
        let gap_target = self.cfg.gap_every.and_then(|n| {
            (n > 0 && self.counter > 0 && (self.counter + 1).is_multiple_of(n))
                .then(|| usize::try_from((self.counter / n) % self.coins.len() as u64).unwrap_or(0))
        });
        for (ci, coin) in self.coins.iter().enumerate() {
            let spec = &coin.spec;
            let (trades, events): (Vec<TradeIn>, Vec<EventIn>) = if pos < self.source_blocks {
                let src = self.first_block + pos;
                (
                    coin.trades.get(&src).cloned().unwrap_or_default(),
                    coin.events
                        .get(&src)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|mut e| {
                            e.order_id += offset;
                            e
                        })
                        .collect(),
                )
            } else {
                // Reset block: cancel everything that rests.
                let mut cancels = Vec::new();
                if let Some(book) = self.books.get(&spec.id)
                    && let SnapshotDepth::L4 { bids, asks } = book.depth_l4()
                {
                    for (side, orders) in [(BookSide::Bid, bids), (BookSide::Ask, asks)] {
                        for o in orders {
                            cancels.push(EventIn {
                                order_id: o.order_id.0,
                                side,
                                price: o.price,
                                action: BookAction::Cancel {},
                            });
                        }
                    }
                }
                (Vec::new(), cancels)
            };
            if !trades.is_empty() {
                let records: Vec<Record> = trades
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        Record::Trade(Trade {
                            prov: self.prov(
                                spec,
                                Seq::new(block, u32::try_from(i).unwrap_or(u32::MAX)),
                                now_ms,
                                now_ns,
                            ),
                            price: t.price,
                            size: t.size,
                            aggressor: t.aggressor,
                            trade_id: TradeId(t.trade_id),
                        })
                    })
                    .collect();
                let key = (spec.id.clone(), RecordKind::Trade);
                batches.push(BlockBatch {
                    instrument: spec.id.clone(),
                    venue: spec.venue.clone(),
                    kind: RecordKind::Trade,
                    block_number: block,
                    block_time_ms: now_ms,
                    prev_block_number: self.prev.get(&key).copied(),
                    records,
                });
                self.prev.insert(key, block);
            }
            if !events.is_empty() {
                let records: Vec<Record> = events
                    .iter()
                    .enumerate()
                    .map(|(i, e)| {
                        Record::BookEvent(BookEvent {
                            prov: self.prov(
                                spec,
                                Seq::new(block, u32::try_from(i).unwrap_or(u32::MAX)),
                                now_ms,
                                now_ns,
                            ),
                            order_id: OrderId(e.order_id),
                            side: e.side,
                            price: e.price,
                            action: e.action,
                        })
                    })
                    .collect();
                let key = (spec.id.clone(), RecordKind::BookEvent);
                let batch = BlockBatch {
                    instrument: spec.id.clone(),
                    venue: spec.venue.clone(),
                    kind: RecordKind::BookEvent,
                    block_number: block,
                    block_time_ms: now_ms,
                    prev_block_number: self.prev.get(&key).copied(),
                    records,
                };
                self.prev.insert(key, block);
                if let Some(book) = self.books.get_mut(&spec.id)
                    && let Err(e) = book.apply_batch(&batch)
                {
                    tracing::error!(instrument = %spec.id, error = %e, "demo book inconsistent");
                }
                if gap_target == Some(ci) {
                    withheld.push(batch);
                } else {
                    batches.push(batch);
                }
            }
        }
        let mut records = Vec::new();
        for book in self.books.values_mut() {
            book.advance_to(block);
        }
        if self.counter.is_multiple_of(self.cfg.checkpoint_every) {
            for coin in &self.coins {
                let spec = &coin.spec;
                let Some(book) = self.books.get(&spec.id) else {
                    continue;
                };
                let template = self.prov(spec, Seq::end_of_block(block), now_ms, now_ns);
                if let Ok(snap) = book.snapshot(&template, Depth::L4) {
                    records.push(Record::BookSnapshot(snap));
                }
            }
        }
        self.counter += 1;
        DemoBlock {
            venue: self.venue.clone(),
            block_number: block,
            block_time_ms: now_ms,
            batches,
            records,
            withheld,
        }
    }

    /// Builds the next block and publishes it.
    ///
    /// # Errors
    ///
    /// Publisher errors.
    pub fn publish_next(
        &mut self,
        publisher: &mut BlockPublisher,
    ) -> Result<DemoBlock, PublishError> {
        let b = self.next_block();
        for batch in &b.batches {
            publisher.publish_batch(batch.clone())?;
        }
        for r in &b.records {
            publisher.publish_record(r.clone())?;
        }
        publisher.end_block(&b.venue, b.block_number, b.block_time_ms)?;
        Ok(b)
    }
}

/// Publishes the feed every `block_interval` until `stop` turns true.
///
/// # Errors
///
/// Publisher errors.
pub async fn run(
    mut feed: DemoFeed,
    mut publisher: BlockPublisher,
    mut stop: watch::Receiver<bool>,
) -> Result<(), PublishError> {
    let mut tick = tokio::time::interval(feed.cfg.block_interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                feed.publish_next(&mut publisher)?;
            }
            r = stop.changed() => {
                if r.is_err() || *stop.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

/// The admission of the synthetic markets.
///
/// # Errors
///
/// Never in practice.
pub fn synthetic_admission() -> Result<AdmissionId, mesh_types::IdError> {
    AdmissionId::new("ADM-0001")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triples(depth: &SnapshotDepth) -> Vec<(u64, String, String)> {
        let SnapshotDepth::L4 { bids, asks } = depth else {
            return vec![];
        };
        bids.iter()
            .chain(asks)
            .map(|o| (o.order_id.0, o.price.to_string(), o.size.to_string()))
            .collect()
    }

    fn expected_triples(v: &Value) -> Vec<(u64, String, String)> {
        ["bids", "asks"]
            .iter()
            .flat_map(|side| v[*side].as_array().unwrap().iter())
            .map(|o| {
                (
                    o["order_id"].as_u64().unwrap(),
                    o["price"].as_str().unwrap().to_owned(),
                    o["size"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn first_cycle_reproduces_the_hl_synth_books_and_cycles_stay_consistent() {
        let files = hl_synth::generate(hl_synth::Config::default()).unwrap();
        let expected: Value = serde_json::from_slice(&files["base/expected.json"]).unwrap();
        let snap_height = expected["snapshot_height"].as_u64().unwrap();
        let mut feed = DemoFeed::new(DemoConfig::default()).unwrap();
        assert_eq!(feed.registry().len(), 3);
        let mut checkpoints = 0;
        for _ in 0..feed.source_blocks {
            let b = feed.next_block();
            for batch in &b.batches {
                batch.validate().unwrap();
                mesh_types::allowlist::check_batch(batch).unwrap();
            }
            checkpoints += b.records.len();
            if b.block_number == snap_height {
                for (_, c) in expected["coins"].as_object().unwrap() {
                    let id = InstrumentId::new(c["instrument"].as_str().unwrap()).unwrap();
                    assert_eq!(
                        triples(&feed.book(&id).unwrap().depth_l4()),
                        expected_triples(&c["book_at_snapshot"]),
                        "{id} at snapshot height"
                    );
                }
            }
        }
        assert_eq!(checkpoints, 3 * 6);
        for (_, c) in expected["coins"].as_object().unwrap() {
            let id = InstrumentId::new(c["instrument"].as_str().unwrap()).unwrap();
            assert_eq!(
                triples(&feed.book(&id).unwrap().depth_l4()),
                expected_triples(&c["final_book"]),
                "{id} final book"
            );
        }
        // Reset block empties every book; the next cycle runs cleanly.
        let reset = feed.next_block();
        assert!(
            reset
                .batches
                .iter()
                .all(|b| b.kind == RecordKind::BookEvent)
        );
        assert!(feed.books.values().all(L4Book::is_empty));
        for _ in 0..feed.cycle_len() {
            let b = feed.next_block();
            for batch in &b.batches {
                batch.validate().unwrap();
                assert!(batch.prev_block_number.is_some());
            }
        }
    }

    #[test]
    fn gap_injection_withholds_a_book_batch() {
        let mut feed = DemoFeed::new(DemoConfig {
            gap_every: Some(5),
            ..DemoConfig::default()
        })
        .unwrap();
        let withheld: usize = (0..30).map(|_| feed.next_block().withheld.len()).sum();
        assert!(withheld > 0);
        assert!(synthetic_admission().is_ok());
    }
}
