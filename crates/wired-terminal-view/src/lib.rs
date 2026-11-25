//! gpui view of one terminal: paints the grid, sends keys, IME text and mouse input to the
//! program, selects, copies, scrolls, pastes, and reports title/exit to its parent.
//!
//! Keyboard pipeline: key bindings (actions in the `Terminal` context) first;
//! then non-text keys and Ctrl/Alt chords are encoded by libghostty; printable text arrives only
//! through the input handler (IME commits and plain typing share that path, so nothing is sent twice).
#![forbid(unsafe_code)]

mod bell;
mod custom_glyphs;
mod frame;
mod ime;
mod link;
mod mouse;
mod paint;
mod paste;
pub mod scrollbar;

use std::{
    cell::RefCell,
    ops::Range,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{
    App, Bounds, Context, ElementInputHandler, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, MouseMoveEvent, MouseUpEvent, NoAction, ParentElement,
    Pixels, Point, Render, ScrollDelta, ScrollEvent, Styled, Subscription, Task, UTF16Selection, Window, actions,
    canvas, div, point, px, size,
};
pub use wired_core::Notice;
use wired_core::{SessionEnd, Terminal, TerminalConfig, TerminalEvent};
use wired_vt::{Cursor, CursorStyle, GridSnapshot, Key, KeyInput, Mods, Scroll};

pub use crate::paint::{FontConfig, Ligatures};
use crate::{
    frame::{FrameState, Layout},
    ime::ImeState,
    mouse::MouseState,
    paint::{FontSet, Metrics, Painter},
    scrollbar::ScrollbarState,
};

actions!(
    terminal,
    [
        /// Copy the selection to the clipboard.
        Copy,
        /// Paste the clipboard into the terminal.
        Paste,
        /// Select the whole screen and scrollback.
        SelectAll,
        /// Increase the font size.
        ZoomIn,
        /// Decrease the font size.
        ZoomOut,
        /// Reset the font size.
        ResetZoom,
        /// Scroll the viewport up one line.
        ScrollUp,
        /// Scroll the viewport down one line.
        ScrollDown,
        /// Scroll the viewport up one page.
        ScrollPageUp,
        /// Scroll the viewport down one page.
        ScrollPageDown,
        /// Scroll to the oldest line.
        ScrollToTop,
        /// Scroll to the live screen.
        ScrollToBottom,
    ]
);

/// Key context of the terminal view for key bindings.
pub const KEY_CONTEXT: &str = "Terminal";

/// Unbinds gpui-component's app-wide Tab, Shift+Tab and Ctrl+C inside the terminal so the shell
/// gets them (completion, interrupt).
pub fn passthrough_key_bindings() -> Vec<KeyBinding> {
    let context = Some(KEY_CONTEXT);
    vec![
        KeyBinding::new("tab", NoAction, context),
        KeyBinding::new("shift-tab", NoAction, context),
        KeyBinding::new("ctrl-c", NoAction, context),
    ]
}

/// Starts a terminal whose events can be handed to [`TerminalView::new`].
pub fn spawn_terminal(
    config: TerminalConfig,
) -> wired_core::Result<(Arc<Terminal>, async_channel::Receiver<TerminalEvent>)> {
    let (sender, receiver) = async_channel::unbounded();
    let terminal = Terminal::spawn(config, move |event| {
        let _ = sender.send_blocking(event);
    })?;
    Ok((Arc::new(terminal), receiver))
}

/// A screen without a program, for [`TerminalView::new`] followed by
/// [`TerminalView::show_start_failure`].
pub fn screen_without_program(
    vt: &wired_vt::VtOptions,
) -> wired_core::Result<(Arc<Terminal>, async_channel::Receiver<TerminalEvent>)> {
    let size = wired_core::Dimensions { cols: 80, rows: 24, ..wired_core::Dimensions::default() };
    let terminal = Terminal::without_program(size, vt)?;
    let (_, receiver) = async_channel::unbounded();
    Ok((Arc::new(terminal), receiver))
}

/// What the view does when its program exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnExit {
    /// Close on exit code 0; otherwise stay and show the code.
    #[default]
    Auto,
    /// Always close.
    Close,
    /// Always stay and show the code.
    Keep,
}

