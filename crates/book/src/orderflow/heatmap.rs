//! L3 heatmap tiles (`heatmap@1`): resting size on a time by price grid.
//!
//! # Grid
//!
//! - **Columns**: `col = floor(t / time_step_ms)`. A column's value is the
//!   book at the end of the column: after the last block whose time is in
//!   the column, or carried forward from the previous column when no block
//!   falls in it (the book did not change).
//! - **Rows**: `row = floor(price / price_step)` on price mantissas. Each
//!   cell sums the resting size of the levels in its row, separately for
//!   bids and asks, over the best `max_levels` levels of each side.
//! - **Tiles**: `tile_cols` columns by `tile_rows` rows, aligned to
//!   multiples of the tile size (`floor(col / tile_cols)`,
//!   `floor(row / tile_rows)`). A tile's cells use local coordinates.
//!
//! A time tile is emitted once time passes its end (a later block, or
//! [`HeatmapBuilder::advance_to`]), as one [`HeatmapTile`] per price tile
//! that has a non-empty cell. At [`HeatmapBuilder::finish`] the open time
//! tile is emitted with columns up to the last block's column. Cells are
//! sparse (non-empty only), ordered by column then row.
//!
//! `price_step` defaults to a power of ten chosen at the first sample so
//! that the mid (or the only side's best price) is between 1000 and 9999
//! steps; deployments should set it per instrument so tiles align across
//! restarts.
//!
//! The inputs hash covers the book events (and snapshots) applied in
//! blocks within the time tile, in block-major order; the first column also
//! depends on the book at the tile start, which the archive fixes
//! (nearest checkpoint plus events).

/// Record types, defined in `mesh-types`.
pub use mesh_types::{HeatCell, HeatmapTile};

use std::collections::BTreeMap;
use std::sync::Arc;

use mesh_types::{BookSide, ContentHasher, Price, Provenance, UnixMillis};
use serde::{Deserialize, Serialize};

use crate::book::OrderBook;
use crate::error::InputError;
use crate::fixed::{add, price, size};
use crate::inputs::header;
use crate::methods;

/// Heatmap settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeatmapConfig {
    /// Column width, ms.
    pub time_step_ms: i64,
    /// Columns per tile.
    pub tile_cols: u32,
    /// Row height; `None` picks a power of ten at the first sample.
    pub price_step: Option<Price>,
    /// Rows per tile.
    pub tile_rows: u32,
    /// Levels sampled per side.
    pub max_levels: usize,
}

impl Default for HeatmapConfig {
    /// One-second columns, one-minute tiles, 64-row tiles, 250 levels.
    fn default() -> Self {
        Self {
            time_step_ms: 1_000,
            tile_cols: 60,
            price_step: None,
            tile_rows: 64,
            max_levels: 250,
        }
    }
}

type Sample = Arc<BTreeMap<i64, (i64, i64)>>;

/// Streams heatmap tiles from the book.
#[derive(Debug, Clone)]
pub struct HeatmapBuilder {
    config: HeatmapConfig,
    step: Option<i64>,
    scales: Option<(u8, u8)>,
    tile: Option<i64>,
    columns: Vec<(i64, Sample)>,
    cur_col: Option<i64>,
    cur_has_block: bool,
    hasher: ContentHasher,
    last_prov: Option<Provenance>,
}

impl HeatmapBuilder {
    /// A builder for `config`.
    ///
    /// # Errors
    ///
    /// Fails on non-positive sizes.
    pub fn new(config: HeatmapConfig) -> Result<Self, InputError> {
        if config.time_step_ms <= 0
            || config.tile_cols == 0
            || config.tile_rows == 0
            || config.max_levels == 0
            || config.price_step.is_some_and(|p| p.mantissa() <= 0)
        {
            return Err(InputError::InvalidConfig(format!(
                "invalid heatmap settings: {config:?}"
            )));
        }
        Ok(Self {
            config,
            step: None,
            scales: None,
            tile: None,
            columns: Vec::new(),
            cur_col: None,
            cur_has_block: false,
            hasher: ContentHasher::new(),
            last_prov: None,
        })
    }

