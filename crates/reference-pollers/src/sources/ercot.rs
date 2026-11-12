//! ERCOT (ADM-0013) from the public report listings and downloads on
//! www.ercot.com. **api.ercot.com is never used**: its terms let ERCOT
//! limit the users served, which fails ADR 0012.
//!
//! A listing (`/misapp/GetReports.do?reportTypeId=<id>`) is an HTML table
//! of documents; each CSV document (`/misdownload/servlets/mirDownload?
//! doclookupId=<n>`) is fetched once and remembered. XML duplicates are
//! skipped. Columns are matched by name (letters and digits only, any
//! case), and every file's header is checked: an unknown column is
//! reported.
//!
//! | Report type | Product | Series (`ercot:ref:...`) | Interval |
//! |---|---|---|---|
//! | 12331 | DAM settlement point prices (NP4-190-CD) | `DAM_SPP.<point>` | hour |
//! | 12328 | DAM hourly LMPs (NP4-183-CD) | `DAM_LMP.<bus>` | hour |
//! | 12329 | DAM clearing prices for capacity (NP4-188-CD) | `DAM_MCPC.<type>` | hour |
//! | 12301 | RT settlement point prices (NP6-905-CD) | `RT_SPP.<point>` | 15 minutes |
//! | 12300 | RT LMPs per SCED run (NP6-788-CD) | `RT_LMP.<point>` | 5 minutes from the run time |
//! | 24898 | RT AS MCPCs, 15-minute (NP6-331-CD) | `RT_MCPC.<type>` | 15 minutes |
//! | 24891 | RT AS MCPCs per SCED (NP6-332-CD) | `RT_MCPC_SCED.<type>.{CAPPED,UNCAPPED}` | 5 minutes from the run time |
//! | 13060 | Historical DAM hub and load-zone prices (NP4-180-ER, workbooks) | `DAM_SPP.<point>` | hour |
//! | 13061 | Historical RTM hub and load-zone prices (NP6-785-ER, workbooks) | `RT_SPP.<point>` | 15 minutes |
//!
//! Values are prices in US dollars per MWh. Intervals are converted from
//! Central prevailing time to UTC; the repeated hour when daylight time
//! ends is the one flagged `DSTFlag`/`RepeatedHourFlag` = `Y`. A corrected
//! file re-posts intervals with new prices, which become revisions. The
//! bus- and SCED-level products are off by default (volume); node-level
//! history older than the listing window is only offered through the API
//! archive and is not backfilled.

use std::collections::HashMap;
use std::time::Duration;

use super::{Bases, EVERY_DAY, ParseError, Parsed, Reader, Schedule, Target, unzip};
use crate::num;
use crate::obs::{Obs, RefObs};
use crate::source::SourceId;
use crate::time::{Date, UsZone, local_to_utc_ms};
use crate::xlsx;

/// Default website base.
pub const BASE: &str = "https://www.ercot.com";

/// Report types polled live by default.
pub const DEFAULT_PRODUCTS: &str = "12331,12329,12301,24898";
/// Every admitted report type.
pub const ALL_PRODUCTS: [u32; 9] = [
    12331, 12328, 12329, 12301, 12300, 24898, 24891, 13060, 13061,
];

/// Interval shape of a product.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// `HourEnding` hours.
    Hourly,
    /// `DeliveryHour` and `DeliveryInterval` quarters.
    Quarter,
    /// `SCEDTimestamp` runs.
    Sced,
}

struct Product {
    id: u32,
    shape: Shape,
    prefix: &'static str,
    name_col: &'static str,
    values: &'static [(&'static str, &'static str)],
    known: &'static [&'static str],
}

