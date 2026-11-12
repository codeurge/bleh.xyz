//! Connectors for Databento Binary Encoding (DBN): files the user holds
//! (`.dbn`, `.dbn.zst`), read with the Apache-2.0 `dbn` crate, and, with the
//! `databento-live` feature, the vendor's live client, authenticated with the
//! user's own key from an environment variable.
//!
//! Written from the published DBN specification and the crates' public
//! APIs. Records used: trades (`TradeMsg`), MBO trade and fill actions,
//! top of book (`Mbp1Msg`, `Mbp10Msg`, `BboMsg`, `Cmbp1Msg`, `CbboMsg`) and
//! OHLCV bar closes. Prices are DBN fixed-point (1e-9); undefined prices are
//! skipped. Symbols come from the file's symbology metadata and in-stream
//! symbol-mapping records; an instrument without a mapping is named by its
//! numeric instrument ID.

use std::path::Path;

use dbn::decode::{DbnMetadata, DecodeRecordRef, DynDecoder};
use dbn::{
    FIXED_PRICE_SCALE, RecordRef, RecordRefEnum, TsSymbolMap, UNDEF_PRICE, VersionUpgradePolicy,
};

use super::boundary::UserSupplied;
use super::feed::{FeedError, UserQuote};

/// Which DBN timestamp to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DbnTs {
    /// The matching-engine event time (`ts_event`).
    #[default]
    Event,
    /// The capture time (`ts_recv`), where the record has one.
    Recv,
}

impl std::str::FromStr for DbnTs {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "event" => Ok(Self::Event),
            "recv" => Ok(Self::Recv),
            o => Err(format!("unknown DBN timestamp {o:?} (event or recv)")),
        }
    }
}

fn px(p: i64) -> Option<f64> {
    #[allow(clippy::cast_precision_loss)]
    (p != UNDEF_PRICE).then(|| p as f64 / FIXED_PRICE_SCALE as f64)
}

fn ts(event: u64, recv: Option<u64>, which: DbnTs) -> i64 {
    let t = match which {
        DbnTs::Event => event,
        DbnTs::Recv => recv.unwrap_or(event),
    };
    i64::try_from(t).unwrap_or(i64::MAX)
}

/// Converts one DBN record into a quote; `symbol` names an instrument ID at
/// a time. Records with no usable price give `None`.
pub fn quote_from_record(
    rec: &RecordRef<'_>,
    which: DbnTs,
    symbol: impl Fn(u64, u32) -> Option<String>,
) -> Option<UserQuote> {
    let e = rec.as_enum().ok()?;
    let hd = rec.header();
    let (t_event, iid) = (hd.ts_event, hd.instrument_id);
    let name = || symbol(t_event, iid).unwrap_or_else(|| iid.to_string());
    let top = |t: i64, bid: i64, ask: i64| {
        let (bid, ask) = (px(bid), px(ask));
        (bid.is_some() || ask.is_some()).then(|| UserQuote {
            symbol: name(),
            ts_ns: t,
            price: None,
            bid,
            ask,
        })
    };
    match e {
        RecordRefEnum::Trade(m) => {
            px(m.price).map(|p| UserQuote::trade(name(), ts(t_event, Some(m.ts_recv), which), p))
        }
        RecordRefEnum::Mbo(m) if matches!(m.action as u8, b'T' | b'F') => {
            px(m.price).map(|p| UserQuote::trade(name(), ts(t_event, Some(m.ts_recv), which), p))
        }
        RecordRefEnum::Mbp1(m) => {
            let l = &m.levels[0];
            top(ts(t_event, Some(m.ts_recv), which), l.bid_px, l.ask_px)
        }
        RecordRefEnum::Mbp10(m) => {
            let l = &m.levels[0];
            top(ts(t_event, Some(m.ts_recv), which), l.bid_px, l.ask_px)
        }
        RecordRefEnum::Bbo(m) => {
            let l = &m.levels[0];
            top(ts(t_event, Some(m.ts_recv), which), l.bid_px, l.ask_px)
        }
        RecordRefEnum::Cmbp1(m) => {
            let l = &m.levels[0];
            top(ts(t_event, Some(m.ts_recv), which), l.bid_px, l.ask_px)
        }
        RecordRefEnum::Cbbo(m) => {
            let l = &m.levels[0];
            top(ts(t_event, Some(m.ts_recv), which), l.bid_px, l.ask_px)
        }
        RecordRefEnum::Ohlcv(m) => {
            px(m.close).map(|p| UserQuote::trade(name(), ts(t_event, None, which), p))
        }
        _ => None,
    }
}

