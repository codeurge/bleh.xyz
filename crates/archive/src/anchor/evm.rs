//! Ethereum L2 anchors: the anchor transaction payload, signing, a
//! submitter behind a trait (with a dry-run implementation), and
//! verification of an anchor against the chain over JSON-RPC.
//!
//! The chain and the anchoring contract are not yet chosen
//! (`docs/specs/manifest-v0.md` § 6.2). Until they are, the payload is the
//! call `anchor(bytes32 manifest_hash)` (selector `keccak256("anchor(bytes32)")[..4]`
//! followed by the hash), sent to a configured contract or, by default, to
//! the signer's own address so the hash is carried as calldata. A verifier
//! accepts either: the transaction input must be exactly that call.

use std::sync::Arc;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use mesh_types::manifest::{AnchorStatus, EvmAnchor};
use mesh_types::{EthAddress, H256, SigningKey, UnixNanos};

use crate::error::{Error, Result};
use crate::hex;
use crate::http::HttpClient;

fn bad(msg: impl Into<String>) -> Error {
    Error::Evm(msg.into())
}

/// RLP item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rlp {
    /// Byte string.
    Bytes(Vec<u8>),
    /// List.
    List(Vec<Rlp>),
}

impl Rlp {
    /// An unsigned integer (big-endian, no leading zeros; zero is empty).
    #[must_use]
    pub fn uint(v: u128) -> Self {
        let b = v.to_be_bytes();
        let first = b.iter().position(|x| *x != 0).unwrap_or(b.len());
        Self::Bytes(b[first..].to_vec())
    }

    /// A big-endian unsigned integer given as bytes (leading zeros dropped).
    #[must_use]
    pub fn uint_bytes(b: &[u8]) -> Self {
        let first = b.iter().position(|x| *x != 0).unwrap_or(b.len());
        Self::Bytes(b[first..].to_vec())
    }

    /// Encodes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        fn len_prefix(out: &mut Vec<u8>, len: usize, short: u8, long: u8) {
            if len <= 55 {
                out.push(short + len as u8);
            } else {
                let lb = (len as u64).to_be_bytes();
                let first = lb.iter().position(|x| *x != 0).unwrap_or(7);
                out.push(long + (8 - first) as u8);
                out.extend_from_slice(&lb[first..]);
            }
        }
        let mut out = Vec::new();
        match self {
            Rlp::Bytes(b) if b.len() == 1 && b[0] < 0x80 => out.push(b[0]),
            Rlp::Bytes(b) => {
                len_prefix(&mut out, b.len(), 0x80, 0xb7);
                out.extend_from_slice(b);
            }
            Rlp::List(items) => {
                let payload: Vec<u8> = items.iter().flat_map(Rlp::encode).collect();
                len_prefix(&mut out, payload.len(), 0xc0, 0xf7);
                out.extend_from_slice(&payload);
            }
        }
        out
    }
}

/// The 4-byte selector of `anchor(bytes32)`.
#[must_use]
pub fn anchor_selector() -> [u8; 4] {
    let h = H256::keccak(&[b"anchor(bytes32)"]);
    [h.0[0], h.0[1], h.0[2], h.0[3]]
}

/// Calldata committing to `hash`.
#[must_use]
pub fn anchor_calldata(hash: &[u8; 32]) -> Vec<u8> {
    [anchor_selector().as_slice(), hash].concat()
}

/// Extracts the committed hash from anchor calldata.
#[must_use]
pub fn committed_hash(calldata: &[u8]) -> Option<[u8; 32]> {
    let rest = calldata.strip_prefix(anchor_selector().as_slice())?;
    rest.try_into().ok()
}

/// Transaction envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxType {
    /// EIP-1559 (type 2).
    Eip1559,
    /// Legacy with EIP-155 replay protection, for chains without EIP-1559.
    Legacy,
}

