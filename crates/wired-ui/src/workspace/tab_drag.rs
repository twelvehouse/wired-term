//! Holding tabs with the pointer: reordering them along the strip, the slides that follow, and
//! handing over to `tab_move` when a held tab leaves the strip.

use super::tab_move::{Home, WindowBack, WindowDrag};
use super::*;
use crate::title_bar::TAB_WIDTH;

const DRAG_START_DISTANCE: f32 = 10.;
const REFERENCE_TAB_WIDTH: f32 = 256.;
/// Pointer travel between reorders for a tab of `REFERENCE_TAB_WIDTH`, so it does not flicker at
/// a boundary.
const REORDER_TRAVEL: f32 = 16.;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Press {
    pub(super) id: u64,
    x: f32,
    pub(super) y: f32,
    /// A fraction of the width, so the same spot stays under the pointer when the width changes.
    pub(super) grab_ratio: f32,
    previous_active: u64,
}

#[derive(Debug, Clone)]
pub(super) struct TabDrag {
    pub(super) id: u64,
    pub(super) grab_ratio: f32,
    pub(super) grab_y: f32,
    /// May lie outside the window; the held tab is clamped to the strip regardless.
    pub(super) pointer_x: f32,
    pub(super) last_reorder_x: Option<f32>,
    /// Activated again when the held tab leaves this window.
    pub(super) previous_active: u64,
    /// Where Escape puts the tab back; `None` leaves it where it is.
    pub(super) home: Option<Home>,
}

#[derive(Debug, Clone, Default)]
pub(super) enum TabPointer {
    #[default]
    Idle,
    /// Not far enough from the press to be a drag yet.
    Pressed(Press),
    Dragging(TabDrag),
    MovingWindow(WindowDrag),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct TabShift {
    /// Positive is right of the slot.
    pub(super) offset: f32,
    pub(super) is_sliding: bool,
    /// Held, or settling after release: drawn above its neighbors.
    pub(super) is_lifted: bool,
}

/// Where a held tab is drawn, from the strip's left.
#[derive(Debug, Clone, Copy)]
pub(super) struct Held {
    pub(super) index: usize,
    pub(super) left: f32,
    offset: f32,
}

fn is_drag_start(press_x: f32, press_y: f32, x: f32, y: f32) -> bool {
    (x - press_x).hypot(y - press_y) > DRAG_START_DISTANCE
}

pub(super) fn grab_ratio(grab_x: f32, width: f32) -> f32 {
    if width > 0. { (grab_x / width).clamp(0., 1.) } else { 0.5 }
}

fn held_left(pointer_x: f32, strip_left: f32, grab_ratio: f32, width: f32, area_width: f32) -> f32 {
    (pointer_x - strip_left - grab_ratio * width).clamp(0., (area_width - width).max(0.))
}

fn reorder_threshold(width: f32) -> f32 {
    (width / REFERENCE_TAB_WIDTH * REORDER_TRAVEL).round()
}

fn may_reorder(last_reorder_x: Option<f32>, pointer_x: f32, width: f32) -> bool {
    last_reorder_x.is_none_or(|last| (pointer_x - last).abs() > reorder_threshold(width))
}

fn ease_out(t: f32) -> f32 {
    1. - (1. - t).powi(2)
}

/// A tab sliding to its slot from `from` px right of it.
#[derive(Debug, Clone, Copy)]
pub(super) struct Slide {
    from: f32,
    started: Instant,
    /// Released by the pointer: passes over its neighbors on the way.
    pub(super) is_released: bool,
}

impl Slide {
    fn new(from: f32, started: Instant) -> Self {
        Self { from, started, is_released: false }
    }

    fn run_fraction(self, now: Instant) -> f32 {
        (now.saturating_duration_since(self.started).as_secs_f32() / ANIMATION.as_secs_f32()).clamp(0., 1.)
    }

    fn offset(self, now: Instant) -> f32 {
        self.from * (1. - ease_out(self.run_fraction(now)))
    }

