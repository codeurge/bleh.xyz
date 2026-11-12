//! A bounded, lock-light broadcast ring with per-subscriber cursors.
//!
//! One or more producers append items; every receiver reads every item at
//! its own pace through its own cursor. Nothing is copied per receiver: an
//! item is stored once as an `Arc<T>` and each receiver clones the `Arc`.
//!
//! - **Bounded.** The ring keeps the newest `capacity` items. Producers
//!   never wait for receivers.
//! - **Lag detection.** A receiver whose cursor falls more than `capacity`
//!   items behind the head gets [`RecvError::Lagged`] with the number of
//!   items it missed, and its cursor jumps to the oldest retained item. It
//!   can also check how far behind it is ([`RingReceiver::behind`]) and how
//!   old its next item is ([`RingReceiver::next_age`]) before it overruns.
//! - **Lock-light.** Each slot has its own reader-writer lock, so readers
//!   only contend with a producer writing the same slot (which happens only
//!   to a receiver that is about to lag). Producers serialize on a short
//!   mutex around the head counter. Waiting receivers are woken through one
//!   [`Notify`]; [`RingSender::publish_quiet`] plus [`RingSender::wake`]
//!   lets a producer append a burst and wake receivers once.
//!
//! When the last [`RingSender`] is dropped the ring closes: receivers drain
//! what is left and then get [`RecvError::Closed`].

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

/// Error from receiving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RecvError {
    /// The receiver fell behind and this many items were overwritten before
    /// it read them. The cursor now points at the oldest retained item.
    #[error("receiver lagged by {0} items")]
    Lagged(u64),
    /// Every sender is gone and the receiver has read everything.
    #[error("ring closed")]
    Closed,
}

struct Slot<T> {
    seq: u64,
    item: Option<Arc<T>>,
    at: Option<Instant>,
}

struct Shared<T> {
    slots: Box<[RwLock<Slot<T>>]>,
    capacity: u64,
    head: AtomicU64,
    write: Mutex<()>,
    notify: Notify,
    senders: AtomicUsize,
    closed: AtomicBool,
    lag_events: AtomicU64,
    skipped: AtomicU64,
}

impl<T> Shared<T> {
    fn slot(&self, seq: u64) -> &RwLock<Slot<T>> {
        // capacity fits in usize because it came from a usize.
        let i = usize::try_from(seq % self.capacity).unwrap_or(0);
        &self.slots[i]
    }
}

/// Counters for one ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RingStats {
    /// Items published since the ring was created.
    pub published: u64,
    /// Times a receiver lagged.
    pub lag_events: u64,
    /// Items receivers missed because they lagged.
    pub skipped: u64,
}

/// Creates a ring that retains `capacity` items (at least 1).
#[must_use]
pub fn ring<T>(capacity: usize) -> RingSender<T> {
    let capacity = capacity.max(1);
    let slots = (0..capacity)
        .map(|_| {
            RwLock::new(Slot {
                seq: u64::MAX,
                item: None,
                at: None,
            })
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    RingSender {
        shared: Arc::new(Shared {
            slots,
            capacity: u64::try_from(capacity).unwrap_or(u64::MAX),
            head: AtomicU64::new(0),
            write: Mutex::new(()),
            notify: Notify::new(),
            senders: AtomicUsize::new(1),
            closed: AtomicBool::new(false),
            lag_events: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
        }),
    }
}

/// Producer handle. Cloning adds a producer; the ring closes when the last
/// one is dropped.
pub struct RingSender<T> {
    shared: Arc<Shared<T>>,
}

impl<T> fmt::Debug for RingSender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RingSender")
            .field("capacity", &self.shared.capacity)
            .field("head", &self.head())
            .finish()
    }
}

impl<T> Clone for RingSender<T> {
    fn clone(&self) -> Self {
        self.shared.senders.fetch_add(1, Ordering::Relaxed);
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T> Drop for RingSender<T> {
    fn drop(&mut self) {
        if self.shared.senders.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.shared.closed.store(true, Ordering::Release);
            self.shared.notify.notify_waiters();
        }
    }
}

impl<T> RingSender<T> {
    /// Appends an item and wakes waiting receivers. Returns its sequence
    /// number.
    pub fn publish(&self, item: T) -> u64 {
        self.publish_arc(Arc::new(item))
    }

    /// Appends a shared item and wakes waiting receivers.
    pub fn publish_arc(&self, item: Arc<T>) -> u64 {
        let seq = self.publish_quiet(item);
        self.wake();
        seq
    }

    /// Appends an item without waking receivers. Call [`RingSender::wake`]
    /// after a burst.
    pub fn publish_quiet(&self, item: Arc<T>) -> u64 {
        let _guard = self
            .shared
            .write
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let seq = self.shared.head.load(Ordering::Relaxed);
        {
            let mut slot = self
                .shared
                .slot(seq)
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            slot.seq = seq;
            slot.item = Some(item);
            slot.at = Some(Instant::now());
        }
        self.shared.head.store(seq + 1, Ordering::Release);
        seq
    }

