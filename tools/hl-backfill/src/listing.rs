//! Listing admitted datasets with a selection (days, heights, coins).
//!
//! Listing never leaves the admitted prefixes: every prefix passed to the
//! store is built from a [`Dataset`] and checked against the denylist
//! first, and every key returned is checked again with
//! [`check_key`](crate::buckets::check_key).

use futures::TryStreamExt;
use object_store::ObjectStore;
use object_store::path::Path;
use serde::Serialize;

use crate::buckets::{
    Dataset, Day, KeyInfo, Layout, check_key, denied_prefix, numeric_dir, parse_key,
};

/// Which objects to list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    /// First day, inclusive.
    pub from: Option<Day>,
    /// Last day, inclusive.
    pub to: Option<Day>,
    /// First block height, inclusive (`explorer_blocks`, `replica_cmds`).
    pub from_height: Option<u64>,
    /// Last block height, inclusive.
    pub to_height: Option<u64>,
    /// Coins to keep (`market_data`); empty keeps every coin.
    pub coins: Vec<String>,
    /// Work split across hosts: `(index, count)` keeps objects whose shard
    /// ordinal is `index` modulo `count`.
    pub shard: Option<(u64, u64)>,
}

impl Selection {
    fn day_ok(&self, d: Day) -> bool {
        self.from.is_none_or(|f| d >= f) && self.to.is_none_or(|t| d <= t)
    }

    fn height_ok(&self, h: u64) -> bool {
        self.from_height.is_none_or(|f| h >= f) && self.to_height.is_none_or(|t| h <= t)
    }

    /// True if an object with this key info is selected.
    #[must_use]
    pub fn keeps(&self, info: &KeyInfo) -> bool {
        info.day.is_none_or(|d| self.day_ok(d))
            && info.height.is_none_or(|h| self.height_ok(h))
            && (self.coins.is_empty()
                || info
                    .coin
                    .as_ref()
                    .is_none_or(|c| self.coins.iter().any(|k| k == c)))
            && self
                .shard
                .is_none_or(|(i, n)| n == 0 || info.shard_ordinal() % n == i)
    }
}

/// One listed source object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceObject {
    /// Dataset.
    #[serde(serialize_with = "ser_display")]
    pub dataset: Dataset,
    /// Bucket name.
    pub bucket: String,
    /// Key, exactly as listed.
    pub key: String,
    /// Size in bytes.
    pub size: u64,
    /// ETag, as listed.
    pub etag: Option<String>,
    /// Parsed key.
    #[serde(skip)]
    pub info: KeyInfo,
}

fn ser_display<S: serde::Serializer, T: std::fmt::Display>(v: &T, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(v)
}

/// Result of listing one dataset.
#[derive(Debug, Default)]
pub struct Listed {
    /// Selected objects, sorted by key info (day, hour, height) then key.
    pub objects: Vec<SourceObject>,
    /// Keys under the prefix that did not match the layout, with reasons.
    pub skipped: Vec<(String, String)>,
    /// List requests made (an estimate: one per call plus one per 1,000
    /// keys returned).
    pub list_requests: u64,
}

/// Errors from listing.
#[derive(Debug, thiserror::Error)]
pub enum ListError {
    /// The store failed.
    #[error("listing {prefix}: {source}")]
    Store {
        /// The prefix being listed.
        prefix: String,
        /// The error.
        source: object_store::Error,
    },
    /// A prefix that would touch an excluded location. Never expected:
    /// prefixes are built from admitted datasets.
    #[error("refusing to list {0}: excluded by ADM-0003")]
    Denied(String),
}

fn prefix_path(p: &str) -> Result<Path, ListError> {
    if denied_prefix(p).is_some() {
        return Err(ListError::Denied(p.to_owned()));
    }
    Path::parse(p).map_err(|e| ListError::Store {
        prefix: p.to_owned(),
        source: e.into(),
    })
}

async fn list_dirs(
    store: &dyn ObjectStore,
    prefix: &str,
    reqs: &mut u64,
) -> Result<Vec<String>, ListError> {
    let path = prefix_path(prefix)?;
    let res = store
        .list_with_delimiter(Some(&path))
        .await
        .map_err(|source| ListError::Store {
            prefix: prefix.to_owned(),
            source,
        })?;
    *reqs += 1 + (res.common_prefixes.len() + res.objects.len()) as u64 / 1000;
    Ok(res
        .common_prefixes
        .iter()
        .filter_map(|p| p.filename().map(str::to_owned))
        .collect())
}

async fn list_all(
    store: &dyn ObjectStore,
    prefix: &str,
    reqs: &mut u64,
) -> Result<Vec<object_store::ObjectMeta>, ListError> {
    let path = prefix_path(prefix)?;
    let metas: Vec<_> = store
        .list(Some(&path))
        .try_collect()
        .await
        .map_err(|source| ListError::Store {
            prefix: prefix.to_owned(),
            source,
        })?;
    *reqs += 1 + metas.len() as u64 / 1000;
    Ok(metas)
}

