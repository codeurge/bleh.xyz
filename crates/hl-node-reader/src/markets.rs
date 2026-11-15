//! Market auto-registration from node metadata (spec § 8).
//!
//! There is no hard-coded market list. The registry is built from the info
//! endpoint's metadata: core perpetuals (`meta`), every HIP-3 deployer
//! namespace (`perpDexs`, then `meta` with `dex`) and HIP-4 outcome markets
//! (`outcomeMeta`). It is rebuilt at start-up, whenever an unknown coin
//! appears, and periodically. Spot markets are outside ADM-0001 and are
//! not registered.
//!
//! Metadata responses are read leniently (unknown metadata fields are
//! ignored): they shape the registry, never record content.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use mesh_types::registry::RegistryError;
use mesh_types::{AdmissionId, Flag, Flags, InstrumentId, InstrumentSpec, MarketClass, Registry};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::assumptions::{
    CoinClass, OUTCOME_DEFAULT_SZ_DECIMALS, classify_coin, outcome_coins, outcome_price_scale,
};
use crate::http::{HttpError, InfoClient};
use crate::metrics::{Metrics, names};

/// The admission covering every Hyperliquid market.
pub const ADMISSION: &str = "ADM-0001";

/// One perpetual in a `meta` universe.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PerpAsset {
    /// Coin name (`BTC`, or `dex:COIN` for HIP-3).
    pub name: String,
    /// Size decimals.
    #[serde(rename = "szDecimals")]
    pub sz_decimals: u8,
    /// True if delisted (still registered: history refers to it).
    #[serde(default, rename = "isDelisted")]
    pub is_delisted: bool,
}

#[derive(Debug, Deserialize)]
struct MetaResponse {
    universe: Vec<PerpAsset>,
}

#[derive(Debug, Deserialize)]
struct PerpDex {
    name: String,
}

/// One HIP-4 outcome in `outcomeMeta` (assumed shape, assumption 9).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OutcomeMarket {
    /// Outcome number; its coins are `#{10 * outcome + side}`.
    pub outcome: u64,
    /// Size decimals, if given.
    #[serde(default, rename = "szDecimals")]
    pub sz_decimals: Option<u8>,
}

#[derive(Debug, Deserialize)]
struct OutcomeMetaResponse {
    outcomes: Vec<OutcomeMarket>,
}

/// A HIP-3 deployer and its perpetuals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DexMeta {
    /// Deployer namespace.
    pub name: String,
    /// Its perpetuals.
    pub perps: Vec<PerpAsset>,
}

/// Everything registration needs from the node.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeMetadata {
    /// Core perpetuals.
    pub perps: Vec<PerpAsset>,
    /// HIP-3 deployers.
    pub dexes: Vec<DexMeta>,
    /// HIP-4 outcomes.
    pub outcomes: Vec<OutcomeMarket>,
}

/// Errors from fetching metadata.
#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    /// The request failed.
    #[error(transparent)]
    Http(#[from] HttpError),
    /// The response has an unexpected shape.
    #[error("metadata {request}: {source}")]
    Shape {
        /// Request type.
        request: String,
        /// Error.
        source: serde_json::Error,
    },
}

fn shape<T: for<'de> Deserialize<'de>>(request: &str, v: Value) -> Result<T, MetadataError> {
    serde_json::from_value(v).map_err(|source| MetadataError::Shape {
        request: request.to_owned(),
        source,
    })
}

/// Parses a `meta` response.
///
/// # Errors
///
/// Fails if `universe` is missing or malformed.
pub fn parse_meta(v: Value) -> Result<Vec<PerpAsset>, MetadataError> {
    Ok(shape::<MetaResponse>("meta", v)?.universe)
}

/// Parses a `perpDexs` response (`[null, {"name": ...}, ...]`; `null` is
/// the core dex).
///
/// # Errors
///
/// Fails on another shape.
pub fn parse_perp_dexs(v: Value) -> Result<Vec<String>, MetadataError> {
    let dexes: Vec<Option<PerpDex>> = shape("perpDexs", v)?;
    Ok(dexes.into_iter().flatten().map(|d| d.name).collect())
}

/// Parses an `outcomeMeta` response. Assumption 9: `{"outcomes": [{"outcome":
/// n, "szDecimals"?: d}, ...]}`.
///
/// # Errors
///
/// Fails on another shape.
pub fn parse_outcome_meta(v: Value) -> Result<Vec<OutcomeMarket>, MetadataError> {
    Ok(shape::<OutcomeMetaResponse>("outcomeMeta", v)?.outcomes)
}

