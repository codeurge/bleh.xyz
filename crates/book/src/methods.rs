//! Method identifiers (`name@version`) of every derived record this crate
//! emits. Each is specified in `docs/operations/book.md`; changing a
//! method's output means a new version.

/// Time bars (`Bar`), direct from trades or rolled up from 1m bars.
pub const BARS: &str = "bars@1";
/// Tick bars: a bar every N trades.
pub const TICK_BARS: &str = "tick-bars@1";
/// Volume bars: a bar when traded size reaches a threshold.
pub const VOLUME_BARS: &str = "volume-bars@1";
/// Range bars: a bar whose high minus low stays within a width.
pub const RANGE_BARS: &str = "range-bars@1";
/// Footprint: per-price bid and ask volume per interval.
pub const FOOTPRINT: &str = "footprint@1";
/// Delta and cumulative volume delta per interval.
pub const DELTA: &str = "delta@1";
/// Volume profile per period.
pub const VOLUME_PROFILE: &str = "volume-profile@1";
/// Point of control and value area of a volume profile.
pub const VALUE_AREA: &str = "value-area@1";
/// Order activity: lifetimes and cancel/replace ratios per interval.
pub const ORDER_ACTIVITY: &str = "order-activity@1";
/// Queue positions of top-of-book orders.
pub const QUEUE_POSITION: &str = "queue-position@1";
/// Sweeps: aggressive orders consuming several price levels in a block.
pub const SWEEP: &str = "sweep@1";
/// L3 heatmap tiles: resting size on a time by price grid.
pub const HEATMAP: &str = "heatmap@1";
/// Liquidity grade and tape/benchmark label.
pub const LIQUIDITY_GRADE: &str = "liquidity-grade@1";

/// Every method, for documentation and tests.
pub const ALL: [&str; 13] = [
    BARS,
    TICK_BARS,
    VOLUME_BARS,
    RANGE_BARS,
    FOOTPRINT,
    DELTA,
    VOLUME_PROFILE,
    VALUE_AREA,
    ORDER_ACTIVITY,
    QUEUE_POSITION,
    SWEEP,
    HEATMAP,
    LIQUIDITY_GRADE,
];

#[cfg(test)]
mod tests {
    use mesh_types::MethodId;

    #[test]
    fn every_method_id_is_valid() {
        for m in super::ALL {
            assert!(MethodId::new(m).is_ok(), "{m}");
        }
    }
}
