//! Conversion between records and Arrow record batches, both ways.
//!
//! The schemas are `mesh_types::arrow::schema(kind)` with Iceberg field IDs
//! added ([`writer_schema`]). Fixed-point values are stored as mantissas
//! with per-row `price_scale` and `size_scale` columns; every price in a row
//! must share one scale and every size another, or encoding fails (the
//! registry fixes them per instrument). A row without any size (a cancel,
//! an empty book side) takes the batch's first seen scale, so the value is
//! deterministic and decoding never needs it.
//!
//! Decoding is exact: `decode(encode(records)) == records`, which is what
//! lets anyone recompute a partition's canonical content hash from its
//! Parquet bytes.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::{
    Date32Type, Int64Type, TimestampMillisecondType, TimestampNanosecondType, UInt8Type,
    UInt32Type, UInt64Type,
};
use arrow_array::{
    Array, ArrayRef, Date32Array, FixedSizeBinaryArray, Int64Array, ListArray, RecordBatch,
    StringArray, StructArray, TimestampMillisecondArray, TimestampNanosecondArray, UInt8Array,
    UInt32Array, UInt64Array,
};
use arrow_buffer::{Buffer, OffsetBuffer};
use arrow_schema::{DataType, Field, FieldRef, Fields, Schema, SchemaRef};
use parquet::arrow::PARQUET_FIELD_ID_META_KEY;
use serde::Serialize;
use serde::de::DeserializeOwned;

use mesh_types::derived::{DerivedHeader, Footprint, FootprintLevel, ProfileLevel, VolumeProfile};
use mesh_types::swap::{Notional, Quantity, SwapPrice, SwapPrint};
use mesh_types::{
    AdmissionId, AggressorSide, Bar, BarInterval, BookAction, BookEvent, BookSide, BookSnapshot,
    ContentHash, Flags, InstrumentId, Level, MethodId, OrderId, Price, Provenance, Record,
    RecordKind, RestingOrder, Seq, Sha256Digest, Size, SnapshotDepth, Trade, TradeId, UnixMillis,
    UnixNanos, VenueId,
};

use crate::error::{Error, Result};
use crate::layout::Day;

/// Assigns Iceberg-style field IDs: every field of a struct first, then the
/// fields nested in each, depth first; a list's element takes the next ID
/// before its contents.
fn with_ids(fields: &Fields, next: &mut i32) -> Vec<Field> {
    let ids: Vec<i32> = fields
        .iter()
        .map(|_| {
            let id = *next;
            *next += 1;
            id
        })
        .collect();
    fields
        .iter()
        .zip(ids)
        .map(|(f, id)| {
            let dt = with_ids_type(f.data_type(), next);
            let mut meta = f.metadata().clone();
            meta.insert(PARQUET_FIELD_ID_META_KEY.to_owned(), id.to_string());
            Field::new(f.name(), dt, f.is_nullable()).with_metadata(meta)
        })
        .collect()
}

fn with_ids_type(dt: &DataType, next: &mut i32) -> DataType {
    match dt {
        DataType::List(item) => {
            let id = *next;
            *next += 1;
            let inner = with_ids_type(item.data_type(), next);
            let mut meta = item.metadata().clone();
            meta.insert(PARQUET_FIELD_ID_META_KEY.to_owned(), id.to_string());
            DataType::List(Arc::new(
                Field::new(item.name(), inner, item.is_nullable()).with_metadata(meta),
            ))
        }
        DataType::Struct(fields) => DataType::Struct(Fields::from(with_ids(fields, next))),
        other => other.clone(),
    }
}

/// The Arrow schema written to Parquet for `kind`: the mesh-types schema
/// with `PARQUET:field_id` on every field (and list element), so Iceberg
/// readers resolve columns by ID.
#[must_use]
pub fn writer_schema(kind: RecordKind) -> SchemaRef {
    let base = mesh_types::arrow::schema(kind);
    let mut next = 1;
    let fields = with_ids(base.fields(), &mut next);
    Arc::new(Schema::new_with_metadata(fields, base.metadata().clone()))
}

/// The field ID of a field built by [`writer_schema`].
#[must_use]
pub fn field_id(field: &Field) -> Option<i32> {
    field
        .metadata()
        .get(PARQUET_FIELD_ID_META_KEY)
        .and_then(|s| s.parse().ok())
}

fn enum_str<T: Serialize>(v: &T) -> Result<String> {
    match serde_json::to_value(v)? {
        serde_json::Value::String(s) => Ok(s),
        other => Err(Error::invalid(format!("enum serialized as {other}"))),
    }
}

fn parse_enum<T: DeserializeOwned>(s: &str) -> Result<T> {
    Ok(serde_json::from_value(serde_json::Value::String(
        s.to_owned(),
    ))?)
}

/// Per-row scale bookkeeping.
#[derive(Debug, Default, Clone, Copy)]
struct RowScales {
    price: Option<u8>,
    size: Option<u8>,
}

impl RowScales {
    fn price(&mut self, p: Price) -> Result<i64> {
        match self.price {
            Some(s) if s != p.scale() => Err(Error::invalid(format!(
                "prices in one row use scales {s} and {}",
                p.scale()
            ))),
            _ => {
                self.price = Some(p.scale());
                Ok(p.mantissa())
            }
        }
    }

    fn size(&mut self, z: Size) -> Result<i64> {
        match self.size {
            Some(s) if s != z.scale() => Err(Error::invalid(format!(
                "sizes in one row use scales {s} and {}",
                z.scale()
            ))),
            _ => {
                self.size = Some(z.scale());
                Ok(z.mantissa())
            }
        }
    }
}

#[derive(Default)]
struct Columns {
    prov: ProvCols,
    derived: DerivedCols,
    scales: Vec<RowScales>,
    cols: HashMap<&'static str, ArrayRef>,
}

/// Provenance columns under construction (shared with the analytics
/// converter).
pub(super) type ProvColumns = ProvCols;
/// Derived-header columns under construction.
pub(super) type DerivedColumns = DerivedCols;