/// Fetches all metadata from an info endpoint. A failing `outcomeMeta`
/// request is tolerated (logged, no outcome markets) because its
/// availability on a local node is still to verify.
///
/// # Errors
///
/// Fails if `meta` or `perpDexs` fail.
pub fn fetch_metadata(client: &InfoClient) -> Result<NodeMetadata, MetadataError> {
    let perps = parse_meta(client.post_json(&json!({"type": "meta"}))?)?;
    let mut dexes = Vec::new();
    for name in parse_perp_dexs(client.post_json(&json!({"type": "perpDexs"}))?)? {
        let perps = parse_meta(client.post_json(&json!({"type": "meta", "dex": name}))?)?;
        dexes.push(DexMeta { name, perps });
    }
    let outcomes = match client
        .post_json(&json!({"type": "outcomeMeta"}))
        .map_err(MetadataError::from)
        .and_then(parse_outcome_meta)
    {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(error = %e, "outcomeMeta unavailable; no outcome markets registered");
            Vec::new()
        }
    };
    Ok(NodeMetadata {
        perps,
        dexes,
        outcomes,
    })
}

/// Default record flags for a market. HIP-3 deployer markets are
/// `oracle_anchored_venue` (the deployer's oracle drives the mark price and
/// funding); core perpetuals and outcome markets are not. Override per coin
/// with [`RegistrationPolicy::flag_overrides`].
#[must_use]
pub fn default_flags(coin: &str) -> Flags {
    match classify_coin(coin) {
        CoinClass::Hip3Perp => Flags::NONE.with(Flag::OracleAnchoredVenue),
        CoinClass::CorePerp | CoinClass::Outcome | CoinClass::Spot => Flags::NONE,
    }
}

/// How metadata becomes instrument specs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationPolicy {
    /// Admission for every spec.
    pub admission: AdmissionId,
    /// Per-coin flags replacing [`default_flags`].
    pub flag_overrides: BTreeMap<String, Flags>,
}

impl RegistrationPolicy {
    /// The default policy: ADM-0001, no overrides.
    ///
    /// # Errors
    ///
    /// Never in practice ([`ADMISSION`] is valid).
    pub fn new() -> Result<Self, mesh_types::IdError> {
        Ok(Self {
            admission: AdmissionId::new(ADMISSION)?,
            flag_overrides: BTreeMap::new(),
        })
    }

    fn flags(&self, coin: &str) -> Flags {
        self.flag_overrides
            .get(coin)
            .copied()
            .unwrap_or_else(|| default_flags(coin))
    }
}

/// The node coin of a HIP-3 asset: names already prefixed `dex:` are kept,
/// bare names get the prefix.
#[must_use]
pub fn hip3_coin(dex: &str, name: &str) -> String {
    if name.split_once(':').is_some() {
        name.to_owned()
    } else {
        format!("{dex}:{name}")
    }
}

/// Turns metadata into specs. Entries that cannot form a spec are reported
/// and skipped.
#[must_use]
pub fn specs_from_metadata(
    meta: &NodeMetadata,
    policy: &RegistrationPolicy,
) -> (Vec<InstrumentSpec>, Vec<String>) {
    let mut wanted: Vec<(MarketClass, String, u8, Option<u8>)> = Vec::new();
    let mut issues = Vec::new();
    for a in &meta.perps {
        if classify_coin(&a.name) == CoinClass::CorePerp {
            wanted.push((MarketClass::Perp, a.name.clone(), a.sz_decimals, None));
        } else {
            issues.push(format!("{}: not a core perpetual name", a.name));
        }
    }
    for d in &meta.dexes {
        for a in &d.perps {
            let coin = hip3_coin(&d.name, &a.name);
            if coin.split_once(':').map(|(ns, _)| ns) == Some(d.name.as_str()) {
                wanted.push((MarketClass::Perp, coin, a.sz_decimals, None));
            } else {
                issues.push(format!("{coin}: not in namespace {}", d.name));
            }
        }
    }
    for o in &meta.outcomes {
        let sz = o.sz_decimals.unwrap_or(OUTCOME_DEFAULT_SZ_DECIMALS);
        for coin in outcome_coins(o.outcome) {
            wanted.push((
                MarketClass::Outcome,
                coin,
                sz,
                Some(outcome_price_scale(sz)),
            ));
        }
    }
    let mut specs = Vec::new();
    for (class, coin, sz, ps) in wanted {
        match InstrumentSpec::hyperliquid(class, &coin, sz, ps, policy.admission.clone()) {
            Ok(s) => specs.push(s.with_flags(policy.flags(&coin))),
            Err(e) => issues.push(format!("{coin}: {e}")),
        }
    }
    (specs, issues)
}

