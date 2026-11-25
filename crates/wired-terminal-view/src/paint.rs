//! Grid painting: runs are shaped as text so ligatures form, then each
//! glyph cluster is placed at the x of its first cell. Colors come from the cell of each glyph,
//! so color changes never break a ligature.

use std::sync::Arc;

use gpui::{
    App, Bounds, ContentMask, Font, FontFeatures, FontStyle, FontWeight, Hsla, Pixels, Point, Rgba, SharedString,
    TextRun, Window, fill, font, point, px, size,
};
use wired_vt::{Cell, CellFlags, CellWidth, Colors, Cursor, CursorStyle, GridSnapshot, Rgb, Row, Underline};

use crate::{
    ViewColors, custom_glyphs,
    ime::{self, ImeState},
};

/// Ligature behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Ligatures {
    /// Always form ligatures.
    On,
    /// Never form ligatures.
    Off,
    /// Form ligatures except across the cursor cell.
    #[default]
    BreakAtCursor,
}

/// Font settings of a terminal view.
#[derive(Debug, Clone, PartialEq)]
pub struct FontConfig {
    /// Font family.
    pub family: SharedString,
    /// Fallback families tried when a glyph is missing or the family is not installed.
    pub fallbacks: Vec<String>,
    /// Size in logical pixels.
    pub size: f32,
    /// Ligature mode.
    pub ligatures: Ligatures,
    /// Line height as a multiple of the font's natural height.
    pub line_height: f32,
    /// Weight of normal text.
    pub weight: f32,
    /// Weight of bold text.
    pub bold_weight: f32,
    /// Added to the cell width, in logical pixels.
    pub cell_width_adjust: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "UDEV Gothic 35NFLG".into(),
            fallbacks: vec!["UDEV Gothic NFLG".into(), "Cascadia Mono".into(), "Consolas".into()],
            size: 12.,
            ligatures: Ligatures::default(),
            line_height: 1.,
            weight: FontWeight::NORMAL.0,
            bold_weight: FontWeight::BOLD.0,
            cell_width_adjust: 0.,
        }
    }
}

/// The four faces used for cell text.
#[derive(Debug, Clone)]
pub(crate) struct FontSet {
    faces: [Font; 4],
}

impl FontSet {
    pub(crate) fn new(config: &FontConfig) -> Self {
        let base = {
            let mut f = font(config.family.clone());
            f.fallbacks = Some(gpui::FontFallbacks::from_fonts(config.fallbacks.clone()));
            if config.ligatures == Ligatures::Off {
                f.features = FontFeatures(Arc::new(vec![("calt".into(), 0), ("liga".into(), 0)]));
            }
            f
        };
        let face = |weight, style| {
            let mut f = base.clone();
            f.weight = weight;
            f.style = style;
            f
        };
        Self {
            faces: [
                face(FontWeight(config.weight), FontStyle::Normal),
                face(FontWeight(config.bold_weight), FontStyle::Normal),
                face(FontWeight(config.weight), FontStyle::Italic),
                face(FontWeight(config.bold_weight), FontStyle::Italic),
            ],
        }
    }

    pub(crate) fn regular(&self) -> &Font {
        &self.faces[0]
    }

    fn face_index(flags: CellFlags) -> usize {
        usize::from(flags.contains(CellFlags::BOLD)) | (usize::from(flags.contains(CellFlags::ITALIC)) << 1)
    }
}

/// Cell geometry, snapped to device pixels so glyphs do not jitter between cells.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Metrics {
    pub(crate) cell_width: Pixels,
    pub(crate) cell_height: Pixels,
    pub(crate) baseline: Pixels,
    pub(crate) font_size: Pixels,
    pub(crate) line_thickness: Pixels,
    /// `line_thickness` in device pixels.
    pub(crate) line_thickness_px: u32,
}

fn snap(value: f32, scale: f32) -> Pixels {
    px((value * scale).round() / scale)
}

impl Metrics {
    pub(crate) fn measure(window: &Window, fonts: &FontSet, font_size: Pixels, config: &FontConfig) -> Self {
        let line_height = config.line_height;
        let text_system = window.text_system();
        let id = text_system.resolve_font(fonts.regular());
        let scale = window.scale_factor();
        let advance = text_system.advance(id, font_size, 'M').map_or(font_size * 0.6, |s| s.width);
        let ascent = f32::from(text_system.ascent(id, font_size));
        let descent = f32::from(text_system.descent(id, font_size)).abs();
        let line_thickness_px = scale.round().max(1.) as u32;
        Self {
            cell_width: snap((f32::from(advance) + config.cell_width_adjust).max(1.), scale).max(px(1. / scale)),
            cell_height: px(((ascent + descent) * line_height * scale).ceil() / scale),
            // Extra line spacing is split evenly above and below the glyphs.
            baseline: snap(ascent + (ascent + descent) * (line_height - 1.) / 2., scale),
            font_size,
            line_thickness: px(line_thickness_px as f32 / scale),
            line_thickness_px,
        }
    }
}

