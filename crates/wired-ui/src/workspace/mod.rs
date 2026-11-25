//! The contents of a window: the title bar with its tabs, and the panes of the active tab.

mod bell;
mod hint;
mod motion;
mod pane_area;
mod panes;
mod picker;
mod rename;
mod rows;
mod shortcut;
mod tab_drag;
mod tab_move;
mod tabs;

pub(crate) use self::tab_move::MovedTab;

use std::{
    cell::Cell,
    collections::HashMap,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{
    AnyElement, App, AppContext, Bounds, Context, CursorStyle, DispatchPhase, ElementId, Entity, Focusable, Hsla,
    InteractiveElement, IntoElement, KeyUpEvent, ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point, Render, ScrollDelta, ScrollEvent, StatefulInteractiveElement, Styled,
    Subscription, Window, actions, canvas, component::input::Escape as InputEscape, div, px,
};
use wired_config::{MouseAction, Profile, ProfileIcon, gesture};
use wired_layout::pane_tree::{
    Axis, Closed, Direction, DividerId, Layout, LayoutParams, PaneId, PaneTree, Rect, Row, RowId, Side,
};
use wired_terminal_view::{
    Notice, TerminalView, TerminalViewEvent,
    scrollbar::{self, ScrollbarState},
};
use wired_vt::VtOptions;

use crate::{
    appearance::Appearance,
    settings::{LoadReport, Settings},
    title_bar::{self, SharedMetrics, TabHandlers, TabLook, TabStrip, TabTitle},
};

use self::hint::ShortcutHint;
use self::motion::{Glide, PaneGhost, PaneMotion, animated_rects, collapse_target, edge, edge_lines, lerp_rect};
use self::panes::Pane;
use self::picker::ProfilePicker;
use self::rename::Renaming;
use self::rows::row_thumb;
use self::tab_drag::{Slide, TabPointer};
use self::tabs::{ClosingTab, StripItem, Tab, strip_order};

actions!(
    workspace,
    [
        /// Open a new window with one tab of the default shell.
        NewWindow,
        /// Enter or leave fullscreen.
        ToggleFullscreen,
        /// Open a new tab with the default shell.
        NewTab,
        /// Activate the next tab.
        NextTab,
        /// Activate the previous tab.
        PreviousTab,
        /// Move the active tab one place left.
        MoveTabLeft,
        /// Move the active tab one place right.
        MoveTabRight,
        /// Edit the title of the active tab.
        RenameTab,
        /// Open the profile picker; the chosen profile opens in a new tab.
        ProfileSelector,
        /// Read `config.toml` and `keybind.toml` again and apply them to every window.
        ReloadConfig,
        /// Close the focused pane (and its tab when it is the last pane).
        ClosePane,
        /// Split the focused pane, the new pane on the right.
        SplitRight,
        /// Split the focused pane, the new pane below.
        SplitBottom,
        /// Split the focused pane, the new pane on the left.
        SplitLeft,
        /// Split the focused pane, the new pane above.
        SplitTop,
        /// Focus the pane to the left (wrapping around).
        FocusPaneLeft,
        /// Focus the pane to the right (wrapping around).
        FocusPaneRight,
        /// Focus the pane above (wrapping around).
        FocusPaneUp,
        /// Focus the pane below (wrapping around).
        FocusPaneDown,
        /// Focus the previous pane in order.
        FocusPreviousPane,
        /// Focus the next pane in order.
        FocusNextPane,
        /// Activate tab 1.
        ActivateTab1,
        /// Activate tab 2.
        ActivateTab2,
        /// Activate tab 3.
        ActivateTab3,
        /// Activate tab 4.
        ActivateTab4,
        /// Activate tab 5.
        ActivateTab5,
        /// Activate tab 6.
        ActivateTab6,
        /// Activate tab 7.
        ActivateTab7,
        /// Activate tab 8.
        ActivateTab8,
        /// Activate tab 9.
        ActivateTab9,
        /// Activate tab 10.
        ActivateTab10,
    ]
);

/// Key context of the window contents.
pub const KEY_CONTEXT: &str = "Workspace";

const FULLSCREEN_EDGE: f32 = 2.;

const ANIMATION: Duration = Duration::from_millis(200);
const OVERLAY_ANIMATION: Duration = Duration::from_millis(300);
const DIM_ANIMATION: Duration = Duration::from_millis(500);

fn progress(since: Option<Instant>, now: Instant) -> f32 {
    progress_over(since, now, ANIMATION)
}

fn progress_over(since: Option<Instant>, now: Instant, duration: Duration) -> f32 {
    let Some(since) = since else { return 1. };
    let t = (now.saturating_duration_since(since).as_secs_f32() / duration.as_secs_f32()).clamp(0., 1.);
    1. - (1. - t).powi(3)
}

/// A divider held by the pointer.
#[derive(Debug, Clone, Copy)]
struct DividerDrag {
    id: DividerId,
    /// Pane tree revision at the press: once panes are added or removed, `id` may name another divider.
    revision: u64,
}

/// A row's scrollbar thumb held by the pointer.
#[derive(Debug, Clone, Copy)]
struct ThumbDrag {
    row: RowId,
    /// The pointer's distance from the thumb's left edge.
    grab: f32,
}

#[derive(Debug)]
struct Transition {
    is_on: bool,
    from: f32,
    started: Option<Instant>,
    duration: Duration,
}

impl Transition {
    fn new(duration: Duration) -> Self {
        Self { is_on: false, from: 0., started: None, duration }
    }

    /// The level (0 off, 1 on), and whether it still changes.
    fn level(&self, now: Instant) -> (f32, bool) {
        let target = if self.is_on { 1. } else { 0. };
        let t = progress_over(self.started, now, self.duration);
        (self.from + (target - self.from) * t, t < 1.)
    }

    /// Returns whether it changed.
    fn set_on(&mut self, is_on: bool, now: Instant) -> bool {
        if self.is_on == is_on {
            return false;
        }
        self.from = self.level(now).0;
        self.started = Some(now);
        self.is_on = is_on;
        true
    }
}

/// Root view of a window.
pub struct Workspace {
    tabs: Vec<Tab>,
    active: usize,
    next_tab_id: u64,
    pointer: TabPointer,
    strip: SharedMetrics,
    window_id: Option<wired_platform::window::WindowId>,
    renaming: Option<Renaming>,
    picker: Option<ProfilePicker>,
    held_shortcut: Option<shortcut::HeldShortcut>,
    hint: ShortcutHint,
    are_numbers_shown: bool,
    numbers_changed_at: Option<Instant>,
    closing: Vec<ClosingTab>,
    vt_options: VtOptions,
    appearance: Appearance,
    background: Hsla,
    pane_area: Rc<Cell<Option<Bounds<Pixels>>>>,
    divider_drag: Option<DividerDrag>,
    thumb_drag: Option<ThumbDrag>,
    /// Part of a notch of Ctrl+Shift+wheel not applied yet (touchpads send small deltas).
    width_wheel_rest: f32,
    bar_slide: Transition,
    dim: Transition,
    is_topmost: bool,
    _settings: Subscription,
    _keys: Subscription,
    _activation: Subscription,
}

impl Workspace {
    /// A workspace whose one tab is `moved` (torn off another window), else a new default-profile tab.
    pub(crate) fn new(
        vt_options: VtOptions,
        appearance: Appearance,
        moved: Option<MovedTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let window_id = Self::register_window(window, cx);
        if let Some(id) = window_id {
            wired_platform::window::set_icon(id, Settings::get(cx).window_icon());
            if appearance.is_system_acrylic {
                wired_platform::window::use_system_acrylic(id);
            }
        }
        let background = appearance.background;
        let settings = cx.observe_global_in::<Settings>(window, |this, window, cx| this.apply_settings(window, cx));
        let handle = window.window_handle();
        // Intercepted because keys that run an action never reach key-down listeners and the terminal
        // stops the ones it encodes; taking Escape here keeps a binding on it from beating a tab drag.
        let workspace = cx.entity().downgrade();
        let keys = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() != handle || event.keystroke.is_modifier_key() {
                return;
            }
            let is_taken = workspace
                .update(cx, |this, cx| {
                    this.on_hint_operation(cx);
                    this.on_picker_key(&event.keystroke, window, cx)
                        || (event.keystroke.key == "escape" && this.cancel_tab_drag(true, window, cx))
                        || this.hold_shortcut(&event.keystroke, &event.context_stack, cx)
                })
                .unwrap_or(false);
            if is_taken {
                cx.stop_propagation();
            }
        });
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.on_window_deactivated(window, cx);
            }
            cx.notify();
        });
        let mut workspace = Self {
            tabs: Vec::new(),
            active: 0,
            next_tab_id: 0,
            pointer: TabPointer::Idle,
            strip: Rc::default(),
            window_id,
            renaming: None,
            picker: None,
            held_shortcut: None,
            hint: ShortcutHint::default(),
            are_numbers_shown: false,
            numbers_changed_at: None,
            closing: Vec::new(),
            vt_options,
            appearance,
            background,
            pane_area: Rc::default(),
            divider_drag: None,
            thumb_drag: None,
            width_wheel_rest: 0.,
            bar_slide: Transition::new(ANIMATION),
            dim: Transition::new(DIM_ANIMATION),
            is_topmost: false,
            _settings: settings,
            _keys: keys,
            _activation: activation,
        };
        if let Some(moved) = moved {
            workspace.adopt_moved(moved, window, cx);
            return workspace;
        }
        match workspace.open_tab(None, window, cx) {
            // The first tab is there from the start, without the opening animation.
            Ok(()) => {
                if let Some(tab) = workspace.tabs.last_mut() {
                    tab.opened_at = None;
                }
            }
            // A window always has a tab, as closing the last one closes the window.
            Err(error) => {
                workspace.show_error(format!("Cannot start the shell: {error:#}"), cx);
                window.remove_window();
            }
        }
        workspace
    }

    /// Releases while the window is away are never seen, so presses and held chords end here. A window
    /// dragged by its tab keeps going: it may lose the focus to the window it was torn from.
    fn on_window_deactivated(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.pointer, TabPointer::MovingWindow(_)) && !self.cancel_tab_drag(false, window, cx) {
            self.end_tab_pointer(cx);
        }
        self.reset_hint(cx);
        self.close_picker(true, window, cx);
        self.held_shortcut = None;
    }

    /// Some taskbars stay above a window that covers the monitor unless it is topmost; only the
    /// active fullscreen window is, so a window switched to is not hidden behind it.
    fn sync_topmost(&mut self, is_topmost: bool, cx: &mut Context<Self>) {
        if self.is_topmost == is_topmost {
            return;
        }
        self.is_topmost = is_topmost;
        if let Some(id) = self.window_id {
            cx.foreground_executor().spawn(async move { wired_platform::window::set_topmost(id, is_topmost) }).detach();
        }
    }

    fn track_top_edge(&mut self, y: f32, cx: &mut Context<Self>) {
        let reach = if self.bar_slide.is_on { title_bar::HEIGHT } else { FULLSCREEN_EDGE };
        if self.bar_slide.set_on(y <= reach, Instant::now()) {
            cx.notify();
        }
    }

    /// Re-reads the settings files; every window applies them, and the report shows in this one.
    pub fn reload_config(&mut self, cx: &mut Context<Self>) {
        let report = Settings::load(cx);
        self.show_report(&report, cx);
    }

    fn apply_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = Settings::get(cx);
        let hides_pointer = settings.config().terminal.hide_pointer_while_typing;
        self.vt_options = crate::vt_options(settings);
        let appearance = Appearance::from_settings(settings, &cx.text_system().all_font_names());
        crate::apply_pointer_hiding(hides_pointer, cx);
        window.set_background_appearance(appearance.backdrop);
        if let Some(id) = self.window_id.filter(|_| appearance.is_system_acrylic) {
            wired_platform::window::use_system_acrylic(id);
        }
        self.background = appearance.background;
        for tab in &self.tabs {
            for (_, pane) in tab.panes.panes() {
                pane.view.update(cx, |view, cx| {
                    view.set_colors(&self.vt_options.colors);
                    view.set_font(appearance.font.clone(), cx);
                    view.set_options(appearance.view, cx);
                });
            }
        }
        self.appearance = appearance;
        cx.notify();
    }

    /// Writes what reading the settings files found into the focused pane.
    pub fn show_report(&self, report: &LoadReport, cx: &mut Context<Self>) {
        let errors = report.errors.iter().map(|e| Notice::error(e.clone()));
        let warnings = report.warnings.iter().map(|w| Notice::warning(w.clone()));
        let notes = report.notes.iter().map(|n| Notice::info(n.clone()));
        let notices: Vec<Notice> = errors.chain(warnings).chain(notes).collect();
        if !notices.is_empty() {
            self.show_notices(&notices, cx);
        }
    }

    /// Shows `message` in the focused pane, or in an error box while the window has no pane.
    fn show_error(&self, message: String, cx: &mut Context<Self>) {
        if !self.show_notices(&[Notice::error(message.clone())], cx) {
            wired_platform::show_error(&message);
        }
    }

    /// Returns `false` if there is no pane to show them in.
    fn show_notices(&self, notices: &[Notice], cx: &mut Context<Self>) -> bool {
        let Some(pane) = self.tabs.get(self.active).and_then(|tab| tab.panes.get(tab.panes.focused())) else {
            return false;
        };
        pane.view.update(cx, |view, cx| view.show_notices(notices, cx));
        true
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        self.closing.retain(|c| progress(Some(c.started), now) < 1.);
        self.forget_gone_widths();
        self.settle_slides(now);
        let (mut strip, is_animating) = self.tab_strip(cx);
        self.dim.set_on(self.is_dimmed(window), now);
        let (dim, is_dimming) = self.dim.level(now);
        strip.dim = crate::overlay::background(dim);
        let is_gliding = self.settle_rows(now, cx);
        if is_animating || is_gliding || is_dimming {
            window.request_animation_frame();
        }
        let buttons = title_bar::BarButtons {
            on_new_tab: Box::new(cx.listener(|this, _, window, cx| {
                if let Err(error) = this.open_tab(None, window, cx) {
                    this.show_error(format!("Cannot start the shell: {error:#}"), cx);
                }
            })),
            on_profiles: Box::new(cx.listener(|this, _, window, cx| this.toggle_picker(window, cx))),
            on_reload: self
                .appearance
                .show_reload_button
                .then(|| Box::new(cx.listener(|this, _, _, cx| this.reload_config(cx))) as title_bar::MouseHandler<_>),
        };
        let panes = self.pane_area(window, cx);
        let picker = self.picker_element(window, cx);
        let is_fullscreen = window.is_fullscreen();
        self.sync_topmost(is_fullscreen && window.is_window_active(), cx);
        if !is_fullscreen && (self.bar_slide.is_on || self.bar_slide.from != 0.) {
            self.bar_slide = Transition::new(ANIMATION);
        }
        let (reveal, is_sliding) = self.bar_slide.level(now);
        if is_sliding {
            window.request_animation_frame();
        }
        let tracker = self.strip_tracker(cx);
        let bar = title_bar::title_bar(
            strip,
            tracker,
            self.background,
            self.appearance.bar,
            if is_fullscreen {
                title_bar::WindowState::Fullscreen
            } else {
                title_bar::WindowState::Windowed { is_maximized: window.is_maximized() }
            },
            window.is_window_hovered(),
            buttons,
        );
        let bar = div().relative().w_full().h(px(title_bar::HEIGHT)).flex_none().child(bar);
        let workspace = cx.entity().downgrade();
        let hint_input = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                // Occluding controls exclude the root hitbox, so observe presses at the window.
                window.on_mouse_event(move |_: &MouseDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = workspace.update(cx, |this, cx| this.on_hint_operation(cx));
                    }
                });
            },
        )
        .absolute()
        .size_full();
        let root = div().id("workspace").key_context(KEY_CONTEXT).relative().size_full().child(hint_input);
        let root = if is_fullscreen {
            // Bar and panes move down together; the panes keep the screen's height (no resize),
            // so their bottom rows go off screen while the bar is in.
            let height = f32::from(window.viewport_size().height);
            let workspace = cx.entity().downgrade();
            let edge = canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble {
                            let y = f32::from(event.position.y);
                            let _ = workspace.update(cx, |this, cx| this.track_top_edge(y, cx));
                        }
                    });
                },
            )
            .absolute()
            .size_full();
            root.overflow_hidden().child(edge).child(
                div()
                    .absolute()
                    .left_0()
                    .w_full()
                    .top(px(title_bar::HEIGHT * (reveal - 1.)))
                    .h(px(height + title_bar::HEIGHT))
                    .flex()
                    .flex_col()
                    .child(bar)
                    .child(panes),
            )
        } else {
            root.flex().flex_col().child(bar).child(panes)
        };
        root.children(picker)
            .capture_key_up(cx.listener(|this, event: &KeyUpEvent, window, cx| {
                this.release_shortcut(&event.keystroke.key, window, cx);
            }))
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, cx| {
                this.on_modifiers_changed(&event.modifiers, cx);
            }))
            .on_action(cx.listener(|this, _: &RenameTab, window, cx| {
                if let Some(id) = this.tabs.get(this.active).map(|t| t.id) {
                    this.start_rename(id, window, cx);
                }
            }))
            // The text field leaves Escape to whoever hosts it.
            .on_action(cx.listener(|this, _: &InputEscape, window, cx| {
                if this.renaming.is_some() {
                    this.finish_rename(false, true, window, cx);
                } else if this.is_picker_open() {
                    this.close_picker(true, window, cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &ProfileSelector, window, cx| this.toggle_picker(window, cx)))
            .on_action(cx.listener(|this, _: &ReloadConfig, _, cx| this.reload_config(cx)))
            .on_action(cx.listener(|_, _: &ToggleFullscreen, window, _| window.toggle_fullscreen()))
            .on_action(cx.listener(|_, _: &NewWindow, _, cx| {
                // After this update: opening a window from inside another window's update.
                cx.defer(|cx| {
                    if let Err(error) = crate::open_window(None, cx) {
                        wired_platform::show_error(&format!("Cannot open a window: {error:#}"));
                    }
                });
            }))
            .on_action(cx.listener(|this, _: &NewTab, window, cx| {
                if let Err(error) = this.open_tab(None, window, cx) {
                    this.show_error(format!("Cannot start the shell: {error:#}"), cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.step_tab(1, window, cx)))
            .on_action(cx.listener(|this, _: &PreviousTab, window, cx| this.step_tab(-1, window, cx)))
            .on_action(cx.listener(|this, _: &MoveTabLeft, _, cx| this.move_active(-1, cx)))
            .on_action(cx.listener(|this, _: &MoveTabRight, _, cx| this.move_active(1, cx)))
            .on_action(cx.listener(|this, _: &ClosePane, window, cx| {
                if let Some(tab) = this.tabs.get(this.active) {
                    let pane = tab.panes.focused();
                    this.close_pane(this.active, pane, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SplitRight, w, cx| this.split(Side::Right, w, cx)))
            .on_action(cx.listener(|this, _: &SplitBottom, w, cx| this.split(Side::Bottom, w, cx)))
            .on_action(cx.listener(|this, _: &SplitLeft, w, cx| this.split(Side::Left, w, cx)))
            .on_action(cx.listener(|this, _: &SplitTop, w, cx| this.split(Side::Top, w, cx)))
            .on_action(cx.listener(|this, _: &FocusPaneLeft, w, cx| this.focus_direction(Direction::Left, w, cx)))
            .on_action(cx.listener(|this, _: &FocusPaneRight, w, cx| this.focus_direction(Direction::Right, w, cx)))
            .on_action(cx.listener(|this, _: &FocusPaneUp, w, cx| this.focus_direction(Direction::Up, w, cx)))
            .on_action(cx.listener(|this, _: &FocusPaneDown, w, cx| this.focus_direction(Direction::Down, w, cx)))
            .on_action(cx.listener(|this, _: &FocusPreviousPane, w, cx| this.focus_step(-1, w, cx)))
            .on_action(cx.listener(|this, _: &FocusNextPane, w, cx| this.focus_step(1, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab1, w, cx| this.activate(0, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab2, w, cx| this.activate(1, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab3, w, cx| this.activate(2, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab4, w, cx| this.activate(3, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab5, w, cx| this.activate(4, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab6, w, cx| this.activate(5, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab7, w, cx| this.activate(6, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab8, w, cx| this.activate(7, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab9, w, cx| this.activate(8, w, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab10, w, cx| this.activate(9, w, cx)))
    }
}
