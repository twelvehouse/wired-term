//! State the view shares with the paint closure of the current frame: geometry of the last
//! painted frame, the measured cell size and the grid size applied to the terminal.

use std::{cell::Cell, time::Instant};

use gpui::{Bounds, Pixels, Point, Window, px};
use wired_core::Terminal;

use crate::{
    PADDING_X, PADDING_Y,
    paint::{FontConfig, FontSet, Metrics},
};

/// Geometry of the last painted frame, read by the input handler and the wheel handler.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Layout {
    /// The whole pane.
    pub(crate) bounds: Bounds<Pixels>,
    /// Top-left of the grid (inside the padding).
    pub(crate) origin: Point<Pixels>,
    pub(crate) cell_width: Pixels,
    pub(crate) cell_height: Pixels,
    pub(crate) cols: usize,
    pub(crate) rows: usize,
    /// Where the IME preedit starts (the cursor, or the last row if the cursor is hidden).
    pub(crate) anchor: (usize, usize),
    /// The window's scale factor: the same logical layout lands elsewhere on screen at another.
    pub(crate) scale: f32,
}

/// Cell size of the font in effect, in logical pixels, and the scale it was measured at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CellSize {
    pub(crate) width: Pixels,
    pub(crate) height: Pixels,
    pub(crate) scale: f32,
}

pub(crate) fn measure_cell(window: &Window, font: &FontConfig) -> (FontSet, Metrics, CellSize) {
    let fonts = FontSet::new(font);
    let metrics = Metrics::measure(window, &fonts, px(font.size), font);
    let cell = CellSize { width: metrics.cell_width, height: metrics.cell_height, scale: window.scale_factor() };
    (fonts, metrics, cell)
}

#[derive(Debug, Default)]
pub(crate) struct FrameState {
    /// The grid size the terminal has (`(0, 0)` until one was applied, or after a failure).
    pub(crate) grid_size: Cell<(u16, u16)>,
    /// When the grid last got other columns or rows (other cell pixels alone do not count).
    pub(crate) grid_changed_at: Cell<Option<Instant>>,
    /// Everything the last successful resize sent (a new scale can change the cell pixels
    /// alone); `None` until one succeeded or after a failure.
    pub(crate) applied: Cell<Option<wired_core::Dimensions>>,
    /// Size the grid is computed from instead of the element's own size: the pane's final size,
    /// set by the parent even while the pane is not painted or is drawn animating.
    pub(crate) target: Cell<Option<gpui::Size<Pixels>>>,
    pub(crate) cell: Cell<Option<CellSize>>,
    pub(crate) layout: Cell<Option<Layout>>,
    /// The drawn screen (snapshot generation, grid origin) the link under the pointer was last
    /// looked up on; a frame that differs looks again.
    pub(crate) hover_checked: Cell<Option<(u64, Option<Point<Pixels>>)>>,
}

impl FrameState {
    /// Resizes the terminal to the grid that fits `area` with `cell`, unless it has exactly
    /// these dimensions already. A failed resize is not recorded, so the next call tries again.
    pub(crate) fn fit(&self, terminal: &Terminal, area: gpui::Size<Pixels>, cell: CellSize) {
        let dimensions = dimensions_for(area, cell);
        if self.applied.get() == Some(dimensions) {
            return;
        }
        let is_applied = terminal.resize(dimensions).is_ok();
        self.applied.set(is_applied.then_some(dimensions));
        let grid = if is_applied { (dimensions.cols, dimensions.rows) } else { (0, 0) };
        if self.grid_size.replace(grid) != grid && is_applied {
            self.grid_changed_at.set(Some(Instant::now()));
        }
    }
}

fn dimensions_for(area: gpui::Size<Pixels>, cell: CellSize) -> wired_core::Dimensions {
    let cols = ((area.width - px(PADDING_X * 2.)) / cell.width).floor().max(2.);
    let rows = ((area.height - px(PADDING_Y * 2.)) / cell.height).floor().max(1.);
    // Bounded by the clamps and by any realistic window size.
    wired_core::Dimensions {
        cols: cols.min(f32::from(u16::MAX)) as u16,
        rows: rows.min(f32::from(u16::MAX)) as u16,
        cell_width_px: (f32::from(cell.width) * cell.scale).round() as u16,
        cell_height_px: (f32::from(cell.height) * cell.scale).round() as u16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_resizes_when_only_the_cell_pixels_change() {
        let size = wired_core::Dimensions { cols: 80, rows: 24, ..wired_core::Dimensions::default() };
        let terminal = Terminal::without_program(size, &wired_vt::VtOptions::default()).expect("terminal");
        let frame = FrameState::default();
        let area = gpui::size(px(816.), px(408.));
        let at_100 = CellSize { width: px(8.), height: px(16.), scale: 1. };
        frame.fit(&terminal, area, at_100);
        let first = frame.applied.get().expect("applied");
        assert_eq!((first.cols, first.rows, first.cell_width_px), (100, 25, 8));
        // Same logical cells on a 150 % monitor: same grid, larger cells in pixels.
        frame.fit(&terminal, area, CellSize { scale: 1.5, ..at_100 });
        let moved = frame.applied.get().expect("applied");
        assert_eq!((moved.cols, moved.rows, moved.cell_width_px, moved.cell_height_px), (100, 25, 12, 24));
        assert_eq!(frame.grid_size.get(), (100, 25));
    }

    #[test]
    fn grid_change_time_moves_only_when_columns_or_rows_change() {
        let size = wired_core::Dimensions { cols: 80, rows: 24, ..wired_core::Dimensions::default() };
        let terminal = Terminal::without_program(size, &wired_vt::VtOptions::default()).expect("terminal");
        let frame = FrameState::default();
        let at_100 = CellSize { width: px(8.), height: px(16.), scale: 1. };
        assert_eq!(frame.grid_changed_at.get(), None);
        frame.fit(&terminal, gpui::size(px(816.), px(408.)), at_100);
        let first = frame.grid_changed_at.get().expect("first size counts as a change");
        frame.fit(&terminal, gpui::size(px(816.), px(408.)), CellSize { scale: 1.5, ..at_100 });
        assert_eq!(frame.grid_changed_at.get(), Some(first), "another scale alone is no change");
        frame.grid_changed_at.set(None);
        frame.fit(&terminal, gpui::size(px(416.), px(408.)), CellSize { scale: 1.5, ..at_100 });
        assert!(frame.grid_changed_at.get().is_some(), "fewer columns are a change");
        assert_eq!(frame.grid_size.get(), (50, 25));
    }
}