/// Picks the numeric directories whose range `[dir, next dir)` may hold a
/// height in the selection.
fn pick_ranges(mut dirs: Vec<(u64, String)>, sel: &Selection) -> Vec<String> {
    dirs.sort();
    let mut out = Vec::new();
    for (i, (start, name)) in dirs.iter().enumerate() {
        let end = dirs
            .get(i + 1)
            .map_or(u64::MAX, |(n, _)| n.saturating_sub(1));
        let below = sel.to_height.is_some_and(|t| *start > t);
        let above = sel.from_height.is_some_and(|f| end < f);
        if !below && !above {
            out.push(name.clone());
        }
    }
    out
}

/// The leaf prefixes to list recursively for a dataset and selection.
async fn leaf_prefixes(
    store: &dyn ObjectStore,
    dataset: Dataset,
    sel: &Selection,
    reqs: &mut u64,
) -> Result<Vec<String>, ListError> {
    let root = dataset.prefix();
    let days_under = |base: String, dirs: Vec<String>| -> Vec<String> {
        dirs.into_iter()
            .filter(|d| Day::parse(d).is_ok_and(|day| d.len() == 8 && sel.day_ok(day)))
            .map(|d| format!("{base}/{d}"))
            .collect()
    };
    match dataset.layout() {
        Layout::Hourly | Layout::MarketData => {
            let base = if dataset.layout() == Layout::Hourly {
                format!("{root}/hourly")
            } else {
                root.to_owned()
            };
            match (sel.from, sel.to) {
                // Bounded range: list each day directly, no directory scan.
                (Some(f), Some(t)) => Ok(f
                    .through(t)
                    .iter()
                    .map(|d| format!("{base}/{}", d.compact()))
                    .collect()),
                _ => {
                    let dirs = list_dirs(store, &base, reqs).await?;
                    Ok(days_under(base, dirs))
                }
            }
        }
        Layout::ReplicaCmds => {
            let mut out = Vec::new();
            for top in list_dirs(store, root, reqs).await? {
                if top.len() == 8 && Day::parse(&top).is_ok() {
                    // replica_cmds/{date}/...
                    out.extend(days_under(root.to_owned(), vec![top]));
                } else {
                    let base = format!("{root}/{top}");
                    let dirs = list_dirs(store, &base, reqs).await?;
                    out.extend(days_under(base, dirs));
                }
            }
            Ok(out)
        }
        Layout::ExplorerBlocks => {
            let numeric = |dirs: Vec<String>| -> Vec<(u64, String)> {
                dirs.into_iter()
                    .filter_map(|d| numeric_dir(&d).map(|n| (n, d)))
                    .collect()
            };
            let mut out = Vec::new();
            let l1 = numeric(list_dirs(store, root, reqs).await?);
            for d1 in pick_ranges(l1, sel) {
                let base = format!("{root}/{d1}");
                let l2 = numeric(list_dirs(store, &base, reqs).await?);
                out.extend(
                    pick_ranges(l2, sel)
                        .into_iter()
                        .map(|d2| format!("{base}/{d2}")),
                );
            }
            Ok(out)
        }
    }
}