/// An unsigned anchor transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsignedTx {
    /// Envelope.
    pub tx_type: TxType,
    /// EIP-155 chain ID.
    pub chain_id: u64,
    /// Sender nonce.
    pub nonce: u64,
    /// Max priority fee per gas (EIP-1559) in wei.
    pub max_priority_fee_per_gas: u128,
    /// Max fee per gas (EIP-1559), or the gas price (legacy), in wei.
    pub max_fee_per_gas: u128,
    /// Gas limit.
    pub gas_limit: u64,
    /// Recipient.
    pub to: EthAddress,
    /// Value in wei (zero for anchors).
    pub value: u128,
    /// Calldata.
    pub data: Vec<u8>,
}

/// A signed transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedTx {
    /// Raw bytes for `eth_sendRawTransaction`.
    pub raw: Vec<u8>,
    /// Transaction hash (`keccak256(raw)`).
    pub hash: H256,
    /// Sender.
    pub from: EthAddress,
}

impl UnsignedTx {
    fn fields(&self) -> Vec<Rlp> {
        match self.tx_type {
            TxType::Eip1559 => vec![
                Rlp::uint(u128::from(self.chain_id)),
                Rlp::uint(u128::from(self.nonce)),
                Rlp::uint(self.max_priority_fee_per_gas),
                Rlp::uint(self.max_fee_per_gas),
                Rlp::uint(u128::from(self.gas_limit)),
                Rlp::Bytes(self.to.0.to_vec()),
                Rlp::uint(self.value),
                Rlp::Bytes(self.data.clone()),
                Rlp::List(vec![]),
            ],
            TxType::Legacy => vec![
                Rlp::uint(u128::from(self.nonce)),
                Rlp::uint(self.max_fee_per_gas),
                Rlp::uint(u128::from(self.gas_limit)),
                Rlp::Bytes(self.to.0.to_vec()),
                Rlp::uint(self.value),
                Rlp::Bytes(self.data.clone()),
            ],
        }
    }

    /// The hash that is signed.
    #[must_use]
    pub fn signing_hash(&self) -> H256 {
        match self.tx_type {
            TxType::Eip1559 => {
                let body = Rlp::List(self.fields()).encode();
                H256::keccak(&[&[0x02], &body])
            }
            TxType::Legacy => {
                let mut f = self.fields();
                f.extend([
                    Rlp::uint(u128::from(self.chain_id)),
                    Rlp::uint(0),
                    Rlp::uint(0),
                ]);
                H256::keccak(&[&Rlp::List(f).encode()])
            }
        }
    }

    /// Signs with `key`.
    #[must_use]
    pub fn sign(&self, key: &SigningKey) -> SignedTx {
        let sig = key.sign_prehash(&self.signing_hash());
        let recid = u128::from(sig.0[64] - 27);
        let (r, s) = (&sig.0[..32], &sig.0[32..64]);
        let mut f = self.fields();
        let raw = match self.tx_type {
            TxType::Eip1559 => {
                f.extend([Rlp::uint(recid), Rlp::uint_bytes(r), Rlp::uint_bytes(s)]);
                [vec![0x02], Rlp::List(f).encode()].concat()
            }
            TxType::Legacy => {
                let v = u128::from(self.chain_id) * 2 + 35 + recid;
                f.extend([Rlp::uint(v), Rlp::uint_bytes(r), Rlp::uint_bytes(s)]);
                Rlp::List(f).encode()
            }
        };
        SignedTx {
            hash: H256::keccak(&[&raw]),
            raw,
            from: key.address(),
        }
    }
}

/// Anchor settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmConfig {
    /// EIP-155 chain ID.
    pub chain_id: u64,
    /// JSON-RPC endpoint (required for submitting and verifying).
    pub rpc_url: Option<String>,
    /// Anchoring contract; `None` sends to the signer's own address.
    pub to: Option<EthAddress>,
    /// Envelope.
    pub tx_type: TxType,
    /// Gas limit; `None` estimates over RPC (dry runs use 60 000).
    pub gas_limit: Option<u64>,
    /// Max fee per gas override, in wei.
    pub max_fee_per_gas: Option<u128>,
    /// Priority fee override, in wei.
    pub max_priority_fee_per_gas: Option<u128>,
    /// Confirmations after which an anchor is `confirmed`.
    pub confirmations: u64,
}

