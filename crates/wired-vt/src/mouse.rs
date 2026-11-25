use std::{
    mem::zeroed,
    ptr::{null, null_mut},
};

use wired_vt_sys as sys;

use crate::{Error, Mods, Result, Vt, check};

/// Pointer position on the grid in cells from its top-left corner. Fractional; may lie outside
/// the grid while dragging.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GridPos {
    /// Columns from the left edge.
    pub x: f64,
    /// Rows from the top edge.
    pub y: f64,
}

/// Where a selection drag wants the viewport to scroll.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Autoscroll {
    /// The pointer is inside the grid.
    #[default]
    None,
    /// The pointer is above the grid.
    Up,
    /// The pointer is below the grid.
    Down,
}

/// A mouse button as reported to programs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    /// Primary button.
    Left,
    /// Wheel button.
    Middle,
    /// Secondary button.
    Right,
    /// Wheel rotated away from the user.
    WheelUp,
    /// Wheel rotated toward the user.
    WheelDown,
}

impl MouseButton {
    fn to_ghostty(self) -> sys::GhosttyMouseButton {
        match self {
            Self::Left => sys::GHOSTTY_MOUSE_BUTTON_LEFT,
            Self::Middle => sys::GHOSTTY_MOUSE_BUTTON_MIDDLE,
            Self::Right => sys::GHOSTTY_MOUSE_BUTTON_RIGHT,
            Self::WheelUp => sys::GHOSTTY_MOUSE_BUTTON_FOUR,
            Self::WheelDown => sys::GHOSTTY_MOUSE_BUTTON_FIVE,
        }
    }
}

/// What happened to the mouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    /// A button went down (wheel steps are presses).
    Press,
    /// A button went up.
    Release,
    /// The pointer moved.
    Motion,
}

#[derive(Debug)]
pub(crate) struct MouseHandles {
    gesture: sys::GhosttySelectionGesture,
    press: sys::GhosttySelectionGestureEvent,
    drag: sys::GhosttySelectionGestureEvent,
    release: sys::GhosttySelectionGestureEvent,
    tick: sys::GhosttySelectionGestureEvent,
    encoder: sys::GhosttyMouseEncoder,
    event: sys::GhosttyMouseEvent,
    last_motion_cell: Option<(u16, u32)>,
}

impl MouseHandles {
    pub(crate) fn null() -> Self {
        Self {
            gesture: null_mut(),
            press: null_mut(),
            drag: null_mut(),
            release: null_mut(),
            tick: null_mut(),
            encoder: null_mut(),
            event: null_mut(),
            last_motion_cell: None,
        }
    }

    /// # Safety
    /// Must be called once, on handles that are all null.
    pub(crate) unsafe fn create(&mut self) -> Result<()> {
        // SAFETY: each constructor writes a fresh handle into its out-pointer.
        unsafe {
            check(sys::ghostty_selection_gesture_new(null(), &mut self.gesture), "selection_gesture_new")?;
            for (event, kind) in [
                (&mut self.press, sys::GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_PRESS),
                (&mut self.drag, sys::GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_DRAG),
                (&mut self.release, sys::GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_RELEASE),
                (&mut self.tick, sys::GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_AUTOSCROLL_TICK),
            ] {
                check(sys::ghostty_selection_gesture_event_new(null(), event, kind), "gesture_event_new")?;
            }
            check(sys::ghostty_mouse_encoder_new(null(), &mut self.encoder), "mouse_encoder_new")?;
            check(sys::ghostty_mouse_event_new(null(), &mut self.event), "mouse_event_new")?;
        }
        Ok(())
    }

    /// # Safety
    /// `terminal` must be the live terminal these handles were used with. Call once.
    pub(crate) unsafe fn free(&mut self, terminal: sys::GhosttyTerminal) {
        // SAFETY: every handle is either null (skipped by the free functions) or was created by
        // its `_new` function; the gesture releases its tracked refs through `terminal`.
        unsafe {
            sys::ghostty_mouse_event_free(self.event);
            sys::ghostty_mouse_encoder_free(self.encoder);
            for event in [self.press, self.drag, self.release, self.tick] {
                sys::ghostty_selection_gesture_event_free(event);
            }
            sys::ghostty_selection_gesture_free(self.gesture, terminal);
        }
    }
}

