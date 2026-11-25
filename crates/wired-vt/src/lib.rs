//! Safe wrapper around libghostty-vt.

mod key;
mod mouse;
mod snapshot;

use std::{
    ffi::c_void,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};

use wired_vt_sys as sys;

pub use key::{Key, KeyInput, Mods};
pub use mouse::{Autoscroll, GridPos, MouseAction, MouseButton};
pub use snapshot::{
    Cell, CellFlags, CellWidth, Colors, Cursor, CursorStyle, GridSnapshot, Rgb, Row, Scrollbar, Underline,
};

/// Errors from libghostty calls.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A libghostty function returned an error code.
    #[error("libghostty {op} failed with code {code}")]
    Ghostty {
        /// The operation that failed.
        op: &'static str,
        /// The `GhosttyResult` code.
        code: i32,
    },
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

fn check(code: sys::GhosttyResult, op: &'static str) -> Result<()> {
    if code == sys::GHOSTTY_SUCCESS { Ok(()) } else { Err(Error::Ghostty { op, code }) }
}

/// Settings applied when the terminal is created.
#[derive(Debug, Clone, Default)]
pub struct VtOptions {
    /// Scrollback limit in lines; also bounds memory.
    pub scrollback_lines: usize,
    /// Colors in effect until the program changes them.
    pub colors: DefaultColors,
}

/// The colors a terminal starts from; programs may override them (OSC 4, 10, 11, 12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultColors {
    /// Default foreground.
    pub foreground: Rgb,
    /// Default background.
    pub background: Rgb,
    /// Cursor color; `None` leaves it to the renderer.
    pub cursor: Option<Rgb>,
    /// The 256-color palette.
    pub palette: [Rgb; 256],
}

impl Default for DefaultColors {
    fn default() -> Self {
        Self {
            foreground: Rgb::new(0xc0, 0xca, 0xf5),
            background: Rgb::new(0x14, 0x15, 0x1a),
            cursor: None,
            palette: default_palette(),
        }
    }
}

/// libghostty's built-in 256-color palette.
pub fn default_palette() -> [Rgb; 256] {
    let mut raw = [sys::GhosttyColorRgb { r: 0, g: 0, b: 0 }; 256];
    // SAFETY: the function writes exactly 256 entries to `out`, which `raw` holds.
    unsafe { sys::ghostty_color_palette_default(raw.as_mut_ptr()) };
    raw.map(from_ghostty_rgb)
}

/// How to move the viewport through the scrollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    /// Oldest line.
    Top,
    /// Live screen.
    Bottom,
    /// Relative rows; negative is up.
    Delta(isize),
}

const MAX_TITLE_CHARS: usize = 256;

#[derive(Debug, Default)]
struct CallbackState {
    replies: Vec<u8>,
    is_title_changed: bool,
    is_bell: bool,
}

/// A libghostty terminal with its render and encoding helpers.
#[derive(Debug)]
pub struct Vt {
    terminal: sys::GhosttyTerminal,
    render_state: sys::GhosttyRenderState,
    row_iter: sys::GhosttyRenderStateRowIterator,
    row_cells: sys::GhosttyRenderStateRowCells,
    key_encoder: sys::GhosttyKeyEncoder,
    key_event: sys::GhosttyKeyEvent,
    mouse: mouse::MouseHandles,
    // Never 0: `new` and `resize` clamp the size and cell pixels to at least 1.
    cols: u16,
    rows: u16,
    cell_width_px: u32,
    cell_height_px: u32,
    scrollback_lines: usize,
    generation: u64,
    /// Owned via `Box::into_raw`; freed in `Drop`. libghostty writes through it during calls
    /// that take `&mut self`, so no Rust reference to it is alive while C runs.
    callbacks: *mut CallbackState,
}

// SAFETY: libghostty objects are plain heap data with no thread affinity; every access goes
// through `&mut self` (or `&self` for pure reads), so moving the owner to another thread is fine.
unsafe impl Send for Vt {}

unsafe extern "C" fn on_write_pty(_t: sys::GhosttyTerminal, userdata: *mut c_void, data: *const u8, len: usize) {
    if userdata.is_null() || data.is_null() {
        return;
    }
    // SAFETY: `userdata` is the `CallbackState` pointer installed in `Vt::new`, alive for the life
    // of the terminal; libghostty calls back synchronously inside a `&mut Vt` method, and `data`
    // is valid for `len` bytes for the duration of the call.
    unsafe {
        let state = &mut *userdata.cast::<CallbackState>();
        state.replies.extend_from_slice(std::slice::from_raw_parts(data, len));
    }
}

unsafe extern "C" fn on_title_changed(_t: sys::GhosttyTerminal, userdata: *mut c_void) {
    if userdata.is_null() {
        return;
    }
    // SAFETY: see `on_write_pty`.
    unsafe { (*userdata.cast::<CallbackState>()).is_title_changed = true };
}

unsafe extern "C" fn on_bell(_t: sys::GhosttyTerminal, userdata: *mut c_void) {
    if userdata.is_null() {
        return;
    }
    // SAFETY: see `on_write_pty`.
    unsafe { (*userdata.cast::<CallbackState>()).is_bell = true };
}

const UNDERLINE_SHIFT: u16 = 8;
const UNDERLINE_MASK: u16 = 0x7;

/// Cells in one libghostty page (its standard capacity, 215 x 215); a page holds about this many
/// cells whatever the width.
const PAGE_CELLS: usize = 215 * 215;

