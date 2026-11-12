//! Error types.
//!
//! [`BookError`] reports an inconsistency between the order book and the
//! events or snapshots applied to it. [`InputError`] reports input that a
//! builder cannot accept (out of order, mixed scales, overflow).
//! [`ProcessError`] is what the streaming [`Processor`](crate::Processor)
//! returns for caller mistakes; book inconsistencies do not stop it (it
//! marks the book desynchronised and waits for a snapshot).

use mesh_types::{
    BatchError, BookSide, DecimalError, InstrumentId, OrderId, Price, Seq, Size, UnixMillis,
    ValidationError,
};

/// An inconsistency between the book and an event or snapshot.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BookError {
    /// The event refers to an order the book does not hold.
    #[error("{seq}: unknown order id {order_id}", order_id = .order_id.0)]
    UnknownOrder {
        /// The order ID in the event.
        order_id: OrderId,
        /// The event's sequence number.
        seq: Seq,
    },
    /// An `add` for an order ID that is already resting.
    #[error("{seq}: order id {order_id} is already resting", order_id = .order_id.0)]
    DuplicateOrder {
        /// The order ID in the event.
        order_id: OrderId,
        /// The event's sequence number.
        seq: Seq,
    },
    /// The event's side differs from the resting order's.
    #[error("{seq}: order {order_id} rests on the {} side, event says {}", .book.as_str(), .event.as_str(), order_id = .order_id.0)]
    SideMismatch {
        /// The order ID.
        order_id: OrderId,
        /// Side in the book.
        book: BookSide,
        /// Side in the event.
        event: BookSide,
        /// The event's sequence number.
        seq: Seq,
    },
    /// The event's price differs from the resting order's.
    #[error("{seq}: order {order_id} rests at {book}, event says {event}", order_id = .order_id.0)]
    PriceMismatch {
        /// The order ID.
        order_id: OrderId,
        /// Price in the book.
        book: Price,
        /// Price in the event.
        event: Price,
        /// The event's sequence number.
        seq: Seq,
    },
    /// The event's `orig_size` differs from the resting size.
    #[error("{seq}: order {order_id} rests with {book}, event says it had {event}", order_id = .order_id.0)]
    OrigSizeMismatch {
        /// The order ID.
        order_id: OrderId,
        /// Size in the book.
        book: Size,
        /// `orig_size` in the event.
        event: Size,
        /// The event's sequence number.
        seq: Seq,
    },
    /// A fill that does not reduce the resting size: the filled quantity
    /// would be zero or negative.
    #[error("{seq}: fill of order {order_id} leaves {new_size}, resting size is {book}", order_id = .order_id.0)]
    NonReducingFill {
        /// The order ID.
        order_id: OrderId,
        /// Size in the book.
        book: Size,
        /// `new_size` in the event.
        new_size: Size,
        /// The event's sequence number.
        seq: Seq,
    },
    /// The event failed its own validation (for example a zero-size add).
    #[error("{seq}: {source}")]
    InvalidEvent {
        /// The event's sequence number.
        seq: Seq,
        /// The validation error.
        source: ValidationError,
    },
    /// The snapshot failed its own validation.
    #[error("invalid snapshot: {0}")]
    InvalidSnapshot(ValidationError),
    /// An L4 book cannot be built from an L2 snapshot.
    #[error("an L4 book needs an L4 snapshot")]
    NotL4,
    /// A price or size scale differs from the book's.
    #[error("{field} scale {found} differs from the book's {expected}")]
    ScaleMismatch {
        /// `price` or `size`.
        field: &'static str,
        /// The book's scale.
        expected: u8,
        /// The record's scale.
        found: u8,
    },
    /// After a block, the best bid is at or above the best ask.
    #[error("book is crossed or locked after {seq}: bid {bid} >= ask {ask}")]
    Crossed {
        /// Best bid.
        bid: Price,
        /// Best ask.
        ask: Price,
        /// Position of the book.
        seq: Seq,
    },
    /// An event is not after the book's position.
    #[error("event {got} is not after the book's position {last}")]
    OutOfOrder {
        /// The book's position.
        last: Seq,
        /// The event's sequence number.
        got: Seq,
    },
    /// A record belongs to another instrument.
    #[error("record for {found} applied to the book of {expected}")]
    InstrumentMismatch {
        /// The book's instrument.
        expected: InstrumentId,
        /// The record's instrument.
        found: InstrumentId,
    },
    /// A level total or a computed value does not fit 64 bits.
    #[error("arithmetic overflow")]
    Overflow,
    /// A fixed-point value could not be built.
    #[error("decimal: {0}")]
    Decimal(#[from] DecimalError),
}

/// Input that a builder cannot accept.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InputError {
    /// A record's `seq` is not after the previous one.
    #[error("record {got} is not after {last}")]
    OutOfOrder {
        /// The previous `seq`.
        last: Seq,
        /// The offending `seq`.
        got: Seq,
    },
    /// A record's time is before the previous record's, or before a time
    /// the builder was already advanced to.
    #[error("time {got:?} is before {last:?}")]
    TimeWentBackwards {
        /// The previous time or watermark.
        last: UnixMillis,
        /// The offending time.
        got: UnixMillis,
    },
    /// Mixed price or size scales.
    #[error("{field} scale {found} differs from {expected}")]
    ScaleMismatch {
        /// `price` or `size`.
        field: &'static str,
        /// The scale seen first.
        expected: u8,
        /// The offending scale.
        found: u8,
    },
    /// A record belongs to another instrument.
    #[error("record for {found} given to a builder for {expected}")]
    InstrumentMismatch {
        /// The builder's instrument.
        expected: InstrumentId,
        /// The record's instrument.
        found: InstrumentId,
    },
    /// A sum or ratio does not fit 64 bits.
    #[error("arithmetic overflow")]
    Overflow,
    /// A fixed-point value could not be built.
    #[error("decimal: {0}")]
    Decimal(#[from] DecimalError),
    /// Canonical encoding failed.
    #[error("canonical encoding: {0}")]
    Canonical(String),
    /// A configuration value is not usable.
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    /// Inputs are inconsistent with each other (for example 1m bars and
    /// trades that do not match in a rollup).
    #[error("inconsistent inputs: {0}")]
    Inconsistent(String),
    /// A record failed its own validation.
    #[error("{0}")]
    Invalid(#[from] ValidationError),
}

impl From<mesh_types::hash::CanonicalError> for InputError {
    fn from(e: mesh_types::hash::CanonicalError) -> Self {
        Self::Canonical(e.to_string())
    }
}

/// An error from the streaming [`Processor`](crate::Processor).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProcessError {
    /// A builder rejected the input.
    #[error(transparent)]
    Input(#[from] InputError),
    /// A batch failed validation.
    #[error("invalid batch: {0}")]
    Batch(#[from] BatchError),
    /// A batch or block is for an earlier block than one already processed.
    #[error("block {got} arrived after block {last}")]
    BlockOutOfOrder {
        /// The last processed block.
        last: u64,
        /// The offending block.
        got: u64,
    },
    /// A batch's kind is not `trade` or `book_event`.
    #[error("unexpected record kind {0}")]
    UnexpectedKind(mesh_types::RecordKind),
    /// A record belongs to another instrument.
    #[error("record for {found} given to the processor for {expected}")]
    InstrumentMismatch {
        /// The processor's instrument.
        expected: InstrumentId,
        /// The record's instrument.
        found: InstrumentId,
    },
    /// The initial snapshot could not be loaded.
    #[error("cannot load snapshot: {0}")]
    Snapshot(BookError),
}
