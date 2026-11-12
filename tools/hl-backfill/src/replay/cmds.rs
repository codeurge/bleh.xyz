//! The simplified `replica_cmds` representation the prototype consumes.
//!
//! Actions use Hyperliquid's documented exchange-endpoint field names
//! (`a`, `b`, `p`, `s`, `r`, `t`, `c`; `cancels[]` of `{a, o}`; ...), so
//! the `action` object of a real signed action parses directly. What the
//! real files add, and this representation replaces:
//!
//! - **Account.** Real actions carry a signature; the signer (or
//!   `vaultAddress`) identifies the account. Here each action carries an
//!   opaque in-memory `account` tag, used only for self-trade prevention
//!   and client-order-ID lookups. Accounts are never written to any output.
//! - **Block envelope.** Each line is one block: `{block_number,
//!   block_time_ms, actions}`. The real per-block wrapper of the archive
//!   files is undocumented and must be read from a downloaded object.
//! - **Order IDs.** The chain assigns order IDs; with
//!   `--replica-cmds-style actions-and-responses` they appear in the
//!   responses. `oids` (on `order`) and `new_oid` (on `modify`) carry them
//!   when known; otherwise the matcher assigns IDs from its own counter.

use serde::{Deserialize, Serialize};

/// One block of actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayBlock {
    /// Block height.
    pub block_number: u64,
    /// Block time, ms since the epoch.
    pub block_time_ms: i64,
    /// Actions in execution order.
    pub actions: Vec<SignedAction>,
}

/// An action and the account that sent it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedAction {
    /// Opaque in-memory account tag (never published).
    pub account: u64,
    /// The action.
    pub action: Action,
}

/// Time in force of a limit order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tif {
    /// Post only: canceled instead of matching on arrival.
    Alo,
    /// Immediate or cancel: the unfilled part is canceled.
    Ioc,
    /// Good until canceled.
    Gtc,
    /// Front-end market order, treated as IOC (to verify).
    FrontendMarket,
}

/// Order type (`t`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum OrderType {
    /// A limit order.
    Limit {
        /// Time in force.
        tif: Tif,
    },
    /// A trigger (stop or take-profit) order.
    #[serde(rename_all = "camelCase")]
    Trigger {
        /// Market or limit on trigger.
        is_market: bool,
        /// Trigger price.
        trigger_px: String,
        /// `tp` or `sl`.
        tpsl: String,
    },
}

/// One order (exchange-endpoint field names).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderWire {
    /// Asset index.
    pub a: u32,
    /// True for a buy.
    pub b: bool,
    /// Limit price.
    pub p: String,
    /// Size.
    pub s: String,
    /// Reduce only.
    pub r: bool,
    /// Order type.
    pub t: OrderType,
    /// Client order ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c: Option<String>,
}

/// `{a, o}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelWire {
    /// Asset index.
    pub a: u32,
    /// Order ID.
    pub o: u64,
}

/// `{asset, cloid}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloidCancelWire {
    /// Asset index.
    pub asset: u32,
    /// Client order ID.
    pub cloid: String,
}

/// An order reference: order ID or client order ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OrderRef {
    /// Order ID.
    Oid(u64),
    /// Client order ID.
    Cloid(String),
}

/// One modify.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModifyWire {
    /// The order to modify.
    pub oid: OrderRef,
    /// The replacement.
    pub order: OrderWire,
    /// Order ID of the replacement, when known from responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_oid: Option<u64>,
}

/// An L1 action relevant to the book.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Action {
    /// Place orders.
    Order {
        /// The orders.
        orders: Vec<OrderWire>,
        /// `na`, `normalTpsl` or `positionTpsl`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        grouping: Option<String>,
        /// Order IDs assigned by the chain, one per order, when known.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        oids: Vec<u64>,
    },
    /// Cancel by order ID.
    Cancel {
        /// The cancels.
        cancels: Vec<CancelWire>,
    },
    /// Cancel by client order ID.
    CancelByCloid {
        /// The cancels.
        cancels: Vec<CloidCancelWire>,
    },
    /// Modify one order.
    Modify {
        /// The order to modify.
        oid: OrderRef,
        /// The replacement.
        order: OrderWire,
        /// Order ID of the replacement, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        new_oid: Option<u64>,
    },
    /// Modify several orders.
    BatchModify {
        /// The modifies.
        modifies: Vec<ModifyWire>,
    },
    /// Cancel all orders at a time (not emulated).
    ScheduleCancel {
        /// When.
        #[serde(default)]
        time: Option<i64>,
    },
    /// Any other action type (transfers, leverage, ...): no book effect
    /// is emulated.
    #[serde(other)]
    Other,
}

/// Parses a JSON-lines stream of [`ReplayBlock`]s.
///
/// # Errors
///
/// Fails on the first malformed line, with its 1-based number.
pub fn parse_blocks(text: &str) -> Result<Vec<ReplayBlock>, (usize, serde_json::Error)> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| serde_json::from_str(l).map_err(|e| (i + 1, e)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_documented_action_shapes() {
        let line = r#"{"block_number":5,"block_time_ms":1000,"actions":[
            {"account":1,"action":{"type":"order","grouping":"na","orders":[
                {"a":0,"b":true,"p":"100.5","s":"0.1","r":false,"t":{"limit":{"tif":"Gtc"}},"c":"0x01"},
                {"a":0,"b":false,"p":"99","s":"1","r":true,"t":{"trigger":{"isMarket":true,"triggerPx":"98","tpsl":"sl"}}}]}},
            {"account":1,"action":{"type":"cancel","cancels":[{"a":0,"o":42}]}},
            {"account":1,"action":{"type":"cancelByCloid","cancels":[{"asset":0,"cloid":"0x01"}]}},
            {"account":2,"action":{"type":"modify","oid":7,"order":{"a":0,"b":true,"p":"1","s":"1","r":false,"t":{"limit":{"tif":"Alo"}}}}},
            {"account":2,"action":{"type":"batchModify","modifies":[{"oid":"0x02","order":{"a":0,"b":true,"p":"1","s":"1","r":false,"t":{"limit":{"tif":"Alo"}}}}]}},
            {"account":3,"action":{"type":"scheduleCancel","time":5000}},
            {"account":3,"action":{"type":"usdSend","amount":"1","destination":"0x0"}}
        ]}"#
        .replace('\n', " ");
        let blocks = parse_blocks(&line).unwrap();
        let acts: Vec<&Action> = blocks[0].actions.iter().map(|a| &a.action).collect();
        assert!(matches!(acts[0], Action::Order { orders, .. } if orders.len() == 2));
        assert!(matches!(
            acts[3],
            Action::Modify {
                oid: OrderRef::Oid(7),
                ..
            }
        ));
        assert!(
            matches!(acts[4], Action::BatchModify { modifies } if modifies[0].oid == OrderRef::Cloid("0x02".into()))
        );
        assert_eq!(acts[6], &Action::Other);
        assert!(parse_blocks("{\"block_number\":1}").is_err());
    }
}