/// Lists one dataset of one bucket store, applying the selection.
///
/// # Errors
///
/// Fails if the store fails. Keys that do not match the layout are
/// returned in [`Listed::skipped`], not as errors.
pub async fn list_dataset(
    store: &dyn ObjectStore,
    dataset: Dataset,
    sel: &Selection,
) -> Result<Listed, ListError> {
    let mut listed = Listed::default();
    let bucket = dataset.bucket().name();
    for prefix in leaf_prefixes(store, dataset, sel, &mut listed.list_requests).await? {
        for meta in list_all(store, &prefix, &mut listed.list_requests).await? {
            let key = meta.location.to_string();
            if let Err(e) = check_key(bucket, &key) {
                listed.skipped.push((key, e.to_string()));
                continue;
            }
            match parse_key(dataset, &key) {
                Ok(info) if sel.keeps(&info) => listed.objects.push(SourceObject {
                    dataset,
                    bucket: bucket.to_owned(),
                    key,
                    size: meta.size,
                    etag: meta.e_tag,
                    info,
                }),
                Ok(_) => {}
                Err(e) => listed.skipped.push((key, e.reason.to_owned())),
            }
        }
    }
    listed.objects.sort_by(|a, b| {
        (a.info.day, a.info.hour, a.info.height, &a.key).cmp(&(
            b.info.day,
            b.info.hour,
            b.info.height,
            &b.key,
        ))
    });
    Ok(listed)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::testutil::{RecordingStore, put};

    /// A synthetic node-data bucket mimicking the documented layout,
    /// including excluded prefixes that must never be touched.
    pub(crate) async fn synthetic_node_bucket() -> RecordingStore {
        let s = RecordingStore::new();
        for key in [
            "node_fills_by_block/hourly/20250727/0.lz4",
            "node_fills_by_block/hourly/20250727/1.lz4",
            "node_fills_by_block/hourly/20250728/23.lz4",
            "node_fills_by_block/hourly/20250728/README",
            "node_fills/hourly/20250601/5.lz4",
            "node_trades/hourly/20250401/7.lz4",
            "misc_events_by_block/hourly/20250727/0.lz4",
            "replica_cmds/2025-04-06T14:33:08Z/20250411/559130000.lz4",
            "replica_cmds/2025-04-06T14:33:08Z/20250412/559230000.lz4",
            "replica_cmds/20250413/559330000.lz4",
            "explorer_blocks/500000000/559100000/559148100.rmp.lz4",
            "explorer_blocks/500000000/559200000/559200000.rmp.lz4",
            "explorer_blocks/600000000/600000000/600000000.rmp.lz4",
        ] {
            put(&s, key, format!("synthetic {key}").into_bytes()).await;
        }
        s.clear_log();
        s
    }

    pub(crate) async fn synthetic_archive_bucket() -> RecordingStore {
        let s = RecordingStore::new();
        for key in [
            "market_data/20230916/9/l2Book/SOL.lz4",
            "market_data/20230916/9/l2Book/BTC.lz4",
            "market_data/20230917/0/l2Book/SOL.lz4",
            "asset_ctxs/20230916.csv.lz4",
        ] {
            put(&s, key, format!("synthetic {key}").into_bytes()).await;
        }
        s.clear_log();
        s
    }

    fn keys(l: &Listed) -> Vec<&str> {
        l.objects.iter().map(|o| o.key.as_str()).collect()
    }

    fn assert_never_touched_denied(s: &RecordingStore) {
        for entry in s.log() {
            assert!(
                denied_prefix(&entry).is_none(),
                "touched excluded location: {entry}"
            );
        }
    }

    #[tokio::test]
    async fn lists_hourly_with_and_without_bounds() {
        let s = synthetic_node_bucket().await;
        let all = list_dataset(&s, Dataset::NodeFillsByBlock, &Selection::default())
            .await
            .unwrap();
        assert_eq!(
            keys(&all),
            [
                "node_fills_by_block/hourly/20250727/0.lz4",
                "node_fills_by_block/hourly/20250727/1.lz4",
                "node_fills_by_block/hourly/20250728/23.lz4",
            ]
        );
        assert_eq!(all.skipped.len(), 1);
        let sel = Selection {
            from: Some(Day::parse("20250728").unwrap()),
            to: Some(Day::parse("20250730").unwrap()),
            ..Selection::default()
        };
        let one = list_dataset(&s, Dataset::NodeFillsByBlock, &sel)
            .await
            .unwrap();
        assert_eq!(keys(&one), ["node_fills_by_block/hourly/20250728/23.lz4"]);
        assert_never_touched_denied(&s);
    }

    #[tokio::test]
    async fn lists_replica_cmds_in_both_layouts() {
        let s = synthetic_node_bucket().await;
        let sel = Selection {
            from: Some(Day::parse("20250412").unwrap()),
            ..Selection::default()
        };
        let l = list_dataset(&s, Dataset::ReplicaCmds, &sel).await.unwrap();
        assert_eq!(
            keys(&l),
            [
                "replica_cmds/2025-04-06T14:33:08Z/20250412/559230000.lz4",
                "replica_cmds/20250413/559330000.lz4",
            ]
        );
        assert_never_touched_denied(&s);
    }

    #[tokio::test]
    async fn lists_explorer_blocks_by_height_range() {
        let s = synthetic_node_bucket().await;
        let sel = Selection {
            from_height: Some(559_150_000),
            to_height: Some(599_999_999),
            ..Selection::default()
        };
        let l = list_dataset(&s, Dataset::ExplorerBlocks, &sel)
            .await
            .unwrap();
        assert_eq!(
            keys(&l),
            ["explorer_blocks/500000000/559200000/559200000.rmp.lz4"]
        );
        // The 600M directory was pruned without listing it.
        assert!(!s.log().iter().any(|e| e.contains("600000000")));
    }

    #[tokio::test]
    async fn market_data_filters_coins_and_never_lists_asset_ctxs() {
        let s = synthetic_archive_bucket().await;
        let sel = Selection {
            coins: vec!["SOL".into()],
            ..Selection::default()
        };
        let l = list_dataset(&s, Dataset::MarketData, &sel).await.unwrap();
        assert_eq!(
            keys(&l),
            [
                "market_data/20230916/9/l2Book/SOL.lz4",
                "market_data/20230917/0/l2Book/SOL.lz4"
            ]
        );
        assert_never_touched_denied(&s);
    }

    #[tokio::test]
    async fn shard_split_partitions_days() {
        let s = synthetic_node_bucket().await;
        let mut seen = Vec::new();
        for i in 0..3 {
            let sel = Selection {
                shard: Some((i, 3)),
                ..Selection::default()
            };
            let l = list_dataset(&s, Dataset::NodeFillsByBlock, &sel)
                .await
                .unwrap();
            seen.extend(l.objects.into_iter().map(|o| o.key));
        }
        seen.sort();
        assert_eq!(seen.len(), 3);
    }
}
