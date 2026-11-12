//! Order-flow analytics.
//!
//! From trades: [`footprint`], [`delta`] (delta and CVD), [`profile`]
//! (volume profile, POC and value area) and [`sweep`]. From the book:
//! [`activity`] (order lifetimes, cancel/replace ratios), [`queue`]
//! (queue positions of top-of-book orders) and [`heatmap`] (L3 heatmap
//! tiles). Every record carries a [`DerivedHeader`](mesh_types::DerivedHeader)
//! with its method and inputs hash; methods are specified in
//! `docs/operations/book.md`.

pub mod activity;
pub mod delta;
pub mod footprint;
pub mod heatmap;
pub mod profile;
pub mod queue;
pub mod sweep;

pub use activity::{LifetimeStats, OrderActivity, OrderActivityBuilder, OrderActivityConfig};
pub use delta::{DeltaBar, DeltaBuilder, deltas};
pub use footprint::{FootprintBuilder, footprints};
pub use heatmap::{HeatCell, HeatmapBuilder, HeatmapConfig, HeatmapTile};
pub use profile::{
    DEFAULT_VALUE_AREA_BPS, ProfileBuilder, ProfileConfig, ProfileSummary, ValueArea, value_area,
    volume_profiles,
};
pub use queue::{QueueConfig, QueueEntry, QueueLevel, QueuePositions, QueueTracker, top_queue};
pub use sweep::{Sweep, SweepConfig, SweepDetector, sweeps};