/// Behavior settings of a terminal view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewOptions {
    /// Copy the selection to the clipboard when the mouse button is released.
    pub copy_on_select: bool,
    /// Let the cursor blink when the program asks for a blinking cursor.
    pub cursor_blink: bool,
    /// Show the overlay scrollbar when there is history to scroll.
    pub scrollbar: bool,
    /// Hide the mouse pointer on any key press until it moves.
    pub hide_pointer_while_typing: bool,
    /// What happens when the program exits.
    pub on_exit: OnExit,
    /// Opacity of the text while another pane of the tab has the focus, 0..=1.
    pub inactive_text_opacity: f32,
    /// Keep showing the (hollow) cursor while another pane of the tab has the focus.
    pub show_inactive_cursor: bool,
    /// What a right-click does (when the program does not take mouse reports).
    pub right_click: RightClick,
    /// The text shakes when the program rings the bell.
    pub bell_visual: bool,
    /// Cursor text and selection colors.
    pub colors: ViewColors,
}

/// Colors the view draws with beyond the terminal's own (which come with each snapshot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ViewColors {
    /// Text under a block cursor; `None` = the background color.
    pub cursor_text: Option<wired_vt::Rgb>,
    /// Opaque background of selected cells; `None` = the text color, translucent.
    pub selection_background: Option<wired_vt::Rgb>,
    /// Text of selected cells; `None` = unchanged.
    pub selection_foreground: Option<wired_vt::Rgb>,
}

/// What a right-click in a terminal does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RightClick {
    /// Copy the selection if there is one, otherwise paste (as Windows Terminal).
    #[default]
    CopyOrPaste,
    /// Paste.
    Paste,
    /// Nothing.
    Nothing,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            copy_on_select: false,
            cursor_blink: true,
            scrollbar: true,
            hide_pointer_while_typing: true,
            on_exit: OnExit::Auto,
            inactive_text_opacity: 0.7,
            show_inactive_cursor: false,
            right_click: RightClick::CopyOrPaste,
            bell_visual: true,
            colors: ViewColors::default(),
        }
    }
}

/// Events for the parent (tab bar, window).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalViewEvent {
    /// The program changed the title.
    TitleChanged(String),
    /// The program rang the bell.
    Bell,
    /// The program exited and the view should be closed.
    Exited(Option<u32>),
    /// The user asked to start the program again in this place (Enter after it ended).
    Restart,
}

/// A terminal's grid size and when it last changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridChange {
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
    /// When the columns or rows last changed.
    pub changed_at: Instant,
}

/// Why a view no longer has a running program while it stays open.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ended {
    Exited { code: Option<u32>, is_output_complete: bool },
    Failed(String),
}

const PADDING_X: f32 = 8.;
const PADDING_Y: f32 = 4.;
const MIN_FONT_SIZE: f32 = 6.;
const MAX_FONT_SIZE: f32 = 72.;
/// DEC private modes of the alternate screen; the wheel sends arrow keys there.
const ALT_SCREEN_MODES: [u16; 3] = [1049, 1047, 47];
/// DEC private mode: the program wants pastes wrapped in markers.
const BRACKETED_PASTE_MODE: u16 = 2004;
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(530);
/// How long text takes to fade between focused and dimmed.
const DIM_ANIMATION: Duration = Duration::from_millis(500);

/// A terminal pane.
pub struct TerminalView {
    terminal: Arc<Terminal>,
    focus: FocusHandle,
    font: FontConfig,
    options: ViewOptions,
    default_font_size: f32,
    snapshot: Rc<RefCell<GridSnapshot>>,
    frame: Rc<FrameState>,
    ime: ImeState,
    ended: Option<Ended>,
    scroll_remainder: f32,
    mouse: MouseState,
    scrollbar: ScrollbarState,
    /// Monotonic time base for click timing.
    epoch: Instant,
    is_focused: bool,
    /// Another pane of the same tab has the focus.
    is_dimmed: bool,
    /// Dim level (0 = focused look, 1 = fully dimmed) when the last change started, and when.
    dim_from: f32,
    dim_started: Instant,
    /// When the bell last started a shake of the text; a new bell restarts it.
    shake_started: Option<Instant>,
    /// The link under the pointer while Ctrl is held, underlined.
    hovered_link: Option<link::Link>,
    /// Blink phase in which a blinking cursor is hidden.
    is_blink_hidden: bool,
    autoscroll_task: Task<()>,
    blink_task: Task<()>,
    _events: Task<()>,
    _blur: Subscription,
    _activation: Subscription,
}