    /// Wakes every waiting receiver.
    pub fn wake(&self) {
        self.shared.notify.notify_waiters();
    }

    /// A receiver that starts at the next published item.
    #[must_use]
    pub fn subscribe(&self) -> RingReceiver<T> {
        RingReceiver {
            shared: Arc::clone(&self.shared),
            cursor: self.head(),
        }
    }

    /// A receiver that starts at the oldest retained item.
    #[must_use]
    pub fn subscribe_from_oldest(&self) -> RingReceiver<T> {
        let head = self.head();
        RingReceiver {
            shared: Arc::clone(&self.shared),
            cursor: head.saturating_sub(self.shared.capacity),
        }
    }

    /// Sequence number the next item will get.
    #[must_use]
    pub fn head(&self) -> u64 {
        self.shared.head.load(Ordering::Acquire)
    }

    /// Number of items retained.
    #[must_use]
    pub fn capacity(&self) -> u64 {
        self.shared.capacity
    }

    /// Counters.
    #[must_use]
    pub fn stats(&self) -> RingStats {
        RingStats {
            published: self.head(),
            lag_events: self.shared.lag_events.load(Ordering::Relaxed),
            skipped: self.shared.skipped.load(Ordering::Relaxed),
        }
    }
}

/// Consumer handle with its own cursor.
pub struct RingReceiver<T> {
    shared: Arc<Shared<T>>,
    cursor: u64,
}

impl<T> fmt::Debug for RingReceiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RingReceiver")
            .field("cursor", &self.cursor)
            .field("behind", &self.behind())
            .finish()
    }
}

impl<T> RingReceiver<T> {
    /// Returns the next item if one is available, `Ok(None)` if the
    /// receiver is caught up.
    ///
    /// # Errors
    ///
    /// [`RecvError::Lagged`] if items were overwritten before this receiver
    /// read them (the next call continues from the oldest retained item);
    /// [`RecvError::Closed`] once every sender is gone and nothing is left.
    pub fn try_recv(&mut self) -> Result<Option<Arc<T>>, RecvError> {
        let head = self.shared.head.load(Ordering::Acquire);
        if self.cursor >= head {
            if self.shared.closed.load(Ordering::Acquire)
                && self.cursor >= self.shared.head.load(Ordering::Acquire)
            {
                return Err(RecvError::Closed);
            }
            return Ok(None);
        }
        let oldest = head.saturating_sub(self.shared.capacity);
        if self.cursor < oldest {
            return Err(self.lagged(oldest));
        }
        let slot = self
            .shared
            .slot(self.cursor)
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        if slot.seq != self.cursor {
            // Overwritten between the head check and the read.
            drop(slot);
            let oldest = self
                .shared
                .head
                .load(Ordering::Acquire)
                .saturating_sub(self.shared.capacity);
            return Err(self.lagged(oldest.max(self.cursor + 1)));
        }
        let item = slot.item.clone();
        drop(slot);
        self.cursor += 1;
        Ok(item)
    }

    fn lagged(&mut self, to: u64) -> RecvError {
        let missed = to - self.cursor;
        self.cursor = to;
        self.shared.lag_events.fetch_add(1, Ordering::Relaxed);
        self.shared.skipped.fetch_add(missed, Ordering::Relaxed);
        RecvError::Lagged(missed)
    }

    /// Waits for the next item.
    ///
    /// # Errors
    ///
    /// As [`RingReceiver::try_recv`].
    pub async fn recv(&mut self) -> Result<Arc<T>, RecvError> {
        let shared = Arc::clone(&self.shared);
        loop {
            let notified = shared.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(item) = self.try_recv()? {
                return Ok(item);
            }
            notified.await;
        }
    }

    /// Number of published items this receiver has not read yet.
    #[must_use]
    pub fn behind(&self) -> u64 {
        self.shared
            .head
            .load(Ordering::Acquire)
            .saturating_sub(self.cursor)
    }

    /// How long ago the next unread item was published, if there is one.
    #[must_use]
    pub fn next_age(&self) -> Option<Duration> {
        if self.behind() == 0 {
            return None;
        }
        let slot = self
            .shared
            .slot(self.cursor)
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        if slot.seq != self.cursor {
            // Already overwritten: as old as anything can be.
            return Some(Duration::MAX);
        }
        slot.at.map(|t| t.elapsed())
    }

    /// Drops everything unread and continues from the next published item.
    /// Returns how many items were dropped.
    pub fn skip_to_head(&mut self) -> u64 {
        let head = self.shared.head.load(Ordering::Acquire);
        let dropped = head.saturating_sub(self.cursor);
        self.cursor = head;
        dropped
    }