    /// Before a block at `block_time` is applied: closes the columns before
    /// it with the current book.
    ///
    /// # Errors
    ///
    /// Fails if time goes backwards or a value cannot be represented.
    pub fn before_block(
        &mut self,
        book: &OrderBook,
        block_time: UnixMillis,
        out: &mut Vec<HeatmapTile>,
    ) -> Result<(), InputError> {
        let col = block_time.0.div_euclid(self.config.time_step_ms);
        match self.cur_col {
            None => {
                self.cur_col = Some(col);
                self.cur_has_block = false;
            }
            Some(c) if col < c => {
                return Err(InputError::TimeWentBackwards {
                    last: UnixMillis(c.saturating_mul(self.config.time_step_ms)),
                    got: block_time,
                });
            }
            Some(c) if col > c => {
                self.fill(book, c, col, out)?;
                self.cur_col = Some(col);
                self.cur_has_block = false;
            }
            Some(_) => {}
        }
        self.roll_tile(col, out)?;
        Ok(())
    }

    /// Adds an input record's canonical encoding (book event or snapshot)
    /// of the current block.
    pub fn on_input(&mut self, canonical: &[u8]) {
        self.hasher.update_canonical(canonical);
    }

    /// After the block was applied. `prov` is its end-of-block provenance.
    pub fn after_block(&mut self, prov: &Provenance) {
        self.cur_has_block = true;
        self.last_prov = Some(prov.clone());
    }

    /// Time reached `ts` with no block: closes the columns before it.
    ///
    /// # Errors
    ///
    /// Fails if a value cannot be represented.
    pub fn advance_to(
        &mut self,
        book: &OrderBook,
        ts: UnixMillis,
        out: &mut Vec<HeatmapTile>,
    ) -> Result<(), InputError> {
        let col = ts.0.div_euclid(self.config.time_step_ms);
        if let Some(c) = self.cur_col
            && col > c
        {
            self.fill(book, c, col, out)?;
            self.cur_col = Some(col);
            self.cur_has_block = false;
            self.roll_tile(col, out)?;
        }
        Ok(())
    }

    /// Emits the open tile, with the current column if a block fell in it.
    ///
    /// # Errors
    ///
    /// Fails if a value cannot be represented.
    pub fn finish(
        &mut self,
        book: &OrderBook,
        out: &mut Vec<HeatmapTile>,
    ) -> Result<(), InputError> {
        if let (Some(c), true) = (self.cur_col, self.cur_has_block) {
            self.fill(book, c, c + 1, out)?;
        }
        self.emit(out)?;
        self.cur_col = None;
        self.cur_has_block = false;
        Ok(())
    }

    /// Drops the open tile (the book desynchronised).
    pub fn reset(&mut self) {
        self.tile = None;
        self.columns.clear();
        self.cur_col = None;
        self.cur_has_block = false;
        self.hasher = ContentHasher::new();
    }

    /// Samples the book into columns `from..to`.
    fn fill(
        &mut self,
        book: &OrderBook,
        from: i64,
        to: i64,
        out: &mut Vec<HeatmapTile>,
    ) -> Result<(), InputError> {
        let sample = self.sample(book)?;
        for c in from..to {
            self.roll_tile(c, out)?;
            if let Some(s) = &sample {
                self.columns.push((c, Arc::clone(s)));
            }
        }
        Ok(())
    }

    /// Emits the open tile if column `col` belongs to another one.
    fn roll_tile(&mut self, col: i64, out: &mut Vec<HeatmapTile>) -> Result<(), InputError> {
        let tile = col.div_euclid(i64::from(self.config.tile_cols));
        if self.tile != Some(tile) {
            self.emit(out)?;
            self.tile = Some(tile);
        }
        Ok(())
    }