/// Result of merging specs into a registry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistryUpdate {
    /// Newly registered instruments.
    pub added: Vec<InstrumentId>,
    /// Specs that differ from the registered one for the same coin (for
    /// example a changed `szDecimals`). The registered spec is kept: a scale
    /// change mid-stream needs an operator decision.
    pub conflicts: Vec<String>,
}

/// Adds every spec whose coin is not yet registered.
///
/// # Errors
///
/// Fails if two new specs collide with each other.
pub fn merge_specs(
    registry: &mut Registry,
    specs: Vec<InstrumentSpec>,
) -> Result<RegistryUpdate, RegistryError> {
    let mut update = RegistryUpdate::default();
    for spec in specs {
        match registry.by_venue_symbol(&spec.venue, &spec.venue_symbol) {
            Some(existing) if *existing == spec => {}
            Some(existing) => update.conflicts.push(format!(
                "{}: registered scales {}/{} flags {:?}, metadata says {}/{} flags {:?}",
                spec.venue_symbol,
                existing.price_scale,
                existing.size_scale,
                existing.flags,
                spec.price_scale,
                spec.size_scale,
                spec.flags
            )),
            None => {
                let id = spec.id.clone();
                registry.insert(spec)?;
                update.added.push(id);
            }
        }
    }
    Ok(update)
}

/// The registry shared by the readers, with optional refresh from node
/// metadata (at start-up, when an unknown coin appears, and periodically).
#[derive(Debug)]
pub struct RegistryHandle {
    registry: RwLock<Registry>,
    refresher: Option<Refresher>,
    metrics: Arc<Metrics>,
}

#[derive(Debug)]
struct Refresher {
    client: InfoClient,
    policy: RegistrationPolicy,
    out_path: Option<PathBuf>,
    min_interval: Duration,
    last: Mutex<Option<Instant>>,
}

impl RegistryHandle {
    /// A fixed registry (no refresh).
    #[must_use]
    pub fn fixed(registry: Registry, metrics: Arc<Metrics>) -> Self {
        metrics.set(names::REGISTRY_INSTRUMENTS, &[], registry.len() as f64);
        Self {
            registry: RwLock::new(registry),
            refresher: None,
            metrics,
        }
    }

    /// A registry refreshed from `client`. `out_path`, if set, receives the
    /// registry JSON after every change. Refreshes run at most once per
    /// `min_interval` unless forced.
    #[must_use]
    pub fn refreshing(
        initial: Registry,
        client: InfoClient,
        policy: RegistrationPolicy,
        out_path: Option<PathBuf>,
        min_interval: Duration,
        metrics: Arc<Metrics>,
    ) -> Self {
        metrics.set(names::REGISTRY_INSTRUMENTS, &[], initial.len() as f64);
        Self {
            registry: RwLock::new(initial),
            refresher: Some(Refresher {
                client,
                policy,
                out_path,
                min_interval,
                last: Mutex::new(None),
            }),
            metrics,
        }
    }

    /// A read guard on the current registry.
    pub fn read(&self) -> RwLockReadGuard<'_, Registry> {
        self.registry.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// A copy of the current registry.
    #[must_use]
    pub fn snapshot(&self) -> Registry {
        self.read().clone()
    }