pub(crate) fn to_hsla(c: Rgb) -> Hsla {
    Rgba { r: f32::from(c.r) / 255., g: f32::from(c.g) / 255., b: f32::from(c.b) / 255., a: 1. }.into()
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Painter<'a> {
    pub(crate) colors: &'a Colors,
    pub(crate) view_colors: ViewColors,
    pub(crate) fonts: &'a FontSet,
    pub(crate) metrics: &'a Metrics,
    /// Multiplies the opacity of glyphs and decorations (not of backgrounds).
    pub(crate) text_alpha: f32,
    /// Multiplies the opacity of the cursor.
    pub(crate) cursor_alpha: f32,
}

/// Foreground and background a cell is drawn with, after inverse video.
fn cell_colors(cell: &Cell, colors: &Colors) -> (Rgb, Option<Rgb>) {
    let fg = cell.fg.unwrap_or(colors.foreground);
    if cell.flags.contains(CellFlags::INVERSE) {
        (cell.bg.unwrap_or(colors.background), Some(fg))
    } else {
        (fg, cell.bg)
    }
}

/// Columns a cell's glyph spans.
fn cell_span(width: CellWidth) -> f32 {
    if width == CellWidth::Wide { 2. } else { 1. }
}

const SELECTION_ALPHA: f32 = 0.35;
const FAINT_ALPHA: f32 = 0.6;

