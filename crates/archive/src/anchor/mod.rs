//! Anchoring signed manifests on Bitcoin (OpenTimestamps) and an Ethereum
//! L2, and keeping `manifest.anchors.json` beside each manifest
//! (`docs/specs/manifest-v0.md` § 6).
//!
//! Anchors are created after signing and are not signed; each proves
//! itself. The anchors file and the `.ots` proof are the only mutable files
//! of a day: they are updated as anchors confirm.

pub mod evm;
pub mod ots;

use std::path::{Path, PathBuf};

use mesh_types::manifest::{
    ANCHORS_FORMAT, AnchorStatus, ManifestAnchors, OtsAnchor, SignedManifest,
};
use mesh_types::{Sha256Digest, UnixNanos};

use crate::error::{Error, IoContext, Result};
use crate::layout::{self, Day};
use crate::manifest;
use crate::partition::write_atomic;

fn local(root: &Path, day: Day, name: &str) -> PathBuf {
    root.join(layout::MANIFESTS_DIR)
        .join(day.to_string())
        .join(name)
}

fn load_manifest(root: &Path, day: Day) -> Result<SignedManifest> {
    manifest::read_signed(root, day)?
        .ok_or_else(|| Error::invalid(format!("no signed manifest for {day}")))
}

/// Reads a day's anchors, or an empty set for the manifest if none exist.
///
/// # Errors
///
/// Fails on I/O or parse errors, or anchors for another manifest.
pub fn read_anchors(root: &Path, day: Day) -> Result<ManifestAnchors> {
    let signed = load_manifest(root, day)?;
    let hash = signed.sha256()?;
    let p = local(root, day, layout::ANCHORS_FILE);
    if !p.exists() {
        return Ok(ManifestAnchors {
            format: ANCHORS_FORMAT.to_owned(),
            manifest_sha256: hash,
            opentimestamps: None,
            ethereum_l2: None,
        });
    }
    let a: ManifestAnchors = serde_json::from_slice(&std::fs::read(&p).at(&p)?)?;
    if a.manifest_sha256 != hash {
        return Err(Error::invalid(
            "manifest.anchors.json refers to a different manifest",
        ));
    }
    Ok(a)
}

/// Writes a day's anchors after checking them against the manifest.
///
/// # Errors
///
/// Fails on I/O errors or inconsistent anchors.
pub fn write_anchors(root: &Path, day: Day, anchors: &ManifestAnchors) -> Result<()> {
    let signed = load_manifest(root, day)?;
    anchors.check(&signed)?;
    let mut bytes = serde_json::to_vec_pretty(anchors)?;
    bytes.push(b'\n');
    write_atomic(&local(root, day, layout::ANCHORS_FILE), &bytes)
}

fn ots_anchor(proof: &ots::DetachedTimestamp) -> OtsAnchor {
    let bytes = proof.to_bytes();
    let height = proof.bitcoin().iter().map(|(h, _)| *h).min();
    OtsAnchor {
        proof_path: layout::OTS_FILE.to_owned(),
        proof_sha256: Sha256Digest::of(&bytes),
        status: if height.is_some() {
            AnchorStatus::Confirmed
        } else {
            AnchorStatus::Pending
        },
        bitcoin_block_height: height,
    }
}

/// Submits the manifest hash to the calendars and stores the pending proof
/// and anchor. Refuses to replace an existing proof.
///
/// # Errors
///
/// Fails if no manifest exists, a proof exists already, or too few
/// calendars answer.
pub async fn ots_stamp(root: &Path, day: Day, calendars: &ots::Calendars) -> Result<OtsAnchor> {
    let signed = load_manifest(root, day)?;
    let hash = signed.sha256()?;
    let proof_path = local(root, day, layout::OTS_FILE);
    if proof_path.exists() {
        return Err(Error::invalid(format!(
            "{} exists; use upgrade instead of stamping again",
            proof_path.display()
        )));
    }
    let proof = calendars.stamp(hash.0).await?;
    write_atomic(&proof_path, &proof.to_bytes())?;
    let anchor = ots_anchor(&proof);
    let mut anchors = read_anchors(root, day)?;
    anchors.opentimestamps = Some(anchor.clone());
    write_anchors(root, day, &anchors)?;
    Ok(anchor)
}