/// libghostty drops history a whole page at a time once the line limit is passed, so the limit
/// gets one page of rows (plus a quarter for the estimate) on top of the lines that must survive.
fn history_limit(lines: usize, cols: u16) -> usize {
    if lines == 0 {
        return 0;
    }
    let page_rows = PAGE_CELLS.div_ceil(usize::from(cols.max(1)));
    lines.saturating_add(page_rows + page_rows / 4)
}

impl Vt {
    /// Creates a terminal of `cols` x `rows` cells.
    pub fn new(cols: u16, rows: u16, options: &VtOptions) -> Result<Self> {
        let mut vt = Self {
            terminal: null_mut(),
            render_state: null_mut(),
            row_iter: null_mut(),
            row_cells: null_mut(),
            key_encoder: null_mut(),
            key_event: null_mut(),
            mouse: mouse::MouseHandles::null(),
            cols: cols.max(1),
            rows: rows.max(1),
            cell_width_px: 1,
            cell_height_px: 1,
            scrollback_lines: options.scrollback_lines,
            generation: 0,
            callbacks: Box::into_raw(Box::default()),
        };
        // SAFETY: each constructor writes a fresh handle into the out-pointer; on error `vt` is
        // dropped and `Drop` frees whatever was created (null handles are skipped).
        unsafe {
            check(sys::ghostty_terminal_new(null(), &mut vt.terminal, vt.cols, vt.rows), "terminal_new")?;
            check(sys::ghostty_render_state_new(null(), &mut vt.render_state), "render_state_new")?;
            check(sys::ghostty_render_state_row_iterator_new(null(), &mut vt.row_iter), "row_iterator_new")?;
            check(sys::ghostty_render_state_row_cells_new(null(), &mut vt.row_cells), "row_cells_new")?;
            check(sys::ghostty_key_encoder_new(null(), &mut vt.key_encoder), "key_encoder_new")?;
            check(sys::ghostty_key_event_new(null(), &mut vt.key_event), "key_event_new")?;
            vt.mouse.create()?;
        }
        vt.configure(options)?;
        Ok(vt)
    }

    fn configure(&mut self, options: &VtOptions) -> Result<()> {
        let write_pty: sys::GhosttyTerminalWritePtyFn = Some(on_write_pty);
        let title_changed: sys::GhosttyTerminalTitleChangedFn = Some(on_title_changed);
        let bell: sys::GhosttyTerminalBellFn = Some(on_bell);
        let scrollback = history_limit(options.scrollback_lines, self.cols);
        // libghostty also caps history by bytes, by default to one page (a few hundred lines); the
        // line limit alone should decide.
        let scrollback_bytes = usize::MAX;
        // Like Ghostty: the default cursor (DECSCUSR 0) blinks and DEC mode 12 is then ignored; the
        // view decides whether to show the blink.
        let default_blink = true;
        // Deliberately not installed: title report (reply injection), clipboard read/write,
        // desktop notifications. Without callbacks libghostty ignores those requests.
        // SAFETY: userdata and callback options take the pointer itself (documented in terminal.h);
        // function pointers are `'static`.
        unsafe {
            let set = |option, value: *const c_void| sys::ghostty_terminal_set(self.terminal, option, value);
            check(set(sys::GHOSTTY_TERMINAL_OPT_USERDATA, self.callbacks.cast_const().cast()), "set userdata")?;
            check(
                set(sys::GHOSTTY_TERMINAL_OPT_WRITE_PTY, write_pty.map_or(null(), |f| f as *const c_void)),
                "set write_pty",
            )?;
            check(
                set(sys::GHOSTTY_TERMINAL_OPT_TITLE_CHANGED, title_changed.map_or(null(), |f| f as *const c_void)),
                "set title_changed",
            )?;
            check(set(sys::GHOSTTY_TERMINAL_OPT_BELL, bell.map_or(null(), |f| f as *const c_void)), "set bell")?;
        }
        self.set_colors(&options.colors)?;
        self.set_option(sys::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES, &scrollback, "set scrollback")?;
        self.set_option(sys::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES, &scrollback_bytes, "set scrollback bytes")?;
        self.set_option(sys::GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_BLINK, &default_blink, "set default cursor blink")
    }

    /// `T` must be the type libghostty documents for `option`.
    fn set_option<T>(&mut self, option: sys::GhosttyTerminalOption, value: &T, op: &'static str) -> Result<()> {
        // SAFETY: valid handle; libghostty copies the value during the call.
        check(unsafe { sys::ghostty_terminal_set(self.terminal, option, (&raw const *value).cast()) }, op)
    }

    fn callbacks(&mut self) -> &mut CallbackState {
        // SAFETY: the pointer comes from `Box::into_raw` and lives until `Drop`; libghostty only
        // writes through it inside our own `&mut self` calls, which are not running now.
        unsafe { &mut *self.callbacks }
    }

