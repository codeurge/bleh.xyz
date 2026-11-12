//! Gateway configuration.

use std::time::Duration;

use mesh_client::Limits;
use mesh_types::Registry;

/// Gateway settings (independent of where the data comes from).
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// Limits announced in `hello` and enforced per connection.
    pub limits: Limits,
    /// Channels per client IP, summed over its connections.
    pub channels_per_ip: u32,
    /// Fan-out ring capacity (items).
    pub ring_capacity: usize,
    /// Rate-limit violations within [`GatewayConfig::violation_window`]
    /// that close the connection (code 4029).
    pub violations_before_close: u32,
    /// Window for counting rate-limit violations.
    pub violation_window: Duration,
    /// A second slow-consumer strike within this window disconnects (4008)
    /// instead of dropping to snapshot.
    pub slow_consumer_window: Duration,
    /// Take the client IP from the first `X-Forwarded-For` entry (only
    /// behind a trusted proxy or CDN).
    pub trust_forwarded_for: bool,
    /// `/health` fails when no block closed for this long.
    pub max_staleness: Duration,
    /// Registry of known instruments; subscriptions to other exact
    /// instruments get `unknown_instrument`. `None` accepts any valid ID.
    pub registry: Option<Registry>,
    /// Hub snapshot request queue length.
    pub snapshot_queue: usize,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            channels_per_ip: 2_000,
            ring_capacity: bus::DEFAULT_RING_CAPACITY,
            violations_before_close: 10,
            violation_window: Duration::from_secs(10),
            slow_consumer_window: Duration::from_secs(60),
            trust_forwarded_for: false,
            max_staleness: Duration::from_secs(5),
            registry: None,
            snapshot_queue: 4096,
        }
    }
}

impl GatewayConfig {
    /// The send-buffer time limit.
    #[must_use]
    pub fn send_buffer_time(&self) -> Duration {
        Duration::from_millis(self.limits.send_buffer_ms)
    }

    /// The minimum interval between `resnapshot`s for one instrument.
    #[must_use]
    pub fn resnapshot_interval(&self) -> Duration {
        Duration::from_millis(self.limits.resnapshot_interval_ms)
    }
}
