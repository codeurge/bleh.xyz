//! Cost estimate and coverage from listings.
//!
//! Lists the selected datasets, sums object sizes, and prices the copy at
//! configurable AWS rates: requester-pays transfer per GiB, GET requests
//! per 1,000 and LIST requests per 1,000. The same listing gives each
//! dataset's coverage (first and last day or height present), which is the
//! coverage record ADM-0003 asks for.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::buckets::Dataset;
use crate::listing::Listed;

/// AWS prices used by the estimate. Defaults are S3 us-east-1 list prices
/// checked on 2026-11-12; pass current values for the bucket's region.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Prices {
    /// USD per GiB transferred out (internet egress: 0.09 for the first
    /// 10 TB a month; 0 when copying from compute in the bucket's region).
    pub transfer_usd_per_gib: f64,
    /// USD per 1,000 GET requests.
    pub get_usd_per_1000: f64,
    /// USD per 1,000 LIST requests.
    pub list_usd_per_1000: f64,
}

impl Default for Prices {
    fn default() -> Self {
        Self {
            transfer_usd_per_gib: 0.09,
            get_usd_per_1000: 0.0004,
            list_usd_per_1000: 0.005,
        }
    }
}

/// Totals and coverage for one dataset.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DatasetEstimate {
    /// Objects selected.
    pub objects: u64,
    /// Total bytes.
    pub bytes: u64,
    /// LIST requests made.
    pub list_requests: u64,
    /// Keys that did not match the layout.
    pub skipped_keys: u64,
    /// First day present (`YYYY-MM-DD`).
    pub first_day: Option<String>,
    /// Last day present.
    pub last_day: Option<String>,
    /// First block height present (height-keyed datasets).
    pub first_height: Option<u64>,
    /// Last block height present.
    pub last_height: Option<u64>,
    /// Estimated cost in USD.
    pub usd: f64,
}

/// The whole estimate.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Estimate {
    /// Prices used.
    pub prices: Option<Prices>,
    /// Per dataset, by name.
    pub datasets: BTreeMap<String, DatasetEstimate>,
    /// Total objects.
    pub total_objects: u64,
    /// Total bytes.
    pub total_bytes: u64,
    /// Total estimated cost in USD.
    pub total_usd: f64,
}

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

impl Estimate {
    /// An empty estimate at `prices`.
    #[must_use]
    pub fn new(prices: Prices) -> Self {
        Self {
            prices: Some(prices),
            ..Self::default()
        }
    }

    /// Adds one dataset's listing.
    pub fn add(&mut self, dataset: Dataset, listed: &Listed) {
        let p = self.prices.unwrap_or_default();
        let mut e = DatasetEstimate {
            objects: listed.objects.len() as u64,
            list_requests: listed.list_requests,
            skipped_keys: listed.skipped.len() as u64,
            ..DatasetEstimate::default()
        };
        for o in &listed.objects {
            e.bytes += o.size;
            if let Some(d) = o.info.day {
                let iso = d.iso();
                if e.first_day.as_ref().is_none_or(|f| iso < *f) {
                    e.first_day = Some(iso.clone());
                }
                if e.last_day.as_ref().is_none_or(|l| iso > *l) {
                    e.last_day = Some(iso);
                }
            }
            if let Some(h) = o.info.height {
                e.first_height = Some(e.first_height.map_or(h, |f| f.min(h)));
                e.last_height = Some(e.last_height.map_or(h, |l| l.max(h)));
            }
        }
        e.usd = cost(&p, e.bytes, e.objects, e.list_requests);
        self.total_objects += e.objects;
        self.total_bytes += e.bytes;
        self.total_usd += e.usd;
        self.datasets.insert(dataset.prefix().to_owned(), e);
    }
}

/// Cost of transferring `bytes` in `gets` objects after `lists` LIST
/// requests.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn cost(p: &Prices, bytes: u64, gets: u64, lists: u64) -> f64 {
    bytes as f64 / GIB * p.transfer_usd_per_gib
        + gets as f64 / 1000.0 * p.get_usd_per_1000
        + lists as f64 / 1000.0 * p.list_usd_per_1000
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listing::tests::synthetic_node_bucket;
    use crate::listing::{Selection, list_dataset};

    #[test]
    fn prices_a_tebibyte() {
        let p = Prices::default();
        let c = cost(&p, 1024 * 1024 * 1024 * 1024, 1_000_000, 1000);
        // 1024 GiB * 0.09 + 1000 * 0.0004 + 1 * 0.005
        assert!((c - (92.16 + 0.4 + 0.005)).abs() < 1e-9, "{c}");
    }

    #[tokio::test]
    async fn reports_coverage_from_listing() {
        let s = synthetic_node_bucket().await;
        let mut est = Estimate::new(Prices::default());
        for d in [Dataset::NodeFillsByBlock, Dataset::ExplorerBlocks] {
            let l = list_dataset(&s, d, &Selection::default()).await.unwrap();
            est.add(d, &l);
        }
        let f = &est.datasets["node_fills_by_block"];
        assert_eq!(f.objects, 3);
        assert_eq!(f.first_day.as_deref(), Some("2025-07-27"));
        assert_eq!(f.last_day.as_deref(), Some("2025-07-28"));
        assert_eq!(f.skipped_keys, 1);
        let e = &est.datasets["explorer_blocks"];
        assert_eq!(
            (e.first_height, e.last_height),
            (Some(559_148_100), Some(600_000_000))
        );
        assert_eq!(est.total_objects, 6);
        assert!(est.total_usd > 0.0);
    }
}