    /// Feeds program output (untrusted bytes) into the terminal.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.generation += 1;
        // SAFETY: valid handle; `bytes` is valid for its length.
        unsafe { sys::ghostty_terminal_vt_write(self.terminal, bytes.as_ptr(), bytes.len()) };
    }

    /// Takes the bytes libghostty wants written back to the program (DSR, DA, ...).
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.callbacks().replies)
    }

    /// Returns the new title if the program changed it since the last call.
    pub fn take_title_change(&mut self) -> Option<String> {
        if !std::mem::take(&mut self.callbacks().is_title_changed) {
            return None;
        }
        Some(self.title())
    }

    /// Returns `true` once per bell (BEL) received since the last call.
    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.callbacks().is_bell)
    }

    fn title(&self) -> String {
        // SAFETY: zeroed is a valid empty GhosttyString.
        let mut s: sys::GhosttyString = unsafe { zeroed() };
        // SAFETY: DATA_TITLE writes a borrowed string valid until the next mutating call; we copy
        // it before returning.
        let raw = unsafe {
            if sys::ghostty_terminal_get(self.terminal, sys::GHOSTTY_TERMINAL_DATA_TITLE, (&raw mut s).cast())
                != sys::GHOSTTY_SUCCESS
                || s.ptr.is_null()
            {
                return String::new();
            }
            std::slice::from_raw_parts(s.ptr, s.len)
        };
        sanitize_title(&String::from_utf8_lossy(raw))
    }

    /// Changes the default colors: foreground, background, cursor and palette.
    pub fn set_colors(&mut self, colors: &DefaultColors) -> Result<()> {
        let foreground = to_ghostty_rgb(colors.foreground);
        self.set_option(sys::GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND, &foreground, "set foreground")?;
        let background = to_ghostty_rgb(colors.background);
        self.set_option(sys::GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND, &background, "set background")?;
        let palette = colors.palette.map(to_ghostty_rgb);
        self.set_option(sys::GHOSTTY_TERMINAL_OPT_COLOR_PALETTE, &palette, "set palette")?;
        match colors.cursor {
            Some(cursor) => {
                self.set_option(sys::GHOSTTY_TERMINAL_OPT_COLOR_CURSOR, &to_ghostty_rgb(cursor), "set cursor color")
            }
            // SAFETY: valid handle; a NULL value clears the default cursor color (terminal.h).
            None => check(
                unsafe { sys::ghostty_terminal_set(self.terminal, sys::GHOSTTY_TERMINAL_OPT_COLOR_CURSOR, null()) },
                "clear cursor color",
            ),
        }
    }

    /// Resizes the grid; libghostty reflows wrapped lines.
    pub fn resize(&mut self, cols: u16, rows: u16, cell_width_px: u32, cell_height_px: u32) -> Result<()> {
        self.generation += 1;
        let (cols, rows) = (cols.max(1), rows.max(1));
        // SAFETY: valid handle.
        let code = unsafe { sys::ghostty_terminal_resize(self.terminal, cols, rows, cell_width_px, cell_height_px) };
        check(code, "terminal_resize")?;
        (self.cols, self.rows) = (cols, rows);
        (self.cell_width_px, self.cell_height_px) = (cell_width_px.max(1), cell_height_px.max(1));
        let limit = history_limit(self.scrollback_lines, cols);
        self.set_option(sys::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES, &limit, "set scrollback")
    }

    /// The size last given to [`Vt::resize`]: columns, rows, cell width and height in pixels.
    pub fn size(&self) -> (u16, u16, u32, u32) {
        (self.cols, self.rows, self.cell_width_px, self.cell_height_px)
    }

    /// Moves the viewport through the scrollback.
    pub fn scroll_viewport(&mut self, scroll: Scroll) {
        self.generation += 1;
        // SAFETY: the union is plain data; zeroed is a valid value.
        let mut behavior: sys::GhosttyTerminalScrollViewport = unsafe { zeroed() };
        match scroll {
            Scroll::Top => behavior.tag = sys::GHOSTTY_TERMINAL_SCROLL_VIEWPORT_TOP,
            Scroll::Bottom => behavior.tag = sys::GHOSTTY_TERMINAL_SCROLL_VIEWPORT_BOTTOM,
            Scroll::Delta(delta) => {
                behavior.tag = sys::GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA;
                behavior.value.delta = delta;
            }
        }
        // SAFETY: valid handle; the tag matches the union field set above.
        unsafe { sys::ghostty_terminal_scroll_viewport(self.terminal, behavior) };
    }

    /// Viewport position in the scrollback.
    pub fn scrollbar(&self) -> Scrollbar {
        // SAFETY: plain data out-parameter of the documented type.
        let mut bar: sys::GhosttyTerminalScrollbar = unsafe { zeroed() };
        // SAFETY: valid handle and out-pointer.
        let code = unsafe {
            sys::ghostty_terminal_get(self.terminal, sys::GHOSTTY_TERMINAL_DATA_SCROLLBAR, (&raw mut bar).cast())
        };
        if code != sys::GHOSTTY_SUCCESS {
            return Scrollbar::default();
        }
        Scrollbar { total: bar.total, offset: bar.offset, len: bar.len }
    }

    /// Returns whether DEC private `mode` (e.g. 2004 = bracketed paste) is set.
    pub fn is_dec_mode_set(&self, mode: u16) -> bool {
        let mut config = sys::GhosttyTerminalModeConfig { mode: mode & 0x7fff, value: false };
        // SAFETY: valid handle; `config.mode` is initialized as DATA_MODE requires.
        let code = unsafe {
            sys::ghostty_terminal_get(self.terminal, sys::GHOSTTY_TERMINAL_DATA_MODE, (&raw mut config).cast())
        };
        code == sys::GHOSTTY_SUCCESS && config.value
    }

    /// Encodes a key press for the program, honoring the modes it enabled (cursor keys, Kitty
    /// keyboard protocol, ...). Returns an empty vector when the key produces nothing.
    pub fn encode_key(&mut self, input: &KeyInput) -> Result<Vec<u8>> {
        let text = input.text.as_deref().unwrap_or("");
        // SAFETY: valid handles; `text` outlives the encode call that reads it, and the utf8 field
        // is reset to empty before returning so no dangling pointer stays in the event.
        unsafe {
            check(sys::ghostty_key_encoder_set_from_terminal(self.key_encoder, self.terminal), "encoder sync")?;
            let event = self.key_event;
            check(sys::ghostty_key_event_set_action(event, sys::GHOSTTY_KEY_ACTION_PRESS), "key action")?;
            check(sys::ghostty_key_event_set_key(event, input.key.to_ghostty()), "key")?;
            check(sys::ghostty_key_event_set_mods(event, input.mods.to_ghostty()), "key mods")?;
            check(
                sys::ghostty_key_event_set_unshifted_codepoint(event, input.key.unshifted_codepoint().unwrap_or(0)),
                "key codepoint",
            )?;
            check(
                sys::ghostty_key_event_set_utf8(event, sys::GhosttyString { ptr: text.as_ptr(), len: text.len() }),
                "key utf8",
            )?;
            let result = self.encode_event(event);
            sys::ghostty_key_event_set_utf8(event, sys::GhosttyString { ptr: null(), len: 0 });
            result
        }
    }

    /// # Safety
    /// `event` must be a valid key event handle.
    unsafe fn encode_event(&mut self, event: sys::GhosttyKeyEvent) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; 64];
        loop {
            let mut written = 0usize;
            // SAFETY: valid handles; `buf` is writable for `buf.len()` bytes.
            let code = unsafe {
                sys::ghostty_key_encoder_encode(self.key_encoder, event, buf.as_mut_ptr(), buf.len(), &mut written)
            };
            if code == sys::GHOSTTY_OUT_OF_SPACE && written > buf.len() && written <= 4096 {
                buf.resize(written, 0);
                continue;
            }
            check(code, "key_encoder_encode")?;
            buf.truncate(written.min(buf.len()));
            return Ok(buf);
        }
    }

    /// Encodes pasted text: strips unsafe control bytes and wraps it in bracketed-paste markers
    /// when the program enabled mode 2004.
    pub fn encode_paste(&self, text: &str) -> Result<Vec<u8>> {
        let bracketed = self.is_dec_mode_set(2004);
        let mut data = text.as_bytes().to_vec();
        let mut needed = 0usize;
        // SAFETY: `data` is a writable buffer of `data.len()` bytes (modified in place); a null
        // output buffer with length 0 is the documented size query.
        let code = unsafe {
            sys::ghostty_paste_encode(data.as_mut_ptr().cast(), data.len(), bracketed, null_mut(), 0, &mut needed)
        };
        if code == sys::GHOSTTY_SUCCESS {
            return Ok(Vec::new());
        }
        if code != sys::GHOSTTY_OUT_OF_SPACE {
            return Err(Error::Ghostty { op: "paste_encode", code });
        }
        let mut out = vec![0u8; needed];
        let mut written = 0usize;
        // SAFETY: as above; `out` is writable for `out.len()` bytes.
        let code = unsafe {
            sys::ghostty_paste_encode(
                data.as_mut_ptr().cast(),
                data.len(),
                bracketed,
                out.as_mut_ptr(),
                out.len(),
                &mut written,
            )
        };
        check(code, "paste_encode")?;
        out.truncate(written.min(out.len()));
        Ok(out)
    }

    /// Returns whether `text` is safe to paste without confirmation (no newline outside
    /// bracketed paste, no bracketed-paste terminator).
    pub fn is_paste_safe(text: &str) -> bool {
        // SAFETY: the pointer is valid for `text.len()` bytes; the function only reads.
        unsafe { sys::ghostty_paste_is_safe(text.as_ptr().cast(), text.len()) }
    }

    /// Changes whenever what the viewport shows can change; a [`GridSnapshot`] with the same
    /// value shows the current state, so cell positions read from it are valid here.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Captures the viewport into `out`, reusing its allocations.
    pub fn snapshot(&mut self, out: &mut GridSnapshot) -> Result<()> {
        // SAFETY: valid handles. Row data read below is only used before the next update.
        unsafe { check(sys::ghostty_render_state_update(self.render_state, self.terminal), "render_state_update")? };

        let colors = self.render_colors()?;
        out.colors = Colors {
            foreground: from_ghostty_rgb(colors.foreground),
            background: from_ghostty_rgb(colors.background),
            cursor: colors.cursor_has_value.then(|| from_ghostty_rgb(colors.cursor)),
        };
        out.cursor = self.render_cursor()?;
        out.scrollbar = self.scrollbar();
        out.generation = self.generation;

        let mut cols: u16 = 0;
        // SAFETY: DATA_COLS writes a uint16_t.
        unsafe {
            check(
                sys::ghostty_render_state_get(
                    self.render_state,
                    sys::GHOSTTY_RENDER_STATE_DATA_COLS,
                    (&raw mut cols).cast(),
                ),
                "render cols",
            )?;
        }
        out.cols = cols;

        let mut row_index = 0usize;
        // SAFETY: the iterator and cells objects were created in `new`; populating them from the
        // freshly updated render state is the documented usage.
        unsafe {
            check(
                sys::ghostty_render_state_get(
                    self.render_state,
                    sys::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                    (&raw mut self.row_iter).cast(),
                ),
                "row iterator",
            )?;
            while sys::ghostty_render_state_row_iterator_next(self.row_iter) {
                check(
                    sys::ghostty_render_state_row_get(
                        self.row_iter,
                        sys::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                        (&raw mut self.row_cells).cast(),
                    ),
                    "row cells",
                )?;
                if out.rows.len() <= row_index {
                    out.rows.push(Row::default());
                }
                let row = &mut out.rows[row_index];
                row.text.clear();
                row.cells.clear();
                let mut raw_row: sys::GhosttyRow = 0;
                let (mut is_wrapped, mut is_continuation) = (false, false);
                if sys::ghostty_render_state_row_get(
                    self.row_iter,
                    sys::GHOSTTY_RENDER_STATE_ROW_DATA_RAW,
                    (&raw mut raw_row).cast(),
                ) == sys::GHOSTTY_SUCCESS
                {
                    sys::ghostty_row_get(raw_row, sys::GHOSTTY_ROW_DATA_WRAP, (&raw mut is_wrapped).cast());
                    sys::ghostty_row_get(
                        raw_row,
                        sys::GHOSTTY_ROW_DATA_WRAP_CONTINUATION,
                        (&raw mut is_continuation).cast(),
                    );
                }
                row.is_wrapped = is_wrapped;
                row.is_wrap_continuation = is_continuation;
                while sys::ghostty_render_state_row_cells_next(self.row_cells) {
                    row.cells.push(read_cell(self.row_cells, &mut row.text, &colors.palette));
                }
                row_index += 1;
            }
        }
        out.rows.truncate(row_index);
        Ok(())
    }

    fn render_colors(&self) -> Result<sys::GhosttyRenderStateColors> {
        // SAFETY: sized plain-data struct; `size` is set before the call as required.
        let mut colors: sys::GhosttyRenderStateColors = unsafe { zeroed() };
        colors.size = size_of::<sys::GhosttyRenderStateColors>();
        // SAFETY: valid handle and out-pointer of the documented type.
        let code = unsafe {
            sys::ghostty_render_state_get(
                self.render_state,
                sys::GHOSTTY_RENDER_STATE_DATA_COLORS,
                (&raw mut colors).cast(),
            )
        };
        check(code, "render colors")?;
        Ok(colors)
    }

    fn render_cursor(&self) -> Result<Option<Cursor>> {
        // SAFETY: sized plain-data struct; `size` is set before the call as required.
        let mut c: sys::GhosttyRenderStateCursor = unsafe { zeroed() };
        c.size = size_of::<sys::GhosttyRenderStateCursor>();
        // SAFETY: valid handle and out-pointer of the documented type.
        let code = unsafe {
            sys::ghostty_render_state_get(self.render_state, sys::GHOSTTY_RENDER_STATE_DATA_CURSOR, (&raw mut c).cast())
        };
        check(code, "render cursor")?;
        if !c.visible || !c.viewport_has_value {
            return Ok(None);
        }
        let style = match c.visual_style {
            sys::GHOSTTY_CURSOR_STYLE_BAR => CursorStyle::Bar,
            sys::GHOSTTY_CURSOR_STYLE_UNDERLINE => CursorStyle::Underline,
            sys::GHOSTTY_CURSOR_STYLE_BLOCK_HOLLOW => CursorStyle::BlockHollow,
            _ => CursorStyle::Block,
        };
        // The cursor never sits on the spacer half of a wide char: report the head cell.
        let col = if c.wide_tail { c.viewport_x.saturating_sub(1) } else { c.viewport_x };
        Ok(Some(Cursor { col, row: c.viewport_y, style, is_blinking: c.blinking, is_wide: c.wide_tail }))
    }
}