#[derive(Default)]
pub(super) struct ProvCols {
    instrument: Vec<String>,
    venue: Vec<String>,
    ts_source_ms: Vec<i64>,
    ts_ingest_ns: Vec<i64>,
    block_number: Vec<u64>,
    event_index: Vec<u32>,
    admission: Vec<String>,
    flags: Vec<u32>,
}

impl ProvCols {
    /// The finished columns, by name.
    pub(super) fn finish(self) -> HashMap<&'static str, ArrayRef> {
        HashMap::from([
            ("instrument", strings(self.instrument)),
            ("venue", strings(self.venue)),
            ("ts_source_ms", utc_ms(self.ts_source_ms)),
            ("ts_ingest_ns", utc_ns(self.ts_ingest_ns)),
            ("block_number", u64s(self.block_number)),
            ("event_index", u32s(self.event_index)),
            ("admission", strings(self.admission)),
            ("flags", u32s(self.flags)),
        ])
    }

    pub(super) fn push(&mut self, p: &Provenance) {
        self.instrument.push(p.instrument.as_str().to_owned());
        self.venue.push(p.venue.as_str().to_owned());
        self.ts_source_ms.push(p.ts_source_ms.0);
        self.ts_ingest_ns.push(p.ts_ingest_ns.0);
        self.block_number.push(p.seq.block_number);
        self.event_index.push(p.seq.event_index);
        self.admission.push(p.admission.as_str().to_owned());
        self.flags.push(p.flags.bits());
    }
}

#[derive(Default)]
pub(super) struct DerivedCols {
    method: Vec<String>,
    inputs_hash: Vec<[u8; 32]>,
    input_count: Vec<u64>,
}

impl DerivedCols {
    /// The finished columns, by name (empty for a non-derived kind).
    pub(super) fn finish(self) -> Result<HashMap<&'static str, ArrayRef>> {
        let flat: Vec<u8> = self.inputs_hash.concat();
        Ok(HashMap::from([
            ("method", strings(self.method)),
            (
                "inputs_hash",
                Arc::new(FixedSizeBinaryArray::try_new(
                    32,
                    Buffer::from_vec(flat),
                    None,
                )?) as ArrayRef,
            ),
            ("input_count", u64s(self.input_count)),
        ]))
    }

    pub(super) fn push(&mut self, d: &DerivedHeader) {
        self.method.push(d.method.as_str().to_owned());
        self.inputs_hash.push(d.inputs_hash.0.0);
        self.input_count.push(d.input_count);
    }
}

fn utc_ms(v: Vec<i64>) -> ArrayRef {
    Arc::new(TimestampMillisecondArray::from(v).with_timezone("UTC"))
}

fn utc_ms_opt(v: Vec<Option<i64>>) -> ArrayRef {
    Arc::new(TimestampMillisecondArray::from(v).with_timezone("UTC"))
}

fn utc_ns(v: Vec<i64>) -> ArrayRef {
    Arc::new(TimestampNanosecondArray::from(v).with_timezone("UTC"))
}

fn strings(v: Vec<String>) -> ArrayRef {
    Arc::new(StringArray::from(v))
}

fn strings_opt(v: Vec<Option<String>>) -> ArrayRef {
    Arc::new(StringArray::from(v))
}

fn i64s(v: Vec<i64>) -> ArrayRef {
    Arc::new(Int64Array::from(v))
}

fn i64s_opt(v: Vec<Option<i64>>) -> ArrayRef {
    Arc::new(Int64Array::from(v))
}

fn u32s(v: Vec<u32>) -> ArrayRef {
    Arc::new(UInt32Array::from(v))
}

fn u64s(v: Vec<u64>) -> ArrayRef {
    Arc::new(UInt64Array::from(v))
}

fn list_field(schema: &Schema, name: &str) -> Result<(FieldRef, Fields)> {
    let f = schema.field_with_name(name)?;
    let DataType::List(item) = f.data_type() else {
        return Err(Error::invalid(format!("{name} is not a list")));
    };
    let DataType::Struct(fields) = item.data_type() else {
        return Err(Error::invalid(format!("{name} items are not structs")));
    };
    Ok((item.clone(), fields.clone()))
}

fn list_of_structs(
    schema: &Schema,
    name: &str,
    lengths: Vec<usize>,
    columns: Vec<ArrayRef>,
) -> Result<ArrayRef> {
    let (item, fields) = list_field(schema, name)?;
    let values = StructArray::try_new(fields, columns, None)?;
    let list = ListArray::try_new(item, offsets(&lengths)?, Arc::new(values), None)?;
    Ok(Arc::new(list))
}

fn offsets(lengths: &[usize]) -> Result<OffsetBuffer<i32>> {
    let mut offsets = Vec::with_capacity(lengths.len() + 1);
    let mut acc: i32 = 0;
    offsets.push(acc);
    for &l in lengths {
        let l = i32::try_from(l).map_err(|_| Error::invalid("list too long"))?;
        acc = acc
            .checked_add(l)
            .ok_or_else(|| Error::invalid("list offsets overflow"))?;
        offsets.push(acc);
    }
    Ok(OffsetBuffer::new(offsets.into()))
}