/// Upgrades the stored proof from the calendars. If `headers` is given,
/// the Bitcoin attestations are verified before the anchor is marked
/// confirmed.
///
/// # Errors
///
/// Fails if the proof is missing or malformed, or a Bitcoin attestation
/// does not verify.
pub async fn ots_upgrade(
    root: &Path,
    day: Day,
    calendars: &ots::Calendars,
    headers: Option<&dyn ots::BitcoinHeaders>,
) -> Result<OtsAnchor> {
    let signed = load_manifest(root, day)?;
    let hash = signed.sha256()?;
    let proof_path = local(root, day, layout::OTS_FILE);
    let mut proof = ots::DetachedTimestamp::parse(&std::fs::read(&proof_path).at(&proof_path)?)?;
    proof.check(&hash.0)?;
    let changed = calendars.upgrade(&mut proof).await?;
    if let Some(h) = headers {
        ots::verify_bitcoin(&proof, h).await?;
    }
    if changed {
        write_atomic(&proof_path, &proof.to_bytes())?;
    }
    let anchor = ots_anchor(&proof);
    let mut anchors = read_anchors(root, day)?;
    anchors.opentimestamps = Some(anchor.clone());
    write_anchors(root, day, &anchors)?;
    Ok(anchor)
}

/// Submits (or dry-runs) the L2 anchor transaction and stores its state in
/// `manifest.l2-tx.json`. Refuses if a broadcast transaction is already
/// pending.
///
/// # Errors
///
/// Fails if no manifest exists, a transaction is pending, or the submitter
/// fails.
pub async fn evm_submit(
    root: &Path,
    day: Day,
    submitter: &dyn evm::AnchorSubmitter,
    now: UnixNanos,
) -> Result<evm::PendingTx> {
    let signed = load_manifest(root, day)?;
    let hash = signed.sha256()?;
    let state = local(root, day, layout::L2_TX_FILE);
    if state.exists() {
        let existing: evm::PendingTx = serde_json::from_slice(&std::fs::read(&state).at(&state)?)?;
        if existing.broadcast {
            return Err(Error::invalid(format!(
                "a broadcast anchor transaction is pending ({}); poll it instead",
                existing.tx_hash
            )));
        }
    }
    let pending = submitter.submit(&hash.0, now).await?;
    let mut bytes = serde_json::to_vec_pretty(&pending)?;
    bytes.push(b'\n');
    write_atomic(&state, &bytes)?;
    Ok(pending)
}