const PRODUCTS: [Product; 9] = [
    Product {
        id: 12331,
        shape: Shape::Hourly,
        prefix: "DAM_SPP",
        name_col: "settlementpoint",
        values: &[("settlementpointprice", "")],
        known: &[
            "deliverydate",
            "hourending",
            "settlementpoint",
            "settlementpointprice",
            "dstflag",
            "repeatedhourflag",
        ],
    },
    Product {
        id: 12328,
        shape: Shape::Hourly,
        prefix: "DAM_LMP",
        name_col: "busname",
        values: &[("lmp", "")],
        known: &["deliverydate", "hourending", "busname", "lmp", "dstflag"],
    },
    Product {
        id: 12329,
        shape: Shape::Hourly,
        prefix: "DAM_MCPC",
        name_col: "ancillarytype",
        values: &[("mcpc", "")],
        known: &[
            "deliverydate",
            "hourending",
            "ancillarytype",
            "mcpc",
            "dstflag",
        ],
    },
    Product {
        id: 12301,
        shape: Shape::Quarter,
        prefix: "RT_SPP",
        name_col: "settlementpointname",
        values: &[("settlementpointprice", "")],
        known: &[
            "deliverydate",
            "deliveryhour",
            "deliveryinterval",
            "settlementpointname",
            "settlementpointtype",
            "settlementpointprice",
            "dstflag",
            "repeatedhourflag",
        ],
    },
    Product {
        id: 12300,
        shape: Shape::Sced,
        prefix: "RT_LMP",
        name_col: "settlementpoint",
        values: &[("lmp", "")],
        known: &[
            "scedtimestamp",
            "repeatedhourflag",
            "settlementpoint",
            "lmp",
        ],
    },
    Product {
        id: 24898,
        shape: Shape::Quarter,
        prefix: "RT_MCPC",
        name_col: "astype",
        values: &[("mcpc", "")],
        known: &[
            "deliverydate",
            "deliveryhour",
            "deliveryinterval",
            "repeatedhourflag",
            "astype",
            "mcpc",
        ],
    },
    Product {
        id: 24891,
        shape: Shape::Sced,
        prefix: "RT_MCPC_SCED",
        name_col: "astype",
        values: &[("cappedmcpc", ".CAPPED"), ("uncappedmcpc", ".UNCAPPED")],
        known: &[
            "scedtimestamp",
            "repeatedhourflag",
            "astype",
            "cappedmcpc",
            "uncappedmcpc",
        ],
    },
    Product {
        id: 13060,
        shape: Shape::Hourly,
        prefix: "DAM_SPP",
        name_col: "settlementpoint",
        values: &[("settlementpointprice", "")],
        known: &[
            "deliverydate",
            "hourending",
            "settlementpoint",
            "settlementpointprice",
            "repeatedhourflag",
            "dstflag",
        ],
    },
    Product {
        id: 13061,
        shape: Shape::Quarter,
        prefix: "RT_SPP",
        name_col: "settlementpointname",
        values: &[("settlementpointprice", "")],
        known: &[
            "deliverydate",
            "deliveryhour",
            "deliveryinterval",
            "repeatedhourflag",
            "settlementpointname",
            "settlementpointtype",
            "settlementpointprice",
            "dstflag",
        ],
    },
];

fn product(id: u32) -> Option<&'static Product> {
    PRODUCTS.iter().find(|p| p.id == id)
}