impl EvmConfig {
    /// Defaults for `chain_id`: EIP-1559, self-send, 12 confirmations.
    #[must_use]
    pub fn new(chain_id: u64) -> Self {
        Self {
            chain_id,
            rpc_url: None,
            to: None,
            tx_type: TxType::Eip1559,
            gas_limit: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            confirmations: 12,
        }
    }
}

/// State of a submitted (or dry-run) anchor transaction, kept beside the
/// manifest as `manifest.l2-tx.json` until it is recorded in the anchors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingTx {
    /// Chain ID.
    pub chain_id: u64,
    /// Transaction hash.
    pub tx_hash: H256,
    /// Signed raw transaction, hex.
    pub raw_tx: String,
    /// Committed manifest hash.
    pub committed_hash: H256,
    /// Sender.
    pub from: EthAddress,
    /// Whether it was broadcast (false for dry runs).
    pub broadcast: bool,
    /// When it was created.
    pub created_ns: UnixNanos,
}

/// Signs and submits anchor transactions.
pub trait AnchorSubmitter: Send + Sync + std::fmt::Debug {
    /// Builds, signs and (unless a dry run) broadcasts a transaction
    /// committing to `hash`.
    fn submit<'a>(&'a self, hash: &'a [u8; 32], now: UnixNanos)
    -> BoxFuture<'a, Result<PendingTx>>;
    /// Checks a submitted transaction; returns the anchor record once it is
    /// included in a block (`pending` until enough confirmations).
    fn poll<'a>(&'a self, tx: &'a PendingTx) -> BoxFuture<'a, Result<Option<EvmAnchor>>>;
}

/// Signs locally and never touches the network. `poll` always returns
/// `None`.
#[derive(Debug)]
pub struct DryRunSubmitter {
    key: SigningKey,
    cfg: EvmConfig,
    nonce: u64,
}

impl DryRunSubmitter {
    /// A dry-run submitter signing with `key` at `nonce`.
    #[must_use]
    pub fn new(key: SigningKey, cfg: EvmConfig, nonce: u64) -> Self {
        Self { key, cfg, nonce }
    }
}

fn build_tx(
    cfg: &EvmConfig,
    from: EthAddress,
    nonce: u64,
    gas: u64,
    max_fee: u128,
    priority: u128,
    hash: &[u8; 32],
) -> UnsignedTx {
    UnsignedTx {
        tx_type: cfg.tx_type,
        chain_id: cfg.chain_id,
        nonce,
        max_priority_fee_per_gas: priority,
        max_fee_per_gas: max_fee,
        gas_limit: gas,
        to: cfg.to.unwrap_or(from),
        value: 0,
        data: anchor_calldata(hash),
    }
}

impl AnchorSubmitter for DryRunSubmitter {
    fn submit<'a>(
        &'a self,
        hash: &'a [u8; 32],
        now: UnixNanos,
    ) -> BoxFuture<'a, Result<PendingTx>> {
        Box::pin(async move {
            let tx = build_tx(
                &self.cfg,
                self.key.address(),
                self.nonce,
                self.cfg.gas_limit.unwrap_or(60_000),
                self.cfg.max_fee_per_gas.unwrap_or(1_000_000_000),
                self.cfg.max_priority_fee_per_gas.unwrap_or(1_000_000),
                hash,
            );
            let signed = tx.sign(&self.key);
            Ok(PendingTx {
                chain_id: self.cfg.chain_id,
                tx_hash: signed.hash,
                raw_tx: format!("0x{}", hex::encode(&signed.raw)),
                committed_hash: H256(*hash),
                from: signed.from,
                broadcast: false,
                created_ns: now,
            })
        })
    }

    fn poll<'a>(&'a self, _tx: &'a PendingTx) -> BoxFuture<'a, Result<Option<EvmAnchor>>> {
        Box::pin(async { Ok(None) })
    }
}

/// Minimal Ethereum JSON-RPC client.
#[derive(Debug, Clone)]
pub struct EvmRpc {
    http: Arc<dyn HttpClient>,
    url: String,
}