/// Polls the pending L2 transaction and records the anchor once included.
/// Returns the anchor, or `None` while not yet included (or for a dry run).
///
/// # Errors
///
/// Fails if no transaction is pending or the transaction failed.
pub async fn evm_poll(
    root: &Path,
    day: Day,
    submitter: &dyn evm::AnchorSubmitter,
) -> Result<Option<mesh_types::manifest::EvmAnchor>> {
    let state = local(root, day, layout::L2_TX_FILE);
    let pending: evm::PendingTx = serde_json::from_slice(&std::fs::read(&state).at(&state)?)?;
    if !pending.broadcast {
        return Ok(None);
    }
    let Some(anchor) = submitter.poll(&pending).await? else {
        return Ok(None);
    };
    let mut anchors = read_anchors(root, day)?;
    anchors.ethereum_l2 = Some(anchor.clone());
    write_anchors(root, day, &anchors)?;
    Ok(Some(anchor))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::http::MockHttp;
    use crate::partition::{PartitionOptions, write_partitions};
    use crate::testutil::{every_kind, key};

    fn signed_day(root: &Path) -> (Day, SignedManifest) {
        write_partitions(root, every_kind(), &PartitionOptions::default()).unwrap();
        let k = key(1);
        let opts = manifest::BuildOptions {
            archive: "hyperliquid-normalized".into(),
            day: "2026-01-01".parse().unwrap(),
            web_seeds: vec!["https://m.example/".into()],
            trackers: vec![],
            created_ns: UnixNanos(1),
            signer: k.address(),
            next_signer: None,
            previous: None,
        };
        let built = manifest::build(root, &opts).unwrap();
        let s = manifest::sign(built.manifest, &k).unwrap();
        manifest::write_signed(root, &s, &built.torrent.metainfo).unwrap();
        (opts.day, s)
    }

    #[tokio::test]
    async fn stamp_then_upgrade_records_anchors() {
        let dir = tempfile::tempdir().unwrap();
        let (day, signed) = signed_day(dir.path());
        let hash = signed.sha256().unwrap();
        let http = MockHttp::new();
        let (resp, commit) =
            ots::tests::calendar_response(&hash.0, "https://alice.btc.calendar.opentimestamps.org");
        http.route(
            "POST",
            "https://a.pool.opentimestamps.org/digest",
            200,
            resp,
        );
        let cal = ots::Calendars::new(
            Arc::new(http.clone()),
            vec!["https://a.pool.opentimestamps.org".into()],
        );
        let a = ots_stamp(dir.path(), day, &cal).await.unwrap();
        assert_eq!(a.status, AnchorStatus::Pending);
        assert!(ots_stamp(dir.path(), day, &cal).await.is_err());
        let stored = read_anchors(dir.path(), day).unwrap();
        assert_eq!(stored.opentimestamps.as_ref(), Some(&a));

        let (up, attested) = ots::tests::upgrade_response(&commit, 900_001);
        http.route(
            "GET",
            &format!(
                "https://alice.btc.calendar.opentimestamps.org/timestamp/{}",
                crate::hex::encode(&commit)
            ),
            200,
            up,
        );
        let header = ots::tests::header_with_root(&attested);
        let bh = ots::block_hash_hex(&header);
        http.route(
            "GET",
            "https://btc.example/block-height/900001",
            200,
            bh.clone(),
        );
        http.route(
            "GET",
            &format!("https://btc.example/block/{bh}/header"),
            200,
            crate::hex::encode(&header),
        );
        let headers = ots::EsploraHeaders::new(Arc::new(http.clone()), "https://btc.example");
        let a = ots_upgrade(dir.path(), day, &cal, Some(&headers))
            .await
            .unwrap();
        assert_eq!(a.status, AnchorStatus::Confirmed);
        assert_eq!(a.bitcoin_block_height, Some(900_001));
        let proof =
            std::fs::read(dir.path().join("_manifests/2026-01-01/manifest.json.ots")).unwrap();
        assert_eq!(Sha256Digest::of(&proof), a.proof_sha256);
        // The proof's digest is the SHA-256 of manifest.json itself.
        let mj = std::fs::read(dir.path().join("_manifests/2026-01-01/manifest.json")).unwrap();
        assert_eq!(
            ots::DetachedTimestamp::parse(&proof).unwrap().digest(),
            Sha256Digest::of(&mj).0
        );
    }

    #[tokio::test]
    async fn evm_dry_run_and_rpc_poll() {
        let dir = tempfile::tempdir().unwrap();
        let (day, signed) = signed_day(dir.path());
        let hash = signed.sha256().unwrap();
        let dry = evm::DryRunSubmitter::new(key(2), evm::EvmConfig::new(10), 0);
        let p = evm_submit(dir.path(), day, &dry, UnixNanos(5))
            .await
            .unwrap();
        assert!(!p.broadcast);
        assert!(evm_poll(dir.path(), day, &dry).await.unwrap().is_none());
        assert!(read_anchors(dir.path(), day).unwrap().ethereum_l2.is_none());

        let http = MockHttp::new();
        let rpc = evm::EvmRpc::new(Arc::new(http.clone()), "https://rpc.example");
        evm::tests::mock_node(
            &http,
            "https://rpc.example",
            10,
            mesh_types::H256([0; 32]),
            vec![],
            0,
            50,
        );
        let mut cfg = evm::EvmConfig::new(10);
        cfg.confirmations = 3;
        let sub = evm::RpcSubmitter::new(key(2), cfg, rpc);
        let p = evm_submit(dir.path(), day, &sub, UnixNanos(6))
            .await
            .unwrap();
        assert!(
            evm_submit(dir.path(), day, &sub, UnixNanos(7))
                .await
                .is_err()
        );
        assert!(evm_poll(dir.path(), day, &sub).await.unwrap().is_none());
        evm::tests::mock_node(
            &http,
            "https://rpc.example",
            10,
            p.tx_hash,
            evm::anchor_calldata(&hash.0),
            48,
            50,
        );
        let a = evm_poll(dir.path(), day, &sub).await.unwrap().unwrap();
        assert_eq!(a.status, AnchorStatus::Confirmed);
        let stored = read_anchors(dir.path(), day).unwrap();
        assert_eq!(stored.ethereum_l2, Some(a));
        stored.check(&signed).unwrap();
    }
}
