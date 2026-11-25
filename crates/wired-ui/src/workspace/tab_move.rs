//! Moving tabs between windows: tearing a tab off into a new window, dragging a window by its
//! only tab, and joining another window's strip. The panes' programs keep running.
//!
//! The window that holds the dragged tab also holds the pointer capture, so it gets every move
//! and the release wherever the pointer is.

use gpui::{AnyWindowHandle, DisplayId, Global, Size, WeakEntity, point, size};
use wired_platform::window::{self as platform, WindowId};

use super::tab_drag::{Press, TabDrag, TabPointer, grab_ratio};
use super::*;
use crate::title_bar::TAB_WIDTH;

/// Slack above and below the strip that still counts as on it (Chromium's `DoesTabStripContain`).
const STRIP_MAGNETISM: f32 = 15.;

#[derive(Debug, Clone)]
pub(super) struct Target {
    window: AnyWindowHandle,
    workspace: WeakEntity<Workspace>,
}

/// Where Escape puts a dragged tab back.
#[derive(Debug, Clone)]
pub(super) struct Home {
    target: Target,
    pub(super) index: usize,
    pub(super) previous_active: u64,
}

impl Home {
    pub(super) fn here(index: usize, previous_active: u64, window: &Window, cx: &Context<Workspace>) -> Self {
        let target = Target { window: window.window_handle(), workspace: cx.entity().downgrade() };
        Self { target, index, previous_active }
    }

    pub(super) fn is_here(&self, cx: &Context<Workspace>) -> bool {
        self.target.workspace.entity_id() == cx.entity_id()
    }
}

/// Where a window dragged by its only tab goes on Escape.
#[derive(Debug, Clone)]
pub(super) enum WindowBack {
    /// Client-area origin to return the window to.
    To { x: i32, y: i32 },
    /// The tab returns to the window it was torn from; this window closes.
    Home(Home),
}

#[derive(Debug, Clone)]
pub(super) struct HeldWindow {
    /// From the client area's top left.
    grab_x: f32,
    grab_y: f32,
    pub(super) tab_left: f32,
    pub(super) back: WindowBack,
    /// A window opened right on top of another must leave that strip before it can join it.
    over_at_start: Option<WindowId>,
}

/// A window dragged by its only tab.
#[derive(Debug, Clone)]
pub(super) struct WindowDrag {
    pub(super) id: u64,
    pub(super) held: HeldWindow,
}

#[derive(Default)]
struct TabTargets(Vec<(WindowId, Target)>);

impl Global for TabTargets {}

/// A torn-off tab on its way to a new window.
pub(crate) struct MovedTab {
    tab: Tab,
    held: HeldWindow,
}

fn strip_contains(left: f32, limit_right: f32, top: f32, x: f32, y: f32) -> bool {
    x >= left && x < limit_right && y >= top - STRIP_MAGNETISM && y <= top + title_bar::HEIGHT + STRIP_MAGNETISM
}

/// Chromium's `kMaximizedWindowInset`.
const WORK_AREA_INSET: f32 = 10.;

/// A maximized or fullscreen source passes its `restored` size, taken as at least half of what
/// is shown (Chromium's `CalculateDraggedWindowSize`).
fn torn_window_size(
    shown: Size<Pixels>,
    restored: Option<Size<Pixels>>,
    work_area: Option<Size<Pixels>>,
) -> Size<Pixels> {
    let base = restored.map_or(shown, |r| size(r.width.max(shown.width / 2.), r.height.max(shown.height / 2.)));
    let Some(area) = work_area else { return base };
    let fit = |value: Pixels, limit: Pixels| {
        if value >= limit { (limit - px(2. * WORK_AREA_INSET)).max(px(1.)) } else { value }
    };
    size(fit(base.width, area.width), fit(base.height, area.height))
}

fn torn_tab_left(held_left: f32, area_width: f32) -> f32 {
    held_left.min(area_width - TAB_WIDTH).max(0.)
}

