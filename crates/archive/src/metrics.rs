//! Process-wide counters, rendered in the Prometheus text format (for the
//! node exporter's textfile collector, or a scrape endpoint the integrator
//! adds).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use crate::error::Result;

/// A set of labelled counters.
#[derive(Debug, Default)]
pub struct Metrics {
    counters: Mutex<BTreeMap<(String, String), u64>>,
}

fn escape(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

impl Metrics {
    /// Adds `by` to the counter `name` with `labels`.
    pub fn inc(&self, name: &str, labels: &[(&str, &str)], by: u64) {
        let labels = labels
            .iter()
            .map(|(k, v)| format!("{k}=\"{}\"", escape(v)))
            .collect::<Vec<_>>()
            .join(",");
        if let Ok(mut c) = self.counters.lock() {
            *c.entry((name.to_owned(), labels)).or_insert(0) += by;
        }
    }

    /// Current value of a counter (labels as rendered, for tests).
    #[must_use]
    pub fn get(&self, name: &str, labels: &[(&str, &str)]) -> u64 {
        let labels = labels
            .iter()
            .map(|(k, v)| format!("{k}=\"{}\"", escape(v)))
            .collect::<Vec<_>>()
            .join(",");
        self.counters
            .lock()
            .ok()
            .and_then(|c| c.get(&(name.to_owned(), labels)).copied())
            .unwrap_or(0)
    }

    /// Renders every counter.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        let Ok(c) = self.counters.lock() else {
            return out;
        };
        let mut last = "";
        for ((name, labels), v) in c.iter() {
            if name != last {
                let _ = writeln!(out, "# TYPE {name} counter");
                last = name;
            }
            if labels.is_empty() {
                let _ = writeln!(out, "{name} {v}");
            } else {
                let _ = writeln!(out, "{name}{{{labels}}} {v}");
            }
        }
        out
    }

    /// Writes [`Metrics::render`] to `path` atomically.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn write_textfile(&self, path: &Path) -> Result<()> {
        crate::partition::write_atomic(path, self.render().as_bytes())
    }
}

/// The process-wide metrics.
pub fn global() -> &'static Metrics {
    static M: OnceLock<Metrics> = OnceLock::new();
    M.get_or_init(Metrics::default)
}

/// Adds to a process-wide counter.
pub fn inc(name: &str, labels: &[(&str, &str)], by: u64) {
    global().inc(name, labels, by);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_prometheus_text() {
        let m = Metrics::default();
        m.inc("a_total", &[("tier", "filtered"), ("stream", "fi\"lls")], 2);
        m.inc("a_total", &[("tier", "filtered"), ("stream", "fi\"lls")], 3);
        m.inc("b_total", &[], 1);
        assert_eq!(
            m.get("a_total", &[("tier", "filtered"), ("stream", "fi\"lls")]),
            5
        );
        assert_eq!(
            m.render(),
            "# TYPE a_total counter\na_total{tier=\"filtered\",stream=\"fi\\\"lls\"} 5\n# TYPE b_total counter\nb_total 1\n"
        );
    }
}