impl Painter<'_> {
    fn faded(&self, color: Hsla) -> Hsla {
        Hsla { a: color.a * self.text_alpha, ..color }
    }

    fn faded_cursor(&self, color: Hsla) -> Hsla {
        Hsla { a: color.a * self.cursor_alpha, ..color }
    }

    fn text_color(&self, cell: &Cell) -> Rgb {
        match self.view_colors.selection_foreground {
            Some(color) if cell.is_selected => color,
            _ => cell_colors(cell, self.colors).0,
        }
    }

    fn glyph_color(&self, cell: &Cell) -> Option<Hsla> {
        if cell.flags.contains(CellFlags::INVISIBLE) {
            return None;
        }
        let mut color = to_hsla(self.text_color(cell));
        if cell.flags.contains(CellFlags::FAINT) {
            color.a = FAINT_ALPHA;
        }
        Some(self.faded(color))
    }

    fn cell_rect(&self, origin: Point<Pixels>, col: usize, cells: f32) -> Bounds<Pixels> {
        let m = self.metrics;
        Bounds::new(point(origin.x + m.cell_width * col as f32, origin.y), size(m.cell_width * cells, m.cell_height))
    }

    /// Paints cell backgrounds (merged into spans of equal color) and the selection.
    pub(crate) fn paint_backgrounds(&self, window: &mut Window, origin: Point<Pixels>, row: &Row) {
        let mut span: Option<(usize, usize, Rgb)> = None;
        let flush = |window: &mut Window, span: Option<(usize, usize, Rgb)>| {
            if let Some((start, end, color)) = span {
                window.paint_quad(fill(self.cell_rect(origin, start, (end - start) as f32), to_hsla(color)));
            }
        };
        for (col, cell) in row.cells.iter().enumerate() {
            match (span, cell_colors(cell, self.colors).1) {
                (Some((start, end, color)), Some(bg)) if end == col && color == bg => {
                    span = Some((start, col + 1, color))
                }
                (_, Some(bg)) => {
                    flush(window, span);
                    span = Some((col, col + 1, bg));
                }
                (_, None) => flush(window, span.take()),
            }
        }
        flush(window, span);

        let selection = self
            .view_colors
            .selection_background
            .map_or_else(|| Hsla { a: SELECTION_ALPHA, ..to_hsla(self.colors.foreground) }, to_hsla);
        for (col, cell) in row.cells.iter().enumerate() {
            if cell.is_selected {
                window.paint_quad(fill(self.cell_rect(origin, col, 1.), selection));
            }
        }
    }

    /// Paints the glyphs of one row. `break_at` splits runs around that column (ligature break
    /// at cursor); `override_color` paints every glyph in one color (text under a block cursor).
    pub(crate) fn paint_text(
        &self,
        window: &mut Window,
        cx: &App,
        origin: Point<Pixels>,
        row: &Row,
        break_at: Option<usize>,
        override_color: Option<Hsla>,
    ) {
        let cells = &row.cells;
        let mut start: Option<(usize, usize)> = None; // (first col, face index)
        for col in 0..=cells.len() {
            let cell = cells.get(col);
            let text = cell.map_or("", |c| row.cell_text(c));
            let is_custom = custom_glyphs::is_custom(text);
            if let Some(c) = cell
                && is_custom
                && let Some(color) = override_color.or_else(|| self.glyph_color(c))
            {
                let rect = self.cell_rect(origin, col, cell_span(c.width));
                custom_glyphs::paint(window, cx, text, rect, color, self.metrics.line_thickness_px);
            }
            let is_break_cell = cell.is_none_or(|c| c.width.is_spacer() || text.is_empty() || text == " ") || is_custom;
            let face = cell.map_or(0, |c| FontSet::face_index(c.flags));
            let at_cursor = break_at.is_some_and(|b| col == b || col == b + 1);
            if let Some((first, run_face)) = start
                && (is_break_cell || at_cursor || face != run_face)
            {
                self.paint_run(window, origin, row, first..col, run_face, override_color);
                start = None;
            }
            if !is_break_cell && start.is_none() {
                start = Some((col, face));
            }
        }
    }

    fn paint_run(
        &self,
        window: &mut Window,
        origin: Point<Pixels>,
        row: &Row,
        cols: std::ops::Range<usize>,
        face: usize,
        override_color: Option<Hsla>,
    ) {
        let m = self.metrics;
        let mut text = String::new();
        let mut byte_to_col: Vec<(usize, usize)> = Vec::new();
        for (col, cell) in row.cells.iter().enumerate().take(cols.end).skip(cols.start) {
            if cell.width.is_spacer() {
                continue;
            }
            byte_to_col.push((text.len(), col));
            text.push_str(row.cell_text(cell));
        }
        if text.is_empty() {
            return;
        }
        let run = TextRun {
            len: text.len(),
            font: self.fonts.faces[face].clone(),
            color: gpui::white(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window.text_system().shape_line(text.into(), m.font_size, &[run], None);

        let mut glyphs = Vec::new();
        for shaped_run in &shaped.runs {
            for glyph in &shaped_run.glyphs {
                let cluster = byte_to_col.partition_point(|(b, _)| *b <= glyph.index).saturating_sub(1);
                glyphs.push((shaped_run.font_id, glyph, cluster));
            }
        }
        let mut cluster_x: Vec<Option<Pixels>> = vec![None; byte_to_col.len()];
        for &(_, glyph, cluster) in &glyphs {
            cluster_x[cluster].get_or_insert(glyph.position.x);
        }
        for &(font_id, glyph, cluster) in &glyphs {
            let col = byte_to_col[cluster].1;
            let cell = &row.cells[col];
            let Some(color) = override_color.or_else(|| self.glyph_color(cell)) else {
                continue;
            };
            let cluster_start = cluster_x[cluster].unwrap_or(glyph.position.x);
            let mut x = origin.x + m.cell_width * col as f32 + (glyph.position.x - cluster_start);
            if cell.width == CellWidth::Wide {
                // Center wide glyphs in their two cells (fonts whose full width != 2 half widths).
                let next = cluster_x.get(cluster + 1).copied().flatten().unwrap_or(shaped.width);
                x += (m.cell_width * 2. - (next - cluster_start)) / 2.;
            }
            let position = point(x, origin.y + m.baseline);
            let _ = if glyph.is_emoji {
                window.paint_emoji(position, font_id, glyph.id, m.font_size)
            } else {
                window.paint_glyph(position, font_id, glyph.id, m.font_size, color)
            };
        }
    }

    pub(crate) fn paint_decorations(&self, window: &mut Window, origin: Point<Pixels>, row: &Row) {
        let m = self.metrics;
        let t = m.line_thickness;
        for (col, cell) in row.cells.iter().enumerate() {
            if cell.width.is_spacer() || cell.flags.contains(CellFlags::INVISIBLE) {
                continue;
            }
            let rect = self.cell_rect(origin, col, cell_span(cell.width));
            let fg = to_hsla(self.text_color(cell));
            let line = |window: &mut Window, y: Pixels, color: Hsla| {
                let bounds = Bounds::new(point(rect.origin.x, y), size(rect.size.width, t));
                window.paint_quad(fill(bounds, self.faded(color)));
            };
            if cell.underline != Underline::None {
                let color = cell.underline_color.map_or(fg, to_hsla);
                let y = origin.y + m.baseline + t * 2.;
                line(window, y, color);
                if cell.underline == Underline::Double {
                    line(window, y + t * 2., color);
                }
            }
            if cell.flags.contains(CellFlags::STRIKETHROUGH) {
                line(window, origin.y + m.cell_height / 2., fg);
            }
            if cell.flags.contains(CellFlags::OVERLINE) {
                line(window, origin.y, fg);
            }
        }
    }

    /// Underlines the `(row, start col, end col)` segments of the hovered link.
    pub(crate) fn paint_link_underline(
        &self,
        window: &mut Window,
        origin: Point<Pixels>,
        segments: &[(u16, u16, u16)],
    ) {
        let m = self.metrics;
        let color = Hsla { a: self.text_alpha, ..to_hsla(self.colors.foreground) };
        for &(row, start, end) in segments {
            let underline = Bounds::new(
                point(
                    origin.x + m.cell_width * f32::from(start),
                    origin.y + m.cell_height * f32::from(row + 1) - m.line_thickness * 2.,
                ),
                size(m.cell_width * f32::from(end - start), m.line_thickness),
            );
            window.paint_quad(fill(underline, color));
        }
    }

    /// Draws the preedit over the grid from the anchor cell, wrapping at the line end, with an
    /// opaque background, an underline and a caret.
    pub(crate) fn paint_preedit(
        &self,
        window: &mut Window,
        cx: &App,
        origin: Point<Pixels>,
        ime: &ImeState,
        anchor_col: usize,
        cols: usize,
    ) {
        let m = self.metrics;
        let background = to_hsla(self.colors.background);
        let foreground = to_hsla(self.colors.foreground);
        for (i, (row, span)) in ime::overlay_rows(&ime.preedit, anchor_col, cols).iter().enumerate() {
            let row_origin = point(origin.x, origin.y + m.cell_height * i as f32);
            let span_bounds = Bounds::new(
                point(row_origin.x + m.cell_width * span.start as f32, row_origin.y),
                size(m.cell_width * span.len() as f32, m.cell_height),
            );
            window.paint_quad(fill(span_bounds, background));
            self.paint_text(window, cx, row_origin, row, None, None);
            let underline = Bounds::new(
                point(span_bounds.origin.x, row_origin.y + m.cell_height - m.line_thickness * 2.),
                size(span_bounds.size.width, m.line_thickness),
            );
            window.paint_quad(fill(underline, foreground));
        }
        let (row, col) = ime::position_of(&ime.preedit, ime.caret_utf16, anchor_col, cols);
        let caret = Bounds::new(
            point(origin.x + m.cell_width * col as f32, origin.y + m.cell_height * row as f32),
            size(m.line_thickness * 2., m.cell_height),
        );
        window.paint_quad(fill(caret, foreground));
    }

    /// Paints the cursor in `cursor.style`.
    pub(crate) fn paint_cursor(
        &self,
        window: &mut Window,
        cx: &App,
        origin: Point<Pixels>,
        snapshot: &GridSnapshot,
        cursor: Cursor,
        break_at: Option<usize>,
    ) {
        let m = self.metrics;
        let Some(row) = snapshot.rows.get(usize::from(cursor.row)) else { return };
        let row_origin = point(origin.x, origin.y + m.cell_height * f32::from(cursor.row));
        let is_wide =
            cursor.is_wide || row.cells.get(usize::from(cursor.col)).is_some_and(|c| c.width == CellWidth::Wide);
        let rect = self.cell_rect(row_origin, usize::from(cursor.col), if is_wide { 2. } else { 1. });
        let color = self.faded_cursor(to_hsla(self.colors.cursor.unwrap_or(self.colors.foreground)));
        let t = m.line_thickness;
        match cursor.style {
            CursorStyle::Block => {
                window.paint_quad(fill(rect, color));
                let text = self.view_colors.cursor_text.unwrap_or(self.colors.background);
                let text_color = self.faded_cursor(to_hsla(text));
                window.with_content_mask(Some(ContentMask { bounds: rect }), |window| {
                    self.paint_text(window, cx, row_origin, row, break_at, Some(text_color));
                });
            }
            CursorStyle::Bar => window.paint_quad(fill(Bounds::new(rect.origin, size(t * 2., m.cell_height)), color)),
            CursorStyle::Underline => window.paint_quad(fill(
                Bounds::new(
                    point(rect.origin.x, rect.origin.y + m.cell_height - t * 2.),
                    size(rect.size.width, t * 2.),
                ),
                color,
            )),
            CursorStyle::BlockHollow => {
                let (o, s) = (rect.origin, rect.size);
                window.paint_quad(fill(Bounds::new(o, size(s.width, t)), color));
                window.paint_quad(fill(Bounds::new(point(o.x, o.y + s.height - t), size(s.width, t)), color));
                window.paint_quad(fill(Bounds::new(o, size(t, s.height)), color));
                window.paint_quad(fill(Bounds::new(point(o.x + s.width - t, o.y), size(t, s.height)), color));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_glyphs_span_two_columns() {
        assert_eq!(cell_span(CellWidth::Wide), 2.);
        assert_eq!(cell_span(CellWidth::Narrow), 1.);
    }
}