/// A transaction as returned by `eth_getTransactionByHash`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxInfo {
    /// Input data.
    pub input: Vec<u8>,
    /// Block number, if included.
    pub block_number: Option<u64>,
    /// Chain ID, if reported.
    pub chain_id: Option<u64>,
}

fn quantity(v: &Json) -> Result<u128> {
    let s = v
        .as_str()
        .ok_or_else(|| bad(format!("expected a hex quantity, got {v}")))?;
    let digits = s
        .strip_prefix("0x")
        .ok_or_else(|| bad("quantity without 0x"))?;
    u128::from_str_radix(if digits.is_empty() { "0" } else { digits }, 16)
        .map_err(|_| bad(format!("bad quantity {s}")))
}

fn quantity_u64(v: &Json) -> Result<u64> {
    u64::try_from(quantity(v)?).map_err(|_| bad("quantity out of range"))
}

impl EvmRpc {
    /// A client for `url`.
    pub fn new(http: Arc<dyn HttpClient>, url: impl Into<String>) -> Self {
        Self {
            http,
            url: url.into(),
        }
    }

    async fn call(&self, method: &str, params: Json) -> Result<Json> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let resp = self
            .http
            .post(
                &self.url,
                &[("Content-Type", "application/json")],
                serde_json::to_vec(&body)?.into(),
            )
            .await?
            .into_ok(method)?;
        let v: Json = serde_json::from_slice(&resp)?;
        if let Some(e) = v.get("error") {
            return Err(bad(format!("{method}: {e}")));
        }
        v.get("result")
            .cloned()
            .ok_or_else(|| bad(format!("{method}: no result")))
    }

    /// `eth_chainId`.
    ///
    /// # Errors
    ///
    /// Fails on transport or RPC errors.
    pub async fn chain_id(&self) -> Result<u64> {
        quantity_u64(&self.call("eth_chainId", json!([])).await?)
    }

    /// `eth_blockNumber`.
    ///
    /// # Errors
    ///
    /// Fails on transport or RPC errors.
    pub async fn block_number(&self) -> Result<u64> {
        quantity_u64(&self.call("eth_blockNumber", json!([])).await?)
    }

    /// Pending nonce of `addr`.
    ///
    /// # Errors
    ///
    /// Fails on transport or RPC errors.
    pub async fn nonce(&self, addr: &EthAddress) -> Result<u64> {
        quantity_u64(
            &self
                .call(
                    "eth_getTransactionCount",
                    json!([addr.to_string(), "pending"]),
                )
                .await?,
        )
    }

    /// `(base fee of the latest block, suggested priority fee)`.
    ///
    /// # Errors
    ///
    /// Fails on transport or RPC errors.
    pub async fn fees(&self) -> Result<(u128, u128)> {
        let block = self
            .call("eth_getBlockByNumber", json!(["latest", false]))
            .await?;
        let base = block
            .get("baseFeePerGas")
            .map(quantity)
            .transpose()?
            .unwrap_or(0);
        let tip = quantity(&self.call("eth_maxPriorityFeePerGas", json!([])).await?)?;
        Ok((base, tip))
    }

    /// `eth_gasPrice`.
    ///
    /// # Errors
    ///
    /// Fails on transport or RPC errors.
    pub async fn gas_price(&self) -> Result<u128> {
        quantity(&self.call("eth_gasPrice", json!([])).await?)
    }

    /// `eth_estimateGas` for a call.
    ///
    /// # Errors
    ///
    /// Fails on transport or RPC errors.
    pub async fn estimate_gas(
        &self,
        from: &EthAddress,
        to: &EthAddress,
        data: &[u8],
    ) -> Result<u64> {
        quantity_u64(
            &self
                .call(
                    "eth_estimateGas",
                    json!([{"from": from.to_string(), "to": to.to_string(), "data": format!("0x{}", hex::encode(data))}]),
                )
                .await?,
        )
    }

    /// `eth_sendRawTransaction`.
    ///
    /// # Errors
    ///
    /// Fails on transport or RPC errors.
    pub async fn send_raw(&self, raw: &[u8]) -> Result<H256> {
        let v = self
            .call(
                "eth_sendRawTransaction",
                json!([format!("0x{}", hex::encode(raw))]),
            )
            .await?;
        v.as_str()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| bad("eth_sendRawTransaction returned no hash"))
    }

    /// `eth_getTransactionReceipt`: `(block number, success)`.
    ///
    /// # Errors
    ///
    /// Fails on transport or RPC errors.
    pub async fn receipt(&self, tx: &H256) -> Result<Option<(u64, bool)>> {
        let v = self
            .call("eth_getTransactionReceipt", json!([tx.to_string()]))
            .await?;
        if v.is_null() {
            return Ok(None);
        }
        let block = quantity_u64(
            v.get("blockNumber")
                .ok_or_else(|| bad("receipt without blockNumber"))?,
        )?;
        let ok = v.get("status").map(quantity).transpose()?.unwrap_or(0) == 1;
        Ok(Some((block, ok)))
    }

    /// `eth_getTransactionByHash`.
    ///
    /// # Errors
    ///
    /// Fails on transport or RPC errors.
    pub async fn transaction(&self, tx: &H256) -> Result<Option<TxInfo>> {
        let v = self
            .call("eth_getTransactionByHash", json!([tx.to_string()]))
            .await?;
        if v.is_null() {
            return Ok(None);
        }
        let input = v
            .get("input")
            .and_then(Json::as_str)
            .and_then(hex::decode)
            .ok_or_else(|| bad("transaction without input"))?;
        let block_number = match v.get("blockNumber") {
            None | Some(Json::Null) => None,
            Some(b) => Some(quantity_u64(b)?),
        };
        let chain_id = match v.get("chainId") {
            None | Some(Json::Null) => None,
            Some(c) => Some(quantity_u64(c)?),
        };
        Ok(Some(TxInfo {
            input,
            block_number,
            chain_id,
        }))
    }
}

