//! Batch runs: input records in, synthetic months out, on a fixed
//! valuation grid (method spec § 14).

use std::io::BufRead;

use mesh_types::{Record, SyntheticMonth};

use crate::engine::Engine;
use crate::error::Error;

/// Valuation grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    /// Step between valuations, ms (positive).
    pub step_ms: i64,
    /// First valuation time; default: the first multiple of the step at or
    /// after the first input.
    pub from_ms: Option<i64>,
    /// Last valuation time (inclusive); default: the last input's time.
    pub to_ms: Option<i64>,
}

/// Output of a batch run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Output {
    /// Records in valuation order.
    pub records: Vec<SyntheticMonth>,
    /// Notices (one per valuation and problem).
    pub notices: Vec<String>,
}

/// Reads JSON-lines records, checking each one. Blank lines are skipped.
///
/// # Errors
///
/// Fails on unreadable input, a line that is not a record, or a record
/// that does not validate.
pub fn read_records(reader: impl BufRead) -> Result<Vec<Record>, Error> {
    let mut out = Vec::new();
    for (i, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let r: Record = serde_json::from_str(&line)
            .map_err(|e| Error::Input(format!("line {}: {e}", i + 1)))?;
        r.validate()
            .map_err(|e| Error::Input(format!("line {}: {e}", i + 1)))?;
        out.push(r);
    }
    Ok(out)
}

/// Runs the engine over `inputs` on `grid`. Inputs are taken in
/// `(ts_source_ms, canonical order)`; each valuation at time `t` sees
/// exactly the inputs with `ts_source_ms <= t` and is positioned at block
/// `floor(t / 1000)`.
///
/// # Errors
///
/// Fails on a non-positive step.
pub fn run(engine: &mut Engine, mut inputs: Vec<Record>, grid: Grid) -> Result<Output, Error> {
    if grid.step_ms <= 0 {
        return Err(Error::Input("the valuation step must be positive".into()));
    }
    inputs.sort_by(|a, b| {
        a.prov()
            .ts_source_ms
            .cmp(&b.prov().ts_source_ms)
            .then_with(|| a.canonical_cmp(b))
    });
    let mut out = Output::default();
    let (Some(first), Some(last)) = (inputs.first(), inputs.last()) else {
        return Ok(out);
    };
    let first_ts = first.prov().ts_source_ms.0;
    let from = grid.from_ms.unwrap_or_else(|| {
        first_ts.div_euclid(grid.step_ms) * grid.step_ms + {
            if first_ts.rem_euclid(grid.step_ms) == 0 {
                0
            } else {
                grid.step_ms
            }
        }
    });
    let to = grid.to_ms.unwrap_or(last.prov().ts_source_ms.0);
    let mut next = 0usize;
    let mut t = from;
    while t <= to {
        while next < inputs.len() && inputs[next].prov().ts_source_ms.0 <= t {
            engine.push(&inputs[next]);
            next += 1;
        }
        let block = u64::try_from(t.div_euclid(1000)).unwrap_or(0);
        let v = engine.value(t, block);
        out.records.extend(v.records);
        out.notices
            .extend(v.notices.into_iter().map(|n| format!("{t}: {n}")));
        t += grid.step_ms;
    }
    Ok(out)
}