/// Reads a DBN file the user holds, plain or zstd-compressed (detected).
pub struct DbnFileFeed {
    decoder: DynDecoder<'static, std::io::BufReader<std::fs::File>>,
    symbols: TsSymbolMap,
    which: DbnTs,
}

impl std::fmt::Debug for DbnFileFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbnFileFeed")
            .field("which", &self.which)
            .finish()
    }
}

impl DbnFileFeed {
    /// Opens a file.
    ///
    /// # Errors
    ///
    /// The file cannot be opened or its metadata decoded.
    pub fn open(path: &Path, which: DbnTs) -> Result<Self, FeedError> {
        let decoder = DynDecoder::from_file(path, VersionUpgradePolicy::UpgradeToV3)
            .map_err(|e| FeedError::Source(format!("DBN: {e}")))?;
        let symbols = TsSymbolMap::from_metadata(decoder.metadata()).unwrap_or_default();
        Ok(Self {
            decoder,
            symbols,
            which,
        })
    }

    /// The next quote, `None` at the end of the file.
    ///
    /// # Errors
    ///
    /// A decoding failure.
    pub fn next_quote(&mut self) -> Result<Option<UserSupplied<UserQuote>>, FeedError> {
        loop {
            let Some(rec) = self
                .decoder
                .decode_record_ref()
                .map_err(|e| FeedError::Source(format!("DBN: {e}")))?
            else {
                return Ok(None);
            };
            // In-stream symbol mappings (files recorded from a live feed).
            let _ = self.symbols.on_record(rec);
            let symbols = &self.symbols;
            if let Some(q) = quote_from_record(&rec, self.which, |t, iid| {
                symbols.get_for_ts(t, iid).cloned()
            }) {
                return Ok(Some(UserSupplied::new(q)));
            }
        }
    }
}

impl Iterator for DbnFileFeed {
    type Item = Result<UserSupplied<UserQuote>, FeedError>;
    fn next(&mut self) -> Option<Self::Item> {
        self.next_quote().transpose()
    }
}

/// The vendor's live client (feature `databento-live`).
#[cfg(feature = "databento-live")]
pub mod live {
    use dbn::{PitSymbolMap, SType, Schema};
    use tokio::sync::mpsc;

    use super::{DbnTs, quote_from_record};
    use crate::byo::boundary::UserSupplied;
    use crate::byo::feed::{FeedError, UserQuote};

    /// Live subscription settings.
    #[derive(Debug, Clone)]
    pub struct LiveConfig {
        /// Name of the environment variable holding the user's API key.
        pub key_env: String,
        /// Dataset code, as the vendor names it.
        pub dataset: String,
        /// Schema, e.g. `trades`, `mbp-1`, `bbo-1s`.
        pub schema: String,
        /// Symbols to subscribe to (raw symbols).
        pub symbols: Vec<String>,
        /// Which timestamp to use.
        pub ts: DbnTs,
    }