    fn is_done(self, now: Instant) -> bool {
        self.run_fraction(now) >= 1.
    }
}

/// Drawn tab widths in order, plus the room closing tabs still take before each one. The room
/// belongs to the place, not the tab, so it stays put when neighbors swap.
pub(super) struct Slots {
    pub(super) widths: Vec<f32>,
    gaps: Vec<f32>,
    tail: f32,
}

impl Slots {
    pub(super) fn lefts(&self) -> Vec<f32> {
        let mut left = 0.;
        self.widths
            .iter()
            .zip(&self.gaps)
            .map(|(width, gap)| {
                let at = left + gap;
                left = at + width;
                at
            })
            .collect()
    }

    fn total(&self) -> f32 {
        let end = self.lefts().last().zip(self.widths.last()).map_or(0., |(left, width)| left + width);
        end + self.tail
    }

    /// The place whose slot would be drawn closest to `left`.
    fn insertion_index(&self, from: usize, left: f32) -> usize {
        let others: Vec<f32> =
            self.widths.iter().enumerate().filter(|(index, _)| *index != from).map(|(_, width)| *width).collect();
        let mut best = (from, f32::INFINITY);
        for place in 0..self.widths.len() {
            let ideal = self.gaps[..=place].iter().sum::<f32>() + others[..place].iter().sum::<f32>();
            let distance = (ideal - left).abs();
            if distance < best.1 {
                best = (place, distance);
            }
        }
        best.0
    }
}

impl Workspace {
    pub(super) fn slots(&self) -> Slots {
        let metrics = self.strip.borrow();
        let width_of = |id| metrics.widths.get(&id).copied().unwrap_or(TAB_WIDTH);
        let ghosts = self.sorted_ghosts();
        let ghost_indexes: Vec<usize> = ghosts.iter().map(|g| g.index).collect();
        let mut slots = Slots { widths: Vec::new(), gaps: Vec::new(), tail: 0. };
        let mut gap = 0.;
        for item in strip_order(self.tabs.len(), &ghost_indexes) {
            match item {
                StripItem::Tab(index) => {
                    slots.widths.push(width_of(self.tabs[index].id));
                    slots.gaps.push(std::mem::take(&mut gap));
                }
                StripItem::Ghost(index) => gap += width_of(ghosts[index].id),
            }
        }
        slots.tail = gap;
        slots
    }

    pub(super) fn forget_gone_widths(&self) {
        let is_present = |id: &u64| self.tabs.iter().any(|t| t.id == *id) || self.closing.iter().any(|c| c.id == *id);
        self.strip.borrow_mut().widths.retain(|id, _| is_present(id));
    }

    /// The held tab is drawn above the clip of overflowing tabs, so it is clamped to the
    /// visible strip rather than to the slots.
    pub(super) fn held_tab(&self, drag: &TabDrag, slots: &Slots) -> Option<Held> {
        let index = self.tabs.iter().position(|t| t.id == drag.id)?;
        let width = *slots.widths.get(index)?;
        let strip = self.strip.borrow();
        let area_width = (strip.limit_right - strip.left).max(0.);
        let left = held_left(drag.pointer_x, strip.left, drag.grab_ratio, width, area_width);
        Some(Held { index, left, offset: left - *slots.lefts().get(index)? })
    }

    fn slide_offset(tab: &Tab, now: Instant) -> (f32, bool) {
        tab.slide.map_or((0., false), |s| (s.offset(now), !s.is_done(now)))
    }

    pub(super) fn settle_slides(&mut self, now: Instant) {
        for tab in &mut self.tabs {
            if tab.slide.is_some_and(|s| s.is_done(now)) {
                tab.slide = None;
            }
        }
    }

    /// Without `slides_moved` the moved tab does not slide, as the pointer holds it.
    pub(super) fn move_tab(&mut self, from: usize, to: usize, slides_moved: bool, now: Instant) {
        if from == to || from.max(to) >= self.tabs.len() {
            return;
        }
        let before = self.slot_lefts();
        let active_id = self.tabs.get(self.active).map(|t| t.id);
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        if let Some(index) = active_id.and_then(|id| self.tabs.iter().position(|t| t.id == id)) {
            self.active = index;
        }
        self.slide_from(&before, now);
        if !slides_moved {
            self.tabs[to].slide = None;
        }
    }

