//! Synthetic DTCC public dissemination files for tests.
//!
//! Every value here is invented. The column layout follows the DTCC PPD
//! User Guide v2.8, Appendix A ("Slice File Fields and Layout", January 2024
//! Re-Write Phase 2), in order. No real repository data is used or stored
//! (AGENTS.md hard rule 1); files are generated in memory per test.

#![allow(dead_code, unreachable_pub, clippy::unwrap_used)]

use std::collections::HashMap;
use std::io::{Cursor, Write};

/// Appendix A column layout, in order.
pub const LAYOUT: &[&str] = &[
    "Dissemination Identifier",
    "Original Dissemination Identifier",
    "Action type",
    "Event type",
    "Event timestamp",
    "Amendment indicator",
    "Asset Class",
    "Product name",
    "Cleared",
    "Mandatory clearing indicator",
    "Execution Timestamp",
    "Effective Date",
    "Expiration Date",
    "Maturity date of the underlier",
    "Non-standardized term indicator",
    "Platform identifier",
    "Prime brokerage transaction indicator",
    "Block trade election indicator",
    "Large notional off-facility swap election indicator",
    "Notional amount-Leg 1",
    "Notional amount-Leg 2",
    "Notional currency-Leg 1",
    "Notional currency-Leg 2",
    "Notional quantity-Leg 1",
    "Notional quantity-Leg 2",
    "Total notional quantity-Leg 1",
    "Total notional quantity-Leg 2",
    "Quantity frequency multiplier-Leg 1",
    "Quantity frequency multiplier-Leg 2",
    "Quantity unit of measure-Leg 1",
    "Quantity unit of measure-Leg 2",
    "Quantity frequency-Leg 1",
    "Quantity frequency-Leg 2",
    "Notional amount in effect on associated effective date-Leg 1",
    "Notional amount in effect on associated effective date-Leg 2",
    "Effective date of the notional amount-Leg 1",
    "Effective date of the notional amount-Leg 2",
    "End date of the notional amount-Leg 1",
    "End date of the notional amount-Leg 2",
    "Call amount-Leg 1",
    "Call currency-Leg 1",
    "Put amount-Leg 1",
    "Put currency-Leg 1",
    "Exchange rate",
    "Exchange rate basis",
    "First exercise date",
    "Fixed rate-Leg 1",
    "Fixed rate-Leg 2",
    "Option Premium Amount",
    "Option Premium Currency",
    "Price",
    "Price unit of measure",
    "Spread-Leg 1",
    "Spread-Leg 2",
    "Spread currency-Leg 1",
    "Spread currency-Leg 2",
    "Strike Price",
    "Strike price currency/currency pair",
    "Post-priced swap indicator",
    "Price currency",
    "Price notation",
    "Spread notation-Leg 1",
    "Spread notation-Leg 2",
    "Strike price notation",
    "Fixed rate day count convention-leg 1",
    "Fixed rate day count convention-leg 2",
    "Floating rate day count convention-leg 1",
    "Floating rate day count convention-leg 2",
    "Floating rate reset frequency period-leg 1",
    "Floating rate reset frequency period-leg 2",
    "Floating rate reset frequency period multiplier-leg 1",
    "Floating rate reset frequency period multiplier-leg 2",
    "Other payment amount",
    "Fixed rate payment frequency period-Leg 1",
    "Floating rate payment frequency period-Leg 1",
    "Fixed rate payment frequency period-Leg 2",
    "Floating rate payment frequency period-Leg 2",
    "Fixed rate payment frequency period multiplier-Leg 1",
    "Floating rate payment frequency period multiplier-Leg 1",
    "Fixed rate payment frequency period multiplier-Leg 2",
    "Floating rate payment frequency period multiplier-Leg 2",
    "Other payment type",
    "Other payment currency",
    "Settlement currency-Leg 1",
    "Settlement currency-Leg 2",
    "Settlement location-Leg 1",
    "Collateralisation category",
    "Custom basket indicator",
    "Index factor",
    "Underlier ID-Leg 1",
    "Underlier ID-Leg 2",
    "Underlier ID source-Leg 1",
    "Underlying Asset Name",
    "Underlying asset subtype or underlying contract subtype-Leg 1",
    "Underlying asset subtype or underlying contract subtype-Leg 2",
    "Embedded Option type",
    "Option Type",
    "Option Style",
    "Package indicator",
    "Package transaction price",
    "Package transaction price currency",
    "Package transaction price notation",
    "Package transaction spread",
    "Package transaction spread currency",
    "Package transaction spread notation",
    "Physical delivery location-Leg 1",
    "Delivery Type",
    "Unique Product Identifier",
    "UPI FISN",
    "UPI Underlier Name",
];