    /// Connects with the user's key, subscribes and sends quotes until the
    /// receiver is dropped or the session ends. The key is read from the
    /// environment, used for authentication only, and never logged.
    ///
    /// # Errors
    ///
    /// Missing key, bad schema, or a connection or protocol failure.
    pub async fn run(
        cfg: LiveConfig,
        tx: mpsc::Sender<Result<UserSupplied<UserQuote>, FeedError>>,
    ) -> Result<(), FeedError> {
        let key = std::env::var(&cfg.key_env).map_err(|_| {
            FeedError::Config(format!(
                "set {} to your own API key (it is read from the environment only)",
                cfg.key_env
            ))
        })?;
        let schema: Schema = cfg
            .schema
            .parse()
            .map_err(|_| FeedError::Config(format!("unknown schema {:?}", cfg.schema)))?;
        let src = |e: databento::Error| FeedError::Source(format!("live: {e}"));
        let mut client = databento::LiveClient::builder()
            .key(key)
            .map_err(src)?
            .dataset(cfg.dataset.clone())
            .build()
            .await
            .map_err(src)?;
        client
            .subscribe(
                databento::live::Subscription::builder()
                    .symbols(cfg.symbols.clone())
                    .schema(schema)
                    .stype_in(SType::RawSymbol)
                    .build(),
            )
            .await
            .map_err(src)?;
        client.start().await.map_err(src)?;
        let mut symbols = PitSymbolMap::new();
        while let Some(rec) = client.next_record().await.map_err(src)? {
            let _ = symbols.on_record(rec);
            if let Some(q) = quote_from_record(&rec, cfg.ts, |_, iid| symbols.get(iid).cloned())
                && tx.send(Ok(UserSupplied::new(q))).await.is_err()
            {
                break;
            }
        }
        let _ = client.close().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbn::encode::{DbnEncoder, EncodeRecord};
    use dbn::{
        BidAskPair, MappingInterval, Mbp1Msg, Metadata, OhlcvMsg, RecordHeader, SType, Schema,
        SymbolMapping, TradeMsg, rtype,
    };

    const T0: u64 = 1_767_225_600_000_000_000; // 2026-01-01T00:00:00Z

    fn fx(p: f64) -> i64 {
        #[allow(clippy::cast_possible_truncation)]
        let v = (p * 1e9).round() as i64;
        v
    }

    /// Writes synthetic DBN records (generated here, not market data).
    fn synthetic_file(dir: &Path, zstd: bool) -> std::path::PathBuf {
        let day = time::Date::from_calendar_date(2026, time::Month::January, 1).unwrap();
        let metadata = Metadata::builder()
            .dataset("SYNTH.TEST")
            .schema(Some(Schema::Trades))
            .start(T0)
            .stype_in(Some(SType::RawSymbol))
            .stype_out(SType::InstrumentId)
            .symbols(vec!["SYN-A".into()])
            .mappings(vec![SymbolMapping {
                raw_symbol: "SYN-A".into(),
                intervals: vec![MappingInterval {
                    start_date: day,
                    end_date: day.next_day().unwrap(),
                    symbol: "7".into(),
                }],
            }])
            .build();
        let trade = |i: u64, iid: u32, p: f64| TradeMsg {
            hd: RecordHeader::new::<TradeMsg>(rtype::MBP_0, 1, iid, T0 + i * 1_000_000_000),
            price: fx(p),
            size: 1,
            ts_recv: T0 + i * 1_000_000_000 + 5_000,
            ..TradeMsg::default()
        };
        let mut mbp = Mbp1Msg {
            hd: RecordHeader::new::<Mbp1Msg>(rtype::MBP_1, 1, 7, T0 + 3_000_000_000),
            ts_recv: T0 + 3_000_000_000,
            ..Mbp1Msg::default()
        };
        mbp.levels[0] = BidAskPair {
            bid_px: fx(99.5),
            ask_px: fx(100.5),
            ..BidAskPair::default()
        };
        let undef = TradeMsg {
            price: UNDEF_PRICE,
            ..trade(4, 7, 0.0)
        };
        let bar = OhlcvMsg {
            hd: RecordHeader::new::<OhlcvMsg>(rtype::OHLCV_1M, 1, 7, T0 + 5_000_000_000),
            close: fx(101.0),
            ..OhlcvMsg::default_for_schema(Schema::Ohlcv1M)
        };
        let mut buf = Vec::new();
        {
            let mut enc = DbnEncoder::new(&mut buf, &metadata).unwrap();
            enc.encode_record(&trade(1, 7, 100.25)).unwrap();
            enc.encode_record(&trade(2, 9, 5.0)).unwrap();
            enc.encode_record(&mbp).unwrap();
            enc.encode_record(&undef).unwrap();
            enc.encode_record(&bar).unwrap();
        }
        let path = dir.join(if zstd { "s.dbn.zst" } else { "s.dbn" });
        if zstd {
            std::fs::write(&path, zstd::encode_all(buf.as_slice(), 3).unwrap()).unwrap();
        } else {
            std::fs::write(&path, &buf).unwrap();
        }
        path
    }

    #[test]
    fn reads_synthetic_dbn_files_plain_and_compressed() {
        let d = tempfile::tempdir().unwrap();
        for z in [false, true] {
            let path = synthetic_file(d.path(), z);
            let q: Vec<UserQuote> = DbnFileFeed::open(&path, DbnTs::Event)
                .unwrap()
                .map(|r| r.unwrap().get().clone())
                .collect();
            assert_eq!(q.len(), 4, "undefined prices are skipped: {q:?}");
            assert_eq!(q[0].symbol, "SYN-A", "named from the symbology metadata");
            assert_eq!(q[0].price, Some(100.25));
            assert_eq!(q[0].ts_ns, i64::try_from(T0).unwrap() + 1_000_000_000);
            assert_eq!(q[1].symbol, "9", "unmapped instruments keep their ID");
            assert_eq!(
                (q[2].bid, q[2].ask, q[2].mid()),
                (Some(99.5), Some(100.5), Some(100.0))
            );
            assert_eq!(q[3].price, Some(101.0), "bar close");
        }
        let path = synthetic_file(d.path(), false);
        let q = DbnFileFeed::open(&path, DbnTs::Recv)
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(q.get().ts_ns, i64::try_from(T0).unwrap() + 1_000_005_000);
    }

    #[test]
    fn missing_files_are_reported() {
        let e = DbnFileFeed::open(Path::new("/nonexistent/x.dbn"), DbnTs::Event).unwrap_err();
        assert!(e.to_string().contains("DBN"), "{e}");
    }
}