    pub(super) fn slot_lefts(&self) -> Vec<(u64, f32)> {
        self.tabs.iter().map(|t| t.id).zip(self.slots().lefts()).collect()
    }

    pub(super) fn slide_from(&mut self, before: &[(u64, f32)], now: Instant) {
        let after = self.slots().lefts();
        for (tab, left) in self.tabs.iter_mut().zip(after) {
            let Some(&(_, was)) = before.iter().find(|(id, _)| *id == tab.id) else { continue };
            if was != left {
                let current = tab.slide.map_or(0., |s| s.offset(now));
                tab.slide = Some(Slide::new(current + was - left, now));
            }
        }
    }

    /// Activates the tab at once, whether or not the press becomes a drag.
    pub(super) fn press_tab(&mut self, id: u64, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.as_ref().is_some_and(|r| r.tab_id == id) {
            return;
        }
        if event.click_count >= 2 {
            self.start_rename(id, window, cx);
            return;
        }
        let Some(index) = self.tabs.iter().position(|t| t.id == id) else { return };
        let (x, y) = (f32::from(event.position.x), f32::from(event.position.y));
        let slots = self.slots();
        let slot_left = slots.lefts().get(index).copied().unwrap_or_default();
        let width = slots.widths.get(index).copied().unwrap_or(TAB_WIDTH);
        let drawn_left = self.strip.borrow().left + slot_left + Self::slide_offset(&self.tabs[index], Instant::now()).0;
        let previous_active = self.tabs.get(self.active).map_or(id, |t| t.id);
        self.pointer =
            TabPointer::Pressed(Press { id, x, y, grab_ratio: grab_ratio(x - drawn_left, width), previous_active });
        self.activate(index, window, cx);
    }