/// Signs locally and submits over JSON-RPC.
#[derive(Debug)]
pub struct RpcSubmitter {
    key: SigningKey,
    cfg: EvmConfig,
    rpc: EvmRpc,
}

impl RpcSubmitter {
    /// A submitter using `rpc`.
    #[must_use]
    pub fn new(key: SigningKey, cfg: EvmConfig, rpc: EvmRpc) -> Self {
        Self { key, cfg, rpc }
    }
}

impl AnchorSubmitter for RpcSubmitter {
    fn submit<'a>(
        &'a self,
        hash: &'a [u8; 32],
        now: UnixNanos,
    ) -> BoxFuture<'a, Result<PendingTx>> {
        Box::pin(async move {
            let chain = self.rpc.chain_id().await?;
            if chain != self.cfg.chain_id {
                return Err(bad(format!(
                    "RPC is on chain {chain}, configured {}",
                    self.cfg.chain_id
                )));
            }
            let from = self.key.address();
            let to = self.cfg.to.unwrap_or(from);
            let nonce = self.rpc.nonce(&from).await?;
            let gas = match self.cfg.gas_limit {
                Some(g) => g,
                None => {
                    let est = self
                        .rpc
                        .estimate_gas(&from, &to, &anchor_calldata(hash))
                        .await?;
                    est + est / 5
                }
            };
            let (max_fee, priority) = match self.cfg.tx_type {
                TxType::Eip1559 => {
                    let (base, tip) = self.rpc.fees().await?;
                    let priority = self.cfg.max_priority_fee_per_gas.unwrap_or(tip);
                    (
                        self.cfg.max_fee_per_gas.unwrap_or(base * 2 + priority),
                        priority,
                    )
                }
                TxType::Legacy => (
                    self.cfg
                        .max_fee_per_gas
                        .unwrap_or(self.rpc.gas_price().await?),
                    0,
                ),
            };
            let signed =
                build_tx(&self.cfg, from, nonce, gas, max_fee, priority, hash).sign(&self.key);
            let sent = self.rpc.send_raw(&signed.raw).await?;
            if sent != signed.hash {
                return Err(bad("node returned a different transaction hash"));
            }
            crate::metrics::inc("archive_evm_anchor_submitted_total", &[], 1);
            Ok(PendingTx {
                chain_id: self.cfg.chain_id,
                tx_hash: signed.hash,
                raw_tx: format!("0x{}", hex::encode(&signed.raw)),
                committed_hash: H256(*hash),
                from,
                broadcast: true,
                created_ns: now,
            })
        })
    }

    fn poll<'a>(&'a self, tx: &'a PendingTx) -> BoxFuture<'a, Result<Option<EvmAnchor>>> {
        Box::pin(async move {
            let Some((block, ok)) = self.rpc.receipt(&tx.tx_hash).await? else {
                return Ok(None);
            };
            if !ok {
                return Err(bad(format!("anchor transaction {} reverted", tx.tx_hash)));
            }
            let head = self.rpc.block_number().await?;
            let confirmed = head.saturating_sub(block) + 1 >= self.cfg.confirmations;
            Ok(Some(EvmAnchor {
                chain_id: tx.chain_id,
                tx_hash: tx.tx_hash,
                block_number: block,
                committed_hash: tx.committed_hash,
                status: if confirmed {
                    AnchorStatus::Confirmed
                } else {
                    AnchorStatus::Pending
                },
            }))
        })
    }
}