    /// Sequence number of the next item this receiver will read.
    #[must_use]
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// A new receiver on the same ring, starting at the next published
    /// item.
    #[must_use]
    pub fn resubscribe(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            cursor: self.shared.head.load(Ordering::Acquire),
        }
    }

    /// True once every sender is gone.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_receiver_sees_every_item_in_order() {
        let tx = ring::<u32>(8);
        let mut a = tx.subscribe();
        let mut b = tx.subscribe();
        for i in 0..5 {
            tx.publish(i);
        }
        for i in 0..5 {
            assert_eq!(*a.try_recv().unwrap().unwrap(), i);
        }
        assert_eq!(a.try_recv().unwrap(), None);
        assert_eq!(b.behind(), 5);
        for i in 0..5 {
            assert_eq!(*b.try_recv().unwrap().unwrap(), i);
        }
        assert_eq!(tx.stats().published, 5);
    }

    #[test]
    fn lagging_receiver_is_told_and_resumes_at_oldest() {
        let tx = ring::<u32>(4);
        let mut rx = tx.subscribe();
        for i in 0..10 {
            tx.publish(i);
        }
        assert_eq!(rx.try_recv(), Err(RecvError::Lagged(6)));
        for i in 6..10 {
            assert_eq!(*rx.try_recv().unwrap().unwrap(), i);
        }
        assert_eq!(rx.try_recv().unwrap(), None);
        let s = tx.stats();
        assert_eq!((s.lag_events, s.skipped), (1, 6));
    }

    #[test]
    fn subscribe_positions_and_skip() {
        let tx = ring::<u32>(4);
        for i in 0..6 {
            tx.publish(i);
        }
        let mut old = tx.subscribe_from_oldest();
        assert_eq!(*old.try_recv().unwrap().unwrap(), 2);
        let mut new = tx.subscribe();
        assert_eq!(new.try_recv().unwrap(), None);
        tx.publish(6);
        assert!(old.next_age().is_some());
        assert_eq!(old.skip_to_head(), 4);
        assert_eq!(old.behind(), 0);
        assert_eq!(old.next_age(), None);
        assert_eq!(*new.try_recv().unwrap().unwrap(), 6);
        let re = new.resubscribe();
        assert_eq!(re.cursor(), 7);
    }

    #[test]
    fn closes_when_last_sender_drops() {
        let tx = ring::<u32>(4);
        let tx2 = tx.clone();
        let mut rx = tx.subscribe();
        tx.publish(1);
        drop(tx);
        assert!(!rx.is_closed());
        tx2.publish(2);
        drop(tx2);
        assert!(rx.is_closed());
        assert_eq!(*rx.try_recv().unwrap().unwrap(), 1);
        assert_eq!(*rx.try_recv().unwrap().unwrap(), 2);
        assert_eq!(rx.try_recv(), Err(RecvError::Closed));
    }

    #[tokio::test]
    async fn async_receivers_wake_on_publish() {
        let tx = ring::<u64>(1024);
        let mut handles = Vec::new();
        for _ in 0..8 {
            let mut rx = tx.subscribe();
            handles.push(tokio::spawn(async move {
                let mut sum = 0u64;
                loop {
                    match rx.recv().await {
                        Ok(v) => sum += *v,
                        Err(RecvError::Closed) => return sum,
                        Err(RecvError::Lagged(_)) => panic!("lagged"),
                    }
                }
            }));
        }
        let producer = tokio::spawn(async move {
            for i in 0..1000u64 {
                tx.publish_quiet(Arc::new(i));
                if i % 10 == 9 {
                    tx.wake();
                    tokio::task::yield_now().await;
                }
            }
            tx.wake();
        });
        producer.await.unwrap();
        for h in handles {
            assert_eq!(h.await.unwrap(), (0..1000).sum::<u64>());
        }
    }

    #[test]
    fn concurrent_producers_and_readers_never_see_torn_order() {
        let tx = ring::<(usize, u64)>(64);
        let mut readers = Vec::new();
        for _ in 0..4 {
            let mut rx = tx.subscribe();
            readers.push(std::thread::spawn(move || {
                let mut last = [None::<u64>; 4];
                let mut seen = 0u64;
                loop {
                    match rx.try_recv() {
                        Ok(Some(v)) => {
                            let (p, n) = *v;
                            if let Some(prev) = last[p] {
                                assert!(n > prev, "per-producer order");
                            }
                            last[p] = Some(n);
                            seen += 1;
                        }
                        Ok(None) => std::thread::yield_now(),
                        Err(RecvError::Lagged(k)) => seen += k,
                        Err(RecvError::Closed) => return seen,
                    }
                }
            }));
        }
        let producers: Vec<_> = (0..4)
            .map(|p| {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    for n in 0..5000u64 {
                        tx.publish((p, n));
                    }
                })
            })
            .collect();
        drop(tx);
        for p in producers {
            p.join().unwrap();
        }
        for r in readers {
            assert_eq!(r.join().unwrap(), 20_000);
        }
    }
}
