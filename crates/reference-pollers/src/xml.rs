//! A small streaming XML reader over `quick-xml`.
//!
//! The XML sources (Treasury's OData Atom feeds, the Board's SDMX files,
//! spreadsheet parts) are read element by element with local names only
//! (namespace prefixes are dropped), so a large file is never held as a
//! tree.

use quick_xml::XmlVersion;
use quick_xml::events::Event;

/// An XML parse failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("XML: {0}")]
pub struct XmlError(pub String);

/// One event, with local names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node<'a> {
    /// An opening tag (or an empty element, followed by its [`Node::End`]).
    Start {
        /// Local name.
        name: &'a str,
        /// Attributes by local name, in document order.
        attrs: &'a [(String, String)],
    },
    /// Character data inside the current element (entities resolved).
    Text(&'a str),
    /// A closing tag.
    End {
        /// Local name.
        name: &'a str,
    },
}

fn local(name: &[u8]) -> String {
    let s = String::from_utf8_lossy(name);
    match s.rsplit_once(':') {
        Some((_, l)) => l.to_owned(),
        None => s.into_owned(),
    }
}

fn entity(name: &str) -> Option<String> {
    Some(
        match name {
            "amp" => "&",
            "lt" => "<",
            "gt" => ">",
            "quot" => "\"",
            "apos" => "'",
            _ => {
                let code = if let Some(h) = name.strip_prefix("#x") {
                    u32::from_str_radix(h, 16).ok()?
                } else {
                    name.strip_prefix('#')?.parse().ok()?
                };
                return char::from_u32(code).map(String::from);
            }
        }
        .to_owned(),
    )
}

/// Walks `bytes`, calling `f` for each start tag, text run and end tag.
/// Text split by entity references is joined into one run.
///
/// # Errors
///
/// Fails on malformed XML, or with the first error `f` returns.
pub fn walk<E: From<XmlError>>(
    bytes: &[u8],
    mut f: impl FnMut(Node<'_>) -> Result<(), E>,
) -> Result<(), E> {
    let mut reader = quick_xml::Reader::from_reader(bytes);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut text = String::new();
    let flush = |text: &mut String, f: &mut dyn FnMut(Node<'_>) -> Result<(), E>| {
        if !text.trim().is_empty() {
            let t = std::mem::take(text);
            f(Node::Text(t.trim()))?;
        }
        text.clear();
        Ok::<(), E>(())
    };
    loop {
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| XmlError(e.to_string()))?;
        let (e, empty) = match ev {
            Event::Start(e) => (Some(e), false),
            Event::Empty(e) => (Some(e), true),
            other => {
                match other {
                    Event::End(e) => {
                        flush(&mut text, &mut f)?;
                        let name = local(e.name().as_ref());
                        f(Node::End { name: &name })?;
                    }
                    Event::Text(t) => {
                        let s = t.decode().map_err(|e| XmlError(e.to_string()))?;
                        text.push_str(&s);
                    }
                    Event::CData(t) => {
                        let s = t.decode().map_err(|e| XmlError(e.to_string()))?;
                        text.push_str(&s);
                    }
                    Event::GeneralRef(r) => {
                        let name = String::from_utf8_lossy(r.as_ref()).into_owned();
                        text.push_str(&entity(&name).unwrap_or_default());
                    }
                    Event::Eof => break,
                    _ => {}
                }
                (None, false)
            }
        };
        if let Some(e) = e {
            {
                flush(&mut text, &mut f)?;
                let name = local(e.name().as_ref());
                let mut attrs = Vec::new();
                for a in e.attributes() {
                    let a = a.map_err(|e| XmlError(e.to_string()))?;
                    let v = a
                        .normalized_value(XmlVersion::Implicit1_0)
                        .map_err(|e| XmlError(e.to_string()))?;
                    attrs.push((local(a.key.as_ref()), v.into_owned()));
                }
                f(Node::Start {
                    name: &name,
                    attrs: &attrs,
                })?;
                if empty {
                    f(Node::End { name: &name })?;
                }
            }
        }
        buf.clear();
    }
    Ok(())
}

/// The value of attribute `name` in `attrs`.
#[must_use]
pub fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_with_local_names() {
        let xml = br#"<?xml version="1.0"?><a:feed xmlns:a="x"><m:p d:k="v &amp; w"><d:X t="1">4.05</d:X><d:Y/>
            <d:Z>A &amp; B</d:Z></m:p></a:feed>"#;
        let mut seen = Vec::new();
        walk::<XmlError>(xml, |n| {
            seen.push(match n {
                Node::Start { name, attrs } => {
                    format!("<{name} {}>", attr(attrs, "k").unwrap_or(""))
                }
                Node::Text(t) => format!("'{t}'"),
                Node::End { name } => format!("</{name}>"),
            });
            Ok(())
        })
        .unwrap();
        assert_eq!(
            seen,
            vec![
                "<feed >",
                "<p v & w>",
                "<X >",
                "'4.05'",
                "</X>",
                "<Y >",
                "</Y>",
                "<Z >",
                "'A & B'",
                "</Z>",
                "</p>",
                "</feed>"
            ]
        );
        assert!(walk::<XmlError>(b"<a><b></a>", |_| Ok(())).is_err());
    }
}
