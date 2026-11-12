//! Reference client for the bleh.xyz gateway (`docs/protocol/gateway-v0.md`).
//!
//! | Module | Contents |
//! |---|---|
//! | [`protocol`] | Every gateway-v0 message type, shared with the gateway |
//! | [`codec`] | JSON and CBOR framing |
//! | [`book`] | [`L4Book`]: order-level book from snapshot plus book events; [`L2Book`]: level book from an L2 snapshot plus `levels` |
//! | [`verify`] | Signer trust, full-stream root checks, proof checks |
//! | [`client`] | [`Client`]: connect, subscribe, keep books, verify, recover, rotate URLs |
//! | [`probe`] | One-shot availability probe (`mesh-client probe`) |
//! | [`keys`] | Signing-key files: address of a secret (`mesh-client keys address`) |
//!
//! ```no_run
//! # async fn demo() -> Result<(), mesh_client::ClientError> {
//! use mesh_client::{Client, ClientConfig, ClientEvent};
//! let mut client = Client::new(ClientConfig {
//!     urls: vec!["wss://gateway.example/v0/ws".into()],
//!     channels: vec!["*|book_event".parse().expect("channel")],
//!     ..ClientConfig::default()
//! });
//! loop {
//!     if let ClientEvent::Block { block_number, .. } = client.next_event().await? {
//!         println!("block {block_number}: {} books", client.books().count());
//!     }
//! }
//! # }
//! ```
//!
//! Operations: `docs/operations/client.md`.

#![cfg_attr(test, allow(clippy::unwrap_used))]
#![warn(missing_docs)]

pub mod book;
pub mod client;
pub mod codec;
pub mod keys;
pub mod probe;
pub mod protocol;
pub mod verify;

pub use book::{BookError, L2Book, L4Book, LevelChanges};
pub use client::{Client, ClientConfig, ClientError, ClientEvent, ClientStats};
pub use protocol::{
    ChannelSpec, ClientMessage, Depth, Encoding, ErrorCode, Hello, InstrumentPattern, Levels,
    Limits, ServerMessage, SnapshotReason,
};
pub use verify::{BlockCollector, SignerSet, VerifyError};
