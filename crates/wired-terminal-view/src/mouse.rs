//! Mouse input: local selection, or mouse reports when the program enabled tracking.
//!
//! Follows Ghostty's rules: with tracking on, presses go to the program unless Shift is held, in
//! which case the left button selects locally; a drag keeps the role its press had.

use std::time::Duration;

use gpui::{
    Bounds, ClipboardItem, Context, Modifiers, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Window,
};
use wired_vt::{Autoscroll, GridPos, Mods, MouseAction, MouseButton};

use crate::{
    Paste, RightClick, TerminalView,
    scrollbar::{self, Thumb},
};

/// Clicks closer together than this count as double/triple clicks (Windows default).
const REPEAT_CLICK_INTERVAL: Duration = Duration::from_millis(500);
/// How often a drag beyond the top or bottom edge scrolls one row.
const AUTOSCROLL_INTERVAL: Duration = Duration::from_millis(15);

/// What a held button is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Press {
    #[default]
    None,
    /// Selecting text (left button only).
    Local,
    /// Reported to the program.
    Reported,
}

/// Mouse state of a terminal view.
#[derive(Debug, Default)]
pub(crate) struct MouseState {
    /// Left, middle, right.
    presses: [Press; 3],
    last_pos: GridPos,
    /// Last pointer position over this view (window coordinates), for Ctrl pressed without a move.
    pointer: Option<Point<Pixels>>,
    /// Ctrl was held at the last pointer or modifier event.
    is_ctrl_held: bool,
    is_rectangle: bool,
    is_autoscrolling: bool,
}

impl MouseState {
    /// Whether a link under the pointer should be underlined.
    pub(crate) fn wants_link(&self) -> bool {
        self.is_ctrl_held && self.pointer.is_some() && self.presses == [Press::None; 3]
    }
}

fn button_index(button: gpui::MouseButton) -> Option<(usize, MouseButton)> {
    match button {
        gpui::MouseButton::Left => Some((0, MouseButton::Left)),
        gpui::MouseButton::Middle => Some((1, MouseButton::Middle)),
        gpui::MouseButton::Right => Some((2, MouseButton::Right)),
        gpui::MouseButton::Navigate(_) => None,
    }
}

const BUTTONS: [MouseButton; 3] = [MouseButton::Left, MouseButton::Middle, MouseButton::Right];

pub(crate) fn to_mods(m: &Modifiers) -> Mods {
    Mods { shift: m.shift, ctrl: m.control, alt: m.alt, super_key: false }
}

impl TerminalView {
    /// Pointer position in cells relative to the grid, `None` before the first paint.
    fn grid_pos(&self, position: Point<Pixels>) -> Option<GridPos> {
        let layout = self.frame.layout.get()?;
        let offset = position - layout.origin;
        Some(GridPos { x: f64::from(offset.x / layout.cell_width), y: f64::from(offset.y / layout.cell_height) })
    }

    fn is_over_grid(&self, pos: GridPos) -> bool {
        self.frame.layout.get().is_some_and(|layout| {
            (0. ..layout.cols as f64).contains(&pos.x) && (0. ..layout.rows as f64).contains(&pos.y)
        })
    }

