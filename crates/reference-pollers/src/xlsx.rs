//! A minimal reader for Office Open XML workbooks (`.xlsx`), enough for
//! the grid operator's historical price workbooks: every worksheet as rows
//! of cell text, shared and inline strings resolved, numbers as written.
//!
//! Formulas, styles and dates stored as serial numbers are not
//! interpreted here; callers convert serial dates with [`serial_date`].

use std::io::{Cursor, Read};

use crate::time::Date;
use crate::xml::{self, Node, XmlError};

/// A workbook read failure.
#[derive(Debug, thiserror::Error)]
pub enum XlsxError {
    /// Not a readable zip archive.
    #[error("workbook zip: {0}")]
    Zip(String),
    /// Malformed XML inside the workbook.
    #[error(transparent)]
    Xml(#[from] XmlError),
}

/// One worksheet: its part name and rows of cell text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sheet {
    /// Part name, such as `xl/worksheets/sheet1.xml`.
    pub name: String,
    /// Rows; missing cells are empty strings.
    pub rows: Vec<Vec<String>>,
}

fn read_part(
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> Result<Option<Vec<u8>>, XlsxError> {
    let mut f = match zip.by_name(name) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(XlsxError::Zip(e.to_string())),
    };
    let mut out = Vec::new();
    f.read_to_end(&mut out)
        .map_err(|e| XlsxError::Zip(e.to_string()))?;
    Ok(Some(out))
}

fn shared_strings(bytes: &[u8]) -> Result<Vec<String>, XlsxError> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    let mut in_t = false;
    xml::walk::<XlsxError>(bytes, |n| {
        match n {
            Node::Start { name: "si", .. } => cur = Some(String::new()),
            Node::Start { name: "t", .. } => in_t = true,
            Node::End { name: "t" } => in_t = false,
            Node::Text(t) if in_t => {
                if let Some(c) = cur.as_mut() {
                    c.push_str(t);
                }
            }
            Node::End { name: "si" } => out.push(cur.take().unwrap_or_default()),
            _ => {}
        }
        Ok(())
    })?;
    Ok(out)
}

/// Zero-based column of a cell reference such as `AB12`.
fn column(cell_ref: &str) -> Option<usize> {
    let mut col = 0usize;
    let mut any = false;
    for b in cell_ref.bytes() {
        if b.is_ascii_uppercase() {
            col = col * 26 + usize::from(b - b'A' + 1);
            any = true;
        } else {
            break;
        }
    }
    (any && col > 0).then(|| col - 1)
}

fn sheet_rows(bytes: &[u8], strings: &[String]) -> Result<Vec<Vec<String>>, XlsxError> {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell_col = 0usize;
    let mut cell_type = String::new();
    let mut value = String::new();
    let mut in_value = false;
    xml::walk::<XlsxError>(bytes, |n| {
        match n {
            Node::Start { name: "row", .. } => row = Vec::new(),
            Node::Start { name: "c", attrs } => {
                cell_col = xml::attr(attrs, "r").and_then(column).unwrap_or(row.len());
                cell_type = xml::attr(attrs, "t").unwrap_or("n").to_owned();
                value.clear();
            }
            Node::Start {
                name: "v" | "t", ..
            } => in_value = true,
            Node::End { name: "v" | "t" } => in_value = false,
            Node::Text(t) if in_value => value.push_str(t),
            Node::End { name: "c" } => {
                let text = if cell_type == "s" {
                    value
                        .trim()
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| strings.get(i).cloned())
                        .unwrap_or_default()
                } else {
                    value.clone()
                };
                if row.len() <= cell_col {
                    row.resize(cell_col + 1, String::new());
                }
                row[cell_col] = text;
            }
            Node::End { name: "row" } => rows.push(std::mem::take(&mut row)),
            _ => {}
        }
        Ok(())
    })?;
    Ok(rows)
}

/// Reads every worksheet of a workbook, in part-number order.
///
/// # Errors
///
/// Fails on a malformed zip or XML part.
pub fn read(bytes: &[u8]) -> Result<Vec<Sheet>, XlsxError> {
    let mut zip =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| XlsxError::Zip(e.to_string()))?;
    let strings = match read_part(&mut zip, "xl/sharedStrings.xml")? {
        Some(b) => shared_strings(&b)?,
        None => Vec::new(),
    };
    let mut names: Vec<(u32, String)> = zip
        .file_names()
        .filter_map(|n| {
            let n = n.ok()?;
            let num = n
                .strip_prefix("xl/worksheets/sheet")?
                .strip_suffix(".xml")?
                .parse()
                .ok()?;
            Some((num, n.into_owned()))
        })
        .collect();
    names.sort();
    let mut out = Vec::new();
    for (_, name) in names {
        if let Some(b) = read_part(&mut zip, &name)? {
            out.push(Sheet {
                rows: sheet_rows(&b, &strings)?,
                name,
            });
        }
    }
    Ok(out)
}

