//! IME composition state and its layout on the grid.

use unicode_width::UnicodeWidthChar;
use wired_vt::{Cell, CellWidth, Row};

/// The current composition (preedit). Empty when not composing.
#[derive(Debug, Clone, Default)]
pub(crate) struct ImeState {
    pub(crate) preedit: String,
    /// Caret inside the preedit as a UTF-16 offset (the unit the platform IME uses).
    pub(crate) caret_utf16: usize,
}

impl ImeState {
    pub(crate) fn is_composing(&self) -> bool {
        !self.preedit.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.preedit.clear();
        self.caret_utf16 = 0;
    }
}

pub(crate) fn len_utf16(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Byte offset of UTF-16 offset `offset_utf16` in `s` (clamped to the end).
pub(crate) fn byte_offset(s: &str, offset_utf16: usize) -> usize {
    let mut units = 0;
    for (byte, ch) in s.char_indices() {
        if units >= offset_utf16 {
            return byte;
        }
        units += ch.len_utf16();
    }
    s.len()
}

fn char_width(ch: char) -> usize {
    ch.width().unwrap_or(1).clamp(1, 2)
}

/// Cell position `(row offset, col)` of the char at UTF-16 offset `offset_utf16` of `text` laid out
/// from `start_col` on a grid `cols` wide. Wide chars that do not fit move to the next row.
pub(crate) fn position_of(text: &str, offset_utf16: usize, start_col: usize, cols: usize) -> (usize, usize) {
    let cols = cols.max(2);
    let prefix_len = byte_offset(text, offset_utf16);
    let (mut row, mut col) = (0, start_col.min(cols - 1));
    for ch in text[..prefix_len].chars() {
        let w = char_width(ch);
        if col + w > cols {
            row += 1;
            col = 0;
        }
        col += w;
    }
    let next_width = text[prefix_len..].chars().next().map_or(1, char_width);
    if col + next_width > cols {
        row += 1;
        col = 0;
    }
    (row, col)
}

/// Number of overlay rows the preedit takes from `start_col` (the same wrapping as
/// [`overlay_rows`]).
pub(crate) fn row_count(text: &str, start_col: usize, cols: usize) -> usize {
    let cols = cols.max(2);
    let (mut rows, mut col) = (1, start_col.min(cols - 1));
    for ch in text.chars() {
        let w = char_width(ch);
        if col + w > cols {
            rows += 1;
            col = 0;
        }
        col += w;
    }
    rows
}

/// Top row of the overlay: the anchor row, moved up just enough that the last overlay row is
/// the grid's last row. Only the overlay moves; the cursor row stays where it is.
pub(crate) fn overlay_top(anchor_row: usize, overlay_rows: usize, grid_rows: usize) -> usize {
    anchor_row.min(grid_rows.saturating_sub(overlay_rows))
}

/// Lays the preedit out as overlay rows starting at `start_col`; each row has `cols` cells, with
/// blank cells where the overlay draws nothing.
pub(crate) fn overlay_rows(text: &str, start_col: usize, cols: usize) -> Vec<(Row, std::ops::Range<usize>)> {
    let cols = cols.max(2);
    let mut rows: Vec<(Row, std::ops::Range<usize>)> = Vec::new();
    let new_row = |start: usize| (Row { cells: vec![Cell::default(); cols], ..Row::default() }, start..start);
    let mut current = new_row(start_col.min(cols - 1));
    let mut col = current.1.start;
    for ch in text.chars() {
        let w = char_width(ch);
        if col + w > cols {
            rows.push(current);
            current = new_row(0);
            col = 0;
        }
        let (row, span) = &mut current;
        let text_start = u32::try_from(row.text.len()).unwrap_or(u32::MAX);
        row.text.push(ch);
        row.cells[col] = Cell {
            text_start,
            text_len: u16::try_from(ch.len_utf8()).unwrap_or(0),
            width: if w == 2 { CellWidth::Wide } else { CellWidth::Narrow },
            ..Cell::default()
        };
        if w == 2 {
            row.cells[col + 1] = Cell { width: CellWidth::SpacerTail, ..Cell::default() };
        }
        col += w;
        span.end = col;
    }
    rows.push(current);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_offsets_map_to_bytes() {
        let s = "a日😀b";
        assert_eq!(byte_offset(s, 0), 0);
        assert_eq!(byte_offset(s, 1), 1);
        assert_eq!(byte_offset(s, 2), 4);
        assert_eq!(byte_offset(s, 4), 8);
        assert_eq!(byte_offset(s, 99), s.len());
    }

    #[test]
    fn wide_char_wraps_instead_of_splitting() {
        // 3 columns left on the row: "日本" needs 4, so "本" wraps.
        assert_eq!(position_of("日本", 0, 7, 10), (0, 7));
        assert_eq!(position_of("日本", 1, 7, 10), (1, 0));
        assert_eq!(position_of("日本語", 2, 7, 10), (1, 2));
    }

    #[test]
    fn overlay_moves_up_at_the_bottom_edge() {
        // Two overlay rows anchored on the last of 5 rows: drawn on rows 3 and 4.
        assert_eq!(overlay_top(4, 2, 5), 3);
        assert_eq!(overlay_top(1, 2, 5), 1);
        // Taller than the grid: starts at the top.
        assert_eq!(overlay_top(4, 9, 5), 0);
    }

    #[test]
    fn overlay_wraps_at_line_end() {
        assert_eq!(row_count("あいう", 6, 10), 2);
        assert_eq!(row_count("", 9, 10), 1);
        let rows = overlay_rows("あいう", 6, 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].1, 6..10);
        assert_eq!(rows[1].1, 0..2);
    }
}