    fn now_ns(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    fn report(&mut self, action: MouseAction, button: Option<MouseButton>, mods: Mods, pos: GridPos) {
        let is_any_pressed = self.mouse.presses.contains(&Press::Reported);
        let encoded = self.terminal.lock().encode_mouse(action, button, mods, pos, is_any_pressed);
        if let Ok(bytes) = encoded {
            let _ = self.terminal.write(bytes);
        }
    }

    fn cell_under(&self, pos: GridPos) -> Option<(u16, u16)> {
        if !self.is_over_grid(pos) {
            return None;
        }
        // On the grid, so both are small and non-negative.
        Some((pos.x.floor() as u16, pos.y.floor() as u16))
    }

    pub(crate) fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if event.button == gpui::MouseButton::Left && self.scrollbar_press(event.position, cx) {
            return;
        }
        let Some((index, button)) = button_index(event.button) else { return };
        let Some(pos) = self.grid_pos(event.position) else { return };
        // Ctrl+click on a link opens it, even while the program takes mouse reports.
        if button == MouseButton::Left
            && event.modifiers.control
            && let Some((col, row)) = self.cell_under(pos)
            && let Some(link) = self.link_at(col, row)
        {
            self.hover_link(None, cx);
            self.open_link(link.uri, window, cx);
            return;
        }
        let mods = to_mods(&event.modifiers);
        let is_reporting = self.is_mouse_reported() && !mods.shift;
        if is_reporting {
            {
                let mut vt = self.terminal.lock();
                let _ = vt.clear_selection();
                vt.reset_selection_gesture();
            }
            self.mouse.presses[index] = Press::Reported;
            self.report(MouseAction::Press, Some(button), mods, pos);
            cx.notify();
            return;
        }
        match button {
            MouseButton::Left => {
                self.mouse.presses[index] = Press::Local;
                self.mouse.last_pos = pos;
                self.mouse.is_rectangle = mods.alt;
                let now = self.now_ns();
                let _ = self.terminal.lock().selection_press(
                    pos,
                    now,
                    u64::try_from(REPEAT_CLICK_INTERVAL.as_nanos()).unwrap_or(u64::MAX),
                );
                cx.notify();
            }
            MouseButton::Right if self.options.right_click != RightClick::Nothing => {
                // As Windows Terminal: a selection is copied instead of pasting. With copy on
                // select it was copied on release already, so right-click always pastes.
                let copies = self.options.right_click == RightClick::CopyOrPaste && !self.options.copy_on_select;
                let is_copied = copies && self.copy_selection(cx);
                let _ = self.terminal.lock().clear_selection();
                if !is_copied {
                    self.paste(&Paste, window, cx);
                }
                cx.notify();
            }
            _ => {}
        }
    }