/// Encodes `records`, which must all be of `kind`, as one record batch.
///
/// # Errors
///
/// Fails if a record has another kind or mixes scales within a row.
pub fn encode(kind: RecordKind, records: &[Record]) -> Result<RecordBatch> {
    let schema = writer_schema(kind);
    if kind.is_analytics() {
        return super::analytics::encode(kind, &schema, records);
    }
    let mut c = Columns::default();
    for r in records {
        if r.kind() != kind {
            return Err(Error::invalid(format!(
                "a {} record in a {kind} batch",
                r.kind()
            )));
        }
        c.prov.push(r.prov());
        if let Some(d) = r.derived() {
            c.derived.push(d);
        }
    }
    match kind {
        RecordKind::Trade => encode_trades(records, &mut c)?,
        RecordKind::BookEvent => encode_book_events(records, &mut c)?,
        RecordKind::BookSnapshot => encode_snapshots(&schema, records, &mut c)?,
        RecordKind::Bar => encode_bars(records, &mut c)?,
        RecordKind::Footprint => encode_footprints(&schema, records, &mut c)?,
        RecordKind::VolumeProfile => encode_profiles(&schema, records, &mut c)?,
        RecordKind::SwapPrint => encode_swaps(records, &mut c)?,
        _ => return Err(Error::invalid(format!("no encoder for {kind}"))),
    }
    let ProvCols {
        instrument,
        venue,
        ts_source_ms,
        ts_ingest_ns,
        block_number,
        event_index,
        admission,
        flags,
    } = std::mem::take(&mut c.prov);
    c.cols.insert("instrument", strings(instrument));
    c.cols.insert("venue", strings(venue));
    c.cols.insert("ts_source_ms", utc_ms(ts_source_ms));
    c.cols.insert("ts_ingest_ns", utc_ns(ts_ingest_ns));
    c.cols.insert("block_number", u64s(block_number));
    c.cols.insert("event_index", u32s(event_index));
    c.cols.insert("admission", strings(admission));
    c.cols.insert("flags", u32s(flags));
    if kind.is_derived() {
        let d = std::mem::take(&mut c.derived);
        c.cols.insert("method", strings(d.method));
        let flat: Vec<u8> = d.inputs_hash.concat();
        c.cols.insert(
            "inputs_hash",
            Arc::new(FixedSizeBinaryArray::try_new(
                32,
                Buffer::from_vec(flat),
                None,
            )?),
        );
        c.cols.insert("input_count", u64s(d.input_count));
    }
    if kind != RecordKind::SwapPrint {
        let default_price = c.scales.iter().find_map(|s| s.price).unwrap_or(0);
        let default_size = c.scales.iter().find_map(|s| s.size).unwrap_or(0);
        let ps: Vec<u8> = c
            .scales
            .iter()
            .map(|s| s.price.unwrap_or(default_price))
            .collect();
        let ss: Vec<u8> = c
            .scales
            .iter()
            .map(|s| s.size.unwrap_or(default_size))
            .collect();
        c.cols.insert("price_scale", Arc::new(UInt8Array::from(ps)));
        c.cols.insert("size_scale", Arc::new(UInt8Array::from(ss)));
    }
    let columns = schema
        .fields()
        .iter()
        .map(|f| {
            c.cols
                .remove(f.name().as_str())
                .ok_or_else(|| Error::invalid(format!("no column {} for {kind}", f.name())))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(RecordBatch::try_new(schema, columns)?)
}

fn encode_trades(records: &[Record], c: &mut Columns) -> Result<()> {
    let (mut price, mut size, mut aggr, mut tid) = (vec![], vec![], vec![], vec![]);
    for r in records {
        let Record::Trade(t) = r else { continue };
        let mut s = RowScales::default();
        price.push(s.price(t.price)?);
        size.push(s.size(t.size)?);
        aggr.push(t.aggressor.as_str().to_owned());
        tid.push(t.trade_id.0);
        c.scales.push(s);
    }
    c.cols.insert("price", i64s(price));
    c.cols.insert("size", i64s(size));
    c.cols.insert("aggressor", strings(aggr));
    c.cols.insert("trade_id", u64s(tid));
    Ok(())
}

fn encode_book_events(records: &[Record], c: &mut Columns) -> Result<()> {
    let (mut oid, mut side, mut price, mut action, mut size, mut orig) =
        (vec![], vec![], vec![], vec![], vec![], vec![]);
    for r in records {
        let Record::BookEvent(e) = r else { continue };
        let mut s = RowScales::default();
        oid.push(e.order_id.0);
        side.push(e.side.as_str().to_owned());
        price.push(s.price(e.price)?);
        action.push(e.action.as_str().to_owned());
        let (sz, o) = match e.action {
            BookAction::Add { size } => (Some(size), None),
            BookAction::Modify {
                orig_size,
                new_size,
            }
            | BookAction::Fill {
                orig_size,
                new_size,
            } => (Some(new_size), orig_size),
            BookAction::Cancel {} => (None, None),
        };
        size.push(sz.map(|z| s.size(z)).transpose()?);
        orig.push(o.map(|z| s.size(z)).transpose()?);
        c.scales.push(s);
    }
    c.cols.insert("order_id", u64s(oid));
    c.cols.insert("side", strings(side));
    c.cols.insert("price", i64s(price));
    c.cols.insert("action", strings(action));
    c.cols.insert("size", i64s_opt(size));
    c.cols.insert("orig_size", i64s_opt(orig));
    Ok(())
}

#[derive(Default)]
struct SideEntries {
    lengths: Vec<usize>,
    order_id: Vec<Option<u64>>,
    price: Vec<i64>,
    size: Vec<i64>,
    ts_order_ms: Vec<Option<i64>>,
    order_count: Vec<Option<u32>>,
}

impl SideEntries {
    fn push_l4(&mut self, orders: &[RestingOrder], s: &mut RowScales) -> Result<()> {
        self.lengths.push(orders.len());
        for o in orders {
            self.order_id.push(Some(o.order_id.0));
            self.price.push(s.price(o.price)?);
            self.size.push(s.size(o.size)?);
            self.ts_order_ms.push(Some(o.ts_order_ms.0));
            self.order_count.push(None);
        }
        Ok(())
    }

    fn push_l2(&mut self, levels: &[Level], s: &mut RowScales) -> Result<()> {
        self.lengths.push(levels.len());
        for l in levels {
            self.order_id.push(None);
            self.price.push(s.price(l.price)?);
            self.size.push(s.size(l.size)?);
            self.ts_order_ms.push(None);
            self.order_count.push(Some(l.order_count));
        }
        Ok(())
    }

    fn into_array(self, schema: &Schema, name: &str) -> Result<ArrayRef> {
        let cols: Vec<ArrayRef> = vec![
            Arc::new(UInt64Array::from(self.order_id)),
            i64s(self.price),
            i64s(self.size),
            utc_ms_opt(self.ts_order_ms),
            Arc::new(UInt32Array::from(self.order_count)),
        ];
        list_of_structs(schema, name, self.lengths, cols)
    }
}

fn encode_snapshots(schema: &Schema, records: &[Record], c: &mut Columns) -> Result<()> {
    let mut depth = vec![];
    let (mut bids, mut asks) = (SideEntries::default(), SideEntries::default());
    for r in records {
        let Record::BookSnapshot(snap) = r else {
            continue;
        };
        let mut s = RowScales::default();
        match &snap.depth {
            SnapshotDepth::L4 { bids: b, asks: a } => {
                depth.push("l4".to_owned());
                bids.push_l4(b, &mut s)?;
                asks.push_l4(a, &mut s)?;
            }
            SnapshotDepth::L2 { bids: b, asks: a } => {
                depth.push("l2".to_owned());
                bids.push_l2(b, &mut s)?;
                asks.push_l2(a, &mut s)?;
            }
        }
        c.scales.push(s);
    }
    c.cols.insert("depth", strings(depth));
    c.cols.insert("bids", bids.into_array(schema, "bids")?);
    c.cols.insert("asks", asks.into_array(schema, "asks")?);
    Ok(())
}

fn encode_bars(records: &[Record], c: &mut Columns) -> Result<()> {
    let mut interval = vec![];
    let mut start = vec![];
    let mut px: [Vec<i64>; 4] = Default::default();
    let mut vol: [Vec<i64>; 3] = Default::default();
    let mut count = vec![];
    for r in records {
        let Record::Bar(b) = r else { continue };
        let mut s = RowScales::default();
        interval.push(b.interval.as_str().to_owned());
        start.push(b.start_ms.0);
        for (col, p) in px.iter_mut().zip([b.open, b.high, b.low, b.close]) {
            col.push(s.price(p)?);
        }
        for (col, z) in vol.iter_mut().zip([b.volume, b.buy_volume, b.sell_volume]) {
            col.push(s.size(z)?);
        }
        count.push(b.trade_count);
        c.scales.push(s);
    }
    let [open, high, low, close] = px;
    let [volume, buy, sell] = vol;
    c.cols.insert("interval", strings(interval));
    c.cols.insert("start_ms", utc_ms(start));
    c.cols.insert("open", i64s(open));
    c.cols.insert("high", i64s(high));
    c.cols.insert("low", i64s(low));
    c.cols.insert("close", i64s(close));
    c.cols.insert("volume", i64s(volume));
    c.cols.insert("buy_volume", i64s(buy));
    c.cols.insert("sell_volume", i64s(sell));
    c.cols.insert("trade_count", u32s(count));
    Ok(())
}

fn encode_footprints(schema: &Schema, records: &[Record], c: &mut Columns) -> Result<()> {
    let (mut interval, mut start, mut lengths) = (vec![], vec![], vec![]);
    let (mut price, mut bid, mut ask, mut count) = (vec![], vec![], vec![], vec![]);
    for r in records {
        let Record::Footprint(f) = r else { continue };
        let mut s = RowScales::default();
        interval.push(f.interval.as_str().to_owned());
        start.push(f.start_ms.0);
        lengths.push(f.levels.len());
        for l in &f.levels {
            price.push(s.price(l.price)?);
            bid.push(s.size(l.bid_volume)?);
            ask.push(s.size(l.ask_volume)?);
            count.push(l.trade_count);
        }
        c.scales.push(s);
    }
    c.cols.insert("interval", strings(interval));
    c.cols.insert("start_ms", utc_ms(start));
    c.cols.insert(
        "levels",
        list_of_structs(
            schema,
            "levels",
            lengths,
            vec![i64s(price), i64s(bid), i64s(ask), u32s(count)],
        )?,
    );
    Ok(())
}

fn encode_profiles(schema: &Schema, records: &[Record], c: &mut Columns) -> Result<()> {
    let (mut start, mut end, mut bucket, mut lengths) = (vec![], vec![], vec![], vec![]);
    let (mut price, mut vol, mut buy, mut sell) = (vec![], vec![], vec![], vec![]);
    for r in records {
        let Record::VolumeProfile(v) = r else {
            continue;
        };
        let mut s = RowScales::default();
        start.push(v.start_ms.0);
        end.push(v.end_ms.0);
        bucket.push(s.price(v.bucket)?);
        lengths.push(v.levels.len());
        for l in &v.levels {
            price.push(s.price(l.price)?);
            vol.push(s.size(l.volume)?);
            buy.push(s.size(l.buy_volume)?);
            sell.push(s.size(l.sell_volume)?);
        }
        c.scales.push(s);
    }
    c.cols.insert("start_ms", utc_ms(start));
    c.cols.insert("end_ms", utc_ms(end));
    c.cols.insert("bucket", i64s(bucket));
    c.cols.insert(
        "levels",
        list_of_structs(
            schema,
            "levels",
            lengths,
            vec![i64s(price), i64s(vol), i64s(buy), i64s(sell)],
        )?,
    );
    Ok(())
}

fn swap_price_parts(p: Option<&SwapPrice>) -> (Option<&'static str>, Option<Price>) {
    match p {
        Some(SwapPrice::Price { value }) => (Some("price"), Some(*value)),
        Some(SwapPrice::Spread { value }) => (Some("spread"), Some(*value)),
        Some(SwapPrice::Rate { value }) => (Some("rate"), Some(*value)),
        Some(SwapPrice::SpreadAmount { value }) => (Some("spread_amount"), Some(*value)),
        None => (None, None),
    }
}

fn swap_price(
    r: &Reader<'_>,
    i: usize,
    type_col: &str,
    col: &str,
    scale_col: &str,
) -> Result<Option<SwapPrice>> {
    let Some(t) = r.str_opt(type_col, i)? else {
        return Ok(None);
    };
    let value = Price::new(
        r.i64(col, i)?,
        r.u8_opt(scale_col, i)?.ok_or_else(|| missing(scale_col))?,
    )
    .map_err(dec)?;
    Ok(Some(match t.as_str() {
        "price" => SwapPrice::Price { value },
        "spread" => SwapPrice::Spread { value },
        "rate" => SwapPrice::Rate { value },
        "spread_amount" => SwapPrice::SpreadAmount { value },
        s => return Err(Error::invalid(format!("{type_col} {s:?}"))),
    }))
}

fn amount(
    r: &Reader<'_>,
    i: usize,
    col: &str,
    scale_col: &str,
    cur_col: &str,
) -> Result<Option<Notional>> {
    let Some(currency) = r.str_opt(cur_col, i)? else {
        return Ok(None);
    };
    Ok(Some(Notional {
        amount: Size::new(
            r.i64(col, i)?,
            r.u8_opt(scale_col, i)?.ok_or_else(|| missing(scale_col))?,
        )
        .map_err(dec)?,
        currency,
    }))
}

fn encode_swaps(records: &[Record], c: &mut Columns) -> Result<()> {
    let mut repo = vec![];
    let mut diss = vec![];
    let mut orig = vec![];
    let mut action = vec![];
    let mut asset = vec![];
    let (mut upi, mut product, mut underlier, mut tenor) = (vec![], vec![], vec![], vec![]);
    let mut expiry: Vec<Option<i32>> = vec![];
    let (mut ptype, mut price, mut pscale) = (vec![], vec![], vec![]);
    let (mut notional, mut nscale, mut ncur) = (vec![], vec![], vec![]);
    let (mut qty, mut qscale, mut qunit) = (vec![], vec![], vec![]);
    let mut event_type = vec![];
    let (mut stype, mut strike, mut sscale) = (vec![], vec![], vec![]);
    let (mut prem, mut prem_scale, mut prem_cur) = (vec![], vec![], vec![]);
    let (mut otype, mut ostyle) = (vec![], vec![]);
    let mut first_ex: Vec<Option<i32>> = vec![];
    let (mut exec, mut dissem) = (vec![], vec![]);
    for r in records {
        let Record::SwapPrint(p) = r else { continue };
        repo.push(p.repository.as_str().to_owned());
        diss.push(p.dissemination_id.clone());
        orig.push(p.original_dissemination_id.clone());
        action.push(enum_str(&p.action)?);
        asset.push(enum_str(&p.asset_class)?);
        upi.push(p.upi.clone());
        product.push(p.product.clone());
        underlier.push(p.underlier.clone());
        tenor.push(p.tenor.clone());
        expiry.push(
            p.expiry
                .as_deref()
                .map(|e| e.parse::<Day>().map(Day::days))
                .transpose()?,
        );
        let (t, v) = swap_price_parts(p.price.as_ref());
        ptype.push(t.map(str::to_owned));
        price.push(v.map(Price::mantissa));
        pscale.push(v.map(Price::scale));
        notional.push(p.notional.as_ref().map(|n| n.amount.mantissa()));
        nscale.push(p.notional.as_ref().map(|n| n.amount.scale()));
        ncur.push(p.notional.as_ref().map(|n| n.currency.clone()));
        qty.push(p.quantity.as_ref().map(|q| q.amount.mantissa()));
        qscale.push(p.quantity.as_ref().map(|q| q.amount.scale()));
        qunit.push(p.quantity.as_ref().map(|q| q.unit.clone()));
        event_type.push(p.event_type.as_ref().map(enum_str).transpose()?);
        let (t, v) = swap_price_parts(p.strike.as_ref());
        stype.push(t.map(str::to_owned));
        strike.push(v.map(Price::mantissa));
        sscale.push(v.map(Price::scale));
        prem.push(p.option_premium.as_ref().map(|n| n.amount.mantissa()));
        prem_scale.push(p.option_premium.as_ref().map(|n| n.amount.scale()));
        prem_cur.push(p.option_premium.as_ref().map(|n| n.currency.clone()));
        otype.push(p.option_type.as_ref().map(enum_str).transpose()?);
        ostyle.push(p.option_style.as_ref().map(enum_str).transpose()?);
        first_ex.push(
            p.first_exercise_date
                .as_deref()
                .map(|e| e.parse::<Day>().map(Day::days))
                .transpose()?,
        );
        exec.push(p.ts_execution_ms.0);
        dissem.push(p.ts_dissemination_ms.0);
    }
    c.cols.insert("repository", strings(repo));
    c.cols.insert("dissemination_id", strings(diss));
    c.cols
        .insert("original_dissemination_id", strings_opt(orig));
    c.cols.insert("action", strings(action));
    c.cols.insert("asset_class", strings(asset));
    c.cols.insert("upi", strings_opt(upi));
    c.cols.insert("product", strings_opt(product));
    c.cols.insert("underlier", strings_opt(underlier));
    c.cols.insert("tenor", strings_opt(tenor));
    c.cols.insert("expiry", Arc::new(Date32Array::from(expiry)));
    c.cols.insert("price_type", strings_opt(ptype));
    c.cols.insert("price", i64s_opt(price));
    c.cols
        .insert("price_scale", Arc::new(UInt8Array::from(pscale)));
    c.cols.insert("notional", i64s_opt(notional));
    c.cols
        .insert("notional_scale", Arc::new(UInt8Array::from(nscale)));
    c.cols.insert("notional_currency", strings_opt(ncur));
    c.cols.insert("quantity", i64s_opt(qty));
    c.cols
        .insert("quantity_scale", Arc::new(UInt8Array::from(qscale)));
    c.cols.insert("quantity_unit", strings_opt(qunit));
    c.cols.insert("event_type", strings_opt(event_type));
    c.cols.insert("strike_type", strings_opt(stype));
    c.cols.insert("strike", i64s_opt(strike));
    c.cols
        .insert("strike_scale", Arc::new(UInt8Array::from(sscale)));
    c.cols.insert("option_premium", i64s_opt(prem));
    c.cols.insert(
        "option_premium_scale",
        Arc::new(UInt8Array::from(prem_scale)),
    );
    c.cols
        .insert("option_premium_currency", strings_opt(prem_cur));
    c.cols.insert("option_type", strings_opt(otype));
    c.cols.insert("option_style", strings_opt(ostyle));
    c.cols
        .insert("first_exercise_date", Arc::new(Date32Array::from(first_ex)));
    c.cols.insert("ts_execution_ms", utc_ms(exec));
    c.cols.insert("ts_dissemination_ms", utc_ms(dissem));
    Ok(())
}

// ---------------------------------------------------------------- decoding

fn missing(name: &str) -> Error {
    Error::invalid(format!("column {name} missing or of the wrong type"))
}

fn column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a ArrayRef> {
    batch.column_by_name(name).ok_or_else(|| missing(name))
}

macro_rules! typed {
    ($fn:ident, $ty:ty) => {
        fn $fn<'a>(arr: &'a ArrayRef, name: &str) -> Result<&'a arrow_array::PrimitiveArray<$ty>> {
            arr.as_primitive_opt::<$ty>().ok_or_else(|| missing(name))
        }
    };
}
typed!(as_i64, Int64Type);
typed!(as_u64, UInt64Type);
typed!(as_u32, UInt32Type);
typed!(as_u8, UInt8Type);
typed!(as_ms, TimestampMillisecondType);
typed!(as_ns, TimestampNanosecondType);
typed!(as_date, Date32Type);

