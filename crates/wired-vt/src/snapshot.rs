/// A 24-bit color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rgb {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

impl Rgb {
    /// Creates a color from its components.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// How much horizontal space a cell takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CellWidth {
    /// One cell.
    #[default]
    Narrow,
    /// First cell of a two-cell character.
    Wide,
    /// Second cell of a wide character; draws nothing.
    SpacerTail,
    /// Empty last cell of a row whose wide character wrapped to the next row; draws nothing.
    SpacerHead,
}

impl CellWidth {
    /// Returns `true` for cells that carry no glyph of their own.
    pub fn is_spacer(self) -> bool {
        matches!(self, Self::SpacerTail | Self::SpacerHead)
    }
}

/// Underline style (SGR 4 and its subparameters).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Underline {
    /// No underline.
    #[default]
    None,
    /// Single line.
    Single,
    /// Double line.
    Double,
    /// Curly (wavy) line.
    Curly,
    /// Dotted line.
    Dotted,
    /// Dashed line.
    Dashed,
}

/// Text attributes of a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CellFlags(u8);

impl CellFlags {
    /// Bold.
    pub const BOLD: Self = Self(1 << 0);
    /// Italic.
    pub const ITALIC: Self = Self(1 << 1);
    /// Faint (dim).
    pub const FAINT: Self = Self(1 << 2);
    /// Blink.
    pub const BLINK: Self = Self(1 << 3);
    /// Inverse video.
    pub const INVERSE: Self = Self(1 << 4);
    /// Invisible.
    pub const INVISIBLE: Self = Self(1 << 5);
    /// Strikethrough.
    pub const STRIKETHROUGH: Self = Self(1 << 6);
    /// Overline.
    pub const OVERLINE: Self = Self(1 << 7);

    pub(crate) const fn from_bits_truncate(bits: u16) -> Self {
        Self((bits & 0xff) as u8)
    }

    /// Returns `true` if all flags in `other` are set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// One cell of a [`Row`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cell {
    /// Byte range of the cell's grapheme in [`Row::text`]; empty for blank and spacer cells.
    pub text_start: u32,
    /// Length in bytes of the cell's grapheme in [`Row::text`].
    pub text_len: u16,
    /// Width class of the cell.
    pub width: CellWidth,
    /// Foreground color set by the program; `None` = default foreground.
    pub fg: Option<Rgb>,
    /// Background color set by the program; `None` = default background.
    pub bg: Option<Rgb>,
    /// Underline color set by the program; `None` = foreground color.
    pub underline_color: Option<Rgb>,
    /// Text attributes.
    pub flags: CellFlags,
    /// Underline style.
    pub underline: Underline,
    /// Whether the cell is part of the selection.
    pub is_selected: bool,
    /// Whether the program attached a hyperlink (OSC 8) to the cell; see [`crate::Vt::hyperlink_uri`].
    pub has_hyperlink: bool,
}

/// One viewport row.
#[derive(Debug, Clone, Default)]
pub struct Row {
    /// Concatenated graphemes of all cells (see [`Cell::text_start`]).
    pub text: String,
    /// Exactly `cols` cells.
    pub cells: Vec<Cell>,
    /// The line continues on the next row (soft wrap).
    pub is_wrapped: bool,
    /// The row continues the line of the row above (soft wrap), which may be above the viewport.
    pub is_wrap_continuation: bool,
}

impl Row {
    /// Returns the grapheme of `cell` (empty for blanks and spacers).
    pub fn cell_text(&self, cell: &Cell) -> &str {
        let start = cell.text_start as usize;
        &self.text[start..start + cell.text_len as usize]
    }
}

/// Cursor shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CursorStyle {
    /// Filled block.
    #[default]
    Block,
    /// Vertical bar at the left edge of the cell.
    Bar,
    /// Line at the bottom of the cell.
    Underline,
    /// Block outline.
    BlockHollow,
}

/// Cursor position and look, present only when the cursor is visible in the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cursor {
    /// Column of the cell the cursor covers (the head cell for wide characters).
    pub col: u16,
    /// Viewport row.
    pub row: u16,
    /// Shape requested by the program (DECSCUSR) or the default.
    pub style: CursorStyle,
    /// Whether the cursor should blink.
    pub is_blinking: bool,
    /// Whether the cursor covers a wide character (two cells).
    pub is_wide: bool,
}

/// Terminal colors in effect for the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Colors {
    /// Default foreground.
    pub foreground: Rgb,
    /// Default background.
    pub background: Rgb,
    /// Cursor color set by the program (OSC 12); `None` = renderer default.
    pub cursor: Option<Rgb>,
}

/// Scrollback position of the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Scrollbar {
    /// Total rows (scrollback + screen).
    pub total: u64,
    /// First visible row.
    pub offset: u64,
    /// Visible rows.
    pub len: u64,
}

impl Scrollbar {
    /// Returns `true` when the viewport shows the bottom of the scrollback.
    pub fn is_at_bottom(&self) -> bool {
        self.offset + self.len >= self.total
    }
}

/// Everything the renderer needs to draw one frame of the viewport.
#[derive(Debug, Clone, Default)]
pub struct GridSnapshot {
    /// Columns.
    pub cols: u16,
    /// Viewport rows; `rows.len() == rows` count.
    pub rows: Vec<Row>,
    /// Cursor, if visible within the viewport.
    pub cursor: Option<Cursor>,
    /// Colors in effect.
    pub colors: Colors,
    /// Viewport position in the scrollback.
    pub scrollbar: Scrollbar,
    /// [`crate::Vt::generation`] when this was taken: the state any further reads must match.
    pub generation: u64,
}