/// Reads the current cell of `cells`, appending its grapheme to `text`.
///
/// # Safety
/// `cells` must be positioned on a cell by `ghostty_render_state_row_cells_next`.
unsafe fn read_cell(
    cells: sys::GhosttyRenderStateRowCells,
    text: &mut String,
    palette: &[sys::GhosttyColorRgb; 256],
) -> Cell {
    let mut cell = Cell::default();
    // SAFETY: (all calls below) `cells` is positioned on a cell; every out-pointer has the type
    // documented for its data kind; buffers are passed with their capacities.
    unsafe {
        let mut raw: sys::GhosttyCell = zeroed();
        if sys::ghostty_render_state_row_cells_get(
            cells,
            sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
            (&raw mut raw).cast(),
        ) == sys::GHOSTTY_SUCCESS
        {
            let mut wide: sys::GhosttyCellWide = sys::GHOSTTY_CELL_WIDE_NARROW;
            sys::ghostty_cell_get(raw, sys::GHOSTTY_CELL_DATA_WIDE, (&raw mut wide).cast());
            cell.width = match wide {
                sys::GHOSTTY_CELL_WIDE_WIDE => CellWidth::Wide,
                sys::GHOSTTY_CELL_WIDE_SPACER_TAIL => CellWidth::SpacerTail,
                sys::GHOSTTY_CELL_WIDE_SPACER_HEAD => CellWidth::SpacerHead,
                _ => CellWidth::Narrow,
            };
            let mut has_hyperlink = false;
            sys::ghostty_cell_get(raw, sys::GHOSTTY_CELL_DATA_HAS_HYPERLINK, (&raw mut has_hyperlink).cast());
            cell.has_hyperlink = has_hyperlink;
        }

        if !cell.width.is_spacer() {
            let mut bytes = [0u8; 64];
            let mut buf = sys::GhosttyBuffer { ptr: bytes.as_mut_ptr(), cap: bytes.len(), len: 0 };
            let code = sys::ghostty_render_state_row_cells_get(
                cells,
                sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
                (&raw mut buf).cast(),
            );
            if code == sys::GHOSTTY_SUCCESS {
                let len = buf.len.min(bytes.len());
                if let Ok(s) = std::str::from_utf8(&bytes[..len]) {
                    cell.text_start = u32::try_from(text.len()).unwrap_or(u32::MAX);
                    cell.text_len = u16::try_from(s.len()).unwrap_or(0);
                    text.push_str(s);
                }
            }
        }

        let mut rgb: sys::GhosttyColorRgb = zeroed();
        if sys::ghostty_render_state_row_cells_get(
            cells,
            sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
            (&raw mut rgb).cast(),
        ) == sys::GHOSTTY_SUCCESS
        {
            cell.fg = Some(from_ghostty_rgb(rgb));
        }
        if sys::ghostty_render_state_row_cells_get(
            cells,
            sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
            (&raw mut rgb).cast(),
        ) == sys::GHOSTTY_SUCCESS
        {
            cell.bg = Some(from_ghostty_rgb(rgb));
        }
        let mut is_selected = false;
        sys::ghostty_render_state_row_cells_get(
            cells,
            sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_SELECTED,
            (&raw mut is_selected).cast(),
        );
        cell.is_selected = is_selected;

        let mut has_styling = false;
        sys::ghostty_render_state_row_cells_get(
            cells,
            sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HAS_STYLING,
            (&raw mut has_styling).cast(),
        );
        if has_styling {
            let mut style: sys::GhosttyStyle = zeroed();
            if sys::ghostty_render_state_row_cells_get(
                cells,
                sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                (&raw mut style).cast(),
            ) == sys::GHOSTTY_SUCCESS
            {
                cell.flags = CellFlags::from_bits_truncate(style.flags);
                cell.underline = match (style.flags >> UNDERLINE_SHIFT) & UNDERLINE_MASK {
                    1 => Underline::Single,
                    2 => Underline::Double,
                    3 => Underline::Curly,
                    4 => Underline::Dotted,
                    5 => Underline::Dashed,
                    _ => Underline::None,
                };
                let uc = style.underline_color;
                cell.underline_color = match u32::from(uc.tag) {
                    t if t == sys::GHOSTTY_STYLE_COLOR_RGB as u32 => Some(from_ghostty_rgb(uc.value.rgb)),
                    t if t == sys::GHOSTTY_STYLE_COLOR_PALETTE as u32 => {
                        Some(from_ghostty_rgb(palette[usize::from(uc.value.palette)]))
                    }
                    _ => None,
                };
            }
        }
    }
    cell
}