fn as_str<'a>(arr: &'a ArrayRef, name: &str) -> Result<&'a StringArray> {
    arr.as_string_opt::<i32>().ok_or_else(|| missing(name))
}

/// Typed access to a batch's columns, with null checks.
struct Reader<'a> {
    batch: &'a RecordBatch,
}

impl<'a> Reader<'a> {
    fn str(&self, name: &str, i: usize) -> Result<&'a str> {
        let a = as_str(column(self.batch, name)?, name)?;
        if a.is_null(i) {
            return Err(Error::invalid(format!("{name} is null in row {i}")));
        }
        Ok(a.value(i))
    }
    fn str_opt(&self, name: &str, i: usize) -> Result<Option<String>> {
        let a = as_str(column(self.batch, name)?, name)?;
        Ok((!a.is_null(i)).then(|| a.value(i).to_owned()))
    }
    fn i64(&self, name: &str, i: usize) -> Result<i64> {
        self.i64_opt(name, i)?
            .ok_or_else(|| Error::invalid(format!("{name} is null in row {i}")))
    }
    fn i64_opt(&self, name: &str, i: usize) -> Result<Option<i64>> {
        let a = as_i64(column(self.batch, name)?, name)?;
        Ok((!a.is_null(i)).then(|| a.value(i)))
    }
    fn u64(&self, name: &str, i: usize) -> Result<u64> {
        let a = as_u64(column(self.batch, name)?, name)?;
        not_null(a, name, i)?;
        Ok(a.value(i))
    }
    fn u32(&self, name: &str, i: usize) -> Result<u32> {
        let a = as_u32(column(self.batch, name)?, name)?;
        not_null(a, name, i)?;
        Ok(a.value(i))
    }
    fn u8_opt(&self, name: &str, i: usize) -> Result<Option<u8>> {
        let a = as_u8(column(self.batch, name)?, name)?;
        Ok((!a.is_null(i)).then(|| a.value(i)))
    }
    fn u8(&self, name: &str, i: usize) -> Result<u8> {
        self.u8_opt(name, i)?
            .ok_or_else(|| Error::invalid(format!("{name} is null in row {i}")))
    }
    fn ms(&self, name: &str, i: usize) -> Result<UnixMillis> {
        let a = as_ms(column(self.batch, name)?, name)?;
        not_null(a, name, i)?;
        Ok(UnixMillis(a.value(i)))
    }
    fn price(&self, name: &str, i: usize) -> Result<Price> {
        Price::new(self.i64(name, i)?, self.u8("price_scale", i)?).map_err(dec)
    }
    fn size(&self, name: &str, i: usize) -> Result<Size> {
        Size::new(self.i64(name, i)?, self.u8("size_scale", i)?).map_err(dec)
    }
    fn size_opt(&self, name: &str, i: usize) -> Result<Option<Size>> {
        self.i64_opt(name, i)?
            .map(|m| Size::new(m, self.u8("size_scale", i)?).map_err(dec))
            .transpose()
    }
    fn list(&self, name: &str, i: usize) -> Result<StructArray> {
        let a = column(self.batch, name)?
            .as_list_opt::<i32>()
            .ok_or_else(|| missing(name))?;
        if a.is_null(i) {
            return Err(Error::invalid(format!("{name} is null in row {i}")));
        }
        a.value(i)
            .as_struct_opt()
            .cloned()
            .ok_or_else(|| missing(name))
    }
}