    fn sample(&mut self, book: &OrderBook) -> Result<Option<Sample>, InputError> {
        let (Some(ps), Some(ss)) = (book.price_scale(), book.size_scale()) else {
            return Ok(None);
        };
        self.scales.get_or_insert((ps, ss));
        let step = match self.step {
            Some(s) => s,
            None => {
                let s = match self.config.price_step {
                    Some(p) => p.rescale(ps)?.mantissa(),
                    None => {
                        let top = book.top();
                        let reference = match (top.bid, top.ask) {
                            (Some(b), Some(a)) => {
                                (i128::from(b.price.mantissa()) + i128::from(a.price.mantissa()))
                                    / 2
                            }
                            (Some(l), None) | (None, Some(l)) => i128::from(l.price.mantissa()),
                            (None, None) => return Ok(None),
                        };
                        auto_step(reference)
                    }
                };
                self.step = Some(s);
                s
            }
        };
        let mut cells: BTreeMap<i64, (i64, i64)> = BTreeMap::new();
        for side in [BookSide::Bid, BookSide::Ask] {
            for l in book.level_refs(side).take(self.config.max_levels) {
                let row = l.price_mantissa().div_euclid(step);
                let cell = cells.entry(row).or_default();
                match side {
                    BookSide::Bid => cell.0 = add(cell.0, l.size_mantissa())?,
                    BookSide::Ask => cell.1 = add(cell.1, l.size_mantissa())?,
                }
            }
        }
        Ok(Some(Arc::new(cells)))
    }

    fn emit(&mut self, out: &mut Vec<HeatmapTile>) -> Result<(), InputError> {
        let hasher = std::mem::take(&mut self.hasher);
        let columns = std::mem::take(&mut self.columns);
        let (Some(tile), Some(prov), Some(step), Some((ps, ss))) =
            (self.tile, &self.last_prov, self.step, self.scales)
        else {
            return Ok(());
        };
        if columns.is_empty() {
            return Ok(());
        }
        let cols = i64::from(self.config.tile_cols);
        let rows = i64::from(self.config.tile_rows);
        let first_col = tile.saturating_mul(cols);
        let mut by_price: BTreeMap<i64, Vec<HeatCell>> = BTreeMap::new();
        for (c, sample) in &columns {
            let local_col = u32::try_from(c - first_col).map_err(|_| InputError::Overflow)?;
            for (row, (bid, ask)) in sample.iter() {
                if *bid == 0 && *ask == 0 {
                    continue;
                }
                let ptile = row.div_euclid(rows);
                let local_row =
                    u32::try_from(row.rem_euclid(rows)).map_err(|_| InputError::Overflow)?;
                by_price.entry(ptile).or_default().push(HeatCell {
                    col: local_col,
                    row: local_row,
                    bid_size: size(*bid, ss)?,
                    ask_size: size(*ask, ss)?,
                });
            }
        }
        for (ptile, cells) in by_price {
            let price_start = ptile
                .checked_mul(rows)
                .and_then(|r| r.checked_mul(step))
                .ok_or(InputError::Overflow)?;
            // Every tile of one time range shares the inputs.
            out.push(HeatmapTile {
                prov: prov.clone(),
                derived: header(methods::HEATMAP, hasher.clone())?,
                time_start_ms: UnixMillis(first_col.saturating_mul(self.config.time_step_ms)),
                time_step_ms: self.config.time_step_ms,
                cols: self.config.tile_cols,
                price_start: price(price_start, ps)?,
                price_step: price(step, ps)?,
                rows: self.config.tile_rows,
                cells,
            });
        }
        Ok(())
    }
}