impl Drop for Vt {
    fn drop(&mut self) {
        // SAFETY: each handle was created by its `_new` function (or is null and skipped) and is
        // freed exactly once; the callback state is freed after the terminal that references it.
        unsafe {
            if !self.terminal.is_null() {
                self.mouse.free(self.terminal);
            }
            if !self.key_event.is_null() {
                sys::ghostty_key_event_free(self.key_event);
            }
            if !self.key_encoder.is_null() {
                sys::ghostty_key_encoder_free(self.key_encoder);
            }
            if !self.row_cells.is_null() {
                sys::ghostty_render_state_row_cells_free(self.row_cells);
            }
            if !self.row_iter.is_null() {
                sys::ghostty_render_state_row_iterator_free(self.row_iter);
            }
            if !self.render_state.is_null() {
                sys::ghostty_render_state_free(self.render_state);
            }
            if !self.terminal.is_null() {
                sys::ghostty_terminal_free(self.terminal);
            }
            drop(Box::from_raw(self.callbacks));
        }
    }
}

fn to_ghostty_rgb(c: Rgb) -> sys::GhosttyColorRgb {
    sys::GhosttyColorRgb { r: c.r, g: c.g, b: c.b }
}

fn from_ghostty_rgb(c: sys::GhosttyColorRgb) -> Rgb {
    Rgb::new(c.r, c.g, c.b)
}