impl Vt {
    fn cell_at(&self, pos: GridPos) -> (u16, u32) {
        let clamp = |v: f64, len: u32| {
            if v.is_finite() { v.floor().clamp(0., f64::from(len.saturating_sub(1))) as u32 } else { 0 }
        };
        let col = clamp(pos.x, u32::from(self.cols));
        (u16::try_from(col).unwrap_or(u16::MAX), clamp(pos.y, u32::from(self.rows)))
    }

    fn surface_pos(&self, pos: GridPos) -> sys::GhosttySurfacePosition {
        let finite = |v: f64| if v.is_finite() { v } else { 0. };
        sys::GhosttySurfacePosition {
            x: finite(pos.x) * f64::from(self.cell_width_px),
            y: finite(pos.y) * f64::from(self.cell_height_px),
        }
    }

    fn grid_ref(&self, (col, row): (u16, u32)) -> Option<sys::GhosttyGridRef> {
        // SAFETY: plain data; zeroed is a valid value for both.
        let (mut point, mut grid_ref): (sys::GhosttyPoint, sys::GhosttyGridRef) = unsafe { (zeroed(), zeroed()) };
        point.tag = sys::GHOSTTY_POINT_TAG_VIEWPORT;
        point.value.coordinate = sys::GhosttyPointCoordinate { x: col, y: row };
        // SAFETY: valid handle and out-pointer.
        let code = unsafe { sys::ghostty_terminal_grid_ref(self.terminal, point, &mut grid_ref) };
        (code == sys::GHOSTTY_SUCCESS).then_some(grid_ref)
    }

    /// The hyperlink (OSC 8) the program attached to viewport cell (`col`, `row`), if any and
    /// if it is valid UTF-8 of at most 8 KiB. Program data: treat it as untrusted.
    pub fn hyperlink_uri(&self, col: u16, row: u16) -> Option<String> {
        const MAX_URI: usize = 8 * 1024;
        let grid_ref = self.grid_ref((col, u32::from(row)))?;
        let mut len = 0usize;
        // SAFETY: valid ref; a null buffer of length 0 only asks for the size.
        let code = unsafe { sys::ghostty_grid_ref_hyperlink_uri(&grid_ref, null_mut(), 0, &mut len) };
        if code != sys::GHOSTTY_OUT_OF_SPACE || len == 0 || len > MAX_URI {
            return None;
        }
        let mut buf = vec![0u8; len];
        let mut written = 0usize;
        // SAFETY: valid ref; the buffer is passed with its capacity.
        let code = unsafe { sys::ghostty_grid_ref_hyperlink_uri(&grid_ref, buf.as_mut_ptr(), buf.len(), &mut written) };
        if code != sys::GHOSTTY_SUCCESS {
            return None;
        }
        buf.truncate(written.min(len));
        String::from_utf8(buf).ok()
    }

    fn gesture_geometry(&self) -> sys::GhosttySelectionGestureGeometry {
        sys::GhosttySelectionGestureGeometry {
            columns: u32::from(self.cols),
            cell_width: self.cell_width_px,
            padding_left: 0,
            screen_height: u32::from(self.rows) * self.cell_height_px,
        }
    }

    /// `T` must be the type libghostty documents for `option`.
    fn set_gesture_option<T>(
        event: sys::GhosttySelectionGestureEvent,
        option: sys::GhosttySelectionGestureEventOption,
        value: &T,
    ) -> Result<()> {
        // SAFETY: valid event handle; libghostty copies the value during the call.
        check(
            unsafe { sys::ghostty_selection_gesture_event_set(event, option, (&raw const *value).cast()) },
            "gesture option",
        )
    }

    fn apply_gesture(&mut self, event: sys::GhosttySelectionGestureEvent) -> Result<bool> {
        // SAFETY: plain data out-parameter.
        let mut selection: sys::GhosttySelection = unsafe { zeroed() };
        // SAFETY: valid handles; the event's required options were set by the caller.
        let code =
            unsafe { sys::ghostty_selection_gesture_event(self.mouse.gesture, self.terminal, event, &mut selection) };
        match code {
            sys::GHOSTTY_SUCCESS => {
                self.set_selection(Some(&selection))?;
                Ok(true)
            }
            sys::GHOSTTY_NO_VALUE => Ok(false),
            code => Err(Error::Ghostty { op: "selection_gesture_event", code }),
        }
    }