fn not_null(a: &dyn Array, name: &str, i: usize) -> Result<()> {
    if a.is_null(i) {
        Err(Error::invalid(format!("{name} is null in row {i}")))
    } else {
        Ok(())
    }
}

fn dec(e: mesh_types::DecimalError) -> Error {
    Error::invalid(format!("decimal: {e}"))
}

fn id_err(e: mesh_types::IdError) -> Error {
    Error::invalid(format!("identifier: {e}"))
}

/// Typed access to a struct array (list items).
struct Items<'a> {
    s: &'a StructArray,
}

impl Items<'_> {
    fn col(&self, name: &str) -> Result<&ArrayRef> {
        self.s.column_by_name(name).ok_or_else(|| missing(name))
    }
    fn i64(&self, name: &str, j: usize) -> Result<i64> {
        let a = as_i64(self.col(name)?, name)?;
        not_null(a, name, j)?;
        Ok(a.value(j))
    }
    fn u64_opt(&self, name: &str, j: usize) -> Result<Option<u64>> {
        let a = as_u64(self.col(name)?, name)?;
        Ok((!a.is_null(j)).then(|| a.value(j)))
    }
    fn u32_opt(&self, name: &str, j: usize) -> Result<Option<u32>> {
        let a = as_u32(self.col(name)?, name)?;
        Ok((!a.is_null(j)).then(|| a.value(j)))
    }
    fn ms_opt(&self, name: &str, j: usize) -> Result<Option<i64>> {
        let a = as_ms(self.col(name)?, name)?;
        Ok((!a.is_null(j)).then(|| a.value(j)))
    }
}