impl std::fmt::Debug for TerminalView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalView").finish_non_exhaustive()
    }
}

impl EventEmitter<TerminalViewEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TerminalView {
    /// Creates the view for a terminal started with [`spawn_terminal`].
    pub fn new(
        terminal: Arc<Terminal>,
        events: async_channel::Receiver<TerminalEvent>,
        font: FontConfig,
        options: ViewOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let (blur, activation) = Self::window_subscriptions(&focus, window, cx);
        let events = cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this.update(cx, |view, cx| view.handle_terminal_event(event, cx)).is_err() {
                    break;
                }
            }
        });
        let mut view = Self {
            scrollbar: ScrollbarState::default(),
            terminal,
            focus,
            default_font_size: font.size,
            font,
            options,
            snapshot: Rc::default(),
            frame: Rc::default(),
            ime: ImeState::default(),
            ended: None,
            scroll_remainder: 0.,
            mouse: MouseState::default(),
            epoch: Instant::now(),
            is_focused: false,
            is_dimmed: false,
            dim_from: 0.,
            dim_started: Instant::now(),
            shake_started: None,
            hovered_link: None,
            is_blink_hidden: false,
            autoscroll_task: Task::ready(()),
            blink_task: Task::ready(()),
            _events: events,
            _blur: blur,
            _activation: activation,
        };
        view.restart_blink(cx);
        view
    }

    fn window_subscriptions(
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Subscription, Subscription) {
        let blur = cx.on_blur(focus, window, |this, _window, cx| {
            if this.ime.is_composing() {
                this.ime.clear();
                cx.notify();
            }
        });
        // Another window in front: hollow, steady cursor (the focus itself stays on the view).
        let activation = cx.observe_window_activation(window, |this, _window, cx| {
            this.restart_blink(cx);
            cx.notify();
        });
        (blur, activation)
    }

    /// Follows the focus and activation of `window` after the view's tab moved there.
    pub fn attach_to_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (blur, activation) = Self::window_subscriptions(&self.focus, window, cx);
        self._blur = blur;
        self._activation = activation;
        self.measure(window);
        cx.notify();
    }

    /// Fades the text and cursor to the inactive look while another pane of the tab has the focus.
    pub fn set_dimmed(&mut self, is_dimmed: bool, cx: &mut Context<Self>) {
        if self.is_dimmed != is_dimmed {
            let now = Instant::now();
            self.dim_from = self.dim_level(now).0;
            self.dim_started = now;
            self.is_dimmed = is_dimmed;
            cx.notify();
        }
    }

    /// Current dim level (0 = focused look, 1 = fully dimmed) and whether it is still fading.
    fn dim_level(&self, now: Instant) -> (f32, bool) {
        let target = if self.is_dimmed { 1. } else { 0. };
        let t = now.saturating_duration_since(self.dim_started).as_secs_f32() / DIM_ANIMATION.as_secs_f32();
        if t >= 1. || self.dim_from == target {
            return (target, false);
        }
        let eased = 1. - (1. - t).powi(3);
        (self.dim_from + (target - self.dim_from) * eased, true)
    }

    /// Opacity of the text and of the cursor at dim level `level`.
    fn faded_alphas(&self, level: f32) -> (f32, f32) {
        let text = 1. - level * (1. - self.options.inactive_text_opacity);
        let cursor = if self.options.show_inactive_cursor { text } else { 1. - level };
        (text, cursor)
    }

    /// Measures the font's cell size now, so [`Self::fit`] need not wait for a paint.
    pub fn measure(&self, window: &Window) {
        let (_, _, cell) = frame::measure_cell(window, &self.font);
        self.frame.cell.set(Some(cell));
    }

    /// Sizes the grid for a pane of `width` x `height` logical pixels, independent of the drawn
    /// size, so hidden or animating panes do not resize the terminal.
    pub fn fit(&self, width: f32, height: f32) {
        let area = size(px(width), px(height));
        self.frame.target.set(Some(area));
        if let Some(cell) = self.frame.cell.get() {
            self.frame.fit(&self.terminal, area, cell);
        }
    }

    /// The terminal's grid size and when its columns or rows last changed; `None` until a size
    /// was applied.
    pub fn grid_change(&self) -> Option<GridChange> {
        let (cols, rows) = self.frame.grid_size.get();
        let changed_at = self.frame.grid_changed_at.get()?;
        (cols > 0).then_some(GridChange { cols, rows, changed_at })
    }

    /// Size in logical pixels a pane needs for a grid of `cols` x `rows` with the current font,
    /// padding included. `None` before the font was first measured.
    pub fn size_for_grid(&self, cols: u16, rows: u16) -> Option<(f32, f32)> {
        let cell = self.frame.cell.get()?;
        Some((
            f32::from(cell.width) * f32::from(cols) + PADDING_X * 2.,
            f32::from(cell.height) * f32::from(rows) + PADDING_Y * 2.,
        ))
    }

    /// Shows why the program could not start; Enter asks for another try ([`TerminalViewEvent::Restart`]).
    pub fn show_start_failure(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        let message = message.into();
        self.show_notices(&[Notice::error(message.clone()), Notice::info("Press Enter to try again")], cx);
        self.ended = Some(Ended::Failed(message));
    }

    fn handle_terminal_event(&mut self, event: TerminalEvent, cx: &mut Context<Self>) {
        match event {
            TerminalEvent::Wakeup => {
                self.terminal.ack_wakeup();
                if let Some(title) = self.terminal.take_title() {
                    cx.emit(TerminalViewEvent::TitleChanged(title));
                }
                if self.terminal.take_bell() {
                    if self.options.bell_visual {
                        self.shake_started = Some(Instant::now());
                    }
                    cx.emit(TerminalViewEvent::Bell);
                }
                cx.notify();
            }
            TerminalEvent::Ended(SessionEnd::Exited { code, is_output_complete }) => {
                // A start failure stays on screen whatever happens next.
                if self.ended.is_some() {
                    return;
                }
                // With output cut off the pane stays, so the user can see that it was.
                let close = is_output_complete
                    && match self.options.on_exit {
                        OnExit::Close => true,
                        OnExit::Keep => false,
                        OnExit::Auto => code == Some(0),
                    };
                if close {
                    cx.emit(TerminalViewEvent::Exited(code));
                } else {
                    self.show_ended(Ended::Exited { code, is_output_complete }, cx);
                }
            }
            TerminalEvent::Ended(SessionEnd::Failed(message)) => {
                if self.ended.is_none() {
                    self.show_ended(Ended::Failed(message), cx);
                }
            }
        }
    }

    /// Writes notices into this terminal's screen instead of showing overlays or dialogs.
    pub fn show_notices(&mut self, notices: &[Notice], cx: &mut Context<Self>) {
        self.terminal.write_notices(notices);
        cx.notify();
    }

    /// Whether mouse events go to the program: it asked for them and is still running. After it
    /// ended, the pane keeps its modes, but clicks select and the wheel scrolls.
    fn is_mouse_reported(&self) -> bool {
        self.ended.is_none() && self.terminal.lock().is_mouse_tracking()
    }

    /// Stops input and tells why the program ended.
    fn show_ended(&mut self, ended: Ended, cx: &mut Context<Self>) {
        let message = match &ended {
            Ended::Exited { code: Some(code), .. } => format!("Process exited with code {code}"),
            Ended::Exited { code: None, .. } => "Process exited".to_owned(),
            Ended::Failed(error) => format!("Terminal stopped: {error}"),
        };
        let mut notices = vec![Notice::error(message)];
        if matches!(ended, Ended::Exited { is_output_complete: false, .. }) {
            notices.push(Notice::warning("Its last output did not arrive in time and is not shown"));
        }
        notices.push(Notice::info("Press Enter to restart"));
        // The end is reported after the program's last output, so the notice comes after it.
        self.ended = Some(ended);
        self.show_notices(&notices, cx);
    }

    fn send_input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        if bytes.is_empty() || self.ended.is_some() {
            return;
        }
        // The write queue is bounded; a paste it turns away is reported on screen.
        if let Err(error) = self.terminal.write(bytes) {
            self.show_notices(&[Notice::error(error.to_string())], cx);
        }
        {
            let mut vt = self.terminal.lock();
            let _ = vt.clear_selection();
            if !vt.scrollbar().is_at_bottom() {
                vt.scroll_viewport(Scroll::Bottom);
            }
        }
        self.restart_blink(cx);
        cx.notify();
    }

    /// Shows the cursor and restarts the blink cycle, so it stays visible while typing.
    fn restart_blink(&mut self, cx: &mut Context<Self>) {
        self.is_blink_hidden = false;
        if !self.options.cursor_blink {
            self.blink_task = Task::ready(());
            return;
        }
        self.blink_task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CURSOR_BLINK_INTERVAL).await;
                let result = this.update(cx, |view, cx| {
                    if view.is_focused {
                        view.is_blink_hidden = !view.is_blink_hidden;
                        cx.notify();
                    }
                });
                if result.is_err() {
                    break;
                }
            }
        });
    }

    /// Applies new behavior settings (config reload).
    pub fn set_options(&mut self, options: ViewOptions, cx: &mut Context<Self>) {
        self.options = options;
        self.restart_blink(cx);
        cx.notify();
    }

    fn scroll(&mut self, scroll: Scroll, cx: &mut Context<Self>) {
        self.terminal.lock().scroll_viewport(scroll);
        cx.notify();
    }

    fn page_rows(&self) -> isize {
        let rows = isize::try_from(self.frame.grid_size.get().1).unwrap_or(isize::MAX);
        (rows - 1).max(1)
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Modifier-only presses never arrive here, so every key counts, as in Windows Terminal.
        // gpui hides the pointer itself only for keys that produce text or run an action.
        if self.options.hide_pointer_while_typing && window.is_window_hovered() {
            wired_platform::hide_pointer_until_mouse_moves();
        }
        let keystroke = &event.keystroke;
        let m = &keystroke.modifiers;
        if self.ended.is_some() {
            if keystroke.key == "enter" && !(m.control || m.alt || m.shift || m.platform) {
                cx.emit(TerminalViewEvent::Restart);
            }
            cx.stop_propagation();
            return;
        }
        // During composition every key belongs to the IME.
        if self.ime.is_composing() {
            return;
        }
        if m.platform {
            return;
        }
        let Some(key) = Key::from_name(&keystroke.key) else { return };
        // AltGr arrives as Ctrl+Alt with a printable character: that is text, not a chord.
        let is_altgr_text =
            m.control && m.alt && keystroke.key_char.as_deref().is_some_and(|c| !c.chars().any(char::is_control));
        if !(key.is_special() || ((m.control || m.alt) && !is_altgr_text)) {
            // Plain text comes through the input handler (`replace_text_in_range`).
            return;
        }
        let input = KeyInput { key, mods: mouse::to_mods(m), text: keystroke.key_char.clone() };
        let encoded = self.terminal.lock().encode_key(&input);
        if let Ok(bytes) = encoded
            && !bytes.is_empty()
        {
            self.send_input(bytes, cx);
            cx.stop_propagation();
        }
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(layout) = self.frame.layout.get() else { return };
        let lines = match event.delta {
            ScrollDelta::Lines(delta) => delta.y,
            ScrollDelta::Pixels(delta) => delta.y / layout.cell_height,
        } + self.scroll_remainder;
        let whole = lines.trunc();
        self.scroll_remainder = lines - whole;
        // Bounded so a bogus delta cannot flood the program with keys.
        let count = (whole.abs() as usize).min(layout.rows.max(1) * 3);
        if count == 0 {
            return;
        }
        let is_up = whole > 0.;
        if self.report_wheel(event.position, &event.modifiers, is_up, count) {
            return;
        }
        let is_alt_screen = self.ended.is_none() && {
            let vt = self.terminal.lock();
            ALT_SCREEN_MODES.iter().any(|&mode| vt.is_dec_mode_set(mode))
        };
        if is_alt_screen {
            // No scrollback on the alternate screen: send arrows like xterm's alternateScroll.
            let input = KeyInput { key: if is_up { Key::Up } else { Key::Down }, mods: Mods::default(), text: None };
            let encoded = self.terminal.lock().encode_key(&input);
            if let Ok(bytes) = encoded {
                let _ = self.terminal.write(bytes.repeat(count));
            }
        } else {
            let delta = isize::try_from(count).unwrap_or(isize::MAX);
            self.scroll(Scroll::Delta(if is_up { -delta } else { delta }), cx);
        }
    }

    /// As in Windows Terminal: with a selection, copy it and clear it; without one the key goes
    /// to the program, so Ctrl+C can be both copy and interrupt.
    fn copy(&mut self, _: &Copy, _window: &mut Window, cx: &mut Context<Self>) {
        if self.copy_selection(cx) {
            let _ = self.terminal.lock().clear_selection();
            cx.notify();
        } else {
            cx.propagate();
        }
    }

    fn select_all(&mut self, _: &SelectAll, _window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.terminal.lock().select_all();
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else { return };
        let is_bracketed = self.terminal.lock().is_dec_mode_set(BRACKETED_PASTE_MODE);
        let text = if is_bracketed { text.as_str() } else { paste::trim_single_line(&text) };
        if text.is_empty() {
            return;
        }
        // Untrusted clipboard text: `encode_paste` strips unsafe control bytes.
        // TODO: confirm unbracketed multi-line paste (`Vt::is_paste_safe`).
        let encoded = self.terminal.lock().encode_paste(text);
        if let Ok(bytes) = encoded {
            self.send_input(bytes, cx);
        }
    }

    /// Changes the terminal's default colors (config reload).
    pub fn set_colors(&self, colors: &wired_vt::DefaultColors) {
        let _ = self.terminal.lock().set_colors(colors);
    }

    /// Applies new font settings (config reload). The zoom level is reset to the new size.
    pub fn set_font(&mut self, font: FontConfig, cx: &mut Context<Self>) {
        self.default_font_size = font.size;
        self.font = font;
        cx.notify();
    }

    fn set_font_size(&mut self, font_size: f32, cx: &mut Context<Self>) {
        self.font.size = font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        cx.notify();
    }

    fn zoom_in(&mut self, _: &ZoomIn, _window: &mut Window, cx: &mut Context<Self>) {
        self.set_font_size(self.font.size + 1., cx);
    }

    fn zoom_out(&mut self, _: &ZoomOut, _window: &mut Window, cx: &mut Context<Self>) {
        self.set_font_size(self.font.size - 1., cx);
    }

    fn reset_zoom(&mut self, _: &ResetZoom, _window: &mut Window, cx: &mut Context<Self>) {
        self.set_font_size(self.default_font_size, cx);
    }

    fn scroll_up(&mut self, _: &ScrollUp, _window: &mut Window, cx: &mut Context<Self>) {
        self.scroll(Scroll::Delta(-1), cx);
    }

    fn scroll_down(&mut self, _: &ScrollDown, _window: &mut Window, cx: &mut Context<Self>) {
        self.scroll(Scroll::Delta(1), cx);
    }

    fn scroll_page_up(&mut self, _: &ScrollPageUp, _window: &mut Window, cx: &mut Context<Self>) {
        self.scroll(Scroll::Delta(-self.page_rows()), cx);
    }

    fn scroll_page_down(&mut self, _: &ScrollPageDown, _window: &mut Window, cx: &mut Context<Self>) {
        self.scroll(Scroll::Delta(self.page_rows()), cx);
    }

    fn scroll_to_top(&mut self, _: &ScrollToTop, _window: &mut Window, cx: &mut Context<Self>) {
        self.scroll(Scroll::Top, cx);
    }

    fn scroll_to_bottom(&mut self, _: &ScrollToBottom, _window: &mut Window, cx: &mut Context<Self>) {
        self.scroll(Scroll::Bottom, cx);
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let preedit = &self.ime.preedit;
        let end = range.end.min(ime::len_utf16(preedit));
        let start = range.start.min(end);
        *adjusted_range = Some(start..end);
        Some(preedit[ime::byte_offset(preedit, start)..ime::byte_offset(preedit, end)].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let caret = if self.ime.is_composing() { self.ime.caret_utf16 } else { 0 };
        Some(UTF16Selection { range: caret..caret, reversed: false })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.ime.is_composing().then(|| 0..ime::len_utf16(&self.ime.preedit))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.ime.clear();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Control characters from WM_CHAR (Enter, Tab, Ctrl+letter) were already encoded from the
        // key event; dropping them here prevents double input.
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        self.ime.clear();
        self.send_input(text.into_bytes(), cx);
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let len = ime::len_utf16(new_text);
        self.ime.preedit = new_text.to_string();
        self.ime.caret_utf16 = new_selected_range.map_or(len, |r| r.end).min(len);
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.frame.layout.get()?;
        let (anchor_row, anchor_col) = layout.anchor;
        let (row, col) = ime::position_of(&self.ime.preedit, range_utf16.start, anchor_col, layout.cols);
        Some(Bounds::new(
            point(
                layout.origin.x + layout.cell_width * col as f32,
                layout.origin.y + layout.cell_height * (anchor_row + row) as f32,
            ),
            size(layout.cell_width, layout.cell_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

struct Prepared {
    fonts: FontSet,
    metrics: Metrics,
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_focused = self.focus.is_focused(window) && window.is_window_active();
        self.is_focused = is_focused;
        let is_blink_hidden = is_focused && self.is_blink_hidden;
        let (dim_level, is_fading) = self.dim_level(Instant::now());
        let (text_alpha, cursor_alpha) = self.faded_alphas(dim_level);
        let shake = self.shake_started.and_then(|at| bell::shake_offset(at.elapsed()));
        if shake.is_none() {
            self.shake_started = None;
        }
        if is_fading || shake.is_some() {
            window.request_animation_frame();
        }
        let terminal = Arc::clone(&self.terminal);
        let snapshot = Rc::clone(&self.snapshot);
        let frame = Rc::clone(&self.frame);
        let frame_for_paint = Rc::clone(&self.frame);
        let snapshot_for_paint = Rc::clone(&self.snapshot);
        let breaks_ligatures_at_cursor = self.font.ligatures == Ligatures::BreakAtCursor;
        let font = self.font.clone();
        let ime = self.ime.clone();
        let link_segments = self.hovered_link.as_ref().map(|l| l.segments.clone()).unwrap_or_default();
        let wants_link = self.mouse.wants_link();
        let view = cx.entity();
        let focus = self.focus.clone();
        // The pointer left through the window edge: no move event says it left the zone.
        if !window.is_window_hovered() && self.scrollbar.drag_grab.is_none() {
            self.scrollbar.set_wide(false);
        }
        let show_scrollbar = self.options.scrollbar;
        let view_colors = self.options.colors;
        // As in xterm.js: I-beam over text, the normal pointer over the scrollbar and while the
        // program receives mouse events.
        let pointer = if self.hovered_link.is_some() {
            gpui::CursorStyle::PointingHand
        } else if self.scrollbar.is_wide() || self.is_mouse_reported() {
            gpui::CursorStyle::Arrow
        } else {
            gpui::CursorStyle::IBeam
        };
        let (scrollbar_width, is_scrollbar_animating) = self.scrollbar.width(Instant::now());
        let scrollbar_color =
            gpui::Hsla { a: self.scrollbar.alpha(), ..paint::to_hsla(self.snapshot.borrow().colors.foreground) };

        let grid = canvas(
            move |bounds, window, _cx| {
                let (fonts, metrics, cell) = frame::measure_cell(window, &font);
                frame.cell.set(Some(cell));
                frame.fit(&terminal, frame.target.get().unwrap_or(bounds.size), cell);
                let _ = terminal.lock().snapshot(&mut snapshot.borrow_mut());
                Prepared { fonts, metrics }
            },
            move |bounds, prepared, window, cx| {
                let snapshot = snapshot_for_paint.borrow();
                let painter = Painter {
                    colors: &snapshot.colors,
                    view_colors,
                    fonts: &prepared.fonts,
                    metrics: &prepared.metrics,
                    text_alpha,
                    cursor_alpha,
                };
                let m = prepared.metrics;
                let grid_origin = bounds.origin + point(px(PADDING_X), px(PADDING_Y));
                // Only the drawing shakes; input and the IME keep the grid where it is.
                let origin = grid_origin + point(px(shake.unwrap_or(0.)), px(0.));
                let row_origin = |row: usize| point(origin.x, origin.y + m.cell_height * row as f32);
                let cursor = snapshot.cursor;
                let break_at = |row: usize| {
                    cursor
                        .filter(|c| breaks_ligatures_at_cursor && usize::from(c.row) == row)
                        .map(|c| usize::from(c.col))
                };

                window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                    for (i, row) in snapshot.rows.iter().enumerate() {
                        painter.paint_backgrounds(window, row_origin(i), row);
                    }
                    for (i, row) in snapshot.rows.iter().enumerate() {
                        painter.paint_text(window, cx, row_origin(i), row, break_at(i), None);
                        painter.paint_decorations(window, row_origin(i), row);
                    }
                    painter.paint_link_underline(window, origin, &link_segments);
                    if !ime.is_composing()
                        && let Some(cursor) = cursor
                        && !(is_blink_hidden && cursor.is_blinking)
                    {
                        let row = usize::from(cursor.row);
                        let style = if is_focused { cursor.style } else { CursorStyle::BlockHollow };
                        painter.paint_cursor(window, cx, origin, &snapshot, Cursor { style, ..cursor }, break_at(row));
                    }

                    let cols = usize::from(snapshot.cols).max(2);
                    let mut anchor = cursor.map_or((snapshot.rows.len().saturating_sub(1), 0), |c| {
                        (usize::from(c.row), usize::from(c.col))
                    });
                    if ime.is_composing() {
                        // The candidate window positions come from the same anchor, so they follow.
                        let rows = ime::row_count(&ime.preedit, anchor.1, cols);
                        anchor.0 = ime::overlay_top(anchor.0, rows, snapshot.rows.len());
                        painter.paint_preedit(window, cx, row_origin(anchor.0), &ime, anchor.1, cols);
                    }
                    if show_scrollbar && let Some(thumb) = scrollbar::thumb(bounds, snapshot.scrollbar) {
                        scrollbar::paint(window, bounds, thumb, scrollbar_width, scrollbar_color);
                        if is_scrollbar_animating {
                            window.request_animation_frame();
                        }
                    }
                    let layout = Layout {
                        bounds,
                        origin: grid_origin,
                        cell_width: m.cell_width,
                        cell_height: m.cell_height,
                        cols,
                        rows: snapshot.rows.len(),
                        anchor,
                        scale: window.scale_factor(),
                    };
                    // Output, scrolling or a new scale can move the preedit while its text stays
                    // the same; the candidate window has to follow it.
                    if frame_for_paint.layout.replace(Some(layout)) != Some(layout) && ime.is_composing() {
                        window.invalidate_character_coordinates();
                    }
                    // New output, a scroll or a move under a still pointer with Ctrl held: the
                    // link under it is looked up again on what was just drawn.
                    if wants_link
                        && frame_for_paint.hover_checked.get() != Some((snapshot.generation, Some(grid_origin)))
                    {
                        let view = view.clone();
                        cx.defer(move |cx| view.update(cx, |this, cx| this.recheck_link(cx)));
                    }
                });
                window.handle_input(&focus, ElementInputHandler::new(bounds, view.clone()), cx);
                // Window-level so drags keep working outside the grid (the window captures the
                // mouse while a button is down).
                let move_view = view.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
                    if phase == gpui::DispatchPhase::Bubble {
                        move_view.update(cx, |this, cx| this.on_mouse_move(event, cx));
                    }
                });
                let up_view = view.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
                    if phase == gpui::DispatchPhase::Bubble {
                        up_view.update(cx, |this, cx| this.on_mouse_up(event, cx));
                    }
                });
            },
        )
        .size_full();

        div()
            .id("terminal")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .cursor(pointer)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll(cx.listener(Self::on_scroll))
            .on_any_mouse_down(cx.listener(Self::on_mouse_down))
            .on_modifiers_changed(cx.listener(Self::on_modifiers_changed))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::reset_zoom))
            .on_action(cx.listener(Self::scroll_up))
            .on_action(cx.listener(Self::scroll_down))
            .on_action(cx.listener(Self::scroll_page_up))
            .on_action(cx.listener(Self::scroll_page_down))
            .on_action(cx.listener(Self::scroll_to_top))
            .on_action(cx.listener(Self::scroll_to_bottom))
            .child(grid)
    }
}