/// Verifies an anchor record against the chain: the transaction exists in
/// the recorded block on the recorded chain, succeeded, and its input is
/// exactly the anchor call for `manifest_hash`. Returns the status the
/// chain supports now.
///
/// # Errors
///
/// Fails if anything does not match.
pub async fn verify_anchor(
    rpc: &EvmRpc,
    anchor: &EvmAnchor,
    manifest_hash: &[u8; 32],
    confirmations: u64,
) -> Result<AnchorStatus> {
    if anchor.committed_hash.0 != *manifest_hash {
        return Err(bad("anchor commits to a different hash"));
    }
    let chain = rpc.chain_id().await?;
    if chain != anchor.chain_id {
        return Err(bad(format!(
            "RPC is on chain {chain}, anchor says {}",
            anchor.chain_id
        )));
    }
    let tx = rpc
        .transaction(&anchor.tx_hash)
        .await?
        .ok_or_else(|| bad(format!("transaction {} not found", anchor.tx_hash)))?;
    if committed_hash(&tx.input) != Some(*manifest_hash) {
        return Err(bad(
            "transaction input is not the anchor call for this manifest",
        ));
    }
    if tx.chain_id.is_some_and(|c| c != anchor.chain_id) {
        return Err(bad("transaction chain ID differs"));
    }
    let (block, ok) = rpc
        .receipt(&anchor.tx_hash)
        .await?
        .ok_or_else(|| bad("transaction has no receipt"))?;
    if !ok {
        return Err(bad("transaction reverted"));
    }
    if block != anchor.block_number || tx.block_number.is_some_and(|b| b != block) {
        return Err(bad(format!(
            "transaction is in block {block}, anchor says {}",
            anchor.block_number
        )));
    }
    let head = rpc.block_number().await?;
    Ok(if head.saturating_sub(block) + 1 >= confirmations {
        AnchorStatus::Confirmed
    } else {
        AnchorStatus::Pending
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::http::{MockHttp, Response};

    #[test]
    fn rlp_vectors() {
        // From the Ethereum RLP specification.
        assert_eq!(
            Rlp::Bytes(b"dog".to_vec()).encode(),
            hex::decode("83646f67").unwrap()
        );
        assert_eq!(
            Rlp::List(vec![
                Rlp::Bytes(b"cat".to_vec()),
                Rlp::Bytes(b"dog".to_vec())
            ])
            .encode(),
            hex::decode("c88363617483646f67").unwrap()
        );
        assert_eq!(Rlp::Bytes(vec![]).encode(), vec![0x80]);
        assert_eq!(Rlp::List(vec![]).encode(), vec![0xc0]);
        assert_eq!(Rlp::uint(0).encode(), vec![0x80]);
        assert_eq!(Rlp::uint(15).encode(), vec![0x0f]);
        assert_eq!(Rlp::uint(1024).encode(), vec![0x82, 0x04, 0x00]);
        let lorem = b"Lorem ipsum dolor sit amet, consectetur adipisicing elit".to_vec();
        let mut want = vec![0xb8, 0x38];
        want.extend_from_slice(&lorem);
        assert_eq!(Rlp::Bytes(lorem).encode(), want);
        // [ [], [[]], [ [], [[]] ] ]
        let e = || Rlp::List(vec![]);
        let set = Rlp::List(vec![
            e(),
            Rlp::List(vec![e()]),
            Rlp::List(vec![e(), Rlp::List(vec![e()])]),
        ]);
        assert_eq!(set.encode(), hex::decode("c7c0c1c0c3c0c1c0").unwrap());
    }

    #[test]
    fn eip155_example_transaction() {
        // The worked example in EIP-155.
        let key = SigningKey::from_bytes(&[0x46; 32]).unwrap();
        let tx = UnsignedTx {
            tx_type: TxType::Legacy,
            chain_id: 1,
            nonce: 9,
            max_priority_fee_per_gas: 0,
            max_fee_per_gas: 20_000_000_000,
            gas_limit: 21_000,
            to: EthAddress([0x35; 20]),
            value: 1_000_000_000_000_000_000,
            data: vec![],
        };
        assert_eq!(
            tx.signing_hash().to_string(),
            "0xdaf5a779ae972f972197303d7b574746c7ef83eadac0f2791ad23db92e4c8e53"
        );
        let signed = tx.sign(&key);
        assert_eq!(
            hex::encode(&signed.raw),
            concat!(
                "f86c098504a817c800825208943535353535353535353535353535353535353535880de0b6b3a7640000",
                "8025a028ef61340bd939bc2195fe537567866003e1a15d3c71ff63e1590620aa636276a067cbe9d8997f76",
                "1aecb703304b3800ccf555c9f3dc64214b297fb1966a3b6d83"
            )
        );
    }

    #[test]
    fn eip1559_anchor_transaction_recovers_signer() {
        let key = SigningKey::from_bytes(&[7; 32]).unwrap();
        let hash = [0x5a; 32];
        let tx = build_tx(
            &EvmConfig::new(8453),
            key.address(),
            3,
            30_000,
            2_000_000_000,
            1_000_000,
            &hash,
        );
        assert_eq!(tx.to, key.address());
        assert_eq!(committed_hash(&tx.data), Some(hash));
        assert_eq!(tx.data.len(), 36);
        let signed = tx.sign(&key);
        assert_eq!(signed.raw[0], 0x02);
        assert_eq!(signed.hash, H256::keccak(&[&signed.raw]));
        // Decode r, s, y_parity from the tail and recover the sender.
        let tail = &signed.raw[signed.raw.len() - 67..];
        assert_eq!(tail[1], 0xa0);
        let mut sig = [0u8; 65];
        sig[..32].copy_from_slice(&tail[2..34]);
        assert_eq!(tail[34], 0xa0);
        sig[32..64].copy_from_slice(&tail[35..67]);
        let parity = tail[0];
        assert!(parity == 0x80 || parity == 0x01);
        sig[64] = 27 + u8::from(parity == 0x01);
        let recovered = mesh_types::EthSignature(sig)
            .recover_prehash(&tx.signing_hash())
            .unwrap();
        assert_eq!(recovered, key.address());
        assert_eq!(
            hex::encode(&anchor_selector()),
            hex::encode(&H256::keccak(&[b"anchor(bytes32)"]).0[..4])
        );
    }

    /// A mock JSON-RPC node holding one anchor transaction.
    pub(crate) fn mock_node(
        http: &MockHttp,
        url: &str,
        chain: u64,
        tx_hash: H256,
        input: Vec<u8>,
        block: u64,
        head: u64,
    ) {
        let input_hex = format!("0x{}", hex::encode(&input));
        let tx_hex = tx_hash.to_string();
        http.handle(url, move |req| {
            let v: Json = serde_json::from_slice(&req.body).unwrap();
            let result = match v["method"].as_str().unwrap() {
                "eth_chainId" => json!(format!("0x{chain:x}")),
                "eth_blockNumber" => json!(format!("0x{head:x}")),
                "eth_getTransactionCount" => json!("0x3"),
                "eth_getBlockByNumber" => json!({"baseFeePerGas": "0x3b9aca00"}),
                "eth_maxPriorityFeePerGas" => json!("0xf4240"),
                "eth_estimateGas" => json!("0x5460"),
                "eth_sendRawTransaction" => {
                    let raw = hex::decode(v["params"][0].as_str().unwrap()).unwrap();
                    json!(H256::keccak(&[&raw]).to_string())
                }
                "eth_getTransactionReceipt" if v["params"][0] == tx_hex => {
                    json!({"blockNumber": format!("0x{block:x}"), "status": "0x1"})
                }
                "eth_getTransactionByHash" if v["params"][0] == tx_hex => json!({
                    "input": input_hex, "blockNumber": format!("0x{block:x}"), "chainId": format!("0x{chain:x}")
                }),
                _ => Json::Null,
            };
            Response {
                status: 200,
                body: serde_json::to_vec(&json!({"jsonrpc": "2.0", "id": 1, "result": result})).unwrap().into(),
            }
        });
    }

    #[tokio::test]
    async fn dry_run_and_rpc_submit_and_verify() {
        let key = SigningKey::from_bytes(&[7; 32]).unwrap();
        let hash = [0x5a; 32];
        let dry = DryRunSubmitter::new(
            SigningKey::from_bytes(&[7; 32]).unwrap(),
            EvmConfig::new(10),
            0,
        );
        let p = dry.submit(&hash, UnixNanos(1)).await.unwrap();
        assert!(!p.broadcast);
        assert_eq!(p.committed_hash, H256(hash));
        assert!(dry.poll(&p).await.unwrap().is_none());

        let http = MockHttp::new();
        let rpc_url = "https://rpc.example/";
        // First a node that does not know the tx yet.
        mock_node(&http, rpc_url, 10, H256([0; 32]), vec![], 0, 100);
        let rpc = EvmRpc::new(Arc::new(http.clone()), rpc_url);
        let sub = RpcSubmitter::new(key, EvmConfig::new(10), rpc.clone());
        let pending = sub.submit(&hash, UnixNanos(2)).await.unwrap();
        assert!(pending.broadcast);
        assert!(sub.poll(&pending).await.unwrap().is_none());
        // Now mined at block 95 with head 100: 6 confirmations < 12.
        mock_node(
            &http,
            rpc_url,
            10,
            pending.tx_hash,
            anchor_calldata(&hash),
            95,
            100,
        );
        let a = sub.poll(&pending).await.unwrap().unwrap();
        assert_eq!((a.block_number, a.status), (95, AnchorStatus::Pending));
        assert_eq!(
            verify_anchor(&rpc, &a, &hash, 12).await.unwrap(),
            AnchorStatus::Pending
        );
        assert_eq!(
            verify_anchor(&rpc, &a, &hash, 6).await.unwrap(),
            AnchorStatus::Confirmed
        );
        // Wrong hash, block or chain fail.
        assert!(verify_anchor(&rpc, &a, &[0; 32], 1).await.is_err());
        let mut wrong_block = a.clone();
        wrong_block.block_number = 94;
        assert!(verify_anchor(&rpc, &wrong_block, &hash, 1).await.is_err());
        let mut wrong_chain = a.clone();
        wrong_chain.chain_id = 1;
        assert!(verify_anchor(&rpc, &wrong_chain, &hash, 1).await.is_err());
        // A submitter configured for another chain refuses.
        let other = RpcSubmitter::new(
            SigningKey::from_bytes(&[7; 32]).unwrap(),
            EvmConfig::new(1),
            rpc,
        );
        assert!(other.submit(&hash, UnixNanos(3)).await.is_err());
    }
}