/// The date of a spreadsheet serial day number (1900 date system, with
/// its fictitious 1900-02-29; serial 60 and below are not supported).
#[must_use]
pub fn serial_date(s: &str) -> Option<Date> {
    let v: f64 = s.trim().parse().ok()?;
    if !(61.0..2_958_466.0).contains(&v) {
        return None;
    }
    // Serial 25569 is 1970-01-01.
    let days = v.floor() as i64 - 25_569;
    Some(Date::from_days(days))
}

/// Writes a minimal workbook: one worksheet per entry, cells that parse as
/// numbers stored as numbers and the rest as shared strings. Used to build
/// synthetic fixtures and test workbooks; not a general writer.
///
/// # Errors
///
/// Fails if the zip cannot be written.
pub fn write_minimal(sheets: &[Vec<Vec<String>>]) -> Result<Vec<u8>, XlsxError> {
    use std::io::Write;
    let zerr = |e: &dyn std::fmt::Display| XlsxError::Zip(e.to_string());
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let mut strings: Vec<String> = Vec::new();
    let mut parts = Vec::new();
    for (i, rows) in sheets.iter().enumerate() {
        let mut x = String::from(
            r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
        );
        for (r, row) in rows.iter().enumerate() {
            x.push_str(&format!(r#"<row r="{}">"#, r + 1));
            for (c, cell) in row.iter().enumerate() {
                let mut col = String::new();
                let mut n = c + 1;
                while n > 0 {
                    let rem = u8::try_from((n - 1) % 26).unwrap_or(0);
                    col.insert(0, char::from(b'A' + rem));
                    n = (n - 1) / 26;
                }
                if !cell.is_empty() && cell.parse::<f64>().is_ok() {
                    x.push_str(&format!(r#"<c r="{col}{}"><v>{cell}</v></c>"#, r + 1));
                } else {
                    let idx = match strings.iter().position(|s| s == cell) {
                        Some(i) => i,
                        None => {
                            strings.push(cell.clone());
                            strings.len() - 1
                        }
                    };
                    x.push_str(&format!(r#"<c r="{col}{}" t="s"><v>{idx}</v></c>"#, r + 1));
                }
            }
            x.push_str("</row>");
        }
        x.push_str("</sheetData></worksheet>");
        parts.push((format!("xl/worksheets/sheet{}.xml", i + 1), x));
    }
    let mut ss = String::from(
        r#"<?xml version="1.0"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#,
    );
    for s in &strings {
        ss.push_str(&format!("<si><t>{}</t></si>", esc(s)));
    }
    ss.push_str("</sst>");
    parts.push(("xl/sharedStrings.xml".into(), ss));
    let mut buf = Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default());
        for (name, body) in parts {
            z.start_file(name, opts).map_err(|e| zerr(&e))?;
            z.write_all(body.as_bytes()).map_err(|e| zerr(&e))?;
        }
        z.finish().map_err(|e| zerr(&e))?;
    }
    Ok(buf.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(sheets: &[Vec<Vec<&str>>]) -> Vec<u8> {
        let owned: Vec<Vec<Vec<String>>> = sheets
            .iter()
            .map(|s| {
                s.iter()
                    .map(|r| r.iter().map(|c| (*c).to_owned()).collect())
                    .collect()
            })
            .collect();
        write_minimal(&owned).unwrap()
    }

    #[test]
    fn reads_shared_and_inline_cells() {
        let wb = build(&[vec![
            vec!["Delivery Date", "Price & Co"],
            vec!["01/02/2015", "21.5"],
        ]]);
        let sheets = read(&wb).unwrap();
        assert_eq!(sheets.len(), 1);
        assert_eq!(sheets[0].rows[0][1], "Price & Co");
        assert_eq!(sheets[0].rows[1], vec!["01/02/2015", "21.5"]);
        assert_eq!(column("A1"), Some(0));
        assert_eq!(column("AB7"), Some(27));
        assert_eq!(serial_date("42006").unwrap().to_string(), "2015-01-02");
        assert!(read(b"not a zip").is_err());
    }
}
