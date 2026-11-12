//! The bleh.xyz side: turns gateway events (live, or from a recording made
//! with `mesh-client --record`) into price observations per instrument.

use std::collections::HashMap;
use std::sync::Arc;

use mesh_types::{BarInterval, BlockBatch, InstrumentId, Price, Record};

use crate::book::L2Book;
use crate::client::ClientEvent;
use crate::protocol::Levels;

/// Which bleh.xyz series an observation belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObsSource {
    /// A trade.
    Trade,
    /// The L2 book mid.
    Mid,
    /// A bar close.
    Bar(BarInterval),
}

/// One bleh.xyz price observation (CC0 data).
#[derive(Debug, Clone, PartialEq)]
pub struct BlehObs {
    /// Instrument.
    pub instrument: InstrumentId,
    /// Series.
    pub source: ObsSource,
    /// Venue (source) time, Unix nanoseconds.
    pub ts_ns: i64,
    /// Price.
    pub price: f64,
}

/// The gateway events the tracker uses.
#[derive(Debug, Clone)]
pub enum BlehInput {
    /// A batch (trades are used).
    Batch(Arc<BlockBatch>),
    /// A single record (bars are used).
    Record(Arc<Record>),
    /// A book snapshot.
    Snapshot(Arc<Record>),
    /// L2 level changes.
    Levels(Arc<Levels>),
    /// A block heartbeat: the venue's data up to this time was delivered.
    Block {
        /// Block time, Unix nanoseconds.
        ts_ns: i64,
    },
    /// A gap for an instrument: its book is no longer valid.
    Gap(InstrumentId),
    /// The connection dropped: every book is invalid.
    Reset,
}

impl BlehInput {
    /// Event time used to order a recording against the user's feed.
    pub fn ts_ns(&self) -> Option<i64> {
        match self {
            Self::Batch(b) => Some(ms_to_ns(b.block_time_ms.0)),
            Self::Record(r) | Self::Snapshot(r) => Some(ms_to_ns(r.prov().ts_source_ms.0)),
            Self::Levels(l) => Some(ms_to_ns(l.block_time_ms.0)),
            Self::Block { ts_ns } => Some(*ts_ns),
            Self::Gap(_) | Self::Reset => None,
        }
    }

    /// From a live client event.
    pub fn from_event(e: &ClientEvent) -> Option<Self> {
        Some(match e {
            ClientEvent::Batch { batch } => Self::Batch(batch.clone()),
            ClientEvent::Record { record } => Self::Record(record.clone()),
            ClientEvent::Snapshot { record, .. } => Self::Snapshot(record.clone()),
            ClientEvent::Levels { levels } => Self::Levels(levels.clone()),
            ClientEvent::Block { block_time_ms, .. } => Self::Block {
                ts_ns: ms_to_ns(block_time_ms.0),
            },
            ClientEvent::Gap { instrument, .. } => Self::Gap(instrument.clone()),
            ClientEvent::Disconnected { .. } => Self::Reset,
            _ => return None,
        })
    }

    /// From one line of a `mesh-client --record` file. Lines for events the
    /// tracker does not use give `Ok(None)`.
    ///
    /// # Errors
    ///
    /// A line that is not a recorded client event.
    pub fn from_recorded_line(line: &str) -> Result<Option<Self>, String> {
        let line = line.trim();
        if line.is_empty() {
            return Ok(None);
        }
        let mut v: serde_json::Value =
            serde_json::from_str(line).map_err(|e| format!("not JSON: {e}"))?;
        let event = v
            .get("event")
            .and_then(serde_json::Value::as_str)
            .ok_or("no \"event\" field")?
            .to_string();
        let mut take = |k: &str| {
            v.get_mut(k)
                .map(serde_json::Value::take)
                .ok_or(format!("{event} without {k}"))
        };
        let bad = |e: serde_json::Error| format!("{event}: {e}");
        Ok(Some(match event.as_str() {
            "batch" => Self::Batch(Arc::new(
                serde_json::from_value(take("batch")?).map_err(bad)?,
            )),
            "record" => Self::Record(Arc::new(
                serde_json::from_value(take("record")?).map_err(bad)?,
            )),
            "snapshot" => Self::Snapshot(Arc::new(
                serde_json::from_value(take("record")?).map_err(bad)?,
            )),
            "levels" => Self::Levels(Arc::new(
                serde_json::from_value(take("levels")?).map_err(bad)?,
            )),
            "block" => {
                let ms: i64 = serde_json::from_value(take("block_time_ms")?).map_err(bad)?;
                Self::Block {
                    ts_ns: ms_to_ns(ms),
                }
            }
            "gap" => Self::Gap(serde_json::from_value(take("instrument")?).map_err(bad)?),
            "disconnected" => Self::Reset,
            _ => return Ok(None),
        }))
    }
}