    pub(crate) fn on_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.scrollbar_move(event.position, cx) {
            self.hover_link(None, cx);
            return;
        }
        let Some(pos) = self.grid_pos(event.position) else { return };
        self.mouse.last_pos = pos;
        let cell = self.cell_under(pos);
        self.mouse.pointer = cell.map(|_| event.position);
        self.mouse.is_ctrl_held = event.modifiers.control;
        self.hover_link(cell.filter(|_| self.mouse.wants_link()), cx);
        if self.mouse.presses[0] == Press::Local {
            if event.pressed_button != Some(gpui::MouseButton::Left) {
                // The release happened where we could not see it (e.g. another window).
                self.finish_selection(pos, cx);
                return;
            }
            self.mouse.is_rectangle = event.modifiers.alt;
            let autoscroll = self.terminal.lock().selection_drag(pos, self.mouse.is_rectangle);
            self.update_autoscroll(autoscroll.unwrap_or_default(), cx);
            cx.notify();
            return;
        }
        let is_reported_drag = self.mouse.presses.contains(&Press::Reported);
        if !is_reported_drag && !self.is_over_grid(pos) {
            return;
        }
        if !self.is_mouse_reported() {
            return;
        }
        let button = BUTTONS.iter().zip(self.mouse.presses).find(|(_, p)| *p == Press::Reported).map(|(b, _)| *b);
        self.report(MouseAction::Motion, button, to_mods(&event.modifiers), pos);
    }

    /// Ctrl pressed or released without moving the pointer.
    pub(crate) fn on_modifiers_changed(
        &mut self,
        event: &gpui::ModifiersChangedEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mouse.is_ctrl_held = event.modifiers.control;
        self.recheck_link(cx);
    }

    /// Looks up the link under the last pointer position again (Ctrl changed, or the screen
    /// under a still pointer did).
    pub(crate) fn recheck_link(&mut self, cx: &mut Context<Self>) {
        let cell = self.mouse.pointer.and_then(|p| self.grid_pos(p)).and_then(|pos| self.cell_under(pos));
        self.hover_link(cell.filter(|_| self.mouse.wants_link()), cx);
    }

    pub(crate) fn on_mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if event.button == gpui::MouseButton::Left && self.scrollbar.drag_grab.take().is_some() {
            // Narrows the bar again if the pointer was released outside the zone.
            self.scrollbar_move(event.position, cx);
            cx.notify();
            return;
        }
        let Some((index, button)) = button_index(event.button) else { return };
        let press = std::mem::take(&mut self.mouse.presses[index]);
        let pos = self.grid_pos(event.position).unwrap_or(self.mouse.last_pos);
        match press {
            Press::None => {}
            Press::Local => self.finish_selection(pos, cx),
            Press::Reported => self.report(MouseAction::Release, Some(button), to_mods(&event.modifiers), pos),
        }
    }

    /// Pane bounds, live viewport and thumb, when the scrollbar is shown.
    fn scrollbar_geometry(&self) -> Option<(Bounds<Pixels>, wired_vt::Scrollbar, Thumb)> {
        if !self.options.scrollbar {
            return None;
        }
        let bounds = self.frame.layout.get()?.bounds;
        let bar = self.terminal.lock().scrollbar();
        Some((bounds, bar, scrollbar::thumb(bounds, bar)?))
    }

    fn scroll_to_row(&self, row: u64, bar: wired_vt::Scrollbar) {
        let delta = i64::try_from(row).unwrap_or(i64::MAX) - i64::try_from(bar.offset).unwrap_or(i64::MAX);
        if delta != 0 {
            self.terminal.lock().scroll_viewport(wired_vt::Scroll::Delta(isize::try_from(delta).unwrap_or(0)));
        }
    }

    /// Left press in the scrollbar zone: grabs the thumb, or jumps there (thumb centered on the
    /// pointer, as Ubuntu does) and grabs it. Returns whether the press was taken.
    fn scrollbar_press(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let Some((bounds, bar, thumb)) = self.scrollbar_geometry() else { return false };
        if !scrollbar::in_zone(bounds, position) {
            return false;
        }
        let grab = if thumb.contains_y(position.y) {
            position.y - thumb.top
        } else {
            let grab = thumb.height / 2.;
            self.scroll_to_row(scrollbar::offset_for(bounds, bar, thumb.height, position.y - grab), bar);
            grab
        };
        self.scrollbar.drag_grab = Some(grab);
        self.scrollbar.set_wide(true);
        cx.notify();
        true
    }

    /// Pointer moved: drags the thumb, or updates the bar's width and highlight. Returns whether a
    /// drag consumed the move.
    fn scrollbar_move(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        if let Some(grab) = self.scrollbar.drag_grab {
            if let Some((bounds, bar, thumb)) = self.scrollbar_geometry() {
                self.scroll_to_row(scrollbar::offset_for(bounds, bar, thumb.height, position.y - grab), bar);
            }
            cx.notify();
            return true;
        }
        let geometry = self.scrollbar_geometry().filter(|(bounds, _, _)| scrollbar::in_zone(*bounds, position));
        let is_hot = geometry.is_some_and(|(_, _, thumb)| thumb.contains_y(position.y));
        let is_hot_changed = self.scrollbar.set_hot(is_hot);
        if self.scrollbar.set_wide(geometry.is_some()) || is_hot_changed {
            cx.notify();
        }
        false
    }

    fn finish_selection(&mut self, pos: GridPos, cx: &mut Context<Self>) {
        self.mouse.presses[0] = Press::None;
        self.mouse.is_autoscrolling = false;
        let _ = self.terminal.lock().selection_release(pos);
        if self.options.copy_on_select {
            self.copy_selection(cx);
        }
        cx.notify();
    }

    /// Copies the selection to the clipboard. Returns whether there was one.
    pub(crate) fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let text = self.terminal.lock().selection_text();
        match text {
            Ok(Some(text)) if !text.is_empty() => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                true
            }
            _ => false,
        }
    }

    fn update_autoscroll(&mut self, autoscroll: Autoscroll, cx: &mut Context<Self>) {
        if autoscroll == Autoscroll::None || self.mouse.is_autoscrolling {
            self.mouse.is_autoscrolling = autoscroll != Autoscroll::None;
            return;
        }
        self.mouse.is_autoscrolling = true;
        self.autoscroll_task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(AUTOSCROLL_INTERVAL).await;
                let keep_going = this.update(cx, |view, cx| {
                    if !view.mouse.is_autoscrolling {
                        return false;
                    }
                    let (pos, is_rectangle) = (view.mouse.last_pos, view.mouse.is_rectangle);
                    let keep_going = view.terminal.lock().selection_autoscroll_tick(pos, is_rectangle).unwrap_or(false);
                    view.mouse.is_autoscrolling = keep_going;
                    cx.notify();
                    keep_going
                });
                if !keep_going.unwrap_or(false) {
                    break;
                }
            }
        });
    }

    /// Wheel steps as reports when the program tracks the mouse. Returns whether it did.
    pub(crate) fn report_wheel(
        &mut self,
        position: Point<Pixels>,
        modifiers: &Modifiers,
        is_up: bool,
        count: usize,
    ) -> bool {
        if !self.is_mouse_reported() {
            return false;
        }
        let Some(pos) = self.grid_pos(position) else { return false };
        let mods = to_mods(modifiers);
        let _ = self.terminal.lock().clear_selection();
        let button = if is_up { MouseButton::WheelUp } else { MouseButton::WheelDown };
        for _ in 0..count {
            self.report(MouseAction::Press, Some(button), mods, pos);
        }
        true
    }
}