    fn set_selection(&mut self, selection: Option<&sys::GhosttySelection>) -> Result<()> {
        let value = selection.map_or(null(), |s| (&raw const *s).cast());
        // SAFETY: valid handle; the selection is copied (and tracked) by libghostty, NULL clears.
        check(
            unsafe { sys::ghostty_terminal_set(self.terminal, sys::GHOSTTY_TERMINAL_OPT_SELECTION, value) },
            "set selection",
        )
    }

    fn gesture_data<T: Default>(&self, data: sys::GhosttySelectionGestureData) -> T {
        let mut value = T::default();
        // SAFETY: valid handles; `T` is the type documented for `data` at every call site.
        unsafe { sys::ghostty_selection_gesture_get(self.mouse.gesture, self.terminal, data, (&raw mut value).cast()) };
        value
    }

    /// Left button pressed for selection. `time_ns` is a monotonic timestamp; presses within
    /// `repeat_interval_ns` near the previous one count as double/triple clicks (word/line).
    pub fn selection_press(&mut self, pos: GridPos, time_ns: u64, repeat_interval_ns: u64) -> Result<()> {
        let Some(grid_ref) = self.grid_ref(self.cell_at(pos)) else { return Ok(()) };
        let event = self.mouse.press;
        let position = self.surface_pos(pos);
        let repeat_distance = f64::from(self.cell_width_px);
        Self::set_gesture_option(event, sys::GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF, &grid_ref)?;
        Self::set_gesture_option(event, sys::GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION, &position)?;
        Self::set_gesture_option(event, sys::GHOSTTY_SELECTION_GESTURE_EVENT_OPT_TIME_NS, &time_ns)?;
        Self::set_gesture_option(
            event,
            sys::GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_INTERVAL_NS,
            &repeat_interval_ns,
        )?;
        Self::set_gesture_option(event, sys::GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_DISTANCE, &repeat_distance)?;
        let is_selected = self.apply_gesture(event)?;
        // A plain single click only sets the anchor; it clears an old selection like Ghostty.
        if !is_selected && self.gesture_data::<u8>(sys::GHOSTTY_SELECTION_GESTURE_DATA_CLICK_COUNT) == 1 {
            self.set_selection(None)?;
        }
        Ok(())
    }

    /// Pointer moved with the left button held after [`Vt::selection_press`].
    pub fn selection_drag(&mut self, pos: GridPos, is_rectangle: bool) -> Result<Autoscroll> {
        if !self.is_selecting() {
            return Ok(Autoscroll::None);
        }
        let Some(grid_ref) = self.grid_ref(self.cell_at(pos)) else { return Ok(Autoscroll::None) };
        let event = self.mouse.drag;
        let position = self.surface_pos(pos);
        let geometry = self.gesture_geometry();
        Self::set_gesture_option(event, sys::GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF, &grid_ref)?;
        Self::set_gesture_option(event, sys::GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION, &position)?;
        Self::set_gesture_option(event, sys::GHOSTTY_SELECTION_GESTURE_EVENT_OPT_GEOMETRY, &geometry)?;
        Self::set_gesture_option(event, sys::GHOSTTY_SELECTION_GESTURE_EVENT_OPT_RECTANGLE, &is_rectangle)?;
        self.apply_gesture(event)?;
        Ok(self.autoscroll())
    }

    /// Scrolls one row in the current autoscroll direction and extends the selection. Returns
    /// `false` when autoscrolling should stop.
    pub fn selection_autoscroll_tick(&mut self, pos: GridPos, is_rectangle: bool) -> Result<bool> {
        if self.autoscroll() == Autoscroll::None {
            return Ok(false);
        }
        self.generation += 1;
        let (x, y) = self.cell_at(pos);
        let event = self.mouse.tick;
        let viewport = sys::GhosttyPointCoordinate { x, y };
        let position = self.surface_pos(pos);
        let geometry = self.gesture_geometry();
        Self::set_gesture_option(event, sys::GHOSTTY_SELECTION_GESTURE_EVENT_OPT_VIEWPORT, &viewport)?;
        Self::set_gesture_option(event, sys::GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION, &position)?;
        Self::set_gesture_option(event, sys::GHOSTTY_SELECTION_GESTURE_EVENT_OPT_GEOMETRY, &geometry)?;
        Self::set_gesture_option(event, sys::GHOSTTY_SELECTION_GESTURE_EVENT_OPT_RECTANGLE, &is_rectangle)?;
        self.apply_gesture(event)
    }