/// The power of ten that puts `reference` (a positive price mantissa)
/// between 1000 and 9999 steps; 1 for small or non-positive references.
fn auto_step(reference: i128) -> i64 {
    let mut step: i64 = 1;
    let mut r = reference;
    while r >= 10_000 {
        r /= 10;
        step = step.saturating_mul(10);
    }
    step
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::add;
    use crate::testutil::*;
    use mesh_types::BookSide::*;

    fn cfg() -> HeatmapConfig {
        HeatmapConfig {
            time_step_ms: 1_000,
            tile_cols: 4,
            price_step: Some(px("1.0")),
            tile_rows: 8,
            max_levels: 10,
        }
    }

    #[test]
    fn columns_carry_forward_and_tiles_split() {
        let mut h = HeatmapBuilder::new(cfg()).unwrap();
        let mut book = OrderBook::new(inst());
        let mut out = Vec::new();
        // Block 1 at t=1s (tile 0 covers columns 0..4).
        let b1 = [
            add(1, 0, 1, Bid, "100.0", "1.00"),
            add(1, 1, 2, Bid, "100.5", "2.00"),
            add(1, 2, 3, Ask, "104.0", "1.00"),
        ];
        h.before_block(&book, UnixMillis(block_ms(1)), &mut out)
            .unwrap();
        book.apply_block(1, UnixMillis(block_ms(1)), &b1).unwrap();
        h.after_block(&prov_eob(1, 1_000));
        // Block 6 at t=6s: columns 1..6 close with the block-1 book; tile 0
        // (cols 0..4) is emitted; column 0 had no sample.
        let b6 = [cancel(6, 0, 2, Bid, "100.5")];
        h.before_block(&book, UnixMillis(block_ms(6)), &mut out)
            .unwrap();
        book.apply_block(6, UnixMillis(block_ms(6)), &b6).unwrap();
        h.after_block(&prov_eob(6, 6_000));
        // Rows: 100 -> row 100 (tile 12, local 4); 104 -> row 104 (tile 13,
        // local 0). Two price tiles.
        assert_eq!(out.len(), 2);
        let t0 = &out[0];
        assert_eq!(t0.time_start_ms, UnixMillis(crate::testutil::BASE_MS));
        assert_eq!(t0.price_start, px("96.0"));
        assert_eq!(t0.cells.len(), 3);
        assert_eq!(t0.cells[0].col, 1);
        assert_eq!(t0.cells[0].row, 4);
        assert_eq!(t0.cells[0].bid_size, sz("3.00"));
        assert_eq!(out[1].price_start, px("104.0"));
        assert_eq!(out[1].cells[0].ask_size, sz("1.00"));

        h.finish(&book, &mut out).unwrap();
        // Tile 1: columns 4, 5 carried (3.00) and 6 after the cancel (1.00).
        let t1: Vec<_> = out[2..]
            .iter()
            .filter(|t| t.price_start == px("96.0"))
            .collect();
        assert_eq!(t1.len(), 1);
        let bids: Vec<_> = t1[0]
            .cells
            .iter()
            .map(|c| (c.col, c.bid_size.to_string()))
            .collect();
        assert_eq!(
            bids,
            [(0, "3.00".into()), (1, "3.00".into()), (2, "1.00".into())]
        );
    }

    #[test]
    fn auto_step_and_config_checks() {
        assert_eq!(auto_step(999), 1);
        assert_eq!(auto_step(10_000), 10);
        assert_eq!(auto_step(1_000_000), 1_000);
        let mut bad = cfg();
        bad.tile_cols = 0;
        assert!(HeatmapBuilder::new(bad).is_err());
    }

    #[test]
    fn advance_closes_tiles_like_a_later_block() {
        let b1 = [add(1, 0, 1, Bid, "100.0", "1.00")];
        let run = |advance: bool| {
            let mut h = HeatmapBuilder::new(cfg()).unwrap();
            let mut book = OrderBook::new(inst());
            let mut out = Vec::new();
            h.before_block(&book, UnixMillis(block_ms(1)), &mut out)
                .unwrap();
            book.apply_block(1, UnixMillis(block_ms(1)), &b1).unwrap();
            h.after_block(&prov_eob(1, 1_000));
            if advance {
                h.advance_to(&book, UnixMillis(block_ms(4)), &mut out)
                    .unwrap();
            }
            let b9 = [add(9, 0, 2, Bid, "99.0", "1.00")];
            h.before_block(&book, UnixMillis(block_ms(9)), &mut out)
                .unwrap();
            book.apply_block(9, UnixMillis(block_ms(9)), &b9).unwrap();
            h.after_block(&prov_eob(9, 9_000));
            h.finish(&book, &mut out).unwrap();
            out
        };
        assert_eq!(run(false), run(true));
    }
}