fn ms_to_ns(ms: i64) -> i64 {
    ms.saturating_mul(1_000_000)
}

/// A fixed-point price as `f64`, for local comparison only.
pub fn price_f64(p: Price) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let m = p.mantissa() as f64;
    m / 10f64.powi(i32::from(p.scale()))
}

/// Keeps L2 books for mids and turns inputs into observations.
#[derive(Debug, Default)]
pub struct BlehTracker {
    books: HashMap<InstrumentId, L2Book>,
}

impl BlehTracker {
    /// Observations from one input, and the heartbeat time if it is one.
    pub fn on_input(&mut self, input: &BlehInput) -> (Vec<BlehObs>, Option<i64>) {
        let mut out = Vec::new();
        match input {
            BlehInput::Batch(b) => {
                for r in &b.records {
                    if let Record::Trade(t) = r {
                        out.push(BlehObs {
                            instrument: t.prov.instrument.clone(),
                            source: ObsSource::Trade,
                            ts_ns: ms_to_ns(t.prov.ts_source_ms.0),
                            price: price_f64(t.price),
                        });
                    }
                }
            }
            BlehInput::Record(r) => {
                if let Record::Bar(b) = &**r {
                    out.push(BlehObs {
                        instrument: b.prov.instrument.clone(),
                        source: ObsSource::Bar(b.interval),
                        ts_ns: ms_to_ns(b.prov.ts_source_ms.0),
                        price: price_f64(b.close),
                    });
                }
            }
            BlehInput::Snapshot(r) => {
                let inst = r.prov().instrument.clone();
                match L2Book::from_record(r) {
                    Ok(book) => {
                        if let Some(m) = mid(&book) {
                            out.push(BlehObs {
                                instrument: inst.clone(),
                                source: ObsSource::Mid,
                                ts_ns: ms_to_ns(r.prov().ts_source_ms.0),
                                price: m,
                            });
                        }
                        self.books.insert(inst, book);
                    }
                    Err(_) => {
                        self.books.remove(&inst);
                    }
                }
            }
            BlehInput::Levels(l) => {
                if let Some(book) = self.books.get_mut(&l.instrument) {
                    if book.apply_levels(l).is_ok() {
                        if let Some(m) = mid(book) {
                            out.push(BlehObs {
                                instrument: l.instrument.clone(),
                                source: ObsSource::Mid,
                                ts_ns: ms_to_ns(l.block_time_ms.0),
                                price: m,
                            });
                        }
                    } else {
                        self.books.remove(&l.instrument);
                    }
                }
            }
            BlehInput::Block { ts_ns } => return (out, Some(*ts_ns)),
            BlehInput::Gap(i) => {
                self.books.remove(i);
            }
            BlehInput::Reset => self.books.clear(),
        }
        (out, None)
    }
}

fn mid(book: &L2Book) -> Option<f64> {
    match book.top() {
        (Some(b), Some(a)) => {
            let (b, a) = (price_f64(b), price_f64(a));
            (b <= a).then_some((b + a) / 2.0)
        }
        _ => None,
    }
}