fn provenance(r: &Reader<'_>, i: usize) -> Result<Provenance> {
    let ingest = as_ns(column(r.batch, "ts_ingest_ns")?, "ts_ingest_ns")?;
    not_null(ingest, "ts_ingest_ns", i)?;
    let flags = Flags::from_bits(r.u32("flags", i)?)
        .map_err(|e| Error::invalid(format!("unknown flag bits {}", e.0)))?;
    Ok(Provenance::new(
        InstrumentId::new(r.str("instrument", i)?).map_err(id_err)?,
        VenueId::new(r.str("venue", i)?).map_err(id_err)?,
        Seq::new(r.u64("block_number", i)?, r.u32("event_index", i)?),
        r.ms("ts_source_ms", i)?,
        UnixNanos(ingest.value(i)),
        AdmissionId::new(r.str("admission", i)?).map_err(id_err)?,
    )
    .with_flags(flags))
}

fn derived_header(r: &Reader<'_>, i: usize) -> Result<DerivedHeader> {
    let hashes = column(r.batch, "inputs_hash")?
        .as_fixed_size_binary_opt()
        .ok_or_else(|| missing("inputs_hash"))?;
    not_null(hashes, "inputs_hash", i)?;
    let h: [u8; 32] = hashes
        .value(i)
        .try_into()
        .map_err(|_| missing("inputs_hash"))?;
    Ok(DerivedHeader {
        method: MethodId::new(r.str("method", i)?).map_err(id_err)?,
        inputs_hash: ContentHash(Sha256Digest(h)),
        input_count: r.u64("input_count", i)?,
    })
}