/// Returns the tab if the target window is gone.
fn give(
    target: &Target,
    tab: Tab,
    cx: &mut App,
    receive: impl FnOnce(&mut Workspace, Tab, &mut Window, &mut Context<Workspace>),
) -> Option<Tab> {
    let mut tab = Some(tab);
    let _ = target.window.update(cx, |_, window, cx| {
        target.workspace.update(cx, |this, cx| {
            if let Some(tab) = tab.take() {
                receive(this, tab, window, cx);
            }
        })
    });
    tab
}

/// Deferred: activating sends messages to the window being updated.
fn activate_later(window: &Window, cx: &mut App) {
    let handle = window.window_handle();
    cx.defer(move |cx| {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
    });
}

/// If no window can be made, the tab goes back to the window it came from.
fn open_torn_off(moved: MovedTab, bounds: Bounds<Pixels>, display: Option<DisplayId>, source: Home, cx: &mut App) {
    let mut moved = Some(moved);
    match crate::open_workspace(&mut moved, bounds, display, cx) {
        Ok((handle, workspace)) => {
            let _ = handle.update(cx, |_, window, cx| {
                workspace.update(cx, |workspace, cx| workspace.hold_window(window, cx));
            });
        }
        Err(error) => {
            wired_platform::show_error(&format!("Cannot open a window for the tab: {error:#}"));
            let Some(MovedTab { tab, .. }) = moved else { return };
            let (index, previous_active) = (source.index, source.previous_active);
            let lost = give(&source.target, tab, cx, |this, tab, window, cx| {
                this.welcome_back(tab, index, previous_active, true, window, cx);
            });
            if lost.is_some() {
                wired_platform::show_error("A torn-off tab was lost: its window could not be opened.");
            }
        }
    }
}

impl Workspace {
    pub(super) fn register_window(window: &Window, cx: &mut Context<Self>) -> Option<WindowId> {
        let id = platform::window_id(window)?;
        let target = Target { window: window.window_handle(), workspace: cx.entity().downgrade() };
        let targets = &mut cx.default_global::<TabTargets>().0;
        targets.retain(|(_, t)| t.workspace.upgrade().is_some());
        targets.push((id, target));
        Some(id)
    }

    pub(super) fn is_on_strip(&self, x: f32, y: f32) -> bool {
        let strip = self.strip.borrow();
        strip_contains(strip.left, strip.limit_right, strip.top, x, y)
    }

    fn window_point(&self, cursor: (i32, i32), window: &Window) -> Option<(f32, f32)> {
        let origin = platform::client_origin(self.window_id?)?;
        let scale = window.scale_factor();
        Some(((cursor.0 - origin.0) as f32 / scale, (cursor.1 - origin.1) as f32 / scale))
    }

    fn takes_tab_at(&self, cursor: (i32, i32), window: &Window) -> bool {
        if window.is_fullscreen() && !self.bar_slide.is_on {
            return false;
        }
        self.window_point(cursor, window).is_some_and(|(x, y)| self.is_on_strip(x, y))
    }

    fn track_drag_over(&mut self, cursor: Option<(i32, i32)>, window: &Window, cx: &mut Context<Self>) {
        if window.is_fullscreen() {
            let y = cursor.and_then(|cursor| self.window_point(cursor, window)).map_or(f32::INFINITY, |(_, y)| y);
            self.track_top_edge(y, cx);
        }
    }

    /// Never returns the window being updated, as gpui cannot update it reentrantly.
    fn target_at(cursor: (i32, i32), skip: Option<WindowId>, cx: &mut App) -> Option<(WindowId, Target)> {
        let under = platform::window_at(cursor.0, cursor.1, skip);
        let targets = cx.try_global::<TabTargets>()?.0.clone();
        let mut found = None;
        for (id, target) in targets.into_iter().filter(|(id, _)| Some(*id) != skip) {
            let is_under = Some(id) == under;
            let takes_tab = target
                .window
                .update(cx, |_, window, cx| {
                    target.workspace.update(cx, |this, cx| {
                        this.track_drag_over(is_under.then_some(cursor), window, cx);
                        is_under && this.takes_tab_at(cursor, window)
                    })
                })
                .is_ok_and(|takes| takes.unwrap_or(false));
            if takes_tab {
                found = Some((id, target));
            }
        }
        found
    }