/// Column name normalized for ERCOT files: lowercase letters and digits
/// only (`DeliveryDate` and `Delivery Date` match).
#[must_use]
pub fn norm(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// One listed document: `(doclookupId, file name)`.
pub type Listed = (u64, String);

/// Reads the documents of a report listing. Rows offering XML are
/// skipped.
#[must_use]
pub fn parse_listing(html: &str) -> Vec<Listed> {
    let mut out = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut starts: Vec<usize> = lower.match_indices("<tr").map(|(i, _)| i).collect();
    starts.push(html.len());
    for w in starts.windows(2) {
        let chunk = &html[w[0]..w[1]];
        let Some(i) = chunk.find("doclookupId=") else {
            continue;
        };
        let id: String = chunk[i + "doclookupId=".len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let Ok(id) = id.parse::<u64>() else { continue };
        let name = chunk
            .split(|c: char| c == '<' || c == '>' || c == '"' || c.is_whitespace())
            .find(|t| t.to_ascii_lowercase().ends_with(".zip") && !t.contains('='))
            .unwrap_or_default()
            .to_owned();
        if name.to_ascii_lowercase().contains("xml") {
            continue;
        }
        out.push((id, name));
    }
    out
}

fn interval(shape: Shape, cells: &HashMap<String, String>) -> Option<(Date, i64, i64)> {
    let repeated = ["dstflag", "repeatedhourflag"].iter().any(|k| {
        cells
            .get(*k)
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("y"))
    });
    let date_of = |s: &str| Date::parse_any(s).or_else(|| xlsx::serial_date(s));
    let (date, start_min, len_min) = match shape {
        Shape::Hourly => {
            let d = date_of(cells.get("deliverydate")?)?;
            let he = cells.get("hourending")?;
            let h: i64 = he.trim().split(':').next()?.trim().parse().ok()?;
            if !(1..=24).contains(&h) {
                return None;
            }
            (d, (h - 1) * 60, 60)
        }
        Shape::Quarter => {
            let d = date_of(cells.get("deliverydate")?)?;
            let h: i64 = cells.get("deliveryhour")?.trim().parse().ok()?;
            let i: i64 = cells.get("deliveryinterval")?.trim().parse().ok()?;
            if !(1..=24).contains(&h) || !(1..=4).contains(&i) {
                return None;
            }
            (d, (h - 1) * 60 + (i - 1) * 15, 15)
        }
        Shape::Sced => {
            let ts = cells.get("scedtimestamp")?;
            let d = Date::parse_us(ts)?;
            let time = ts.split_whitespace().nth(1)?;
            let mut it = time.split(':');
            let h: i64 = it.next()?.parse().ok()?;
            let m: i64 = it.next()?.parse().ok()?;
            (d, h * 60 + m, 5)
        }
    };
    let start = local_to_utc_ms(date, start_min, UsZone::Central, repeated);
    Some((date, start, start + len_min * 60_000))
}

/// Parses rows of one product given a header.
fn parse_table(
    p: &Product,
    header: &[String],
    rows: impl Iterator<Item = Vec<String>>,
    out: &mut Parsed,
) {
    let cols: Vec<String> = header.iter().map(|h| norm(h)).collect();
    for c in &cols {
        if !c.is_empty() && !p.known.contains(&c.as_str()) {
            out.unknown.insert(format!("{}:{c}", p.id));
        }
    }
    for row in rows {
        if row.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        let cells: HashMap<String, String> = cols
            .iter()
            .cloned()
            .zip(row.into_iter().map(|c| c.trim().to_owned()))
            .collect();
        let Some((date, start, end)) = interval(p.shape, &cells) else {
            out.skip("malformed");
            continue;
        };
        let Some(name) = cells.get(p.name_col).filter(|n| !n.is_empty()) else {
            out.skip("malformed");
            continue;
        };
        let name = num::symbol_segment(name);
        for (col, suffix) in p.values {
            match cells.get(*col).map(|v| num::decimal(v)) {
                Some(Ok(Some(v))) => {
                    let mut r = RefObs::usd_price(format!("{}.{name}{suffix}", p.prefix), v, date);
                    r.period = Some((start, end));
                    out.obs.push(Obs::Ref(r));
                }
                Some(Ok(None)) | None => out.skip("missing"),
                Some(Err(_)) => out.skip("malformed"),
            }
        }
    }
}

/// Parses a downloaded document of `product` (a zip of CSV or of a
/// workbook, or a bare CSV).
///
/// # Errors
///
/// Fails on an unknown product or an unreadable file.
pub fn parse_document(product_id: u32, body: &[u8]) -> Result<Parsed, ParseError> {
    let p =
        product(product_id).ok_or_else(|| ParseError(format!("unknown product {product_id}")))?;
    let members = if body.starts_with(b"PK\x03\x04") && !looks_like_xlsx(body) {
        unzip(body)?
    } else {
        vec![("document".to_owned(), body.to_vec())]
    };
    let mut out = Parsed::default();
    for (name, bytes) in members {
        let lname = name.to_ascii_lowercase();
        if lname.ends_with(".xml") {
            continue;
        }
        if lname.ends_with(".xlsx") || looks_like_xlsx(&bytes) {
            for sheet in xlsx::read(&bytes).map_err(ParseError::new)? {
                let mut rows = sheet.rows.into_iter();
                // The header is the first row naming a date column.
                let header = rows
                    .by_ref()
                    .find(|r| r.iter().any(|c| norm(c) == "deliverydate"));
                if let Some(h) = header {
                    parse_table(p, &h, rows, &mut out);
                }
            }
            continue;
        }
        let mut r = csv::ReaderBuilder::new()
            .has_headers(true)
            .flexible(true)
            .from_reader(bytes.as_slice());
        let header: Vec<String> = r
            .headers()
            .map_err(ParseError::new)?
            .iter()
            .map(str::to_owned)
            .collect();
        let rows: Vec<Vec<String>> = r
            .records()
            .filter_map(Result::ok)
            .map(|rec| rec.iter().map(str::to_owned).collect())
            .collect();
        parse_table(p, &header, rows.into_iter(), &mut out);
    }
    Ok(out)
}

fn looks_like_xlsx(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
        && zip::ZipArchive::new(std::io::Cursor::new(bytes)).is_ok_and(|z| {
            z.file_names()
                .any(|n| n.is_ok_and(|n| n.starts_with("xl/")))
        })
}

/// The ERCOT reader.
#[derive(Debug, Clone)]
pub struct ErcotReader {
    base: String,
    products: Vec<u32>,
}

impl ErcotReader {
    /// A reader using `bases` (key `ercot`; products from the
    /// `ercot-products` entry, a comma list of report type IDs).
    #[must_use]
    pub fn new(bases: &Bases) -> Self {
        let products = bases
            .get("ercot-products", DEFAULT_PRODUCTS)
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .filter(|id| ALL_PRODUCTS.contains(id) && !matches!(id, 13060 | 13061))
            .collect();
        Self {
            base: bases.get("ercot", BASE),
            products,
        }
    }

    fn listing(&self, id: u32, filter: &str) -> Target {
        Target::poll(
            format!("ercot:listing:{id}:{filter}"),
            format!("{}/misapp/GetReports.do?reportTypeId={id}", self.base),
            format!("listing:{id}:{filter}"),
        )
    }
}

impl Reader for ErcotReader {
    fn id(&self) -> SourceId {
        SourceId::Ercot
    }

    fn live_targets(&self, _today: Date) -> Vec<Target> {
        self.products
            .iter()
            .map(|id| self.listing(*id, "all"))
            .collect()
    }

    fn backfill_targets(&self, year: i32) -> Vec<Target> {
        if year < 2010 {
            return Vec::new();
        }
        [13060, 13061]
            .into_iter()
            .map(|id| self.listing(id, &year.to_string()))
            .collect()
    }

    fn parse(&self, target: &Target, body: &[u8]) -> Result<Parsed, ParseError> {
        let mut parts = target.kind.splitn(3, ':');
        let (kind, id, rest) = (parts.next(), parts.next(), parts.next().unwrap_or(""));
        let id: u32 = id
            .and_then(|i| i.parse().ok())
            .ok_or_else(|| ParseError(format!("bad target kind {}", target.kind)))?;
        match kind {
            Some("listing") => {
                let mut out = Parsed::default();
                for (doc, name) in parse_listing(&String::from_utf8_lossy(body)) {
                    if rest != "all" && !name.contains(rest) {
                        continue;
                    }
                    out.follow.push(Target {
                        key: format!("ercot:doc:{doc}"),
                        url: format!(
                            "{}/misdownload/servlets/mirDownload?mimic_duns=000000000&doclookupId={doc}",
                            self.base
                        ),
                        kind: format!("doc:{id}:{name}"),
                        once: true,
                    });
                }
                Ok(out)
            }
            Some("doc") => parse_document(id, body),
            _ => Err(ParseError(format!("bad target kind {}", target.kind))),
        }
    }

    fn schedule(&self) -> Schedule {
        // Real-time prices post after every 15-minute interval.
        Schedule {
            days: EVERY_DAY,
            from_min: 0,
            to_min: 24 * 60,
            inside: Duration::from_secs(300),
            outside: Duration::from_secs(300),
        }
    }

    fn revision_days(&self) -> u32 {
        10
    }

    fn live_window_days(&self) -> i64 {
        35
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_and_intervals() {
        let html = r#"<table><tr><td class="name">cdr.00012331.0000000000000000.20261112.131540.DAMSPNP4190_csv.zip</td>
            <td><a href="/misdownload/servlets/mirDownload?mimic_duns=000000000&doclookupId=111">zip</a></td></tr>
            <tr><td>cdr.00012331.0000000000000000.20261112.131540.DAMSPNP4190_xml.zip</td>
            <td><a href="/misdownload/servlets/mirDownload?mimic_duns=000000000&doclookupId=112">zip</a></td></tr></table>"#;
        let l = parse_listing(html);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].0, 111);
        assert!(l[0].1.ends_with("_csv.zip"));
        let mut cells = HashMap::new();
        cells.insert("deliverydate".to_owned(), "11/01/2026".to_owned());
        cells.insert("hourending".to_owned(), "02:00".to_owned());
        cells.insert("dstflag".to_owned(), "N".to_owned());
        let (_, a, b) = interval(Shape::Hourly, &cells).unwrap();
        cells.insert("dstflag".to_owned(), "Y".to_owned());
        let (_, c, _) = interval(Shape::Hourly, &cells).unwrap();
        assert_eq!(b - a, 3_600_000);
        assert_eq!(
            c - a,
            3_600_000,
            "the repeated hour is an hour later in UTC"
        );
        assert_eq!(norm("Delivery Date"), "deliverydate");
    }
}