/// The provenance of every row, as JSON (for the analytics converter).
pub(super) fn prov_json_rows(batch: &RecordBatch) -> Result<Vec<serde_json::Value>> {
    let r = Reader { batch };
    (0..batch.num_rows())
        .map(|i| Ok(serde_json::to_value(provenance(&r, i)?)?))
        .collect()
}

/// The derived header of every row, as JSON (for the analytics converter).
pub(super) fn derived_json_rows(batch: &RecordBatch) -> Result<Vec<serde_json::Value>> {
    let r = Reader { batch };
    (0..batch.num_rows())
        .map(|i| Ok(serde_json::to_value(derived_header(&r, i)?)?))
        .collect()
}

fn interval(s: &str) -> Result<BarInterval> {
    BarInterval::ALL
        .into_iter()
        .find(|b| b.as_str() == s)
        .ok_or_else(|| Error::invalid(format!("unknown interval {s:?}")))
}

/// Decodes a record batch of `kind` (as written by [`encode`], or read back
/// from Parquet) into records.
///
/// # Errors
///
/// Fails on missing columns, nulls in required fields or invalid values.
pub fn decode(kind: RecordKind, batch: &RecordBatch) -> Result<Vec<Record>> {
    if kind.is_analytics() {
        return super::analytics::decode(kind, batch);
    }
    let r = Reader { batch };
    (0..batch.num_rows())
        .map(|i| decode_row(kind, &r, i))
        .collect()
}