/// Returns `true` for bidi formatting characters (marks, embeddings, overrides, isolates), which
/// untrusted text could use to reorder what is shown.
pub fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

fn sanitize_title(raw: &str) -> String {
    raw.chars().filter(|&c| !c.is_control() && !is_bidi_control(c)).take(MAX_TITLE_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen_text(vt: &mut Vt) -> Vec<String> {
        let mut snap = GridSnapshot::default();
        vt.snapshot(&mut snap).expect("snapshot");
        snap.rows
            .iter()
            .map(|row| {
                row.cells
                    .iter()
                    .filter(|c| !c.width.is_spacer())
                    .map(|c| if c.text_len == 0 { " " } else { row.cell_text(c) })
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn feeds_text_and_reads_it_back() {
        let mut vt = Vt::new(20, 3, &VtOptions::default()).expect("vt");
        vt.feed("hello\r\n日本語".as_bytes());
        let rows = screen_text(&mut vt);
        assert_eq!(rows[0], "hello");
        assert_eq!(rows[1], "日本語");
    }

    #[test]
    fn wide_char_occupies_two_cells() {
        let mut vt = Vt::new(10, 2, &VtOptions::default()).expect("vt");
        vt.feed("日a".as_bytes());
        let mut snap = GridSnapshot::default();
        vt.snapshot(&mut snap).expect("snapshot");
        let cells = &snap.rows[0].cells;
        assert_eq!(cells[0].width, CellWidth::Wide);
        assert_eq!(cells[1].width, CellWidth::SpacerTail);
        assert_eq!(snap.rows[0].cell_text(&cells[2]), "a");
        assert_eq!(snap.cursor.map(|c| c.col), Some(3));
    }

    #[test]
    fn reads_hyperlinks_and_soft_wraps() {
        let mut vt = Vt::new(10, 4, &VtOptions::default()).expect("vt");
        vt.feed(b"\x1b]8;;https://example.com/x\x1b\\link\x1b]8;;\x1b\\ no\r\n0123456789abc");
        let mut snap = GridSnapshot::default();
        vt.snapshot(&mut snap).expect("snapshot");
        let links: Vec<bool> = snap.rows[0].cells.iter().take(6).map(|c| c.has_hyperlink).collect();
        assert_eq!(links, [true, true, true, true, false, false]);
        assert_eq!(vt.hyperlink_uri(2, 0).as_deref(), Some("https://example.com/x"));
        assert_eq!(vt.hyperlink_uri(5, 0), None);
        assert_eq!((snap.rows[0].is_wrapped, snap.rows[1].is_wrapped, snap.rows[2].is_wrapped), (false, true, false));
        assert!(snap.rows[2].is_wrap_continuation && !snap.rows[1].is_wrap_continuation);
        assert_eq!(snap.generation, vt.generation());
        vt.feed(b"x");
        assert_ne!(snap.generation, vt.generation());
    }

    #[test]
    fn answers_cursor_position_report() {
        let mut vt = Vt::new(20, 5, &VtOptions::default()).expect("vt");
        vt.feed(b"ab\x1b[6n");
        assert_eq!(vt.take_replies(), b"\x1b[1;3R");
    }

    #[test]
    fn never_answers_title_report() {
        let mut vt = Vt::new(20, 5, &VtOptions::default()).expect("vt");
        vt.feed(b"\x1b]2;rm -rf ~\x07\x1b[21t");
        assert!(vt.take_replies().is_empty());
        assert_eq!(vt.take_title_change().as_deref(), Some("rm -rf ~"));
    }

    #[test]
    fn title_strips_controls_and_bidi() {
        assert_eq!(sanitize_title("a\u{202E}b\u{1b}c\u{9b}d\u{200F}e"), "abcde");
        assert_eq!(sanitize_title(&"x".repeat(1000)).len(), MAX_TITLE_CHARS);
    }

    #[test]
    fn bold_and_rgb_style_are_reported() {
        let mut vt = Vt::new(20, 2, &VtOptions::default()).expect("vt");
        vt.feed(b"\x1b[1;38;2;255;128;0mX");
        let mut snap = GridSnapshot::default();
        vt.snapshot(&mut snap).expect("snapshot");
        let cell = snap.rows[0].cells[0];
        assert!(cell.flags.contains(CellFlags::BOLD));
        assert_eq!(cell.fg, Some(Rgb::new(255, 128, 0)));
    }

    #[test]
    fn default_colors_set_the_palette_and_cursor_until_changed_again() {
        let mut vt = Vt::new(20, 2, &VtOptions::default()).expect("vt");
        vt.feed(b"\x1b[31mX\x1b[38;5;200mY");
        let read = |vt: &mut Vt| {
            let mut snap = GridSnapshot::default();
            vt.snapshot(&mut snap).expect("snapshot");
            (snap.rows[0].cells[0].fg, snap.rows[0].cells[1].fg, snap.colors.cursor)
        };
        let builtin = default_palette();
        assert_eq!(read(&mut vt), (Some(builtin[1]), Some(builtin[200]), None));

        let red = Rgb::new(0xff, 0x75, 0x7f);
        let cursor = Rgb::new(1, 2, 3);
        let mut colors = DefaultColors { cursor: Some(cursor), ..DefaultColors::default() };
        colors.palette[1] = red;
        vt.set_colors(&colors).expect("set colors");
        assert_eq!(read(&mut vt), (Some(red), Some(builtin[200]), Some(cursor)));

        vt.set_colors(&DefaultColors::default()).expect("reset colors");
        assert_eq!(read(&mut vt), (Some(builtin[1]), Some(builtin[200]), None));
    }

    #[test]
    fn paste_strips_escape_and_brackets_when_enabled() {
        let mut vt = Vt::new(20, 2, &VtOptions::default()).expect("vt");
        let plain = vt.encode_paste("a\x1b[201~b\n").expect("paste");
        assert!(!plain.contains(&0x1b));
        vt.feed(b"\x1b[?2004h");
        let bracketed = vt.encode_paste("ls\n").expect("paste");
        assert!(bracketed.starts_with(b"\x1b[200~"));
        assert!(bracketed.ends_with(b"\x1b[201~"));
        assert!(!Vt::is_paste_safe("ls\n"));
    }

    #[test]
    fn encodes_special_keys() {
        let mut vt = Vt::new(20, 2, &VtOptions::default()).expect("vt");
        let up = KeyInput { key: Key::Up, mods: Mods::default(), text: None };
        assert_eq!(vt.encode_key(&up).expect("encode"), b"\x1b[A");
        let ctrl_c = KeyInput { key: Key::Char('c'), mods: Mods { ctrl: true, ..Mods::default() }, text: None };
        assert_eq!(vt.encode_key(&ctrl_c).expect("encode"), b"\x03");
    }

    fn at(x: f64, y: f64) -> GridPos {
        GridPos { x, y }
    }

    #[test]
    fn drag_selects_and_copies_wrapped_line_without_newline() {
        let mut vt = Vt::new(10, 4, &VtOptions::default()).expect("vt");
        vt.resize(10, 4, 8, 16).expect("resize");
        vt.feed(b"0123456789abcdef\r\nnext");
        vt.selection_press(at(0.1, 0.5), 1, 0).expect("press");
        vt.selection_drag(at(5.9, 1.5), false).expect("drag");
        vt.selection_release(at(5.9, 1.5)).expect("release");
        assert_eq!(vt.selection_text().expect("text").as_deref(), Some("0123456789abcdef"));
    }

    #[test]
    fn double_click_selects_word_and_single_click_clears() {
        let mut vt = Vt::new(20, 2, &VtOptions::default()).expect("vt");
        vt.resize(20, 2, 8, 16).expect("resize");
        vt.feed(b"hello world");
        let interval = 500_000_000;
        for time in [1_000, 2_000] {
            vt.selection_press(at(7.5, 0.5), time, interval).expect("press");
            vt.selection_release(at(7.5, 0.5)).expect("release");
        }
        assert_eq!(vt.selection_text().expect("text").as_deref(), Some("world"));
        vt.selection_press(at(1.5, 0.5), 10_000_000_000, interval).expect("press");
        assert_eq!(vt.selection_text().expect("text"), None);
    }

    #[test]
    fn select_all_and_clear() {
        let mut vt = Vt::new(20, 3, &VtOptions::default()).expect("vt");
        vt.feed(b"one\r\ntwo");
        vt.select_all().expect("select all");
        assert_eq!(vt.selection_text().expect("text").as_deref(), Some("one\ntwo"));
        vt.clear_selection().expect("clear");
        assert_eq!(vt.selection_text().expect("text"), None);
    }

    #[test]
    fn mouse_reports_only_when_tracking_in_sgr() {
        let mut vt = Vt::new(20, 5, &VtOptions::default()).expect("vt");
        vt.resize(20, 5, 8, 16).expect("resize");
        assert!(!vt.is_mouse_tracking());
        let press = |vt: &mut Vt| {
            vt.encode_mouse(MouseAction::Press, Some(MouseButton::Left), Mods::default(), at(2.5, 1.5), true)
        };
        assert!(press(&mut vt).expect("encode").is_empty());
        vt.feed(b"\x1b[?1000h\x1b[?1006h");
        assert!(vt.is_mouse_tracking());
        assert_eq!(press(&mut vt).expect("encode"), b"\x1b[<0;3;2M");
    }

    #[test]
    fn cursor_blink_follows_decscusr() {
        let mut vt = Vt::new(10, 2, &VtOptions::default()).expect("vt");
        let blinking = |vt: &mut Vt| {
            let mut snap = GridSnapshot::default();
            vt.snapshot(&mut snap).expect("snapshot");
            snap.cursor.map(|c| c.is_blinking)
        };
        assert_eq!(blinking(&mut vt), Some(true));
        vt.feed(b"\x1b[2 q");
        assert_eq!(blinking(&mut vt), Some(false));
        vt.feed(b"\x1b[5 q");
        assert_eq!(blinking(&mut vt), Some(true));
        // ncurses `cnorm` must not stop the blink.
        vt.feed(b"\x1b[?12l\x1b[?25h");
        assert_eq!(blinking(&mut vt), Some(true));
    }

    #[test]
    fn scrollback_moves_viewport() {
        let mut vt = Vt::new(10, 3, &VtOptions::default()).expect("vt");
        for i in 0..20 {
            vt.feed(format!("{i}\r\n").as_bytes());
        }
        assert!(vt.scrollbar().is_at_bottom());
        vt.scroll_viewport(Scroll::Delta(-5));
        assert!(!vt.scrollbar().is_at_bottom());
        vt.scroll_viewport(Scroll::Bottom);
        assert!(vt.scrollbar().is_at_bottom());
    }

    #[test]
    fn scrollback_keeps_at_least_the_configured_lines() {
        let options = VtOptions { scrollback_lines: 10_000, ..VtOptions::default() };
        for cols in [20u16, 80, 120, 215, 300] {
            let rows = 30;
            let mut vt = Vt::new(cols, rows, &options).expect("vt");
            for n in [10_500, 13_333, 20_000, 31_000] {
                let mut vt2 = Vt::new(cols, rows, &options).expect("vt");
                for i in 0..n {
                    vt2.feed(format!("{i}\r\n").as_bytes());
                }
                let history = vt2.scrollbar().total - u64::from(rows);
                let slack = 2 * history_limit(1, cols) as u64;
                assert!(history >= 10_000, "cols {cols}, {n} lines: history {history}");
                assert!(history <= 10_000 + slack, "cols {cols}, {n} lines: history {history}");
            }
            vt.resize(cols / 2 + 1, rows, 8, 16).expect("resize");
            for i in 0..20_000 {
                vt.feed(format!("{i}\r\n").as_bytes());
            }
            assert!(vt.scrollbar().total - u64::from(rows) >= 10_000, "cols {cols} after resize");
        }
    }

    #[test]
    fn no_scrollback_stays_zero() {
        assert_eq!(history_limit(0, 80), 0);
    }
}