/// Dissemination timestamp column (added October 2024; its position in the
/// CSV is unverified, so tests append it at the end).
pub const DISSEMINATION_TS: &str = "Dissemination Timestamp";

/// A synthetic report: header plus rows given as column-to-value maps.
#[derive(Debug, Clone)]
pub struct Report {
    pub columns: Vec<String>,
    pub rows: Vec<HashMap<String, String>>,
}

impl Report {
    /// Appendix A layout plus the dissemination timestamp.
    pub fn standard() -> Self {
        let mut columns: Vec<String> = LAYOUT.iter().map(|s| (*s).to_string()).collect();
        columns.push(DISSEMINATION_TS.to_string());
        Self {
            columns,
            rows: Vec::new(),
        }
    }

    /// Appendix A layout exactly (no dissemination timestamp).
    pub fn appendix_a() -> Self {
        Self {
            columns: LAYOUT.iter().map(|s| (*s).to_string()).collect(),
            rows: Vec::new(),
        }
    }

    pub fn with_column(mut self, name: &str) -> Self {
        self.columns.push(name.to_string());
        self
    }

    pub fn row(mut self, values: &[(&str, &str)]) -> Self {
        for (k, _) in values {
            assert!(
                self.columns.iter().any(|c| c == k),
                "unknown test column {k}"
            );
        }
        self.rows.push(
            values
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        );
        self
    }

    fn quote(v: &str) -> String {
        if v.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", v.replace('"', "\"\""))
        } else {
            v.to_string()
        }
    }

    /// CSV text (quoted header, as DTCC writes it).
    pub fn csv(&self) -> String {
        let mut out = String::new();
        let header: Vec<String> = self
            .columns
            .iter()
            .map(|c| format!("\"{}\"", c.replace('"', "\"\"")))
            .collect();
        out.push_str(&header.join(","));
        out.push_str("\r\n");
        for r in &self.rows {
            let cells: Vec<String> = self
                .columns
                .iter()
                .map(|c| Self::quote(r.get(c).map_or("", String::as_str)))
                .collect();
            out.push_str(&cells.join(","));
            out.push_str("\r\n");
        }
        out
    }

    /// A zip archive holding the CSV as `<stem>.csv`.
    pub fn zip(&self, stem: &str) -> Vec<u8> {
        zip_entries(&[(&format!("{stem}.csv"), self.csv().as_bytes())])
    }
}

/// A zip archive with the given entries (deflate).
pub fn zip_entries(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(body).unwrap();
        }
        w.finish().unwrap();
    }
    buf.into_inner()
}

/// Common columns of a new trade.
pub fn base_trade<'a>(
    id: &'a str,
    asset: &'a str,
    exec: &'a str,
    dissem: &'a str,
) -> Vec<(&'a str, &'a str)> {
    vec![
        ("Dissemination Identifier", id),
        ("Action type", "NEWT"),
        ("Event type", "TRAD"),
        ("Event timestamp", exec),
        ("Amendment indicator", "FALSE"),
        ("Asset Class", asset),
        ("Cleared", "N"),
        ("Execution Timestamp", exec),
        ("Platform identifier", "XOFF"),
        ("Block trade election indicator", "FALSE"),
        (
            "Large notional off-facility swap election indicator",
            "FALSE",
        ),
        (DISSEMINATION_TS, dissem),
    ]
}