fn decode_row(kind: RecordKind, r: &Reader<'_>, i: usize) -> Result<Record> {
    let prov = provenance(r, i)?;
    Ok(match kind {
        RecordKind::Trade => Record::Trade(Trade {
            prov,
            price: r.price("price", i)?,
            size: r.size("size", i)?,
            aggressor: match r.str("aggressor", i)? {
                "buy" => AggressorSide::Buy,
                "sell" => AggressorSide::Sell,
                "unknown" => AggressorSide::Unknown,
                s => return Err(Error::invalid(format!("aggressor {s:?}"))),
            },
            trade_id: TradeId(r.u64("trade_id", i)?),
        }),
        RecordKind::BookEvent => {
            let new = r.size_opt("size", i)?;
            let orig = r.size_opt("orig_size", i)?;
            let need = |v: Option<Size>| {
                v.ok_or_else(|| Error::invalid(format!("size is null in row {i}")))
            };
            let action = match r.str("action", i)? {
                "add" => BookAction::Add { size: need(new)? },
                "modify" => BookAction::Modify {
                    orig_size: orig,
                    new_size: need(new)?,
                },
                "fill" => BookAction::Fill {
                    orig_size: orig,
                    new_size: need(new)?,
                },
                "cancel" => BookAction::Cancel {},
                s => return Err(Error::invalid(format!("action {s:?}"))),
            };
            Record::BookEvent(BookEvent {
                prov,
                order_id: OrderId(r.u64("order_id", i)?),
                side: match r.str("side", i)? {
                    "bid" => BookSide::Bid,
                    "ask" => BookSide::Ask,
                    s => return Err(Error::invalid(format!("side {s:?}"))),
                },
                price: r.price("price", i)?,
                action,
            })
        }
        RecordKind::BookSnapshot => {
            let ps = r.u8("price_scale", i)?;
            let ss = r.u8("size_scale", i)?;
            let bids = r.list("bids", i)?;
            let asks = r.list("asks", i)?;
            let depth = match r.str("depth", i)? {
                "l4" => SnapshotDepth::L4 {
                    bids: l4_side(&bids, ps, ss)?,
                    asks: l4_side(&asks, ps, ss)?,
                },
                "l2" => SnapshotDepth::L2 {
                    bids: l2_side(&bids, ps, ss)?,
                    asks: l2_side(&asks, ps, ss)?,
                },
                s => return Err(Error::invalid(format!("depth {s:?}"))),
            };
            Record::BookSnapshot(BookSnapshot { prov, depth })
        }
        RecordKind::Bar => Record::Bar(Bar {
            prov,
            derived: derived_header(r, i)?,
            interval: interval(r.str("interval", i)?)?,
            start_ms: r.ms("start_ms", i)?,
            open: r.price("open", i)?,
            high: r.price("high", i)?,
            low: r.price("low", i)?,
            close: r.price("close", i)?,
            volume: r.size("volume", i)?,
            buy_volume: r.size("buy_volume", i)?,
            sell_volume: r.size("sell_volume", i)?,
            trade_count: r.u32("trade_count", i)?,
        }),
        RecordKind::Footprint => {
            let ps = r.u8("price_scale", i)?;
            let ss = r.u8("size_scale", i)?;
            let s = r.list("levels", i)?;
            let it = Items { s: &s };
            let levels = (0..s.len())
                .map(|j| {
                    Ok(FootprintLevel {
                        price: Price::new(it.i64("price", j)?, ps).map_err(dec)?,
                        bid_volume: Size::new(it.i64("bid_volume", j)?, ss).map_err(dec)?,
                        ask_volume: Size::new(it.i64("ask_volume", j)?, ss).map_err(dec)?,
                        trade_count: it
                            .u32_opt("trade_count", j)?
                            .ok_or_else(|| missing("trade_count"))?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Record::Footprint(Footprint {
                prov,
                derived: derived_header(r, i)?,
                interval: interval(r.str("interval", i)?)?,
                start_ms: r.ms("start_ms", i)?,
                levels,
            })
        }
        RecordKind::VolumeProfile => {
            let ps = r.u8("price_scale", i)?;
            let ss = r.u8("size_scale", i)?;
            let s = r.list("levels", i)?;
            let it = Items { s: &s };
            let levels = (0..s.len())
                .map(|j| {
                    Ok(ProfileLevel {
                        price: Price::new(it.i64("price", j)?, ps).map_err(dec)?,
                        volume: Size::new(it.i64("volume", j)?, ss).map_err(dec)?,
                        buy_volume: Size::new(it.i64("buy_volume", j)?, ss).map_err(dec)?,
                        sell_volume: Size::new(it.i64("sell_volume", j)?, ss).map_err(dec)?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Record::VolumeProfile(VolumeProfile {
                prov,
                derived: derived_header(r, i)?,
                start_ms: r.ms("start_ms", i)?,
                end_ms: r.ms("end_ms", i)?,
                bucket: r.price("bucket", i)?,
                levels,
            })
        }
        RecordKind::SwapPrint => {
            let price = swap_price(r, i, "price_type", "price", "price_scale")?;
            let notional = amount(r, i, "notional", "notional_scale", "notional_currency")?;
            let quantity = match r.str_opt("quantity_unit", i)? {
                None => None,
                Some(unit) => Some(Quantity {
                    amount: Size::new(
                        r.i64("quantity", i)?,
                        r.u8_opt("quantity_scale", i)?
                            .ok_or_else(|| missing("quantity_scale"))?,
                    )
                    .map_err(dec)?,
                    unit,
                }),
            };
            let exp = as_date(column(r.batch, "expiry")?, "expiry")?;
            let first_ex = as_date(
                column(r.batch, "first_exercise_date")?,
                "first_exercise_date",
            )?;
            Record::SwapPrint(SwapPrint {
                prov,
                repository: VenueId::new(r.str("repository", i)?).map_err(id_err)?,
                dissemination_id: r.str("dissemination_id", i)?.to_owned(),
                original_dissemination_id: r.str_opt("original_dissemination_id", i)?,
                action: parse_enum(r.str("action", i)?)?,
                asset_class: parse_enum(r.str("asset_class", i)?)?,
                upi: r.str_opt("upi", i)?,
                product: r.str_opt("product", i)?,
                underlier: r.str_opt("underlier", i)?,
                tenor: r.str_opt("tenor", i)?,
                expiry: (!exp.is_null(i)).then(|| Day::from_days(exp.value(i)).to_string()),
                price,
                notional,
                quantity,
                event_type: r
                    .str_opt("event_type", i)?
                    .map(|s| parse_enum(&s))
                    .transpose()?,
                strike: swap_price(r, i, "strike_type", "strike", "strike_scale")?,
                option_premium: amount(
                    r,
                    i,
                    "option_premium",
                    "option_premium_scale",
                    "option_premium_currency",
                )?,
                option_type: r
                    .str_opt("option_type", i)?
                    .map(|s| parse_enum(&s))
                    .transpose()?,
                option_style: r
                    .str_opt("option_style", i)?
                    .map(|s| parse_enum(&s))
                    .transpose()?,
                first_exercise_date: (!first_ex.is_null(i))
                    .then(|| Day::from_days(first_ex.value(i)).to_string()),
                ts_execution_ms: r.ms("ts_execution_ms", i)?,
                ts_dissemination_ms: r.ms("ts_dissemination_ms", i)?,
            })
        }
        other => return Err(Error::invalid(format!("no decoder for {other}"))),
    })
}

fn l4_side(s: &StructArray, ps: u8, ss: u8) -> Result<Vec<RestingOrder>> {
    let it = Items { s };
    (0..s.len())
        .map(|j| {
            Ok(RestingOrder {
                order_id: OrderId(
                    it.u64_opt("order_id", j)?
                        .ok_or_else(|| missing("order_id"))?,
                ),
                price: Price::new(it.i64("price", j)?, ps).map_err(dec)?,
                size: Size::new(it.i64("size", j)?, ss).map_err(dec)?,
                ts_order_ms: UnixMillis(
                    it.ms_opt("ts_order_ms", j)?
                        .ok_or_else(|| missing("ts_order_ms"))?,
                ),
            })
        })
        .collect()
}

fn l2_side(s: &StructArray, ps: u8, ss: u8) -> Result<Vec<Level>> {
    let it = Items { s };
    (0..s.len())
        .map(|j| {
            Ok(Level {
                price: Price::new(it.i64("price", j)?, ps).map_err(dec)?,
                size: Size::new(it.i64("size", j)?, ss).map_err(dec)?,
                order_count: it
                    .u32_opt("order_count", j)?
                    .ok_or_else(|| missing("order_count"))?,
            })
        })
        .collect()
}