    fn adopt(&mut self, mut tab: Tab, index: usize, window: &mut Window, cx: &mut Context<Self>) -> u64 {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.adopt_panes(&mut tab, id, window, cx);
        tab.id = id;
        tab.slide = None;
        tab.opened_at = None;
        // Its timer belongs to the window the tab left.
        tab.motion = None;
        self.tabs.insert(index.min(self.tabs.len()), tab);
        self.update_numbers();
        id
    }

    fn take_out(&mut self, index: usize, previous_active: u64, window: &mut Window, cx: &mut Context<Self>) -> Tab {
        self.pointer = TabPointer::Idle;
        if self.renaming.as_ref().is_some_and(|r| r.tab_id == self.tabs[index].id) {
            self.finish_rename(true, false, window, cx);
        }
        let before = self.slot_lefts();
        let tab = self.tabs.remove(index);
        self.update_numbers();
        if !self.tabs.is_empty() {
            self.slide_from(&before, Instant::now());
            let next = self.tabs.iter().position(|t| t.id == previous_active);
            self.activate(next.unwrap_or(index.min(self.tabs.len() - 1)), window, cx);
        }
        tab
    }

    pub(super) fn tear_off(&mut self, drag: TabDrag, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cursor) = platform::cursor_position() else {
            self.follow_pointer(drag);
            return;
        };
        let slots = self.slots();
        let Some(held) = self.held_tab(&drag, &slots) else {
            self.pointer = TabPointer::Idle;
            return;
        };
        let (strip_left, strip_top, limit_right) = {
            let strip = self.strip.borrow();
            (strip.left, strip.top, strip.limit_right)
        };
        let target = Self::target_at(cursor, None, cx).map(|(_, target)| target);
        let mut tab = self.take_out(held.index, drag.previous_active, window, cx);
        if let Some(target) = target {
            let home = drag.home.clone();
            let grab = (drag.grab_ratio, drag.grab_y);
            match give(&target, tab, cx, |this, tab, window, cx| this.take_in(tab, grab, cursor, home, window, cx)) {
                None => return,
                Some(back) => tab = back,
            }
        }
        let shown = window.bounds();
        let restored =
            (window.is_maximized() || window.is_fullscreen()).then(|| window.window_bounds().get_bounds().size);
        let monitor = platform::monitor_at(cursor.0, cursor.1);
        let scale = monitor.map_or(window.scale_factor(), |m| m.scale);
        let work_area = monitor.map(|m| size(px(m.work_area.0 as f32 / m.scale), px(m.work_area.1 as f32 / m.scale)));
        let size = torn_window_size(shown.size, restored, work_area);
        let trailing = f32::from(window.viewport_size().width) - limit_right;
        let tab_left = torn_tab_left(held.left, f32::from(size.width) - trailing - strip_left);
        let (grab_x, grab_y) = (strip_left + tab_left + drag.grab_ratio * TAB_WIDTH, strip_top + drag.grab_y);
        // gpui places a window on a display in that display's logical pixels; without the display
        // it converts with the primary monitor's scale, or falls back to its default size when
        // the point is on another monitor.
        let origin = point(px(cursor.0 as f32 / scale - grab_x), px(cursor.1 as f32 / scale - grab_y));
        let display = monitor.map(|m| DisplayId::new(m.id));
        let source = Home::here(held.index, drag.previous_active, window, cx);
        let home = drag.home.unwrap_or_else(|| source.clone());
        let held = HeldWindow { grab_x, grab_y, tab_left, back: WindowBack::Home(home), over_at_start: None };
        let moved = MovedTab { tab, held };
        cx.defer(move |cx| open_torn_off(moved, Bounds { origin, size }, display, source, cx));
    }

    /// `grab` is the held point: a fraction of the tab width, and px below its top.
    fn take_in(
        &mut self,
        tab: Tab,
        grab: (f32, f32),
        cursor: (i32, i32),
        home: Option<Home>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.end_tab_pointer(cx);
        let previous_active = self.tabs.get(self.active).map_or(u64::MAX, |t| t.id);
        let id = self.adopt(tab, self.tabs.len(), window, cx);
        self.activate(self.tabs.len() - 1, window, cx);
        let pointer_x = self.window_point(cursor, window).map_or(0., |(x, _)| x);
        let (grab_ratio, grab_y) = grab;
        self.follow_pointer(TabDrag { id, grab_ratio, grab_y, pointer_x, last_reorder_x: None, previous_active, home });
        // The others take their places at once, as in Chromium, so the strip does not flicker.
        for tab in &mut self.tabs {
            tab.slide = None;
        }
        if let Some(id) = self.window_id {
            platform::capture_pointer(id);
        }
        activate_later(window, cx);
        cx.notify();
    }

    pub(super) fn adopt_moved(&mut self, moved: MovedTab, window: &mut Window, cx: &mut Context<Self>) {
        let MovedTab { tab, held } = moved;
        let id = self.adopt(tab, 0, window, cx);
        self.active = 0;
        self.focus_active(window, cx);
        self.pointer = TabPointer::MovingWindow(WindowDrag { id, held });
    }

    fn hold_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let TabPointer::MovingWindow(drag) = &self.pointer {
            let held = drag.held.clone();
            if let Some(id) = self.window_id {
                platform::capture_pointer(id);
            }
            self.place_window(&held, window, cx);
        }
    }

    pub(super) fn start_window_drag(&mut self, press: Press, window: &mut Window, cx: &mut Context<Self>) {
        if window.is_fullscreen() {
            return;
        }
        let Some((id, origin)) = self.window_id.and_then(|id| Some((id, platform::client_origin(id)?))) else {
            return;
        };
        let slots = self.slots();
        let (Some(&width), Some(&tab_left)) = (slots.widths.first(), slots.lefts().first()) else { return };
        let over_at_start =
            platform::cursor_position().and_then(|cursor| Self::target_at(cursor, Some(id), cx)).map(|(over, _)| over);
        let held = HeldWindow {
            grab_x: self.strip.borrow().left + tab_left + press.grab_ratio * width,
            grab_y: press.y,
            tab_left,
            back: WindowBack::To { x: origin.0, y: origin.1 },
            over_at_start,
        };
        if window.is_maximized() {
            cx.foreground_executor().spawn(async move { platform::restore(id) }).detach();
        }
        let drag = WindowDrag { id: press.id, held };
        self.pointer = TabPointer::MovingWindow(drag.clone());
        self.drag_window(drag, window, cx);
    }

    /// Over another window's strip, the tab joins it and this window closes.
    pub(super) fn drag_window(&mut self, mut drag: WindowDrag, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cursor) = platform::cursor_position() else { return };
        let found = Self::target_at(cursor, self.window_id, cx);
        let over = found.as_ref().map(|(id, _)| *id);
        if drag.held.over_at_start.is_some() && drag.held.over_at_start != over {
            drag.held.over_at_start = None;
            self.pointer = TabPointer::MovingWindow(drag.clone());
        }
        let Some((_, target)) = found.filter(|(id, _)| drag.held.over_at_start != Some(*id)) else {
            self.place_window(&drag.held, window, cx);
            return;
        };
        let Some(index) = self.tabs.iter().position(|t| t.id == drag.id) else { return };
        let (strip_left, strip_top) = {
            let strip = self.strip.borrow();
            (strip.left, strip.top)
        };
        let width = self.slots().widths[index];
        let grab =
            (grab_ratio(drag.held.grab_x - strip_left - drag.held.tab_left, width), drag.held.grab_y - strip_top);
        let home = match drag.held.back.clone() {
            WindowBack::Home(home) => Some(home),
            WindowBack::To { .. } => None,
        };
        let tab = self.take_out(index, drag.id, window, cx);
        match give(&target, tab, cx, |this, tab, window, cx| this.take_in(tab, grab, cursor, home, window, cx)) {
            None => window.remove_window(),
            Some(tab) => {
                self.tabs.insert(index, tab);
                self.pointer = TabPointer::MovingWindow(drag);
            }
        }
    }

    fn place_window(&self, held: &HeldWindow, window: &Window, cx: &mut Context<Self>) {
        let Some(cursor) = platform::cursor_position() else { return };
        let scale = window.scale_factor();
        let x = (cursor.0 as f32 - held.grab_x * scale).round() as i32;
        let y = (cursor.1 as f32 - held.grab_y * scale).round() as i32;
        self.move_window_to(x, y, cx);
    }

    /// Deferred: the move sends messages to the window being updated.
    pub(super) fn move_window_to(&self, x: i32, y: i32, cx: &mut Context<Self>) {
        if let Some(id) = self.window_id {
            cx.foreground_executor().spawn(async move { platform::move_client_origin(id, x, y) }).detach();
        }
    }

    /// If the home window is gone, the tab stays here.
    pub(super) fn send_home(
        &mut self,
        id: u64,
        previous_active: u64,
        home: Home,
        brings_home_front: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.tabs.iter().position(|t| t.id == id) else { return };
        let tab = self.take_out(index, previous_active, window, cx);
        let (home_index, home_active) = (home.index, home.previous_active);
        let kept = give(&home.target, tab, cx, |this, tab, window, cx| {
            this.welcome_back(tab, home_index, home_active, brings_home_front, window, cx);
        });
        if let Some(tab) = kept {
            let id = self.adopt(tab, index, window, cx);
            if let Some(index) = self.tabs.iter().position(|t| t.id == id) {
                self.activate(index, window, cx);
            }
        } else if self.tabs.is_empty() {
            window.remove_window();
        }
    }

    /// `brings_front` is false when the user switched to another app.
    fn welcome_back(
        &mut self,
        tab: Tab,
        index: usize,
        previous_active: u64,
        brings_front: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.end_tab_pointer(cx);
        let before = self.slot_lefts();
        let id = self.adopt(tab, index, window, cx);
        self.slide_from(&before, Instant::now());
        let active = self.tabs.iter().position(|t| t.id == previous_active);
        if let Some(index) = active.or_else(|| self.tabs.iter().position(|t| t.id == id)) {
            self.activate(index, window, cx);
        }
        if brings_front {
            activate_later(window, cx);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strip_reaches_fifteen_px_above_and_below_the_bar() {
        let on = |x, y| strip_contains(10., 500., 0., x, y);
        assert!(on(10., 18.));
        assert!(!on(9.9, 18.), "left of the strip");
        assert!(!on(500., 18.), "at the limit the buttons leave");
        assert!(on(499., 18.));
        assert!(on(100., -15.) && !on(100., -15.1));
        assert!(on(100., title_bar::HEIGHT + 15.) && !on(100., title_bar::HEIGHT + 15.1));
    }

    #[test]
    fn a_torn_off_window_has_the_size_shown_and_fits_the_work_area() {
        let s = |w: f32, h: f32| size(px(w), px(h));
        let area = Some(s(1920., 1040.));
        assert_eq!(
            torn_window_size(s(1000., 620.), None, area),
            s(1000., 620.),
            "the size shown, not a stale restored size"
        );
        assert_eq!(torn_window_size(s(2400., 620.), None, area), s(1900., 620.), "cut to the work area, inset");
        assert_eq!(
            torn_window_size(s(1920., 1040.), Some(s(800., 500.)), area),
            s(960., 520.),
            "maximized: at least half"
        );
        assert_eq!(torn_window_size(s(1920., 1040.), Some(s(3000., 600.)), area), s(1900., 600.));
        assert_eq!(torn_window_size(s(1000., 620.), None, None), s(1000., 620.));
    }

    #[test]
    fn a_torn_off_tab_keeps_its_place_within_the_new_strip() {
        assert_eq!(torn_tab_left(300., 900.), 300.);
        assert_eq!(torn_tab_left(1500., 900.), 900. - TAB_WIDTH);
        assert_eq!(torn_tab_left(300., TAB_WIDTH / 2.), 0., "a strip narrower than a tab");
    }
}