    /// Left button released.
    pub fn selection_release(&mut self, pos: GridPos) -> Result<()> {
        if !self.is_selecting() {
            return Ok(());
        }
        let event = self.mouse.release;
        match self.grid_ref(self.cell_at(pos)) {
            Some(grid_ref) => Self::set_gesture_option(event, sys::GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF, &grid_ref)?,
            // SAFETY: valid event handle; NULL clears the option ("not over a cell").
            None => unsafe {
                sys::ghostty_selection_gesture_event_set(event, sys::GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF, null());
            },
        }
        self.apply_gesture(event)?;
        Ok(())
    }

    fn is_selecting(&self) -> bool {
        self.gesture_data::<u8>(sys::GHOSTTY_SELECTION_GESTURE_DATA_CLICK_COUNT) > 0
    }

    fn autoscroll(&self) -> Autoscroll {
        match self
            .gesture_data::<sys::GhosttySelectionGestureAutoscroll>(sys::GHOSTTY_SELECTION_GESTURE_DATA_AUTOSCROLL)
        {
            sys::GHOSTTY_SELECTION_GESTURE_AUTOSCROLL_UP => Autoscroll::Up,
            sys::GHOSTTY_SELECTION_GESTURE_AUTOSCROLL_DOWN => Autoscroll::Down,
            _ => Autoscroll::None,
        }
    }

    /// Cancels the click sequence (mouse reporting took over).
    pub fn reset_selection_gesture(&mut self) {
        // SAFETY: valid handles.
        unsafe { sys::ghostty_selection_gesture_reset(self.mouse.gesture, self.terminal) };
    }

    /// Selects all screen and scrollback content.
    pub fn select_all(&mut self) -> Result<()> {
        // SAFETY: plain data out-parameter.
        let mut selection: sys::GhosttySelection = unsafe { zeroed() };
        // SAFETY: valid handle and out-pointer.
        match unsafe { sys::ghostty_terminal_select_all(self.terminal, &mut selection) } {
            sys::GHOSTTY_SUCCESS => self.set_selection(Some(&selection)),
            sys::GHOSTTY_NO_VALUE => Ok(()),
            code => Err(Error::Ghostty { op: "select_all", code }),
        }
    }

    /// Removes the selection.
    pub fn clear_selection(&mut self) -> Result<()> {
        if self.selection().is_none() {
            return Ok(());
        }
        self.set_selection(None)
    }

    fn selection(&self) -> Option<sys::GhosttySelection> {
        // SAFETY: plain data out-parameter.
        let mut selection: sys::GhosttySelection = unsafe { zeroed() };
        // SAFETY: valid handle; DATA_SELECTION writes a GhosttySelection or returns NO_VALUE.
        let code = unsafe {
            sys::ghostty_terminal_get(self.terminal, sys::GHOSTTY_TERMINAL_DATA_SELECTION, (&raw mut selection).cast())
        };
        (code == sys::GHOSTTY_SUCCESS).then_some(selection)
    }