    /// Refreshes from metadata (subject to the minimum interval unless
    /// `force`). Returns the update, or `None` if no refresh ran or it
    /// failed (failures are logged and counted).
    pub fn refresh(&self, force: bool) -> Option<RegistryUpdate> {
        let r = self.refresher.as_ref()?;
        {
            let mut last = r.last.lock().unwrap_or_else(PoisonError::into_inner);
            if !force && last.is_some_and(|t| t.elapsed() < r.min_interval) {
                return None;
            }
            *last = Some(Instant::now());
        }
        let meta = match fetch_metadata(&r.client) {
            Ok(m) => m,
            Err(e) => {
                self.metrics
                    .inc(names::REGISTRY_REFRESHES, &[("result", "error")]);
                tracing::warn!(error = %e, "registry refresh failed");
                return None;
            }
        };
        let (specs, issues) = specs_from_metadata(&meta, &r.policy);
        for i in &issues {
            tracing::warn!(issue = %i, "metadata entry not registered");
        }
        let mut reg = self
            .registry
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let update = match merge_specs(&mut reg, specs) {
            Ok(u) => u,
            Err(e) => {
                self.metrics
                    .inc(names::REGISTRY_REFRESHES, &[("result", "error")]);
                tracing::warn!(error = %e, "registry merge failed");
                return None;
            }
        };
        self.metrics
            .inc(names::REGISTRY_REFRESHES, &[("result", "ok")]);
        self.metrics
            .set(names::REGISTRY_INSTRUMENTS, &[], reg.len() as f64);
        for c in &update.conflicts {
            tracing::error!(conflict = %c, "metadata disagrees with the registry; keeping the registered spec");
        }
        if !update.added.is_empty() {
            tracing::info!(added = ?update.added, "registered new markets");
            if let Some(path) = &r.out_path {
                match serde_json::to_vec_pretty(&*reg) {
                    Ok(json) => {
                        if let Err(e) = std::fs::write(path, json) {
                            tracing::warn!(error = %e, path = %path.display(), "cannot write registry");
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "cannot serialize registry"),
                }
            }
        }
        Some(update)
    }

    /// Refreshes if any admitted coin in `coins` is not registered.
    pub fn ensure<'a, I: IntoIterator<Item = &'a str>>(&self, coins: I) {
        if self.refresher.is_none() {
            return;
        }
        let venue = crate::normalize::venue();
        let unknown = {
            let reg = self.read();
            coins.into_iter().any(|c| {
                crate::assumptions::is_admitted_coin(c) && reg.by_venue_symbol(&venue, c).is_none()
            })
        };
        if unknown {
            self.refresh(false);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::http::tests::serve;

    fn ok(body: &Value) -> String {
        let b = body.to_string();
        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{b}", b.len())
    }

    fn fixture_registry() -> Registry {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/synthetic/hl/markets.json"
        );
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn metadata_reproduces_the_fixture_registry() {
        // Synthetic metadata for the fixture markets, in the info shapes.
        let (url, rx) = serve(vec![
            ok(&json!({"universe": [{"name": "SYN", "szDecimals": 3, "maxLeverage": 10}]})),
            ok(&json!([null, {"name": "syn", "fullName": "synthetic", "deployer": "0x00"}])),
            ok(&json!({"universe": [{"name": "syn:IDX", "szDecimals": 2, "maxLeverage": 5}]})),
            ok(&json!({"outcomes": [{"outcome": 900, "name": "synthetic outcome"}]})),
        ]);
        let client = InfoClient::new(&url, Duration::from_secs(5)).unwrap();
        let meta = fetch_metadata(&client).unwrap();
        let requests: Vec<String> = rx.try_iter().collect();
        assert_eq!(
            serde_json::from_str::<Value>(&requests[2]).unwrap(),
            json!({"type": "meta", "dex": "syn"})
        );
        let (specs, issues) = specs_from_metadata(&meta, &RegistrationPolicy::new().unwrap());
        assert!(issues.is_empty(), "{issues:?}");
        let mut reg = Registry::new();
        let up = merge_specs(&mut reg, specs).unwrap();
        assert_eq!(up.added.len(), 4);
        let fixture = fixture_registry();
        for spec in fixture.iter() {
            assert_eq!(reg.get(&spec.id), Some(spec), "{}", spec.id);
        }
        // Both sides of the outcome are registered.
        assert!(
            reg.get(&InstrumentId::new("hyperliquid:outcome:#9000").unwrap())
                .is_some()
        );
    }

    #[test]
    fn merge_reports_conflicts_and_keeps_existing() {
        let mut reg = fixture_registry();
        let policy = RegistrationPolicy::new().unwrap();
        let meta = NodeMetadata {
            perps: vec![
                PerpAsset {
                    name: "SYN".into(),
                    sz_decimals: 2,
                    is_delisted: false,
                },
                PerpAsset {
                    name: "NEW".into(),
                    sz_decimals: 1,
                    is_delisted: false,
                },
                PerpAsset {
                    name: "@3".into(),
                    sz_decimals: 1,
                    is_delisted: false,
                },
            ],
            dexes: vec![DexMeta {
                name: "abc".into(),
                perps: vec![PerpAsset {
                    name: "X".into(),
                    sz_decimals: 9,
                    is_delisted: false,
                }],
            }],
            outcomes: vec![],
        };
        let (specs, issues) = specs_from_metadata(&meta, &policy);
        assert_eq!(issues.len(), 2, "{issues:?}");
        let up = merge_specs(&mut reg, specs).unwrap();
        assert_eq!(
            up.added,
            vec![InstrumentId::new("hyperliquid:perp:NEW").unwrap()]
        );
        assert_eq!(up.conflicts.len(), 1);
        assert_eq!(
            reg.get(&InstrumentId::new("hyperliquid:perp:SYN").unwrap())
                .unwrap()
                .size_scale,
            3
        );
        assert_eq!(hip3_coin("abc", "X"), "abc:X");
        assert_eq!(hip3_coin("abc", "abc:X"), "abc:X");
        assert!(default_flags("abc:X").contains(Flag::OracleAnchoredVenue));
        assert!(default_flags("NEW").is_empty());
    }
}
