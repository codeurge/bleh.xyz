//! Rate limits and per-IP caps.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// A token bucket: `rate` tokens per second, at most `burst` stored.
#[derive(Debug, Clone)]
pub(crate) struct TokenBucket {
    rate: f64,
    burst: f64,
    tokens: f64,
    last: Instant,
}

impl TokenBucket {
    pub(crate) fn new(rate: u32, burst: u32) -> Self {
        let burst = f64::from(burst.max(1));
        Self {
            rate: f64::from(rate),
            burst,
            tokens: burst,
            last: Instant::now(),
        }
    }

    /// Takes a token if one is available.
    pub(crate) fn try_take(&mut self) -> bool {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + dt * self.rate).min(self.burst);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Counts events in a sliding window.
#[derive(Debug, Clone)]
pub(crate) struct WindowCounter {
    window: Duration,
    events: std::collections::VecDeque<Instant>,
}

impl WindowCounter {
    pub(crate) fn new(window: Duration) -> Self {
        Self {
            window,
            events: std::collections::VecDeque::new(),
        }
    }

    /// Records an event and returns the count within the window.
    pub(crate) fn hit(&mut self) -> usize {
        let now = Instant::now();
        self.events.push_back(now);
        while let Some(&t) = self.events.front() {
            if now.duration_since(t) <= self.window {
                break;
            }
            self.events.pop_front();
        }
        self.events.len()
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct IpUse {
    connections: u32,
    channels: u32,
}

/// Connections and channels per client IP.
#[derive(Debug, Default)]
pub(crate) struct IpTracker {
    inner: Mutex<HashMap<IpAddr, IpUse>>,
}

impl IpTracker {
    /// Registers a connection unless `ip` already has `max` connections.
    pub(crate) fn try_connect(self: &Arc<Self>, ip: IpAddr, max: u32) -> Option<IpGuard> {
        let mut m = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let u = m.entry(ip).or_default();
        if u.connections >= max {
            return None;
        }
        u.connections += 1;
        Some(IpGuard {
            tracker: Arc::clone(self),
            ip,
            channels: 0,
        })
    }

    /// Connections from `ip`.
    #[cfg(test)]
    pub(crate) fn connections(&self, ip: IpAddr) -> u32 {
        let m = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        m.get(&ip).map_or(0, |u| u.connections)
    }
}

/// One registered connection; releases its slot and channels on drop.
#[derive(Debug)]
pub(crate) struct IpGuard {
    tracker: Arc<IpTracker>,
    ip: IpAddr,
    channels: u32,
}

impl IpGuard {
    /// Adds channels unless the IP would exceed `max`.
    pub(crate) fn add_channels(&mut self, n: u32, max: u32) -> bool {
        let mut m = self
            .tracker
            .inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let u = m.entry(self.ip).or_default();
        if u.channels.saturating_add(n) > max {
            return false;
        }
        u.channels += n;
        self.channels += n;
        true
    }

    /// Releases channels.
    pub(crate) fn remove_channels(&mut self, n: u32) {
        let n = n.min(self.channels);
        let mut m = self
            .tracker
            .inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(u) = m.get_mut(&self.ip) {
            u.channels = u.channels.saturating_sub(n);
        }
        self.channels -= n;
    }

    /// The client IP.
    pub(crate) fn ip(&self) -> IpAddr {
        self.ip
    }
}

impl Drop for IpGuard {
    fn drop(&mut self) {
        let mut m = self
            .tracker
            .inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(u) = m.get_mut(&self.ip) {
            u.connections = u.connections.saturating_sub(1);
            u.channels = u.channels.saturating_sub(self.channels);
            if u.connections == 0 {
                m.remove(&self.ip);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_allows_burst_then_rate() {
        let mut b = TokenBucket::new(1, 3);
        assert!(b.try_take() && b.try_take() && b.try_take());
        assert!(!b.try_take());
        std::thread::sleep(Duration::from_millis(1100));
        assert!(b.try_take());
        assert!(!b.try_take());
    }

    #[test]
    fn window_counter() {
        let mut w = WindowCounter::new(Duration::from_millis(50));
        assert_eq!(w.hit(), 1);
        assert_eq!(w.hit(), 2);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(w.hit(), 1);
    }

    #[test]
    fn ip_caps_release_on_drop() {
        let t = Arc::new(IpTracker::default());
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let mut a = t.try_connect(ip, 2).unwrap();
        let mut b = t.try_connect(ip, 2).unwrap();
        assert!(t.try_connect(ip, 2).is_none());
        assert!(a.add_channels(3, 5));
        assert!(!b.add_channels(3, 5));
        assert!(b.add_channels(2, 5));
        a.remove_channels(10);
        assert!(b.add_channels(3, 5));
        drop(a);
        assert_eq!(t.connections(ip), 1);
        let c = t.try_connect(ip, 2).unwrap();
        assert_eq!(c.ip(), ip);
        drop(b);
        drop(c);
        assert_eq!(t.connections(ip), 0);
        assert!(t.inner.lock().unwrap().is_empty());
    }
}