    /// The selected text as it should go to the clipboard: soft-wrapped lines joined, trailing
    /// spaces trimmed. `None` without a selection.
    pub fn selection_text(&self) -> Result<Option<String>> {
        let Some(selection) = self.selection() else { return Ok(None) };
        let mut formatter: sys::GhosttyFormatter = null_mut();
        // SAFETY: valid terminal; the formatter is freed below on every path.
        check(unsafe { sys::ghostty_formatter_terminal_new(null(), &mut formatter, self.terminal) }, "formatter_new")?;
        let result = (|| {
            let emit: sys::GhosttyFormatterFormat = sys::GHOSTTY_FORMATTER_FORMAT_PLAIN;
            let yes = true;
            // SAFETY: valid formatter; each value has the type documented for its option and is
            // copied during the call.
            unsafe {
                let set = |option, value: *const std::ffi::c_void| sys::ghostty_formatter_set(formatter, option, value);
                check(set(sys::GHOSTTY_FORMATTER_OPT_EMIT, (&raw const emit).cast()), "formatter emit")?;
                check(set(sys::GHOSTTY_FORMATTER_OPT_UNWRAP, (&raw const yes).cast()), "formatter unwrap")?;
                check(set(sys::GHOSTTY_FORMATTER_OPT_TRIM, (&raw const yes).cast()), "formatter trim")?;
                check(set(sys::GHOSTTY_FORMATTER_OPT_SELECTION, (&raw const selection).cast()), "formatter selection")?;
            }
            let mut ptr: *mut u8 = null_mut();
            let mut len = 0usize;
            // SAFETY: valid formatter; on success `ptr` owns `len` bytes from the default allocator.
            check(unsafe { sys::ghostty_formatter_format_alloc(formatter, null(), &mut ptr, &mut len) }, "format")?;
            if ptr.is_null() {
                return Ok(Some(String::new()));
            }
            // SAFETY: `ptr` is valid for `len` bytes until freed right after copying.
            let text = unsafe {
                let text = String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len)).into_owned();
                sys::ghostty_free(null(), ptr, len);
                text
            };
            Ok(Some(text))
        })();
        // SAFETY: created above, freed once.
        unsafe { sys::ghostty_formatter_free(formatter) };
        result
    }

    /// Whether the program enabled any mouse tracking mode.
    pub fn is_mouse_tracking(&self) -> bool {
        let mut value = false;
        // SAFETY: valid handle; DATA_MOUSE_TRACKING writes a bool.
        unsafe {
            sys::ghostty_terminal_get(self.terminal, sys::GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING, (&raw mut value).cast())
        };
        value
    }

    /// Encodes a mouse event in the tracking mode and format the program enabled. Empty when the
    /// event is not reported (wrong mode, or motion within the last reported cell).
    pub fn encode_mouse(
        &mut self,
        action: MouseAction,
        button: Option<MouseButton>,
        mods: Mods,
        pos: GridPos,
        is_any_button_pressed: bool,
    ) -> Result<Vec<u8>> {
        let cell = self.cell_at(pos);
        if action == MouseAction::Motion {
            if self.mouse.last_motion_cell == Some(cell) {
                return Ok(Vec::new());
            }
            self.mouse.last_motion_cell = Some(cell);
        } else {
            self.mouse.last_motion_cell = None;
        }
        // SAFETY: sized plain-data struct; `size` is set as required.
        let mut size: sys::GhosttyMouseEncoderSize = unsafe { zeroed() };
        size.size = std::mem::size_of::<sys::GhosttyMouseEncoderSize>();
        size.cell_width = self.cell_width_px;
        size.cell_height = self.cell_height_px;
        size.screen_width = u32::from(self.cols) * size.cell_width;
        size.screen_height = u32::from(self.rows) * size.cell_height;
        let position = self.surface_pos(pos);
        let (encoder, event) = (self.mouse.encoder, self.mouse.event);
        let ghostty_action = match action {
            MouseAction::Press => sys::GHOSTTY_MOUSE_ACTION_PRESS,
            MouseAction::Release => sys::GHOSTTY_MOUSE_ACTION_RELEASE,
            MouseAction::Motion => sys::GHOSTTY_MOUSE_ACTION_MOTION,
        };
        // SAFETY: valid handles; option values have their documented types and are copied.
        unsafe {
            check(sys::ghostty_mouse_encoder_set_from_terminal(encoder, self.terminal), "mouse encoder sync")?;
            check(
                sys::ghostty_mouse_encoder_set(encoder, sys::GHOSTTY_MOUSE_ENCODER_OPT_SIZE, (&raw const size).cast()),
                "mouse encoder size",
            )?;
            check(
                sys::ghostty_mouse_encoder_set(
                    encoder,
                    sys::GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED,
                    (&raw const is_any_button_pressed).cast(),
                ),
                "mouse encoder buttons",
            )?;
            check(sys::ghostty_mouse_event_set_action(event, ghostty_action), "mouse action")?;
            match button {
                Some(button) => check(sys::ghostty_mouse_event_set_button(event, button.to_ghostty()), "mouse button")?,
                None => check(sys::ghostty_mouse_event_clear_button(event), "mouse button")?,
            }
            check(sys::ghostty_mouse_event_set_mods(event, mods.to_ghostty()), "mouse mods")?;
            check(
                sys::ghostty_mouse_event_set_position(
                    event,
                    sys::GhosttyMousePosition { x: position.x as f32, y: position.y as f32 },
                ),
                "mouse position",
            )?;
            let mut buf = [0u8; 128];
            let mut written = 0usize;
            check(
                sys::ghostty_mouse_encoder_encode(encoder, event, buf.as_mut_ptr(), buf.len(), &mut written),
                "mouse encode",
            )?;
            Ok(buf[..written.min(buf.len())].to_vec())
        }
    }
}