    /// A move without the left button means its release went unseen.
    fn on_tab_pointer_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            self.end_tab_pointer(cx);
            return;
        }
        let (x, y) = (f32::from(event.position.x), f32::from(event.position.y));
        match &self.pointer {
            TabPointer::Idle => return,
            TabPointer::Pressed(press) if is_drag_start(press.x, press.y, x, y) => {
                let press = *press;
                let Some(origin_index) = self.tabs.iter().position(|t| t.id == press.id) else {
                    self.pointer = TabPointer::Idle;
                    return;
                };
                self.tabs[origin_index].slide = None;
                if self.tabs.len() == 1 {
                    self.start_window_drag(press, window, cx);
                } else {
                    let drag = TabDrag {
                        id: press.id,
                        grab_ratio: press.grab_ratio,
                        grab_y: press.y - self.strip.borrow().top,
                        pointer_x: x,
                        last_reorder_x: None,
                        previous_active: press.previous_active,
                        home: Some(Home::here(origin_index, press.previous_active, window, cx)),
                    };
                    self.drag_tab(drag, y, window, cx);
                }
            }
            TabPointer::Pressed(_) => return,
            TabPointer::Dragging(drag) => {
                let drag = TabDrag { pointer_x: x, ..drag.clone() };
                self.drag_tab(drag, y, window, cx);
            }
            TabPointer::MovingWindow(drag) => {
                let drag = drag.clone();
                self.drag_window(drag, window, cx);
            }
        }
        cx.notify();
    }

    fn drag_tab(&mut self, drag: TabDrag, y: f32, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_on_strip(drag.pointer_x, y) {
            self.follow_pointer(drag);
        } else {
            self.tear_off(drag, window, cx);
        }
    }

    pub(super) fn follow_pointer(&mut self, mut drag: TabDrag) {
        let slots = self.slots();
        let Some(held) = self.held_tab(&drag, &slots) else {
            self.pointer = TabPointer::Idle;
            return;
        };
        let to = slots.insertion_index(held.index, held.left);
        if to != held.index && may_reorder(drag.last_reorder_x, drag.pointer_x, slots.widths[to]) {
            drag.last_reorder_x = Some(drag.pointer_x);
            self.move_tab(held.index, to, false, Instant::now());
        }
        self.pointer = TabPointer::Dragging(drag);
    }

    fn drop_held(&mut self, held: Held, to: usize, now: Instant) {
        self.move_tab(held.index, to, false, now);
        let Some(&slot_left) = self.slots().lefts().get(to) else { return };
        self.tabs[to].slide = Some(Slide { is_released: true, ..Slide::new(held.left - slot_left, now) });
    }

    /// Returns whether there was a press or drag to end. Whatever opens, closes or moves a tab
    /// calls this first, so no index kept by a drag can go stale.
    pub(super) fn end_tab_pointer(&mut self, cx: &mut Context<Self>) -> bool {
        let held = self.held(&self.slots());
        match std::mem::take(&mut self.pointer) {
            TabPointer::Idle => return false,
            TabPointer::Pressed(_) => {}
            TabPointer::Dragging(_) | TabPointer::MovingWindow(_) => {
                if let Some(held) = held {
                    self.drop_held(held, held.index, Instant::now());
                }
            }
        }
        cx.notify();
        true
    }

    /// Returns whether a drag was cancelled. Only Escape brings the home window to the front, not
    /// a focus loss.
    pub(super) fn cancel_tab_drag(&mut self, is_escape: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.pointer.clone() {
            TabPointer::Idle | TabPointer::Pressed(_) => return false,
            TabPointer::Dragging(drag) => match drag.home {
                Some(home) if home.is_here(cx) => {
                    let held = self.held(&self.slots());
                    self.pointer = TabPointer::Idle;
                    if let Some(held) = held {
                        self.drop_held(held, home.index, Instant::now());
                    }
                    if let Some(index) = self.tabs.iter().position(|t| t.id == home.previous_active) {
                        self.activate(index, window, cx);
                    }
                }
                Some(home) => self.send_home(drag.id, drag.previous_active, home, is_escape, window, cx),
                None => {
                    self.end_tab_pointer(cx);
                }
            },
            TabPointer::MovingWindow(drag) => match drag.held.back {
                WindowBack::To { x, y } => {
                    self.pointer = TabPointer::Idle;
                    self.move_window_to(x, y, cx);
                }
                WindowBack::Home(home) => self.send_home(drag.id, drag.id, home, is_escape, window, cx),
            },
        }
        cx.notify();
        true
    }

    fn held(&self, slots: &Slots) -> Option<Held> {
        match &self.pointer {
            TabPointer::Dragging(drag) => self.held_tab(drag, slots),
            TabPointer::MovingWindow(drag) => {
                let index = self.tabs.iter().position(|t| t.id == drag.id)?;
                let left = drag.held.tab_left;
                Some(Held { index, left, offset: left - *slots.lefts().get(index)? })
            }
            TabPointer::Idle | TabPointer::Pressed(_) => None,
        }
    }

    /// Also returns how far the drawn tabs reach past the strip's end, so the trailing buttons
    /// move over.
    pub(super) fn tab_shifts(&self, now: Instant) -> (Vec<TabShift>, f32) {
        let slots = self.slots();
        let lefts = slots.lefts();
        let held = self.held(&slots);
        let mut reach = 0_f32;
        let shifts = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let shift = match held {
                    Some(held) if held.index == index => {
                        TabShift { offset: held.offset, is_sliding: false, is_lifted: true }
                    }
                    _ => {
                        let (offset, is_sliding) = Self::slide_offset(tab, now);
                        TabShift { offset, is_sliding, is_lifted: tab.slide.is_some_and(|s| s.is_released) }
                    }
                };
                reach = reach.max(lefts[index] + shift.offset + slots.widths[index]);
                shift
            })
            .collect();
        (shifts, (reach - slots.total()).max(0.))
    }

    pub(super) fn strip_tracker(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let metrics = Rc::clone(&self.strip);
        let workspace = cx.entity().downgrade();
        canvas(
            move |bounds, _, _| {
                let mut strip = metrics.borrow_mut();
                strip.left = f32::from(bounds.origin.x);
                strip.top = f32::from(bounds.origin.y);
            },
            // Registered every frame: a fast click or touchpad tap releases before the next frame.
            move |_, _, window, _| {
                let on_move = workspace.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = on_move.update(cx, |this, cx| this.on_tab_pointer_move(event, window, cx));
                    }
                });
                // A press during a drag means pointer capture was lost and the release went elsewhere.
                let on_down = workspace.clone();
                window.on_mouse_event(move |_: &MouseDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = on_down.update(cx, |this, cx| {
                            if matches!(this.pointer, TabPointer::Dragging(_) | TabPointer::MovingWindow(_)) {
                                this.end_tab_pointer(cx);
                            }
                        });
                    }
                });
                let on_up = workspace.clone();
                // Capture phase: the panes take the release in bubble before the strip would.
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        let is_ended = on_up.update(cx, |this, cx| this.end_tab_pointer(cx)).unwrap_or(false);
                        if is_ended {
                            cx.stop_propagation();
                        }
                    }
                });
            },
        )
        .absolute()
        .size_full()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tight(widths: &[f32]) -> Slots {
        Slots { widths: widths.to_vec(), gaps: vec![0.; widths.len()], tail: 0. }
    }

    fn four() -> Slots {
        tight(&[100.; 4])
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 1e-3, "{actual} != {expected}");
    }

    #[test]
    fn a_press_is_a_drag_only_beyond_ten_px_from_the_press_point() {
        assert!(!is_drag_start(50., 20., 60., 20.), "exactly 10 px is not enough");
        assert!(is_drag_start(50., 20., 60.5, 20.));
        assert!(is_drag_start(50., 20., 50., 9.), "vertical travel counts");
        assert!(!is_drag_start(50., 20., 57., 27.), "9.9 px diagonally");
        assert!(is_drag_start(50., 20., 58., 27.), "10.6 px diagonally");
    }

    #[test]
    fn the_grab_point_is_a_fraction_of_the_tab_width() {
        assert_close(grab_ratio(50., 200.), 0.25);
        assert_close(grab_ratio(-5., 200.), 0.);
        assert_close(grab_ratio(250., 200.), 1.);
        assert_close(grab_ratio(10., 0.), 0.5);
    }

    #[test]
    fn the_held_tab_keeps_the_grabbed_spot_under_the_pointer_when_its_width_changes() {
        assert_close(held_left(300., 0., 0.25, 200., 1000.), 250.);
        assert_close(held_left(300., 0., 0.25, 100., 1000.), 275.);
        assert_close(held_left(300., 40., 0.25, 200., 1000.), 210.);
    }

    #[test]
    fn the_held_tab_stops_at_the_left_end_and_at_the_end_of_the_drag_area() {
        assert_close(held_left(-500., 0., 0.5, 100., 800.), 0.);
        assert_close(held_left(30., 0., 0.5, 100., 800.), 0.);
        assert_close(held_left(9000., 0., 0.5, 100., 800.), 700.);
        assert_close(held_left(9000., 0., 0.5, 900., 800.), 0.);
    }

    #[test]
    fn moving_the_pointer_into_the_empty_strip_past_the_last_tab_is_allowed_up_to_the_limit() {
        assert_close(held_left(700., 0., 0., 100., 900.), 700.);
        assert_close(held_left(850., 0., 0., 100., 900.), 800.);
    }

    #[test]
    fn crowded_tabs_that_overflow_the_strip_do_not_let_the_held_tab_pass_its_limit() {
        let crowded = tight(&[80.; 10]);
        let visible = 600.;
        assert!(crowded.total() > visible);
        assert_close(held_left(9000., 40., 0.5, 80., visible), 520.);
        assert_close(held_left(9000., 40., 0.5, 80., visible) + 80., visible);
    }

    #[test]
    fn a_tab_is_passed_once_the_held_tab_has_covered_half_of_it() {
        assert_eq!(four().insertion_index(0, 0.), 0);
        assert_eq!(four().insertion_index(0, 49.), 0);
        assert_eq!(four().insertion_index(0, 51.), 1);
        assert_eq!(four().insertion_index(0, 149.), 1);
        assert_eq!(four().insertion_index(0, 151.), 2);
        assert_eq!(four().insertion_index(3, 249.), 2);
        assert_eq!(four().insertion_index(3, 251.), 3);
    }

    #[test]
    fn dragging_left_mirrors_dragging_right() {
        assert_eq!(four().insertion_index(3, 149.), 1);
        assert_eq!(four().insertion_index(3, 151.), 2);
        assert_eq!(four().insertion_index(2, 49.), 0);
    }

    #[test]
    fn a_tab_held_at_either_end_of_the_strip_or_beyond_takes_the_end_place() {
        assert_eq!(four().insertion_index(0, 300.), 3);
        assert_eq!(four().insertion_index(0, 5000.), 3);
        assert_eq!(four().insertion_index(3, 0.), 0);
    }

    #[test]
    fn a_tab_that_has_moved_stays_until_it_is_nearer_to_the_place_it_left() {
        assert_eq!(four().insertion_index(0, 100.), 1);
        assert_eq!(four().insertion_index(1, 100.), 1);
        assert_eq!(four().insertion_index(1, 51.), 1);
        assert_eq!(four().insertion_index(1, 49.), 0);
    }

    #[test]
    fn unequal_widths_pick_the_nearest_place_not_the_nearest_center() {
        let slots = tight(&[80., 200., 80.]);
        assert_eq!(slots.insertion_index(0, 99.), 0);
        assert_eq!(slots.insertion_index(0, 101.), 1);
        assert_eq!(slots.insertion_index(0, 239.), 1);
        assert_eq!(slots.insertion_index(0, 241.), 2);
    }

    fn with_a_closing_tab() -> Slots {
        Slots { widths: vec![100.; 3], gaps: vec![0., 100., 0.], tail: 0. }
    }

    #[test]
    fn slots_follow_the_widths_and_the_room_of_closing_tabs() {
        assert_eq!(tight(&[100., 80., 60.]).lefts(), [0., 100., 180.]);
        assert_eq!(tight(&[100., 80., 60.]).total(), 240.);
        assert!(tight(&[]).lefts().is_empty());
        assert_eq!(with_a_closing_tab().lefts(), [0., 200., 300.]);
        let trailing = Slots { tail: 50., ..tight(&[100.; 2]) };
        assert_eq!(trailing.total(), 250.);
    }

    #[test]
    fn a_closing_tab_keeps_its_room_when_the_tabs_next_to_it_swap() {
        let slots = with_a_closing_tab();
        assert_eq!(slots.insertion_index(0, 99.), 0);
        assert_eq!(slots.insertion_index(0, 101.), 1);
        assert_eq!(slots.insertion_index(2, 249.), 1);
        assert_eq!(slots.insertion_index(2, 251.), 2);
    }

    #[test]
    fn the_travel_needed_between_reorders_scales_with_the_tab_width() {
        assert_eq!(reorder_threshold(256.), 16.);
        assert_eq!(reorder_threshold(200.), 13.);
        assert_eq!(reorder_threshold(80.), 5.);
    }

    #[test]
    fn after_a_reorder_the_pointer_must_travel_the_threshold_before_the_next() {
        assert!(may_reorder(None, 500., 256.), "nothing has been reordered yet");
        assert!(!may_reorder(Some(100.), 100., 256.));
        assert!(!may_reorder(Some(100.), 116., 256.), "exactly the threshold is not enough");
        assert!(may_reorder(Some(100.), 117., 256.));
        assert!(!may_reorder(Some(100.), 84., 256.));
        assert!(may_reorder(Some(100.), 83., 256.));
        assert!(!may_reorder(Some(100.), 104., 80.));
        assert!(may_reorder(Some(100.), 106., 80.));
    }

    #[test]
    fn a_slide_closes_the_gap_with_a_quadratic_ease_out_over_the_animation() {
        let start = Instant::now();
        let slide = Slide::new(100., start);
        assert_close(slide.offset(start), 100.);
        assert_close(slide.offset(start + ANIMATION / 2), 25.);
        assert_close(slide.offset(start + ANIMATION), 0.);
        assert_close(slide.offset(start + ANIMATION * 3), 0.);
        assert!(!slide.is_done(start) && !slide.is_done(start + ANIMATION / 2) && slide.is_done(start + ANIMATION));
    }
}
